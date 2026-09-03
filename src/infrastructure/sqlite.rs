use std::{collections::HashSet, path::Path, str::FromStr, time::Duration};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sqlx::{
    AssertSqlSafe, FromRow, Row, Sqlite, SqlitePool, Transaction,
    migrate::Migrator,
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous},
};

use crate::{
    application::ports::{
        AuthError, AuthRepository, ContentLink, ContentRepository, Engagement,
        EngagementRepository, LikeRepository, MediaRepository, MediaRepositoryError,
        PasskeyRepository, PortableRepository, PreparedContent, PreviewLinkRepository,
        PublicSnapshotRepository, PublicationState, RepositoryError, RevisionMediaReferences,
        SearchHit, SearchRepository, SetupRegistration, SiteRepository,
    },
    application::{
        media_gc,
        site_compiler::{PublicRedirect, SiteSnapshotV1},
    },
    domain::auth::{PreviewLinkRecord, SecretHash, SessionRecord, SetupPurpose, StoredPasskey},
    domain::content::{
        Content, ContentId, ContentKind, ContentRevision, Publication, SaveIntent, Slug, Tag,
    },
    domain::media::{MediaAsset, MediaId, MediaVariant},
    domain::search::{self, SearchTerms},
    domain::theme::{Locale, NavigationItem, SiteSettings},
    portable::{
        PortableContent, PortableEngagement, PortableOwner, PortablePasskey,
        PortablePublicationState, PortableRecoveryCode, PortableRedirect, PortableSiteV1,
    },
};
use uuid::Uuid;

pub(crate) static MIGRATOR: Migrator = sqlx::migrate!("./migrations");

#[derive(Clone, Debug)]
pub struct SqliteRepository {
    pool: SqlitePool,
}

#[async_trait]
impl AuthRepository for SqliteRepository {
    async fn store_setup_token(
        &self,
        token_hash: SecretHash,
        purpose: SetupPurpose,
        expires_at: DateTime<Utc>,
    ) -> Result<(), AuthError> {
        sqlx::query("INSERT INTO setup_tokens (token_hash, purpose, expires_at) VALUES (?, ?, ?)")
            .bind(token_hash.as_bytes().as_slice())
            .bind(purpose.as_str())
            .bind(expires_at)
            .execute(&self.pool)
            .await
            .map_err(auth_storage)?;
        Ok(())
    }

    async fn consume_setup_token(
        &self,
        token_hash: SecretHash,
        purpose: SetupPurpose,
        now: DateTime<Utc>,
    ) -> Result<bool, AuthError> {
        let result = sqlx::query(
            "UPDATE setup_tokens SET consumed_at = ?
             WHERE token_hash = ? AND purpose = ? AND consumed_at IS NULL AND expires_at >= ?",
        )
        .bind(now)
        .bind(token_hash.as_bytes().as_slice())
        .bind(purpose.as_str())
        .bind(now)
        .execute(&self.pool)
        .await
        .map_err(auth_storage)?;
        Ok(result.rows_affected() == 1)
    }

    async fn store_session(&self, session: &SessionRecord) -> Result<(), AuthError> {
        sqlx::query(
            "INSERT INTO sessions (
                token_hash, csrf_token_hash, created_at, expires_at, last_seen_at, reauthenticated_at
             ) VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(session.token_hash.as_bytes().as_slice())
        .bind(session.csrf_hash.as_bytes().as_slice())
        .bind(session.created_at)
        .bind(session.expires_at)
        .bind(session.last_seen_at)
        .bind(session.reauthenticated_at)
        .execute(&self.pool)
        .await
        .map_err(auth_storage)?;
        Ok(())
    }

    async fn find_session(
        &self,
        token_hash: SecretHash,
        now: DateTime<Utc>,
    ) -> Result<Option<SessionRecord>, AuthError> {
        let row = sqlx::query(
            "UPDATE sessions SET last_seen_at = ?
             WHERE token_hash = ? AND expires_at > ?
             RETURNING csrf_token_hash, created_at, expires_at, last_seen_at, reauthenticated_at",
        )
        .bind(now)
        .bind(token_hash.as_bytes().as_slice())
        .bind(now)
        .fetch_optional(&self.pool)
        .await
        .map_err(auth_storage)?;
        row.map(|row| {
            Ok(SessionRecord {
                token_hash,
                csrf_hash: secret_hash_from_row(&row, "csrf_token_hash")?,
                created_at: row.try_get("created_at").map_err(auth_storage)?,
                expires_at: row.try_get("expires_at").map_err(auth_storage)?,
                last_seen_at: row.try_get("last_seen_at").map_err(auth_storage)?,
                reauthenticated_at: row.try_get("reauthenticated_at").map_err(auth_storage)?,
            })
        })
        .transpose()
    }

    async fn extend_session(
        &self,
        token_hash: SecretHash,
        expires_at: DateTime<Utc>,
        now: DateTime<Utc>,
    ) -> Result<bool, AuthError> {
        let result = sqlx::query(
            "UPDATE sessions SET expires_at = ?, last_seen_at = ?
             WHERE token_hash = ? AND expires_at > ?",
        )
        .bind(expires_at)
        .bind(now)
        .bind(token_hash.as_bytes().as_slice())
        .bind(now)
        .execute(&self.pool)
        .await
        .map_err(auth_storage)?;
        Ok(result.rows_affected() == 1)
    }

    async fn rotate_session(
        &self,
        old_token_hash: SecretHash,
        replacement: &SessionRecord,
        now: DateTime<Utc>,
    ) -> Result<bool, AuthError> {
        let mut transaction = self.pool.begin().await.map_err(auth_storage)?;
        let result = sqlx::query("DELETE FROM sessions WHERE token_hash = ? AND expires_at > ?")
            .bind(old_token_hash.as_bytes().as_slice())
            .bind(now)
            .execute(&mut *transaction)
            .await
            .map_err(auth_storage)?;
        if result.rows_affected() == 0 {
            transaction.rollback().await.map_err(auth_storage)?;
            return Ok(false);
        }
        sqlx::query(
            "INSERT INTO sessions (
                token_hash, csrf_token_hash, created_at, expires_at, last_seen_at, reauthenticated_at
             ) VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(replacement.token_hash.as_bytes().as_slice())
        .bind(replacement.csrf_hash.as_bytes().as_slice())
        .bind(replacement.created_at)
        .bind(replacement.expires_at)
        .bind(replacement.last_seen_at)
        .bind(replacement.reauthenticated_at)
        .execute(&mut *transaction)
        .await
        .map_err(auth_storage)?;
        transaction.commit().await.map_err(auth_storage)?;
        Ok(true)
    }

    async fn revoke_session(&self, token_hash: SecretHash) -> Result<bool, AuthError> {
        let result = sqlx::query("DELETE FROM sessions WHERE token_hash = ?")
            .bind(token_hash.as_bytes().as_slice())
            .execute(&self.pool)
            .await
            .map_err(auth_storage)?;
        Ok(result.rows_affected() == 1)
    }

    async fn replace_recovery_codes(
        &self,
        code_hashes: &[SecretHash],
        now: DateTime<Utc>,
    ) -> Result<(), AuthError> {
        let mut transaction = self.pool.begin().await.map_err(auth_storage)?;
        sqlx::query("DELETE FROM recovery_codes")
            .execute(&mut *transaction)
            .await
            .map_err(auth_storage)?;
        for hash in code_hashes {
            sqlx::query("INSERT INTO recovery_codes (code_hash, created_at) VALUES (?, ?)")
                .bind(hash.as_bytes().as_slice())
                .bind(now)
                .execute(&mut *transaction)
                .await
                .map_err(auth_storage)?;
        }
        transaction.commit().await.map_err(auth_storage)?;
        Ok(())
    }

    async fn consume_recovery_code(
        &self,
        code_hash: SecretHash,
        now: DateTime<Utc>,
    ) -> Result<bool, AuthError> {
        let result = sqlx::query(
            "UPDATE recovery_codes SET consumed_at = ?
             WHERE code_hash = ? AND consumed_at IS NULL",
        )
        .bind(now)
        .bind(code_hash.as_bytes().as_slice())
        .execute(&self.pool)
        .await
        .map_err(auth_storage)?;
        Ok(result.rows_affected() == 1)
    }

    async fn exchange_recovery_code(
        &self,
        code_hash: SecretHash,
        session: &SessionRecord,
        now: DateTime<Utc>,
    ) -> Result<bool, AuthError> {
        let mut transaction = self.pool.begin().await.map_err(auth_storage)?;
        let consumed = sqlx::query(
            "UPDATE recovery_codes SET consumed_at = ?
             WHERE code_hash = ? AND consumed_at IS NULL",
        )
        .bind(now)
        .bind(code_hash.as_bytes().as_slice())
        .execute(&mut *transaction)
        .await
        .map_err(auth_storage)?;
        if consumed.rows_affected() != 1 {
            transaction.rollback().await.map_err(auth_storage)?;
            return Ok(false);
        }
        insert_session(&mut transaction, session).await?;
        transaction.commit().await.map_err(auth_storage)?;
        Ok(true)
    }
}

#[async_trait]
impl PasskeyRepository for SqliteRepository {
    async fn owner_handle(&self) -> Result<Option<Uuid>, AuthError> {
        let bytes: Option<Vec<u8>> =
            sqlx::query_scalar("SELECT user_handle FROM owner WHERE singleton = 1")
                .fetch_optional(&self.pool)
                .await
                .map_err(auth_storage)?;
        bytes
            .map(|bytes| Uuid::from_slice(&bytes).map_err(auth_storage))
            .transpose()
    }

