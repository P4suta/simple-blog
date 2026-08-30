use std::{path::Path, str::FromStr, time::Duration};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sqlx::{
    FromRow, Row, Sqlite, SqlitePool, Transaction,
    migrate::Migrator,
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous},
};

use crate::{
    application::ports::{
        AuthError, AuthRepository, ContentRepository, MediaRepository, MediaRepositoryError,
        PasskeyRepository, PreparedContent, RepositoryError, SetupRegistration, SiteRepository,
    },
    domain::auth::{SecretHash, SessionRecord, SetupPurpose, StoredPasskey},
    domain::content::{
        Content, ContentId, ContentKind, ContentRevision, Publication, SaveIntent, Slug, Tag,
    },
    domain::media::{MediaAsset, MediaId, MediaVariant},
    domain::theme::{ColorScheme, FontPreset, Locale, NavigationItem, SiteSettings},
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
        Ok(Self { pool })
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

    async fn hydrate_many(&self, rows: Vec<ContentRow>) -> Result<Vec<Content>, RepositoryError> {
        let mut contents = Vec::with_capacity(rows.len());
        for row in rows {
            contents.push(self.hydrate(row).await?);
        }
        Ok(contents)
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
        let current = sqlx::query("SELECT slug, version, created_at FROM contents WHERE id = ?")
            .bind(id.as_i64())
            .fetch_optional(&mut *transaction)
            .await
            .map_err(storage)?
            .ok_or(RepositoryError::NotFound)?;
        let old_slug: String = current.try_get("slug").map_err(storage)?;
        let actual_version: i64 = current.try_get("version").map_err(storage)?;
        let created_at: DateTime<Utc> = current.try_get("created_at").map_err(storage)?;
        if actual_version != expected_version {
            return Err(RepositoryError::Conflict {
                expected: expected_version,
                actual: Some(actual_version),
            });
        }
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
        if intent == SaveIntent::Autosave {
            prune_autosaves(&mut transaction, id).await?;
        }
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
             WHERE slug = ? AND status = 'public' AND publish_at <= ?",
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
             WHERE redirects.old_slug = ?",
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
             WHERE status = 'public' AND publish_at <= ?
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
}

#[async_trait]
impl SiteRepository for SqliteRepository {
    async fn site_settings(&self) -> Result<SiteSettings, RepositoryError> {
        let row = sqlx::query(
            "SELECT site_title, site_description, locale, logo_media_id, favicon_media_id,
                    accent_color, font_preset, content_width, color_scheme, custom_css
             FROM site_settings WHERE singleton = 1",
        )
        .fetch_one(&self.pool)
        .await
        .map_err(storage)?;
        let locale: String = row.try_get("locale").map_err(storage)?;
        let font: String = row.try_get("font_preset").map_err(storage)?;
        let scheme: String = row.try_get("color_scheme").map_err(storage)?;
        let width: i64 = row.try_get("content_width").map_err(storage)?;
        Ok(SiteSettings {
            site_title: row.try_get("site_title").map_err(storage)?,
            site_description: row.try_get("site_description").map_err(storage)?,
            locale: match locale.as_str() {
                "ja" => Locale::Ja,
                "en" => Locale::En,
                _ => return Err(RepositoryError::Storage("invalid locale".into())),
            },
            logo_media_id: row.try_get("logo_media_id").map_err(storage)?,
            favicon_media_id: row.try_get("favicon_media_id").map_err(storage)?,
            accent_color: row.try_get("accent_color").map_err(storage)?,
            font_preset: match font.as_str() {
                "sans" => FontPreset::Sans,
                "serif" => FontPreset::Serif,
                _ => return Err(RepositoryError::Storage("invalid font preset".into())),
            },
            content_width: u16::try_from(width).map_err(storage)?,
            color_scheme: match scheme.as_str() {
                "system" => ColorScheme::System,
                "light" => ColorScheme::Light,
                "dark" => ColorScheme::Dark,
                _ => return Err(RepositoryError::Storage("invalid color scheme".into())),
            },
            custom_css: row.try_get("custom_css").map_err(storage)?,
        })
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
                favicon_media_id = ?, accent_color = ?, font_preset = ?, content_width = ?,
                color_scheme = ?, custom_css = ?, updated_at = ?
             WHERE singleton = 1",
        )
        .bind(&settings.site_title)
        .bind(&settings.site_description)
        .bind(settings.locale.as_str())
        .bind(&settings.logo_media_id)
        .bind(&settings.favicon_media_id)
        .bind(&settings.accent_color)
        .bind(settings.font_preset.as_str())
        .bind(i64::from(settings.content_width))
        .bind(settings.color_scheme.as_str())
        .bind(&settings.custom_css)
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
        transaction.commit().await.map_err(storage)?;
        Ok(())
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
        })
    }
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