    async fn list_passkeys(&self) -> Result<Vec<StoredPasskey>, AuthError> {
        let rows = sqlx::query(
            "SELECT credential_id, name, passkey_json FROM passkeys ORDER BY created_at, name",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(auth_storage)?;
        rows.into_iter()
            .map(|row| {
                Ok(StoredPasskey {
                    credential_id: row.try_get("credential_id").map_err(auth_storage)?,
                    name: row.try_get("name").map_err(auth_storage)?,
                    passkey_json: row.try_get("passkey_json").map_err(auth_storage)?,
                })
            })
            .collect()
    }

    async fn setup_token_purpose(
        &self,
        token_hash: SecretHash,
        now: DateTime<Utc>,
    ) -> Result<Option<SetupPurpose>, AuthError> {
        let purpose: Option<String> = sqlx::query_scalar(
            "SELECT purpose FROM setup_tokens
             WHERE token_hash = ? AND consumed_at IS NULL AND expires_at >= ?",
        )
        .bind(token_hash.as_bytes().as_slice())
        .bind(now)
        .fetch_optional(&self.pool)
        .await
        .map_err(auth_storage)?;
        purpose
            .map(|purpose| {
                SetupPurpose::parse(&purpose)
                    .ok_or_else(|| AuthError::Storage("setup token has an invalid purpose".into()))
            })
            .transpose()
    }

    async fn complete_setup_registration(
        &self,
        registration: SetupRegistration<'_>,
    ) -> Result<bool, AuthError> {
        let SetupRegistration {
            setup_token_hash,
            purpose,
            user_handle,
            passkey,
            session,
            recovery_code_hashes,
            now,
        } = registration;
        let mut transaction = self.pool.begin().await.map_err(auth_storage)?;
        let consumed = sqlx::query(
            "UPDATE setup_tokens SET consumed_at = ?
             WHERE token_hash = ? AND purpose = ? AND consumed_at IS NULL AND expires_at >= ?",
        )
        .bind(now)
        .bind(setup_token_hash.as_bytes().as_slice())
        .bind(purpose.as_str())
        .bind(now)
        .execute(&mut *transaction)
        .await
        .map_err(auth_storage)?;
        if consumed.rows_affected() != 1 {
            transaction.rollback().await.map_err(auth_storage)?;
            return Ok(false);
        }

        let owner: Option<Vec<u8>> =
            sqlx::query_scalar("SELECT user_handle FROM owner WHERE singleton = 1")
                .fetch_optional(&mut *transaction)
                .await
                .map_err(auth_storage)?;
        match (purpose, owner) {
            (SetupPurpose::Initial, None) => {
                sqlx::query(
                    "INSERT INTO owner (singleton, user_handle, created_at) VALUES (1, ?, ?)",
                )
                .bind(user_handle.as_bytes().as_slice())
                .bind(now)
                .execute(&mut *transaction)
                .await
                .map_err(auth_storage)?;
            }
            (SetupPurpose::Recovery, Some(existing))
                if existing.as_slice() == user_handle.as_bytes() =>
            {
                sqlx::query("DELETE FROM passkeys")
                    .execute(&mut *transaction)
                    .await
                    .map_err(auth_storage)?;
                sqlx::query("DELETE FROM sessions")
                    .execute(&mut *transaction)
                    .await
                    .map_err(auth_storage)?;
            }
            _ => {
                transaction.rollback().await.map_err(auth_storage)?;
                return Ok(false);
            }
        }

        sqlx::query(
            "INSERT INTO passkeys (credential_id, name, passkey_json, created_at)
             VALUES (?, ?, ?, ?)",
        )
        .bind(&passkey.credential_id)
        .bind(&passkey.name)
        .bind(&passkey.passkey_json)
        .bind(now)
        .execute(&mut *transaction)
        .await
        .map_err(auth_storage)?;
        sqlx::query("DELETE FROM recovery_codes")
            .execute(&mut *transaction)
            .await
            .map_err(auth_storage)?;
        for hash in recovery_code_hashes {
            sqlx::query("INSERT INTO recovery_codes (code_hash, created_at) VALUES (?, ?)")
                .bind(hash.as_bytes().as_slice())
                .bind(now)
                .execute(&mut *transaction)
                .await
                .map_err(auth_storage)?;
        }
        insert_session(&mut transaction, session).await?;
        transaction.commit().await.map_err(auth_storage)?;
        Ok(true)
    }

    async fn complete_authentication(
        &self,
        credential_id: &[u8],
        passkey_json: &str,
        session: &SessionRecord,
        now: DateTime<Utc>,
    ) -> Result<bool, AuthError> {
        let mut transaction = self.pool.begin().await.map_err(auth_storage)?;
        let updated = sqlx::query(
            "UPDATE passkeys SET passkey_json = ?, last_used_at = ? WHERE credential_id = ?",
        )
        .bind(passkey_json)
        .bind(now)
        .bind(credential_id)
        .execute(&mut *transaction)
        .await
        .map_err(auth_storage)?;
        if updated.rows_affected() != 1 {
            transaction.rollback().await.map_err(auth_storage)?;
            return Ok(false);
        }
        insert_session(&mut transaction, session).await?;
        transaction.commit().await.map_err(auth_storage)?;
        Ok(true)
    }

    async fn add_passkey(
        &self,
        passkey: &StoredPasskey,
        now: DateTime<Utc>,
    ) -> Result<(), AuthError> {
        sqlx::query(
            "INSERT INTO passkeys (credential_id, name, passkey_json, created_at)
             VALUES (?, ?, ?, ?)",
        )
        .bind(&passkey.credential_id)
        .bind(&passkey.name)
        .bind(&passkey.passkey_json)
        .bind(now)
        .execute(&self.pool)
        .await
        .map_err(auth_storage)?;
        Ok(())
    }

    async fn remove_passkey(&self, credential_id: &[u8]) -> Result<bool, AuthError> {
        let removed = sqlx::query(
            "DELETE FROM passkeys
             WHERE credential_id = ? AND (SELECT COUNT(*) FROM passkeys) > 1",
        )
        .bind(credential_id)
        .execute(&self.pool)
        .await
        .map_err(auth_storage)?;
        Ok(removed.rows_affected() == 1)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SqlitePragmas {
    pub foreign_keys: bool,
    pub journal_mode: String,
    pub busy_timeout_ms: i64,
}

#[derive(Debug, FromRow)]
struct ContentRow {
    id: i64,
    kind: String,
    title: String,
    slug: String,
    summary: String,
    body_markdown: String,
    body_html: String,
    status: String,
    publish_at: Option<DateTime<Utc>>,
    cover_media_id: Option<String>,
    seo_title: Option<String>,
    seo_description: Option<String>,
    version: i64,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    deleted_at: Option<DateTime<Utc>>,
}

impl SqliteRepository {
    pub async fn connect(path: &Path) -> Result<Self, RepositoryError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(storage)?;
        }
        let options = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .foreign_keys(true)
            .journal_mode(SqliteJournalMode::Wal)
            .synchronous(SqliteSynchronous::Normal)
            .busy_timeout(Duration::from_secs(5));
        let pool = SqlitePoolOptions::new()
            .min_connections(1)
            .max_connections(5)
            .acquire_timeout(Duration::from_secs(10))
            .after_connect(|connection, _metadata| {
                Box::pin(async move {
                    sqlx::query("PRAGMA foreign_keys = ON")
                        .execute(&mut *connection)
                        .await?;
                    sqlx::query("PRAGMA busy_timeout = 5000")
                        .execute(&mut *connection)
                        .await?;
                    Ok(())
                })
            })
            .connect_with(options)
            .await
            .map_err(storage)?;
        MIGRATOR.run(&pool).await.map_err(storage)?;
        let repository = Self { pool };
        repository.backfill_search_index().await?;
        Ok(repository)
    }

    /// Indexes any content rows the search index does not know yet — the one
    /// bridge existing databases need, since normalization and kana folding
    /// happen in Rust and cannot run inside a SQL migration.
    async fn backfill_search_index(&self) -> Result<(), RepositoryError> {
        let rows = sqlx::query(
            "SELECT id, title, body_html FROM contents
             WHERE id NOT IN (SELECT rowid FROM search_index)",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(storage)?;
        if rows.is_empty() {
            return Ok(());
        }
        let mut transaction = self.pool.begin().await.map_err(storage)?;
        for row in rows {
            let id: i64 = row.try_get("id").map_err(storage)?;
            let title: String = row.try_get("title").map_err(storage)?;
            let body_html: String = row.try_get("body_html").map_err(storage)?;
            index_search_document(
                &mut transaction,
                ContentId::from_i64(id),
                &title,
                &body_html,
            )
            .await?;
        }
        transaction.commit().await.map_err(storage)
    }

    #[must_use]
    pub const fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    pub async fn close(&self) {
        self.pool.close().await;
    }

    pub async fn pragmas(&self) -> Result<SqlitePragmas, RepositoryError> {
        let mut connection = self.pool.acquire().await.map_err(storage)?;
        let foreign_keys: i64 = sqlx::query_scalar("PRAGMA foreign_keys")
            .fetch_one(&mut *connection)
            .await
            .map_err(storage)?;
        let journal_mode: String = sqlx::query_scalar("PRAGMA journal_mode")
            .fetch_one(&mut *connection)
            .await
            .map_err(storage)?;
        let busy_timeout_ms: i64 = sqlx::query_scalar("PRAGMA busy_timeout")
            .fetch_one(&mut *connection)
            .await
            .map_err(storage)?;
        Ok(SqlitePragmas {
            foreign_keys: foreign_keys == 1,
            journal_mode,
            busy_timeout_ms,
        })
    }

    async fn hydrate(&self, row: ContentRow) -> Result<Content, RepositoryError> {
        let tags = load_tags(&self.pool, ContentId::from_i64(row.id)).await?;
        row.into_content(tags)
    }

    /// Hydrates a list with one tag query instead of one per row: the whole
    /// `content_tags` join is small (at most twenty tags per piece) and reading
    /// it once keeps a save from costing as many pool acquisitions as there
    /// are pieces on the site.
    async fn hydrate_many(&self, rows: Vec<ContentRow>) -> Result<Vec<Content>, RepositoryError> {
        if rows.len() <= 1 {
            let mut contents = Vec::with_capacity(rows.len());
            for row in rows {
                contents.push(self.hydrate(row).await?);
            }
            return Ok(contents);
        }
        let mut tags_by_content = load_all_tags(&self.pool).await?;
        rows.into_iter()
            .map(|row| {
                let tags = tags_by_content.remove(&row.id).unwrap_or_default();
                row.into_content(tags)
            })
            .collect()
    }
}

#[async_trait]
impl ContentRepository for SqliteRepository {
    async fn create(
        &self,
        prepared: PreparedContent,
        intent: SaveIntent,
        now: DateTime<Utc>,
    ) -> Result<Content, RepositoryError> {
        let mut transaction = self.pool.begin().await.map_err(storage)?;
        ensure_slug_available(&mut transaction, &prepared.draft.slug, None).await?;
        let (status, publish_at) = publication_columns(&prepared.draft.publication);
        let result = sqlx::query(
            "INSERT INTO contents (
                kind, title, slug, summary, body_markdown, body_html, status, publish_at,
                cover_media_id, seo_title, seo_description, version, created_at, updated_at
             ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 1, ?, ?)",
        )
        .bind(prepared.draft.kind.as_str())
        .bind(&prepared.draft.title)
        .bind(prepared.draft.slug.as_str())
        .bind(&prepared.draft.summary)
        .bind(&prepared.draft.body_markdown)
        .bind(&prepared.body_html)
        .bind(status)
        .bind(publish_at)
        .bind(&prepared.draft.cover_media_id)
        .bind(&prepared.draft.seo_title)
        .bind(&prepared.draft.seo_description)
        .bind(now)
        .bind(now)
        .execute(&mut *transaction)
        .await
        .map_err(|error| map_write_error(error, &prepared.draft.slug))?;
        let id = ContentId::from_i64(result.last_insert_rowid());
        replace_tags(&mut transaction, id, &prepared.tags).await?;
        let content = content_from_prepared(id, prepared, 1, now, now);
        insert_revision(&mut transaction, &content, intent, now).await?;
        index_search_document(&mut transaction, id, &content.title, &content.body_html).await?;
        refresh_publication_state(
            &mut transaction,
            now,
            content.publication.is_visible_at(now),
        )
        .await?;
        transaction.commit().await.map_err(storage)?;
        Ok(content)
    }

    async fn update(
        &self,
        id: ContentId,
        expected_version: i64,
        prepared: PreparedContent,
        intent: SaveIntent,
        now: DateTime<Utc>,
    ) -> Result<Content, RepositoryError> {
        let mut transaction = self.pool.begin().await.map_err(storage)?;
        let UpdateTarget {
            old_slug,
            created_at,
            was_visible,
        } = load_update_target(&mut transaction, id, expected_version, now).await?;
        if old_slug != prepared.draft.slug.as_str() {
            ensure_slug_available(&mut transaction, &prepared.draft.slug, Some(id)).await?;
            sqlx::query("DELETE FROM redirects WHERE old_slug = ? AND content_id = ?")
                .bind(prepared.draft.slug.as_str())
                .bind(id.as_i64())
                .execute(&mut *transaction)
                .await
                .map_err(storage)?;
        }

        let (status, publish_at) = publication_columns(&prepared.draft.publication);
        let result = sqlx::query(
            "UPDATE contents SET
                kind = ?, title = ?, slug = ?, summary = ?, body_markdown = ?, body_html = ?,
                status = ?, publish_at = ?, cover_media_id = ?, seo_title = ?, seo_description = ?,
                version = version + 1, updated_at = ?
             WHERE id = ? AND version = ?",
        )
        .bind(prepared.draft.kind.as_str())
        .bind(&prepared.draft.title)
        .bind(prepared.draft.slug.as_str())
        .bind(&prepared.draft.summary)
        .bind(&prepared.draft.body_markdown)
        .bind(&prepared.body_html)
        .bind(status)
        .bind(publish_at)
        .bind(&prepared.draft.cover_media_id)
        .bind(&prepared.draft.seo_title)
        .bind(&prepared.draft.seo_description)
        .bind(now)
        .bind(id.as_i64())
        .bind(expected_version)
        .execute(&mut *transaction)
        .await
        .map_err(|error| map_write_error(error, &prepared.draft.slug))?;
        if result.rows_affected() != 1 {
            let actual = sqlx::query_scalar("SELECT version FROM contents WHERE id = ?")
                .bind(id.as_i64())
                .fetch_optional(&mut *transaction)
                .await
                .map_err(storage)?;
            return Err(RepositoryError::Conflict {
                expected: expected_version,
                actual,
            });
        }

        if old_slug != prepared.draft.slug.as_str() {
            sqlx::query(
                "INSERT INTO redirects (old_slug, content_id, created_at)
                 VALUES (?, ?, ?)
                 ON CONFLICT(old_slug) DO UPDATE SET content_id = excluded.content_id",
            )
            .bind(old_slug)
            .bind(id.as_i64())
            .bind(now)
            .execute(&mut *transaction)
            .await
            .map_err(storage)?;
        }
        replace_tags(&mut transaction, id, &prepared.tags).await?;
        let content = content_from_prepared(id, prepared, expected_version + 1, created_at, now);
        insert_revision(&mut transaction, &content, intent, now).await?;
        index_search_document(&mut transaction, id, &content.title, &content.body_html).await?;
        if intent == SaveIntent::Autosave {
            prune_autosaves(&mut transaction, id).await?;
        }
        refresh_publication_state(
            &mut transaction,
            now,
            was_visible || content.publication.is_visible_at(now),
        )
        .await?;
        transaction.commit().await.map_err(storage)?;
        Ok(content)
    }

    async fn find_by_id(&self, id: ContentId) -> Result<Option<Content>, RepositoryError> {
        let row = sqlx::query_as::<_, ContentRow>("SELECT * FROM contents WHERE id = ?")
            .bind(id.as_i64())
            .fetch_optional(&self.pool)
            .await
            .map_err(storage)?;
        match row {
            Some(row) => self.hydrate(row).await.map(Some),
            None => Ok(None),
        }
    }

    async fn find_public_by_slug(
        &self,
        slug: &Slug,
        now: DateTime<Utc>,
    ) -> Result<Option<Content>, RepositoryError> {
        let row = sqlx::query_as::<_, ContentRow>(
            "SELECT * FROM contents
             WHERE slug = ? AND status = 'public' AND publish_at <= ? AND deleted_at IS NULL",
        )
        .bind(slug.as_str())
        .bind(now)
        .fetch_optional(&self.pool)
        .await
        .map_err(storage)?;
        match row {
            Some(row) => self.hydrate(row).await.map(Some),
            None => Ok(None),
        }
    }

    async fn resolve_redirect(&self, old_slug: &Slug) -> Result<Option<Slug>, RepositoryError> {
        let target: Option<String> = sqlx::query_scalar(
            "SELECT contents.slug FROM redirects
             JOIN contents ON contents.id = redirects.content_id
             WHERE redirects.old_slug = ? AND contents.deleted_at IS NULL",
        )
        .bind(old_slug.as_str())
        .fetch_optional(&self.pool)
        .await
        .map_err(storage)?;
        target
            .map(Slug::parse)
            .transpose()
            .map_err(|_| RepositoryError::Storage("database contains an invalid slug".into()))
    }

    async fn list_revisions(
        &self,
        content_id: ContentId,
    ) -> Result<Vec<ContentRevision>, RepositoryError> {
        let rows = sqlx::query(
            "SELECT id, intent, snapshot_json, created_at FROM revisions
             WHERE content_id = ? ORDER BY created_at DESC, id DESC",
        )
        .bind(content_id.as_i64())
        .fetch_all(&self.pool)
        .await
        .map_err(storage)?;
        rows.into_iter()
            .map(|row| {
                let intent: String = row.try_get("intent").map_err(storage)?;
                let snapshot: String = row.try_get("snapshot_json").map_err(storage)?;
                Ok(ContentRevision {
                    id: row.try_get("id").map_err(storage)?,
                    content_id,
                    intent: SaveIntent::from_str(&intent)
                        .map_err(|error| RepositoryError::Storage(error.to_owned()))?,
                    snapshot: serde_json::from_str(&snapshot).map_err(storage)?,
                    created_at: row.try_get("created_at").map_err(storage)?,
                })
            })
            .collect()
    }

    async fn find_revision(
        &self,
        content_id: ContentId,
        revision_id: i64,
    ) -> Result<Option<ContentRevision>, RepositoryError> {
        let row = sqlx::query(
            "SELECT id, intent, snapshot_json, created_at FROM revisions
             WHERE content_id = ? AND id = ?",
        )
        .bind(content_id.as_i64())
        .bind(revision_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(storage)?;
        row.map(|row| {
            let intent: String = row.try_get("intent").map_err(storage)?;
            let snapshot: String = row.try_get("snapshot_json").map_err(storage)?;
            Ok(ContentRevision {
                id: row.try_get("id").map_err(storage)?,
                content_id,
                intent: SaveIntent::from_str(&intent)
                    .map_err(|error| RepositoryError::Storage(error.to_owned()))?,
                snapshot: serde_json::from_str(&snapshot).map_err(storage)?,
                created_at: row.try_get("created_at").map_err(storage)?,
            })
        })
        .transpose()
    }

    async fn list_public_posts(
        &self,
        now: DateTime<Utc>,
        limit: u32,
        offset: u32,
    ) -> Result<Vec<Content>, RepositoryError> {
        let rows = sqlx::query_as::<_, ContentRow>(
            "SELECT * FROM contents
             WHERE kind = 'post' AND status = 'public' AND publish_at <= ?
               AND deleted_at IS NULL
             ORDER BY publish_at DESC, id DESC LIMIT ? OFFSET ?",
        )
        .bind(now)
        .bind(i64::from(limit.min(100)))
        .bind(i64::from(offset))
        .fetch_all(&self.pool)
        .await
        .map_err(storage)?;
        self.hydrate_many(rows).await
    }

    async fn list_public_by_tag(
        &self,
        tag: &Slug,
        now: DateTime<Utc>,
    ) -> Result<Vec<Content>, RepositoryError> {
        let rows = sqlx::query_as::<_, ContentRow>(
            "SELECT contents.* FROM contents
             JOIN content_tags ON content_tags.content_id = contents.id
             JOIN tags ON tags.id = content_tags.tag_id
             WHERE tags.slug = ? AND contents.status = 'public' AND contents.publish_at <= ?
               AND contents.deleted_at IS NULL
             ORDER BY contents.publish_at DESC, contents.id DESC",
        )
        .bind(tag.as_str())
        .bind(now)
        .fetch_all(&self.pool)
        .await
        .map_err(storage)?;
        self.hydrate_many(rows).await
    }

    async fn list_all_public(&self, now: DateTime<Utc>) -> Result<Vec<Content>, RepositoryError> {
        let rows = sqlx::query_as::<_, ContentRow>(
            "SELECT * FROM contents
             WHERE status = 'public' AND publish_at <= ? AND deleted_at IS NULL
             ORDER BY publish_at DESC, id DESC",
        )
        .bind(now)
        .fetch_all(&self.pool)
        .await
        .map_err(storage)?;
        self.hydrate_many(rows).await
    }

    async fn list_all_content(&self) -> Result<Vec<Content>, RepositoryError> {
        let rows = sqlx::query_as::<_, ContentRow>(
            "SELECT * FROM contents ORDER BY updated_at DESC, id DESC",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(storage)?;
        self.hydrate_many(rows).await
    }

    async fn neighbor_posts(
        &self,
        id: ContentId,
        publish_at: DateTime<Utc>,
        now: DateTime<Utc>,
    ) -> Result<(Option<ContentLink>, Option<ContentLink>), RepositoryError> {
        // Ties on publish_at are broken by id so the ordering is total and
        // the chain never skips or repeats an entry.
        let older = self
            .neighbor(
                "SELECT id, slug, title FROM contents
                 WHERE kind = 'post' AND status = 'public' AND publish_at <= ?
                   AND deleted_at IS NULL
                   AND (publish_at < ? OR (publish_at = ? AND id < ?))
                 ORDER BY publish_at DESC, id DESC LIMIT 1",
                id,
                publish_at,
                now,
            )
            .await?;
        let newer = self
            .neighbor(
                "SELECT id, slug, title FROM contents
                 WHERE kind = 'post' AND status = 'public' AND publish_at <= ?
                   AND deleted_at IS NULL
                   AND (publish_at > ? OR (publish_at = ? AND id > ?))
                 ORDER BY publish_at ASC, id ASC LIMIT 1",
                id,
                publish_at,
                now,
            )
            .await?;
        Ok((older, newer))
    }

    async fn move_to_trash(
        &self,
        id: ContentId,
        expected_version: i64,
        now: DateTime<Utc>,
    ) -> Result<Content, RepositoryError> {
        let mut transaction = self.pool.begin().await.map_err(storage)?;
        let current = load_content_in(&mut transaction, id)
            .await?
            .ok_or(RepositoryError::NotFound)?;
        if current.is_trashed() {
            transaction.commit().await.map_err(storage)?;
            return Ok(current);
        }
        if current.version != expected_version {
            return Err(RepositoryError::Conflict {
                expected: expected_version,
                actual: Some(current.version),
            });
        }
        let was_visible = current.publication.is_visible_at(now);
        let result = sqlx::query(
            "UPDATE contents SET deleted_at = ?, updated_at = ?, version = version + 1
             WHERE id = ? AND version = ? AND deleted_at IS NULL",
        )
        .bind(now)
        .bind(now)
        .bind(id.as_i64())
        .bind(expected_version)
        .execute(&mut *transaction)
        .await
        .map_err(storage)?;
        if result.rows_affected() != 1 {
            return Err(RepositoryError::Conflict {
                expected: expected_version,
                actual: None,
            });
        }
        refresh_publication_state(&mut transaction, now, was_visible).await?;
        let trashed = load_content_in(&mut transaction, id)
            .await?
            .ok_or(RepositoryError::NotFound)?;
        transaction.commit().await.map_err(storage)?;
        tracing::info!(event = "content.trashed", content_id = %id, was_visible);
        Ok(trashed)
    }

    async fn restore_from_trash(
        &self,
        id: ContentId,
        now: DateTime<Utc>,
    ) -> Result<Content, RepositoryError> {
        let mut transaction = self.pool.begin().await.map_err(storage)?;
        let current = load_content_in(&mut transaction, id)
            .await?
            .ok_or(RepositoryError::NotFound)?;
        if !current.is_trashed() {
            transaction.commit().await.map_err(storage)?;
            return Ok(current);
        }
        sqlx::query(
            "UPDATE contents SET deleted_at = NULL, updated_at = ?, version = version + 1
             WHERE id = ?",
        )
        .bind(now)
        .bind(id.as_i64())
        .execute(&mut *transaction)
        .await
        .map_err(storage)?;
        let visible_again = current.publication.is_visible_at(now);
        refresh_publication_state(&mut transaction, now, visible_again).await?;
        let restored = load_content_in(&mut transaction, id)
            .await?
            .ok_or(RepositoryError::NotFound)?;
        transaction.commit().await.map_err(storage)?;
        tracing::info!(event = "content.restored", content_id = %id, visible_again);
        Ok(restored)
    }

    async fn delete_permanently(&self, id: ContentId) -> Result<(), RepositoryError> {
        let mut transaction = self.pool.begin().await.map_err(storage)?;
        let result = sqlx::query("DELETE FROM contents WHERE id = ? AND deleted_at IS NOT NULL")
            .bind(id.as_i64())
            .execute(&mut *transaction)
            .await
            .map_err(storage)?;
        if result.rows_affected() != 1 {
            return Err(RepositoryError::NotFound);
        }
        // The FTS table has no foreign key; everything else cascades.
        sqlx::query("DELETE FROM search_index WHERE rowid = ?")
            .bind(id.as_i64())
            .execute(&mut *transaction)
            .await
            .map_err(storage)?;
        transaction.commit().await.map_err(storage)?;
        tracing::info!(event = "content.deleted_permanently", content_id = %id);
        Ok(())
    }
}

/// What an update needs to know about the row it is about to replace.
struct UpdateTarget {
    old_slug: String,
    created_at: DateTime<Utc>,
    was_visible: bool,
}

/// Reads the current row and applies the guards every edit shares: the piece
/// must exist, must not sit in the trash, and must still be at the version
/// the editor last saw.
async fn load_update_target(
    transaction: &mut Transaction<'_, Sqlite>,
    id: ContentId,
    expected_version: i64,
    now: DateTime<Utc>,
) -> Result<UpdateTarget, RepositoryError> {
    let current = sqlx::query(
        "SELECT slug, version, created_at, status, publish_at, deleted_at
         FROM contents WHERE id = ?",
    )
    .bind(id.as_i64())
    .fetch_optional(&mut **transaction)
    .await
    .map_err(storage)?
    .ok_or(RepositoryError::NotFound)?;
    let deleted_at: Option<DateTime<Utc>> = current.try_get("deleted_at").map_err(storage)?;
    if deleted_at.is_some() {
        return Err(RepositoryError::Validation(
            "content is in the trash; restore it before editing".into(),
        ));
    }
    let actual_version: i64 = current.try_get("version").map_err(storage)?;
    if actual_version != expected_version {
        return Err(RepositoryError::Conflict {
            expected: expected_version,
            actual: Some(actual_version),
        });
    }
    let current_status: String = current.try_get("status").map_err(storage)?;
    let current_publish_at: Option<DateTime<Utc>> =
        current.try_get("publish_at").map_err(storage)?;
    Ok(UpdateTarget {
        old_slug: current.try_get("slug").map_err(storage)?,
        created_at: current.try_get("created_at").map_err(storage)?,
        was_visible: current_status == "public"
            && current_publish_at.is_some_and(|publish_at| publish_at <= now),
    })
}

/// One piece with its tags, read inside the caller's transaction.
async fn load_content_in(
    transaction: &mut Transaction<'_, Sqlite>,
    id: ContentId,
) -> Result<Option<Content>, RepositoryError> {
    let row = sqlx::query_as::<_, ContentRow>("SELECT * FROM contents WHERE id = ?")
        .bind(id.as_i64())
        .fetch_optional(&mut **transaction)
        .await
        .map_err(storage)?;
    let Some(row) = row else {
        return Ok(None);
    };
    let tags = snapshot_tags(transaction, id).await?;
    row.into_content(tags).map(Some)
}

/// Rewrites the search index row for one content, inside the same transaction
/// as the content write so index and content can never drift.
async fn index_search_document(
    transaction: &mut Transaction<'_, Sqlite>,
    id: ContentId,
    title: &str,
    body_html: &str,
) -> Result<(), RepositoryError> {
    let title = search::normalize(title);
    let body = search::normalize(&search::html_to_text(body_html));
    let title_fold = search::fold(&title);
    let body_fold = search::fold(&body);
    // FTS5 tables have no upsert; delete-then-insert is the documented idiom.
    sqlx::query("DELETE FROM search_index WHERE rowid = ?")
        .bind(id.as_i64())
        .execute(&mut **transaction)
        .await
        .map_err(storage)?;
    sqlx::query(
        "INSERT INTO search_index (rowid, title_fold, body_fold, title, body)
         VALUES (?, ?, ?, ?, ?)",
    )
    .bind(id.as_i64())
    .bind(&title_fold)
    .bind(&body_fold)
    .bind(&title)
    .bind(&body)
    .execute(&mut **transaction)
    .await
    .map_err(storage)?;
    Ok(())
}

/// Builds the hybrid search statement for one parsed query: an optional FTS
/// MATCH for trigram-capable terms, one LIKE group per short term, and a
/// ranking clause that prefers bm25 (title-weighted) when FTS took part.
fn build_search_sql(terms: &SearchTerms) -> AssertSqlSafe<String> {
    // The FTS table keeps its real name: SQLite rejects MATCH through an
    // alias ("no such column") in a plain join like this.
    let mut sql = String::from(
        "SELECT c.slug, search_index.title, search_index.body, c.kind, c.publish_at
         FROM search_index JOIN contents c ON c.id = search_index.rowid
         WHERE c.status = 'public' AND c.publish_at <= ? AND c.deleted_at IS NULL",
    );
    if !terms.fts.is_empty() {
        sql.push_str(" AND search_index MATCH ?");
    }
    for _ in &terms.like {
        sql.push_str(
            " AND (search_index.title_fold LIKE ? ESCAPE '\\' OR search_index.body_fold LIKE ? ESCAPE '\\')",
        );
    }
    if terms.fts.is_empty() {
        // No bm25 without a MATCH: rank by how much of the query hits the
        // title, then by recency.
        sql.push_str(" ORDER BY (0");
        for _ in &terms.like {
            sql.push_str(" + (search_index.title_fold LIKE ? ESCAPE '\\')");
        }
        sql.push_str(") DESC, c.publish_at DESC");
    } else {
        sql.push_str(" ORDER BY bm25(search_index, 4.0, 1.0), c.publish_at DESC");
    }
    sql.push_str(" LIMIT ?");
    // SQL SAFETY: this function appends only the literal fragments above.
    // Search terms are never interpolated; `search` binds every value below.
    AssertSqlSafe(sql)
}

#[async_trait]
impl SearchRepository for SqliteRepository {
    /// Hybrid CJK-first search: terms of three or more characters go through
    /// the trigram FTS index (with bm25 ranking, title weighted above body);
    /// one- and two-character terms — most Japanese words — are LIKE filters
    /// over the folded columns. Both kinds combine as AND.
    async fn search(
        &self,
        terms: &SearchTerms,
        now: DateTime<Utc>,
        limit: u32,
    ) -> Result<Vec<SearchHit>, RepositoryError> {
        if terms.is_empty() {
            return Ok(Vec::new());
        }
        let mut query = sqlx::query(build_search_sql(terms)).bind(now);
        if !terms.fts.is_empty() {
            let match_expression = terms
                .fts
                .iter()
                .map(|term| search::quote_fts(term))
                .collect::<Vec<_>>()
                .join(" ");
            query = query.bind(match_expression);
        }
        let like_patterns: Vec<String> = terms
            .like
            .iter()
            .map(|term| format!("%{}%", search::escape_like(term)))
            .collect();
        for pattern in &like_patterns {
            query = query.bind(pattern).bind(pattern);
        }
        if terms.fts.is_empty() {
            for pattern in &like_patterns {
                query = query.bind(pattern);
            }
        }
        let rows = query
            .bind(i64::from(limit.min(100)))
            .fetch_all(&self.pool)
            .await
            .map_err(storage)?;

        rows.into_iter()
            .map(|row| {
                let slug: String = row.try_get("slug").map_err(storage)?;
                let kind: String = row.try_get("kind").map_err(storage)?;
                Ok(SearchHit {
                    slug: Slug::parse(&slug)
                        .map_err(|error| RepositoryError::Storage(error.to_string()))?,
                    title: row.try_get("title").map_err(storage)?,
                    body: row.try_get("body").map_err(storage)?,
                    kind: match kind.as_str() {
                        "page" => ContentKind::Page,
                        _ => ContentKind::Post,
                    },
                    publish_at: row.try_get("publish_at").map_err(storage)?,
                })
            })
            .collect()
    }
}

#[async_trait]
impl EngagementRepository for SqliteRepository {
    async fn record_view(&self, id: ContentId) -> Result<(), RepositoryError> {
        sqlx::query(
            "INSERT INTO content_views (content_id, view_count) VALUES (?, 1)
             ON CONFLICT(content_id) DO UPDATE SET view_count = view_count + 1",
        )
        .bind(id.as_i64())
        .execute(&self.pool)
        .await
        .map_err(storage)?;
        Ok(())
    }

    async fn engagement_totals(
        &self,
    ) -> Result<std::collections::HashMap<ContentId, Engagement>, RepositoryError> {
        let rows = sqlx::query(
            "SELECT c.id,
                    COALESCE(l.like_count, 0) AS likes,
                    COALESCE(v.view_count, 0) AS views
             FROM contents c
             LEFT JOIN content_likes l ON l.content_id = c.id
             LEFT JOIN content_views v ON v.content_id = c.id",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(storage)?;
        rows.into_iter()
            .map(|row| {
                let id: i64 = row.try_get("id").map_err(storage)?;
                let likes: i64 = row.try_get("likes").map_err(storage)?;
                let views: i64 = row.try_get("views").map_err(storage)?;
                Ok((
                    ContentId::from_i64(id),
                    Engagement {
                        likes: u64::try_from(likes).unwrap_or_default(),
                        views: u64::try_from(views).unwrap_or_default(),
                    },
                ))
            })
            .collect()
    }
}

impl SqliteRepository {
    async fn neighbor(
        &self,
        sql: &'static str,
        id: ContentId,
        publish_at: DateTime<Utc>,
        now: DateTime<Utc>,
    ) -> Result<Option<ContentLink>, RepositoryError> {
        let row = sqlx::query(sql)
            .bind(now)
            .bind(publish_at)
            .bind(publish_at)
            .bind(id.as_i64())
            .fetch_optional(&self.pool)
            .await
            .map_err(storage)?;
        row.map(|row| {
            let slug: String = row.try_get("slug").map_err(storage)?;
            Ok(ContentLink {
                id: ContentId::from_i64(row.try_get("id").map_err(storage)?),
                slug: Slug::parse(&slug)
                    .map_err(|error| RepositoryError::Storage(error.to_string()))?,
                title: row.try_get("title").map_err(storage)?,
            })
        })
        .transpose()
    }

    async fn content_is_visible(
        &self,
        id: ContentId,
        now: DateTime<Utc>,
    ) -> Result<bool, RepositoryError> {
        let visible: i64 = sqlx::query_scalar(
            "SELECT EXISTS (
                SELECT 1 FROM contents
                WHERE id = ? AND status = 'public' AND publish_at <= ? AND deleted_at IS NULL
             )",
        )
        .bind(id.as_i64())
        .bind(now)
        .fetch_one(&self.pool)
        .await
        .map_err(storage)?;
        Ok(visible == 1)
    }
}

#[async_trait]
impl LikeRepository for SqliteRepository {
    async fn add_like(&self, id: ContentId, now: DateTime<Utc>) -> Result<u64, RepositoryError> {
        // The inner SELECT gates the upsert on public visibility, so an
        // invisible id inserts nothing and RETURNING stays empty.
        let count: Option<i64> = sqlx::query_scalar(
            "INSERT INTO content_likes (content_id, like_count)
             SELECT id, 1 FROM contents
             WHERE id = ? AND status = 'public' AND publish_at <= ? AND deleted_at IS NULL
             ON CONFLICT(content_id) DO UPDATE SET like_count = like_count + 1
             RETURNING like_count",
        )
        .bind(id.as_i64())
        .bind(now)
        .fetch_optional(&self.pool)
        .await
        .map_err(storage)?;
        count.map_or(Err(RepositoryError::NotFound), |value| {
            Ok(unsigned_count(value))
        })
    }

    async fn remove_like(&self, id: ContentId, now: DateTime<Utc>) -> Result<u64, RepositoryError> {
        if !self.content_is_visible(id, now).await? {
            return Err(RepositoryError::NotFound);
        }
        let count: Option<i64> = sqlx::query_scalar(
            "UPDATE content_likes SET like_count = MAX(like_count - 1, 0)
             WHERE content_id = ?
             RETURNING like_count",
        )
        .bind(id.as_i64())
        .fetch_optional(&self.pool)
        .await
        .map_err(storage)?;
        Ok(count.map_or(0, unsigned_count))
    }

    async fn like_count(&self, id: ContentId, now: DateTime<Utc>) -> Result<u64, RepositoryError> {
        if !self.content_is_visible(id, now).await? {
            return Err(RepositoryError::NotFound);
        }
        let count: Option<i64> =
            sqlx::query_scalar("SELECT like_count FROM content_likes WHERE content_id = ?")
                .bind(id.as_i64())
                .fetch_optional(&self.pool)
                .await
                .map_err(storage)?;
        Ok(count.map_or(0, unsigned_count))
    }
}

fn unsigned_count(value: i64) -> u64 {
    // like_count carries a CHECK (>= 0); a negative value cannot be read back.
    u64::try_from(value).unwrap_or_default()
}

#[async_trait]
impl SiteRepository for SqliteRepository {
    async fn site_settings(&self) -> Result<SiteSettings, RepositoryError> {
        let row = sqlx::query(SITE_SETTINGS_SELECT)
            .fetch_one(&self.pool)
            .await
            .map_err(storage)?;
        settings_from_row(&row)
    }

    async fn navigation(&self) -> Result<Vec<NavigationItem>, RepositoryError> {
        let rows = sqlx::query(
            "SELECT id, label, destination, is_external, position
             FROM navigation ORDER BY position",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(storage)?;
        rows.into_iter()
            .map(|row| {
                let position: i64 = row.try_get("position").map_err(storage)?;
                Ok(NavigationItem {
                    id: row.try_get("id").map_err(storage)?,
                    label: row.try_get("label").map_err(storage)?,
                    destination: row.try_get("destination").map_err(storage)?,
                    is_external: row.try_get::<i64, _>("is_external").map_err(storage)? == 1,
                    position: u16::try_from(position).map_err(storage)?,
                })
            })
            .collect()
    }

    async fn save_configuration(
        &self,
        settings: &SiteSettings,
        navigation: &[NavigationItem],
        now: DateTime<Utc>,
    ) -> Result<(), RepositoryError> {
        let mut transaction = self.pool.begin().await.map_err(storage)?;
        sqlx::query(
            "UPDATE site_settings SET
                site_title = ?, site_description = ?, locale = ?, logo_media_id = ?,
                favicon_media_id = ?, custom_css = ?, timezone = ?, author_name = ?,
                custom_css_backup = ?, updated_at = ?
             WHERE singleton = 1",
        )
        .bind(&settings.site_title)
        .bind(&settings.site_description)
        .bind(settings.locale.as_str())
        .bind(&settings.logo_media_id)
        .bind(&settings.favicon_media_id)
        .bind(&settings.custom_css)
        .bind(&settings.timezone)
        .bind(&settings.author_name)
        .bind(&settings.custom_css_backup)
        .bind(now)
        .execute(&mut *transaction)
        .await
        .map_err(storage)?;
        sqlx::query("DELETE FROM navigation")
            .execute(&mut *transaction)
            .await
            .map_err(storage)?;
        for item in navigation {
            sqlx::query(
                "INSERT INTO navigation (label, destination, is_external, position)
                 VALUES (?, ?, ?, ?)",
            )
            .bind(&item.label)
            .bind(&item.destination)
            .bind(i64::from(item.is_external))
            .bind(i64::from(item.position))
            .execute(&mut *transaction)
            .await
            .map_err(storage)?;
        }
        refresh_publication_state(&mut transaction, now, true).await?;
        transaction.commit().await.map_err(storage)?;
        Ok(())
    }
}

#[async_trait]
impl PublicSnapshotRepository for SqliteRepository {
    async fn publication_state(&self) -> Result<PublicationState, RepositoryError> {
        let row = sqlx::query(
            "SELECT public_revision, next_publish_at
             FROM publication_state WHERE singleton = 1",
        )
        .fetch_one(&self.pool)
        .await
        .map_err(storage)?;
        publication_state_from_row(&row)
    }

    #[tracing::instrument(name = "publication.clock.advance", skip_all, fields(now = %now))]
    async fn advance_publication_clock(&self, now: DateTime<Utc>) -> Result<bool, RepositoryError> {
        let mut transaction = self.pool.begin().await.map_err(storage)?;
        let next: Option<DateTime<Utc>> =
            sqlx::query_scalar("SELECT next_publish_at FROM publication_state WHERE singleton = 1")
                .fetch_one(&mut *transaction)
                .await
                .map_err(storage)?;
        if next.is_none_or(|next| next > now) {
            transaction.commit().await.map_err(storage)?;
            tracing::debug!(event = "publication.clock.unchanged");
            return Ok(false);
        }
        refresh_publication_state(&mut transaction, now, true).await?;
        transaction.commit().await.map_err(storage)?;
        tracing::info!(event = "publication.clock.advanced");
        Ok(true)
    }

    #[tracing::instrument(
        name = "publication.snapshot.load",
        skip_all,
        fields(effective_at = %effective_at)
    )]
    async fn public_snapshot(
        &self,
        effective_at: DateTime<Utc>,
    ) -> Result<SiteSnapshotV1, RepositoryError> {
        let mut transaction = self.pool.begin().await.map_err(storage)?;

        // This first read establishes the WAL snapshot. Every subsequent row
        // belongs to the same logical publication revision.
        let state_row = sqlx::query(
            "SELECT public_revision, next_publish_at
             FROM publication_state WHERE singleton = 1",
        )
        .fetch_one(&mut *transaction)
        .await
        .map_err(storage)?;
        let state = publication_state_from_row(&state_row)?;
        let settings = snapshot_settings(&mut transaction).await?;
        let navigation = snapshot_navigation(&mut transaction).await?;
        let contents = snapshot_contents(&mut transaction, effective_at).await?;
        let redirects = snapshot_redirects(&mut transaction, effective_at).await?;
        let media = snapshot_media(&mut transaction).await?;
        transaction.commit().await.map_err(storage)?;

        tracing::info!(
            event = "publication.snapshot.loaded",
            public_revision = state.revision,
            content_count = contents.len(),
            redirect_count = redirects.len(),
            media_count = media.len()
        );
        Ok(SiteSnapshotV1 {
            public_revision: state.revision,
            effective_at,
            settings,
            navigation,
            contents,
            redirects,
            media,
        })
    }
}

#[async_trait]
impl PortableRepository for SqliteRepository {
    #[tracing::instrument(
        name = "portable.snapshot.load",
        skip_all,
        fields(exported_at = %exported_at)
    )]
    async fn portable_site(
        &self,
        canonical_origin: &str,
        exported_at: DateTime<Utc>,
    ) -> Result<PortableSiteV1, RepositoryError> {
        let mut transaction = self.pool.begin().await.map_err(storage)?;
        let settings = snapshot_settings(&mut transaction).await?;
        let navigation = snapshot_navigation(&mut transaction).await?;
        let contents = portable_contents(&mut transaction).await?;
        let redirects = portable_redirects(&mut transaction).await?;
        let media = snapshot_media(&mut transaction).await?;
        let engagement = portable_engagement(&mut transaction).await?;
        let owner = portable_owner(&mut transaction).await?;
        let publication = portable_publication(&mut transaction).await?;
        transaction.commit().await.map_err(storage)?;
        let site = PortableSiteV1 {
            format_version: crate::portable::PORTABLE_SITE_FORMAT_VERSION,
            exported_at,
            canonical_origin: canonical_origin.to_owned(),
            settings,
            navigation,
            contents,
            redirects,
            media,
            engagement,
            owner,
            publication,
        };
        site.validate()
            .map_err(|error| RepositoryError::Storage(error.to_string()))?;
        tracing::info!(
            event = "portable.snapshot.loaded",
            content_count = site.contents.len(),
            revision_count = site
                .contents
                .iter()
                .map(|content| content.revisions.len())
                .sum::<usize>(),
            media_count = site.media.len(),
            has_owner = site.owner.is_some()
        );
        Ok(site)
    }

    #[tracing::instrument(
        name = "portable.snapshot.replace",
        skip_all,
        fields(
            content_count = site.contents.len(),
            media_count = site.media.len(),
            public_revision = site.publication.public_revision
        ),
        err
    )]
    async fn replace_portable_site(
        &self,
        site: &PortableSiteV1,
        markdown: &dyn crate::application::ports::MarkdownRenderer,
    ) -> Result<(), RepositoryError> {
        site.validate()
            .map_err(|error| RepositoryError::Validation(error.to_string()))?;
        let mut transaction = self.pool.begin().await.map_err(storage)?;
        clear_portable_state(&mut transaction).await?;
        insert_portable_media(&mut transaction, &site.media).await?;
        insert_portable_settings(&mut transaction, site).await?;
        insert_portable_contents(&mut transaction, &site.contents, markdown).await?;
        insert_portable_redirects(&mut transaction, &site.redirects).await?;
        insert_portable_engagement(&mut transaction, &site.engagement).await?;
        insert_portable_owner(&mut transaction, site.owner.as_ref()).await?;
        sqlx::query(
            "UPDATE publication_state SET public_revision = ?, next_publish_at = ?, updated_at = ?
             WHERE singleton = 1",
        )
        .bind(i64::try_from(site.publication.public_revision).map_err(storage)?)
        .bind(site.publication.next_publish_at)
        .bind(site.exported_at)
        .execute(&mut *transaction)
        .await
        .map_err(storage)?;
        transaction.commit().await.map_err(storage)?;
        tracing::info!(event = "portable.snapshot.replaced");
        Ok(())
    }
}

async fn portable_contents(
    transaction: &mut Transaction<'_, Sqlite>,
) -> Result<Vec<PortableContent>, RepositoryError> {
    let rows = sqlx::query_as::<_, ContentRow>("SELECT * FROM contents ORDER BY id")
        .fetch_all(&mut **transaction)
        .await
        .map_err(storage)?;
    let mut contents = Vec::with_capacity(rows.len());
    for row in rows {
        let id = ContentId::from_i64(row.id);
        let tags = snapshot_tags(transaction, id).await?;
        let current = row.into_content(tags)?;
        let revisions = portable_revisions(transaction, id).await?;
        contents.push(PortableContent { current, revisions });
    }
    Ok(contents)
}

async fn portable_revisions(
    transaction: &mut Transaction<'_, Sqlite>,
    content_id: ContentId,
) -> Result<Vec<ContentRevision>, RepositoryError> {
    let rows = sqlx::query(
        "SELECT id, intent, snapshot_json, created_at FROM revisions
         WHERE content_id = ? ORDER BY id",
    )
    .bind(content_id.as_i64())
    .fetch_all(&mut **transaction)
    .await
    .map_err(storage)?;
    rows.into_iter()
        .map(|row| {
            let intent: String = row.try_get("intent").map_err(storage)?;
            let snapshot: String = row.try_get("snapshot_json").map_err(storage)?;
            Ok(ContentRevision {
                id: row.try_get("id").map_err(storage)?,
                content_id,
                intent: SaveIntent::from_str(&intent)
                    .map_err(|error| RepositoryError::Storage(error.to_owned()))?,
                snapshot: serde_json::from_str(&snapshot).map_err(storage)?,
                created_at: row.try_get("created_at").map_err(storage)?,
            })
        })
        .collect()
}

async fn portable_redirects(
    transaction: &mut Transaction<'_, Sqlite>,
) -> Result<Vec<PortableRedirect>, RepositoryError> {
    let rows =
        sqlx::query("SELECT old_slug, content_id, created_at FROM redirects ORDER BY old_slug")
            .fetch_all(&mut **transaction)
            .await
            .map_err(storage)?;
    rows.into_iter()
        .map(|row| {
            let old_slug: String = row.try_get("old_slug").map_err(storage)?;
            Ok(PortableRedirect {
                old_slug: Slug::parse(old_slug).map_err(|_| {
                    RepositoryError::Storage("database contains an invalid redirect slug".into())
                })?,
                content_id: ContentId::from_i64(row.try_get("content_id").map_err(storage)?),
                created_at: row.try_get("created_at").map_err(storage)?,
            })
        })
        .collect()
}

async fn portable_engagement(
    transaction: &mut Transaction<'_, Sqlite>,
) -> Result<std::collections::BTreeMap<i64, PortableEngagement>, RepositoryError> {
    let rows = sqlx::query(
        "SELECT contents.id,
                COALESCE(content_likes.like_count, 0) AS likes,
                COALESCE(content_views.view_count, 0) AS views
         FROM contents
         LEFT JOIN content_likes ON content_likes.content_id = contents.id
         LEFT JOIN content_views ON content_views.content_id = contents.id
         ORDER BY contents.id",
    )
    .fetch_all(&mut **transaction)
    .await
    .map_err(storage)?;
    rows.into_iter()
        .map(|row| {
            let id: i64 = row.try_get("id").map_err(storage)?;
            let likes: i64 = row.try_get("likes").map_err(storage)?;
            let views: i64 = row.try_get("views").map_err(storage)?;
            Ok((
                id,
                PortableEngagement {
                    likes: u64::try_from(likes).map_err(storage)?,
                    views: u64::try_from(views).map_err(storage)?,
                },
            ))
        })
        .collect()
}

async fn portable_owner(
    transaction: &mut Transaction<'_, Sqlite>,
) -> Result<Option<PortableOwner>, RepositoryError> {
    use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};

    let owner = sqlx::query("SELECT user_handle, created_at FROM owner WHERE singleton = 1")
        .fetch_optional(&mut **transaction)
        .await
        .map_err(storage)?;
    let Some(owner) = owner else {
        return Ok(None);
    };
    let handle: Vec<u8> = owner.try_get("user_handle").map_err(storage)?;
    let passkey_rows = sqlx::query(
        "SELECT credential_id, name, passkey_json, created_at, last_used_at
         FROM passkeys ORDER BY credential_id",
    )
    .fetch_all(&mut **transaction)
    .await
    .map_err(storage)?;
    let passkeys = passkey_rows
        .into_iter()
        .map(|row| {
            let credential_id: Vec<u8> = row.try_get("credential_id").map_err(storage)?;
            Ok(PortablePasskey {
                credential_id: URL_SAFE_NO_PAD.encode(credential_id),
                name: row.try_get("name").map_err(storage)?,
                passkey_json: row.try_get("passkey_json").map_err(storage)?,
                created_at: row.try_get("created_at").map_err(storage)?,
                last_used_at: row.try_get("last_used_at").map_err(storage)?,
            })
        })
        .collect::<Result<Vec<_>, RepositoryError>>()?;
    let recovery_rows = sqlx::query(
        "SELECT code_hash, consumed_at, created_at FROM recovery_codes ORDER BY code_hash",
    )
    .fetch_all(&mut **transaction)
    .await
    .map_err(storage)?;
    let recovery_codes = recovery_rows
        .into_iter()
        .map(|row| {
            let code_hash: Vec<u8> = row.try_get("code_hash").map_err(storage)?;
            Ok(PortableRecoveryCode {
                code_hash: hex::encode(code_hash),
                consumed_at: row.try_get("consumed_at").map_err(storage)?,
                created_at: row.try_get("created_at").map_err(storage)?,
            })
        })
        .collect::<Result<Vec<_>, RepositoryError>>()?;
    Ok(Some(PortableOwner {
        user_handle: Uuid::from_slice(&handle).map_err(storage)?,
        created_at: owner.try_get("created_at").map_err(storage)?,
        passkeys,
        recovery_codes,
    }))
}

async fn portable_publication(
    transaction: &mut Transaction<'_, Sqlite>,
) -> Result<PortablePublicationState, RepositoryError> {
    let row = sqlx::query(
        "SELECT public_revision, next_publish_at FROM publication_state WHERE singleton = 1",
    )
    .fetch_one(&mut **transaction)
    .await
    .map_err(storage)?;
    let revision: i64 = row.try_get("public_revision").map_err(storage)?;
    Ok(PortablePublicationState {
        public_revision: u64::try_from(revision).map_err(storage)?,
        next_publish_at: row.try_get("next_publish_at").map_err(storage)?,
    })
}

async fn clear_portable_state(
    transaction: &mut Transaction<'_, Sqlite>,
) -> Result<(), RepositoryError> {
    for statement in [
        "DELETE FROM sessions",
        "DELETE FROM preview_links",
        "DELETE FROM setup_tokens",
        "DELETE FROM recovery_codes",
        "DELETE FROM passkeys",
        "DELETE FROM owner",
        "DELETE FROM redirects",
        "DELETE FROM revisions",
        "DELETE FROM content_tags",
        "DELETE FROM search_index",
        "DELETE FROM content_likes",
        "DELETE FROM content_views",
        "DELETE FROM tags",
        "DELETE FROM contents",
        "DELETE FROM navigation",
        "UPDATE site_settings SET logo_media_id = NULL, favicon_media_id = NULL WHERE singleton = 1",
        "DELETE FROM media_variants",
        "DELETE FROM media",
    ] {
        sqlx::query(statement)
            .execute(&mut **transaction)
            .await
            .map_err(storage)?;
    }
    Ok(())
}

async fn insert_portable_media(
    transaction: &mut Transaction<'_, Sqlite>,
    media: &[MediaAsset],
) -> Result<(), RepositoryError> {
    for asset in media {
        sqlx::query(
            "INSERT INTO media (
                id, original_name, mime_type, extension, width, height, byte_size,
                alt_text, caption, animated, created_at
             ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(asset.id.as_str())
        .bind(&asset.original_name)
        .bind(&asset.mime_type)
        .bind(&asset.extension)
        .bind(i64::from(asset.width))
        .bind(i64::from(asset.height))
        .bind(i64::try_from(asset.byte_size).map_err(storage)?)
        .bind(&asset.alt_text)
        .bind(&asset.caption)
        .bind(i64::from(asset.animated))
        .bind(asset.created_at)
        .execute(&mut **transaction)
        .await
        .map_err(storage)?;
        for variant in &asset.variants {
            sqlx::query(
                "INSERT INTO media_variants (media_id, width, height, byte_size, filename)
                 VALUES (?, ?, ?, ?, ?)",
            )
            .bind(asset.id.as_str())
            .bind(i64::from(variant.width))
            .bind(i64::from(variant.height))
            .bind(i64::try_from(variant.byte_size).map_err(storage)?)
            .bind(&variant.filename)
            .execute(&mut **transaction)
            .await
            .map_err(storage)?;
        }
    }
    Ok(())
}

async fn insert_portable_settings(
    transaction: &mut Transaction<'_, Sqlite>,
    site: &PortableSiteV1,
) -> Result<(), RepositoryError> {
    sqlx::query(
        "UPDATE site_settings SET site_title = ?, site_description = ?, locale = ?,
                logo_media_id = ?, favicon_media_id = ?, custom_css = ?, timezone = ?,
                author_name = ?, custom_css_backup = ?, updated_at = ?
         WHERE singleton = 1",
    )
    .bind(&site.settings.site_title)
    .bind(&site.settings.site_description)
    .bind(site.settings.locale.as_str())
    .bind(&site.settings.logo_media_id)
    .bind(&site.settings.favicon_media_id)
    .bind(&site.settings.custom_css)
    .bind(&site.settings.timezone)
    .bind(&site.settings.author_name)
    .bind(&site.settings.custom_css_backup)
    .bind(site.exported_at)
    .execute(&mut **transaction)
    .await
    .map_err(storage)?;
    for item in &site.navigation {
        sqlx::query(
            "INSERT INTO navigation (id, label, destination, is_external, position)
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind(item.id)
        .bind(&item.label)
        .bind(&item.destination)
        .bind(i64::from(item.is_external))
        .bind(i64::from(item.position))
        .execute(&mut **transaction)
        .await
        .map_err(storage)?;
    }
    Ok(())
}

async fn insert_portable_contents(
    transaction: &mut Transaction<'_, Sqlite>,
    contents: &[PortableContent],
    markdown: &dyn crate::application::ports::MarkdownRenderer,
) -> Result<(), RepositoryError> {
    for record in contents {
        let content = &record.current;
        let (status, publish_at) = publication_columns(&content.publication);
        let body_html = markdown.render(&content.body_markdown).html;
        sqlx::query(
            "INSERT INTO contents (
                id, kind, title, slug, summary, body_markdown, body_html, status, publish_at,
                cover_media_id, seo_title, seo_description, version, created_at, updated_at,
                deleted_at
             ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(content.id.as_i64())
        .bind(content.kind.as_str())
        .bind(&content.title)
        .bind(content.slug.as_str())
        .bind(&content.summary)
        .bind(&content.body_markdown)
        .bind(&body_html)
        .bind(status)
        .bind(publish_at)
        .bind(&content.cover_media_id)
        .bind(&content.seo_title)
        .bind(&content.seo_description)
        .bind(content.version)
        .bind(content.created_at)
        .bind(content.updated_at)
        .bind(content.deleted_at)
        .execute(&mut **transaction)
        .await
        .map_err(storage)?;
        insert_portable_tags(transaction, content).await?;
        index_search_document(transaction, content.id, &content.title, &body_html).await?;
        for revision in &record.revisions {
            let mut snapshot = revision.snapshot.clone();
            snapshot.body_html = markdown.render(&snapshot.body_markdown).html;
            let snapshot = serde_json::to_string(&snapshot).map_err(storage)?;
            sqlx::query(
                "INSERT INTO revisions (id, content_id, intent, snapshot_json, created_at)
                 VALUES (?, ?, ?, ?, ?)",
            )
            .bind(revision.id)
            .bind(revision.content_id.as_i64())
            .bind(revision.intent.as_str())
            .bind(snapshot)
            .bind(revision.created_at)
            .execute(&mut **transaction)
            .await
            .map_err(storage)?;
        }
    }
    Ok(())
}

async fn insert_portable_tags(
    transaction: &mut Transaction<'_, Sqlite>,
    content: &Content,
) -> Result<(), RepositoryError> {
    for (position, tag) in content.tags.iter().enumerate() {
        sqlx::query("INSERT INTO tags (name, slug) VALUES (?, ?) ON CONFLICT(slug) DO NOTHING")
            .bind(&tag.name)
            .bind(tag.slug.as_str())
            .execute(&mut **transaction)
            .await
            .map_err(storage)?;
        let tag_id: i64 = sqlx::query_scalar("SELECT id FROM tags WHERE slug = ?")
            .bind(tag.slug.as_str())
            .fetch_one(&mut **transaction)
            .await
            .map_err(storage)?;
        sqlx::query("INSERT INTO content_tags (content_id, tag_id, position) VALUES (?, ?, ?)")
            .bind(content.id.as_i64())
            .bind(tag_id)
            .bind(i64::try_from(position).map_err(storage)?)
            .execute(&mut **transaction)
            .await
            .map_err(storage)?;
    }
    Ok(())
}

async fn insert_portable_redirects(
    transaction: &mut Transaction<'_, Sqlite>,
    redirects: &[PortableRedirect],
) -> Result<(), RepositoryError> {
    for redirect in redirects {
        sqlx::query("INSERT INTO redirects (old_slug, content_id, created_at) VALUES (?, ?, ?)")
            .bind(redirect.old_slug.as_str())
            .bind(redirect.content_id.as_i64())
            .bind(redirect.created_at)
            .execute(&mut **transaction)
            .await
            .map_err(storage)?;
    }
    Ok(())
}

async fn insert_portable_engagement(
    transaction: &mut Transaction<'_, Sqlite>,
    engagement: &std::collections::BTreeMap<i64, PortableEngagement>,
) -> Result<(), RepositoryError> {
    for (&content_id, totals) in engagement {
        sqlx::query("INSERT INTO content_likes (content_id, like_count) VALUES (?, ?)")
            .bind(content_id)
            .bind(i64::try_from(totals.likes).map_err(storage)?)
            .execute(&mut **transaction)
            .await
            .map_err(storage)?;
        sqlx::query("INSERT INTO content_views (content_id, view_count) VALUES (?, ?)")
            .bind(content_id)
            .bind(i64::try_from(totals.views).map_err(storage)?)
            .execute(&mut **transaction)
            .await
            .map_err(storage)?;
    }
    Ok(())
}

async fn insert_portable_owner(
    transaction: &mut Transaction<'_, Sqlite>,
    owner: Option<&PortableOwner>,
) -> Result<(), RepositoryError> {
    use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};

    let Some(owner) = owner else {
        return Ok(());
    };
    sqlx::query("INSERT INTO owner (singleton, user_handle, created_at) VALUES (1, ?, ?)")
        .bind(owner.user_handle.as_bytes().as_slice())
        .bind(owner.created_at)
        .execute(&mut **transaction)
        .await
        .map_err(storage)?;
    for passkey in &owner.passkeys {
        let credential = URL_SAFE_NO_PAD
            .decode(&passkey.credential_id)
            .map_err(storage)?;
        sqlx::query(
            "INSERT INTO passkeys (
                credential_id, name, passkey_json, created_at, last_used_at
             ) VALUES (?, ?, ?, ?, ?)",
        )
        .bind(credential)
        .bind(&passkey.name)
        .bind(&passkey.passkey_json)
        .bind(passkey.created_at)
        .bind(passkey.last_used_at)
        .execute(&mut **transaction)
        .await
        .map_err(storage)?;
    }
    for recovery in &owner.recovery_codes {
        let hash = hex::decode(&recovery.code_hash).map_err(storage)?;
        sqlx::query(
            "INSERT INTO recovery_codes (code_hash, consumed_at, created_at) VALUES (?, ?, ?)",
        )
        .bind(hash)
        .bind(recovery.consumed_at)
        .bind(recovery.created_at)
        .execute(&mut **transaction)
        .await
        .map_err(storage)?;
    }
    Ok(())
}

#[async_trait]
impl PreviewLinkRepository for SqliteRepository {
    async fn store_preview_link(&self, link: &PreviewLinkRecord) -> Result<(), AuthError> {
        sqlx::query(
            "INSERT INTO preview_links (token_hash, content_id, created_at, expires_at)
             VALUES (?, ?, ?, ?)",
        )
        .bind(link.token_hash.as_bytes().as_slice())
        .bind(link.content_id.as_i64())
        .bind(link.created_at)
        .bind(link.expires_at)
        .execute(&self.pool)
        .await
        .map_err(auth_storage)?;
        Ok(())
    }

    async fn find_preview_link(
        &self,
        token_hash: SecretHash,
        now: DateTime<Utc>,
    ) -> Result<Option<ContentId>, AuthError> {
        let content_id: Option<i64> = sqlx::query_scalar(
            "SELECT content_id FROM preview_links WHERE token_hash = ? AND expires_at > ?",
        )
        .bind(token_hash.as_bytes().as_slice())
        .bind(now)
        .fetch_optional(&self.pool)
        .await
        .map_err(auth_storage)?;
        Ok(content_id.map(ContentId::from_i64))
    }

    async fn revoke_preview_links(&self, content_id: ContentId) -> Result<u64, AuthError> {
        let result = sqlx::query("DELETE FROM preview_links WHERE content_id = ?")
            .bind(content_id.as_i64())
            .execute(&self.pool)
            .await
            .map_err(auth_storage)?;
        Ok(result.rows_affected())
    }
}

#[async_trait]
impl RevisionMediaReferences for SqliteRepository {
    async fn revision_media_ids(&self) -> Result<HashSet<String>, RepositoryError> {
        let rows: Vec<(String, Option<String>)> = sqlx::query_as(
            "SELECT snapshot_json, json_extract(snapshot_json, '$.cover_media_id') FROM revisions",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(storage)?;
        let mut referenced = HashSet::new();
        for (snapshot, cover) in rows {
            media_gc::collect_media_references(&snapshot, &mut referenced);
            if let Some(cover) = cover {
                referenced.insert(cover);
            }
        }
        Ok(referenced)
    }
}

#[async_trait]
impl MediaRepository for SqliteRepository {
    async fn save_media(&self, media: &MediaAsset) -> Result<MediaAsset, MediaRepositoryError> {
        let mut transaction = self.pool.begin().await.map_err(media_storage)?;
        sqlx::query(
            "INSERT INTO media (
                id, original_name, mime_type, extension, width, height, byte_size,
                alt_text, caption, animated, created_at
             ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(id) DO UPDATE SET
                original_name = excluded.original_name,
                mime_type = excluded.mime_type,
                extension = excluded.extension,
                width = excluded.width,
                height = excluded.height,
                byte_size = excluded.byte_size,
                alt_text = excluded.alt_text,
                caption = excluded.caption,
                animated = excluded.animated",
        )
        .bind(media.id.as_str())
        .bind(&media.original_name)
        .bind(&media.mime_type)
        .bind(&media.extension)
        .bind(i64::from(media.width))
        .bind(i64::from(media.height))
        .bind(i64::try_from(media.byte_size).map_err(media_storage)?)
        .bind(&media.alt_text)
        .bind(&media.caption)
        .bind(i64::from(media.animated))
        .bind(media.created_at)
        .execute(&mut *transaction)
        .await
        .map_err(media_storage)?;
        sqlx::query("DELETE FROM media_variants WHERE media_id = ?")
            .bind(media.id.as_str())
            .execute(&mut *transaction)
            .await
            .map_err(media_storage)?;
        for variant in &media.variants {
            sqlx::query(
                "INSERT INTO media_variants (media_id, width, height, byte_size, filename)
                 VALUES (?, ?, ?, ?, ?)",
            )
            .bind(media.id.as_str())
            .bind(i64::from(variant.width))
            .bind(i64::from(variant.height))
            .bind(i64::try_from(variant.byte_size).map_err(media_storage)?)
            .bind(&variant.filename)
            .execute(&mut *transaction)
            .await
            .map_err(media_storage)?;
        }
        transaction.commit().await.map_err(media_storage)?;
        self.find_media(&media.id)
            .await?
            .ok_or_else(|| MediaRepositoryError::Storage("media disappeared after save".into()))
    }

    async fn find_media(&self, id: &MediaId) -> Result<Option<MediaAsset>, MediaRepositoryError> {
        let row = sqlx::query(
            "SELECT id, original_name, mime_type, extension, width, height, byte_size,
                    alt_text, caption, animated, created_at
             FROM media WHERE id = ?",
        )
        .bind(id.as_str())
        .fetch_optional(&self.pool)
        .await
        .map_err(media_storage)?;
        let Some(row) = row else {
            return Ok(None);
        };
        let variants = load_media_variants(&self.pool, id).await?;
        media_from_row(&row, variants).map(Some)
    }

    async fn delete_media(&self, id: &MediaId) -> Result<(), MediaRepositoryError> {
        let mut transaction = self.pool.begin().await.map_err(media_storage)?;
        sqlx::query("DELETE FROM media_variants WHERE media_id = ?")
            .bind(id.as_str())
            .execute(&mut *transaction)
            .await
            .map_err(media_storage)?;
        sqlx::query("DELETE FROM media WHERE id = ?")
            .bind(id.as_str())
            .execute(&mut *transaction)
            .await
            .map_err(media_storage)?;
        transaction.commit().await.map_err(media_storage)
    }

    async fn update_media_alt_text(
        &self,
        id: &MediaId,
        alt_text: &str,
        now: DateTime<Utc>,
    ) -> Result<bool, MediaRepositoryError> {
        let mut transaction = self.pool.begin().await.map_err(media_storage)?;
        let result = sqlx::query("UPDATE media SET alt_text = ? WHERE id = ?")
            .bind(alt_text)
            .bind(id.as_str())
            .execute(&mut *transaction)
            .await
            .map_err(media_storage)?;
        if result.rows_affected() == 0 {
            return Ok(false);
        }
        refresh_publication_state(&mut transaction, now, true)
            .await
            .map_err(media_storage)?;
        transaction.commit().await.map_err(media_storage)?;
        Ok(true)
    }

    async fn mime_type_for_filename(
        &self,
        filename: &str,
    ) -> Result<Option<String>, MediaRepositoryError> {
        let original: Option<String> =
            sqlx::query_scalar("SELECT mime_type FROM media WHERE id || '.' || extension = ?")
                .bind(filename)
                .fetch_optional(&self.pool)
                .await
                .map_err(media_storage)?;
        if original.is_some() {
            return Ok(original);
        }
        let variant: Option<String> =
            sqlx::query_scalar("SELECT media_id FROM media_variants WHERE filename = ?")
                .bind(filename)
                .fetch_optional(&self.pool)
                .await
                .map_err(media_storage)?;
        Ok(variant.map(|_| "image/webp".to_owned()))
    }

    async fn list_media(&self) -> Result<Vec<MediaAsset>, MediaRepositoryError> {
        let ids: Vec<String> =
            sqlx::query_scalar("SELECT id FROM media ORDER BY created_at DESC, id")
                .fetch_all(&self.pool)
                .await
                .map_err(media_storage)?;
        let mut media = Vec::with_capacity(ids.len());
        for id in ids {
            let id = MediaId::parse(id)
                .map_err(|error| MediaRepositoryError::Storage(error.to_string()))?;
            if let Some(asset) = self.find_media(&id).await? {
                media.push(asset);
            }
        }
        Ok(media)
    }
}

impl ContentRow {
    fn into_content(self, tags: Vec<Tag>) -> Result<Content, RepositoryError> {
        let kind = ContentKind::from_str(&self.kind)
            .map_err(|error| RepositoryError::Storage(error.to_owned()))?;
        let slug = Slug::parse(self.slug)
            .map_err(|_| RepositoryError::Storage("database contains an invalid slug".into()))?;
        let publication = match (self.status.as_str(), self.publish_at) {
            ("draft", None) => Publication::Draft,
            ("public", Some(publish_at)) => Publication::Public { publish_at },
            _ => {
                return Err(RepositoryError::Storage(
                    "database contains an invalid publication state".into(),
                ));
            }
        };
        Ok(Content {
            id: ContentId::from_i64(self.id),
            kind,
            title: self.title,
            slug,
            summary: self.summary,
            body_markdown: self.body_markdown,
            body_html: self.body_html,
            tags,
            cover_media_id: self.cover_media_id,
            seo_title: self.seo_title,
            seo_description: self.seo_description,
            publication,
            version: self.version,
            created_at: self.created_at,
            updated_at: self.updated_at,
            deleted_at: self.deleted_at,
        })
    }
}

/// Every tag assignment on the site, grouped by content id and ordered by
/// position within each piece.
async fn load_all_tags(
    pool: &SqlitePool,
) -> Result<std::collections::HashMap<i64, Vec<Tag>>, RepositoryError> {
    let rows = sqlx::query(
        "SELECT content_tags.content_id, tags.name, tags.slug FROM content_tags
         JOIN tags ON tags.id = content_tags.tag_id
         ORDER BY content_tags.content_id, content_tags.position",
    )
    .fetch_all(pool)
    .await
    .map_err(storage)?;
    let mut grouped: std::collections::HashMap<i64, Vec<Tag>> = std::collections::HashMap::new();
    for row in rows {
        let content_id: i64 = row.try_get("content_id").map_err(storage)?;
        let slug: String = row.try_get("slug").map_err(storage)?;
        grouped.entry(content_id).or_default().push(Tag {
            name: row.try_get("name").map_err(storage)?,
            slug: Slug::parse(slug).map_err(|_| {
                RepositoryError::Storage("database contains an invalid tag slug".into())
            })?,
        });
    }
    Ok(grouped)
}

async fn load_tags(pool: &SqlitePool, content_id: ContentId) -> Result<Vec<Tag>, RepositoryError> {
    let rows = sqlx::query(
        "SELECT tags.name, tags.slug FROM content_tags
         JOIN tags ON tags.id = content_tags.tag_id
         WHERE content_tags.content_id = ? ORDER BY content_tags.position",
    )
    .bind(content_id.as_i64())
    .fetch_all(pool)
    .await
    .map_err(storage)?;
    rows.into_iter()
        .map(|row| {
            let slug: String = row.try_get("slug").map_err(storage)?;
            Ok(Tag {
                name: row.try_get("name").map_err(storage)?,
                slug: Slug::parse(slug).map_err(|_| {
                    RepositoryError::Storage("database contains an invalid tag slug".into())
                })?,
            })
        })
        .collect()
}

const SITE_SETTINGS_SELECT: &str = "SELECT site_title, site_description, locale, logo_media_id,
            favicon_media_id, custom_css, timezone, author_name, custom_css_backup
     FROM site_settings WHERE singleton = 1";

fn settings_from_row(row: &sqlx::sqlite::SqliteRow) -> Result<SiteSettings, RepositoryError> {
    let locale: String = row.try_get("locale").map_err(storage)?;
    Ok(SiteSettings {
        site_title: row.try_get("site_title").map_err(storage)?,
        site_description: row.try_get("site_description").map_err(storage)?,
        locale: locale_from_database(&locale)?,
        logo_media_id: row.try_get("logo_media_id").map_err(storage)?,
        favicon_media_id: row.try_get("favicon_media_id").map_err(storage)?,
        custom_css: row.try_get("custom_css").map_err(storage)?,
        timezone: row.try_get("timezone").map_err(storage)?,
        author_name: row.try_get("author_name").map_err(storage)?,
        custom_css_backup: row.try_get("custom_css_backup").map_err(storage)?,
    })
}

async fn snapshot_settings(
    transaction: &mut Transaction<'_, Sqlite>,
) -> Result<SiteSettings, RepositoryError> {
    let row = sqlx::query(SITE_SETTINGS_SELECT)
        .fetch_one(&mut **transaction)
        .await
        .map_err(storage)?;
    settings_from_row(&row)
}

async fn snapshot_navigation(
    transaction: &mut Transaction<'_, Sqlite>,
) -> Result<Vec<NavigationItem>, RepositoryError> {
    let rows = sqlx::query(
        "SELECT id, label, destination, is_external, position
         FROM navigation ORDER BY position",
    )
    .fetch_all(&mut **transaction)
    .await
    .map_err(storage)?;
    rows.into_iter()
        .map(|row| {
            let position: i64 = row.try_get("position").map_err(storage)?;
            Ok(NavigationItem {
                id: row.try_get("id").map_err(storage)?,
                label: row.try_get("label").map_err(storage)?,
                destination: row.try_get("destination").map_err(storage)?,
                is_external: row.try_get::<i64, _>("is_external").map_err(storage)? == 1,
                position: u16::try_from(position).map_err(storage)?,
            })
        })
        .collect()
}

async fn snapshot_contents(
    transaction: &mut Transaction<'_, Sqlite>,
    effective_at: DateTime<Utc>,
) -> Result<Vec<Content>, RepositoryError> {
    let rows = sqlx::query_as::<_, ContentRow>(
        "SELECT * FROM contents
         WHERE status = 'public' AND publish_at <= ? AND deleted_at IS NULL
         ORDER BY publish_at DESC, id DESC",
    )
    .bind(effective_at)
    .fetch_all(&mut **transaction)
    .await
    .map_err(storage)?;
    let mut contents = Vec::with_capacity(rows.len());
    for row in rows {
        let id = ContentId::from_i64(row.id);
        let tags = snapshot_tags(transaction, id).await?;
        contents.push(row.into_content(tags)?);
    }
    Ok(contents)
}

async fn snapshot_tags(
    transaction: &mut Transaction<'_, Sqlite>,
    content_id: ContentId,
) -> Result<Vec<Tag>, RepositoryError> {
    let rows = sqlx::query(
        "SELECT tags.name, tags.slug FROM content_tags
         JOIN tags ON tags.id = content_tags.tag_id
         WHERE content_tags.content_id = ? ORDER BY content_tags.position",
    )
    .bind(content_id.as_i64())
    .fetch_all(&mut **transaction)
    .await
    .map_err(storage)?;
    rows.into_iter()
        .map(|row| {
            let slug: String = row.try_get("slug").map_err(storage)?;
            Ok(Tag {
                name: row.try_get("name").map_err(storage)?,
                slug: Slug::parse(slug).map_err(|_| {
                    RepositoryError::Storage("database contains an invalid tag slug".into())
                })?,
            })
        })
        .collect()
}

async fn snapshot_redirects(
    transaction: &mut Transaction<'_, Sqlite>,
    effective_at: DateTime<Utc>,
) -> Result<Vec<PublicRedirect>, RepositoryError> {
    let rows = sqlx::query(
        "SELECT redirects.old_slug, contents.slug AS target_slug
         FROM redirects
         JOIN contents ON contents.id = redirects.content_id
         WHERE contents.status = 'public' AND contents.publish_at <= ?
           AND contents.deleted_at IS NULL
         ORDER BY redirects.old_slug",
    )
    .bind(effective_at)
    .fetch_all(&mut **transaction)
    .await
    .map_err(storage)?;
    rows.into_iter()
        .map(|row| {
            let from: String = row.try_get("old_slug").map_err(storage)?;
            let to: String = row.try_get("target_slug").map_err(storage)?;
            Ok(PublicRedirect {
                from: Slug::parse(from).map_err(|_| {
                    RepositoryError::Storage("database contains an invalid redirect slug".into())
                })?,
                to: Slug::parse(to).map_err(|_| {
                    RepositoryError::Storage("database contains an invalid redirect target".into())
                })?,
            })
        })
        .collect()
}

async fn snapshot_media(
    transaction: &mut Transaction<'_, Sqlite>,
) -> Result<Vec<MediaAsset>, RepositoryError> {
    let rows = sqlx::query(
        "SELECT id, original_name, mime_type, extension, width, height, byte_size,
                alt_text, caption, animated, created_at
         FROM media ORDER BY created_at DESC, id",
    )
    .fetch_all(&mut **transaction)
    .await
    .map_err(storage)?;
    let mut media = Vec::with_capacity(rows.len());
    for row in rows {
        let raw_id: String = row.try_get("id").map_err(storage)?;
        let id = MediaId::parse(&raw_id).map_err(storage)?;
        let variants = snapshot_media_variants(transaction, &id).await?;
        media.push(
            media_from_row(&row, variants)
                .map_err(|error| RepositoryError::Storage(error.to_string()))?,
        );
    }
    Ok(media)
}

async fn snapshot_media_variants(
    transaction: &mut Transaction<'_, Sqlite>,
    media_id: &MediaId,
) -> Result<Vec<MediaVariant>, RepositoryError> {
    let rows = sqlx::query(
        "SELECT width, height, byte_size, filename FROM media_variants
         WHERE media_id = ? ORDER BY width",
    )
    .bind(media_id.as_str())
    .fetch_all(&mut **transaction)
    .await
    .map_err(storage)?;
    rows.into_iter()
        .map(|row| {
            let width: i64 = row.try_get("width").map_err(storage)?;
            let height: i64 = row.try_get("height").map_err(storage)?;
            let byte_size: i64 = row.try_get("byte_size").map_err(storage)?;
            Ok(MediaVariant {
                width: u32::try_from(width).map_err(storage)?,
                height: u32::try_from(height).map_err(storage)?,
                byte_size: u64::try_from(byte_size).map_err(storage)?,
                filename: row.try_get("filename").map_err(storage)?,
            })
        })
        .collect()
}

fn publication_state_from_row(
    row: &sqlx::sqlite::SqliteRow,
) -> Result<PublicationState, RepositoryError> {
    let revision: i64 = row.try_get("public_revision").map_err(storage)?;
    Ok(PublicationState {
        revision: u64::try_from(revision).map_err(storage)?,
        next_publish_at: row.try_get("next_publish_at").map_err(storage)?,
    })
}

fn locale_from_database(locale: &str) -> Result<Locale, RepositoryError> {
    match locale {
        "en" => Ok(Locale::En),
        "ja" => Ok(Locale::Ja),
        "zh" => Ok(Locale::Zh),
        _ => Err(RepositoryError::Storage("invalid locale".into())),
    }
}

async fn refresh_publication_state(
    transaction: &mut Transaction<'_, Sqlite>,
    now: DateTime<Utc>,
    increment: bool,
) -> Result<(), RepositoryError> {
    let next_publish_at: Option<DateTime<Utc>> = sqlx::query_scalar(
        "SELECT MIN(publish_at) FROM contents
         WHERE status = 'public' AND publish_at > ? AND deleted_at IS NULL",
    )
    .bind(now)
    .fetch_one(&mut **transaction)
    .await
    .map_err(storage)?;
    sqlx::query(
        "UPDATE publication_state SET
            public_revision = public_revision + ?, next_publish_at = ?, updated_at = ?
         WHERE singleton = 1",
    )
    .bind(i64::from(increment))
    .bind(next_publish_at)
    .bind(now)
    .execute(&mut **transaction)
    .await
    .map_err(storage)?;
    Ok(())
}

async fn ensure_slug_available(
    transaction: &mut Transaction<'_, Sqlite>,
    slug: &Slug,
    own_content: Option<ContentId>,
) -> Result<(), RepositoryError> {
    let count: i64 = sqlx::query_scalar(
        "SELECT
            (SELECT count(*) FROM contents WHERE slug = ? AND (? IS NULL OR id != ?)) +
            (SELECT count(*) FROM redirects WHERE old_slug = ? AND (? IS NULL OR content_id != ?))",
    )
    .bind(slug.as_str())
    .bind(own_content.map(ContentId::as_i64))
    .bind(own_content.map(ContentId::as_i64))
    .bind(slug.as_str())
    .bind(own_content.map(ContentId::as_i64))
    .bind(own_content.map(ContentId::as_i64))
    .fetch_one(&mut **transaction)
    .await
    .map_err(storage)?;
    if count > 0 {
        Err(RepositoryError::SlugTaken(slug.clone()))
    } else {
        Ok(())
    }
}

async fn replace_tags(
    transaction: &mut Transaction<'_, Sqlite>,
    content_id: ContentId,
    tags: &[Tag],
) -> Result<(), RepositoryError> {
    sqlx::query("DELETE FROM content_tags WHERE content_id = ?")
        .bind(content_id.as_i64())
        .execute(&mut **transaction)
        .await
        .map_err(storage)?;
    for (position, tag) in tags.iter().enumerate() {
        let tag_id: i64 = sqlx::query_scalar(
            "INSERT INTO tags (name, slug) VALUES (?, ?)
             ON CONFLICT(slug) DO UPDATE SET name = excluded.name
             RETURNING id",
        )
        .bind(&tag.name)
        .bind(tag.slug.as_str())
        .fetch_one(&mut **transaction)
        .await
        .map_err(storage)?;
        sqlx::query("INSERT INTO content_tags (content_id, tag_id, position) VALUES (?, ?, ?)")
            .bind(content_id.as_i64())
            .bind(tag_id)
            .bind(i64::try_from(position).map_err(storage)?)
            .execute(&mut **transaction)
            .await
            .map_err(storage)?;
    }
    Ok(())
}

async fn insert_revision(
    transaction: &mut Transaction<'_, Sqlite>,
    content: &Content,
    intent: SaveIntent,
    now: DateTime<Utc>,
) -> Result<(), RepositoryError> {
    let snapshot = serde_json::to_string(content).map_err(storage)?;
    sqlx::query(
        "INSERT INTO revisions (content_id, intent, snapshot_json, created_at)
         VALUES (?, ?, ?, ?)",
    )
    .bind(content.id.as_i64())
    .bind(intent.as_str())
    .bind(snapshot)
    .bind(now)
    .execute(&mut **transaction)
    .await
    .map_err(storage)?;
    Ok(())
}

async fn prune_autosaves(
    transaction: &mut Transaction<'_, Sqlite>,
    content_id: ContentId,
) -> Result<(), RepositoryError> {
    sqlx::query(
        "DELETE FROM revisions
         WHERE content_id = ? AND intent = 'autosave' AND id NOT IN (
            SELECT id FROM revisions
            WHERE content_id = ? AND intent = 'autosave'
            ORDER BY created_at DESC, id DESC LIMIT 50
         )",
    )
    .bind(content_id.as_i64())
    .bind(content_id.as_i64())
    .execute(&mut **transaction)
    .await
    .map_err(storage)?;
    Ok(())
}

fn content_from_prepared(
    id: ContentId,
    prepared: PreparedContent,
    version: i64,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
) -> Content {
    Content {
        id,
        kind: prepared.draft.kind,
        title: prepared.draft.title,
        slug: prepared.draft.slug,
        summary: prepared.draft.summary,
        body_markdown: prepared.draft.body_markdown,
        body_html: prepared.body_html,
        tags: prepared.tags,
        cover_media_id: prepared.draft.cover_media_id,
        seo_title: prepared.draft.seo_title,
        seo_description: prepared.draft.seo_description,
        publication: prepared.draft.publication,
        version,
        created_at,
        updated_at,
        deleted_at: None,
    }
}

const fn publication_columns(publication: &Publication) -> (&'static str, Option<DateTime<Utc>>) {
    match publication {
        Publication::Draft => ("draft", None),
        Publication::Public { publish_at } => ("public", Some(*publish_at)),
    }
}

fn map_write_error(error: sqlx::Error, slug: &Slug) -> RepositoryError {
    if let sqlx::Error::Database(database) = &error
        && (database.is_unique_violation()
            || database.message().contains("slug is historical")
            || database.message().contains("slug is active"))
    {
        return RepositoryError::SlugTaken(slug.clone());
    }
    storage(error)
}

fn storage(error: impl std::fmt::Display) -> RepositoryError {
    RepositoryError::Storage(error.to_string())
}

fn auth_storage(error: impl std::fmt::Display) -> AuthError {
    AuthError::Storage(error.to_string())
}

fn secret_hash_from_row(
    row: &sqlx::sqlite::SqliteRow,
    column: &str,
) -> Result<SecretHash, AuthError> {
    let bytes: Vec<u8> = row.try_get(column).map_err(auth_storage)?;
    let bytes: [u8; 32] = bytes
        .try_into()
        .map_err(|_| AuthError::Storage(format!("{column} has an invalid length")))?;
    Ok(SecretHash::new(bytes))
}

async fn insert_session(
    transaction: &mut Transaction<'_, Sqlite>,
    session: &SessionRecord,
) -> Result<(), AuthError> {
    sqlx::query(
        "INSERT INTO sessions (
            token_hash, csrf_token_hash, created_at, expires_at, last_seen_at, reauthenticated_at
         ) VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(session.token_hash.as_bytes().as_slice())
    .bind(session.csrf_hash.as_bytes().as_slice())
    .bind(session.created_at)
    .bind(session.expires_at)
    .bind(session.last_seen_at)
    .bind(session.reauthenticated_at)
    .execute(&mut **transaction)
    .await
    .map_err(auth_storage)?;
    Ok(())
}

async fn load_media_variants(
    pool: &SqlitePool,
    media_id: &MediaId,
) -> Result<Vec<MediaVariant>, MediaRepositoryError> {
    let rows = sqlx::query(
        "SELECT width, height, byte_size, filename FROM media_variants
         WHERE media_id = ? ORDER BY width",
    )
    .bind(media_id.as_str())
    .fetch_all(pool)
    .await
    .map_err(media_storage)?;
    rows.into_iter()
        .map(|row| {
            let width: i64 = row.try_get("width").map_err(media_storage)?;
            let height: i64 = row.try_get("height").map_err(media_storage)?;
            let byte_size: i64 = row.try_get("byte_size").map_err(media_storage)?;
            Ok(MediaVariant {
                width: u32::try_from(width).map_err(media_storage)?,
                height: u32::try_from(height).map_err(media_storage)?,
                byte_size: u64::try_from(byte_size).map_err(media_storage)?,
                filename: row.try_get("filename").map_err(media_storage)?,
            })
        })
        .collect()
}

fn media_from_row(
    row: &sqlx::sqlite::SqliteRow,
    variants: Vec<MediaVariant>,
) -> Result<MediaAsset, MediaRepositoryError> {
    let raw_id: String = row.try_get("id").map_err(media_storage)?;
    let id =
        MediaId::parse(raw_id).map_err(|error| MediaRepositoryError::Storage(error.to_string()))?;
    let extension: String = row.try_get("extension").map_err(media_storage)?;
    let width: i64 = row.try_get("width").map_err(media_storage)?;
    let height: i64 = row.try_get("height").map_err(media_storage)?;
    let byte_size: i64 = row.try_get("byte_size").map_err(media_storage)?;
    Ok(MediaAsset {
        original_filename: format!("{id}.{extension}"),
        id,
        original_name: row.try_get("original_name").map_err(media_storage)?,
        mime_type: row.try_get("mime_type").map_err(media_storage)?,
        extension,
        width: u32::try_from(width).map_err(media_storage)?,
        height: u32::try_from(height).map_err(media_storage)?,
        byte_size: u64::try_from(byte_size).map_err(media_storage)?,
        alt_text: row.try_get("alt_text").map_err(media_storage)?,
        caption: row.try_get("caption").map_err(media_storage)?,
        animated: row.try_get::<i64, _>("animated").map_err(media_storage)? == 1,
        variants,
        created_at: row.try_get("created_at").map_err(media_storage)?,
    })
}

fn media_storage(error: impl std::fmt::Display) -> MediaRepositoryError {
    MediaRepositoryError::Storage(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dynamic_search_sql_never_interpolates_term_data() {
        let terms = SearchTerms {
            fts: vec!["\"; DROP TABLE contents; --".to_owned()],
            like: vec!["%' OR 1 = 1; --".to_owned()],
        };

        let sql = build_search_sql(&terms).0;

        assert!(!sql.contains("DROP TABLE"));
        assert!(!sql.contains("OR 1 = 1"));
        assert_eq!(sql.matches('?').count(), 5);
    }
}
