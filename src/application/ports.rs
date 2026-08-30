//! Small capability-oriented ports keep external libraries out of the domain.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use thiserror::Error;

use crate::domain::auth::{SecretHash, SessionRecord, SetupPurpose, StoredPasskey};
use crate::domain::content::{
    Content, ContentDraft, ContentId, ContentRevision, SaveIntent, Slug, Tag,
};
use crate::domain::media::{MediaAsset, MediaId};
use crate::domain::theme::{NavigationItem, SiteSettings};
use uuid::Uuid;

pub trait Clock: Send + Sync {
    fn now(&self) -> DateTime<Utc>;
}

/// The safe representation persisted alongside canonical Markdown.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RenderedMarkdown {
    pub html: String,
}

/// Converts canonical Markdown into HTML safe enough to embed in a template.
pub trait MarkdownRenderer: Send + Sync {
    fn render(&self, markdown: &str) -> RenderedMarkdown;
}

#[derive(Clone, Debug)]
pub struct PreparedContent {
    pub draft: ContentDraft,
    pub body_html: String,
    pub tags: Vec<Tag>,
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum RepositoryError {
    #[error("content was changed (expected version {expected}, actual {actual:?})")]
    Conflict { expected: i64, actual: Option<i64> },
    #[error("slug is already active or historical: {0}")]
    SlugTaken(Slug),
    #[error("content does not exist")]
    NotFound,
    #[error("invalid content: {0}")]
    Validation(String),
    #[error("storage failure: {0}")]
    Storage(String),
}

#[async_trait]
pub trait ContentRepository: Send + Sync {
    async fn create(
        &self,
        content: PreparedContent,
        intent: SaveIntent,
        now: DateTime<Utc>,
    ) -> Result<Content, RepositoryError>;

    async fn update(
        &self,
        id: ContentId,
        expected_version: i64,
        content: PreparedContent,
        intent: SaveIntent,
        now: DateTime<Utc>,
    ) -> Result<Content, RepositoryError>;

    async fn find_by_id(&self, id: ContentId) -> Result<Option<Content>, RepositoryError>;

    async fn find_public_by_slug(
        &self,
        slug: &Slug,
        now: DateTime<Utc>,
    ) -> Result<Option<Content>, RepositoryError>;

    async fn resolve_redirect(&self, old_slug: &Slug) -> Result<Option<Slug>, RepositoryError>;

    async fn list_revisions(
        &self,
        content_id: ContentId,
    ) -> Result<Vec<ContentRevision>, RepositoryError>;

    async fn find_revision(
        &self,
        content_id: ContentId,
        revision_id: i64,
    ) -> Result<Option<ContentRevision>, RepositoryError>;

    async fn list_public_posts(
        &self,
        now: DateTime<Utc>,
        limit: u32,
        offset: u32,
    ) -> Result<Vec<Content>, RepositoryError>;

    async fn list_public_by_tag(
        &self,
        tag: &Slug,
        now: DateTime<Utc>,
    ) -> Result<Vec<Content>, RepositoryError>;

    async fn list_all_public(&self, now: DateTime<Utc>) -> Result<Vec<Content>, RepositoryError>;

    async fn list_all_content(&self) -> Result<Vec<Content>, RepositoryError>;
}

#[async_trait]
pub trait SiteRepository: Send + Sync {
    async fn site_settings(&self) -> Result<SiteSettings, RepositoryError>;
    async fn navigation(&self) -> Result<Vec<NavigationItem>, RepositoryError>;
    async fn save_configuration(
        &self,
        settings: &SiteSettings,
        navigation: &[NavigationItem],
        now: DateTime<Utc>,
    ) -> Result<(), RepositoryError>;
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum MediaRepositoryError {
    #[error("media storage failure: {0}")]
    Storage(String),
}

#[async_trait]
pub trait MediaRepository: Send + Sync {
    async fn save_media(&self, media: &MediaAsset) -> Result<MediaAsset, MediaRepositoryError>;
    async fn find_media(&self, id: &MediaId) -> Result<Option<MediaAsset>, MediaRepositoryError>;
    async fn list_media(&self) -> Result<Vec<MediaAsset>, MediaRepositoryError>;
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum AuthError {
    #[error("authentication storage failure: {0}")]
    Storage(String),
}

#[async_trait]
pub trait AuthRepository: Send + Sync {
    async fn store_setup_token(
        &self,
        token_hash: SecretHash,
        purpose: SetupPurpose,
        expires_at: DateTime<Utc>,
    ) -> Result<(), AuthError>;

    async fn consume_setup_token(
        &self,
        token_hash: SecretHash,
        purpose: SetupPurpose,
        now: DateTime<Utc>,
    ) -> Result<bool, AuthError>;

    async fn store_session(&self, session: &SessionRecord) -> Result<(), AuthError>;

    async fn find_session(
        &self,
        token_hash: SecretHash,
        now: DateTime<Utc>,
    ) -> Result<Option<SessionRecord>, AuthError>;

    async fn rotate_session(
        &self,
        old_token_hash: SecretHash,
        replacement: &SessionRecord,
        now: DateTime<Utc>,
    ) -> Result<bool, AuthError>;

    async fn replace_recovery_codes(
        &self,
        code_hashes: &[SecretHash],
        now: DateTime<Utc>,
    ) -> Result<(), AuthError>;

    async fn consume_recovery_code(
        &self,
        code_hash: SecretHash,
        now: DateTime<Utc>,
    ) -> Result<bool, AuthError>;

    async fn exchange_recovery_code(
        &self,
        code_hash: SecretHash,
        session: &SessionRecord,
        now: DateTime<Utc>,
    ) -> Result<bool, AuthError>;
}

pub struct SetupRegistration<'a> {
    pub setup_token_hash: SecretHash,
    pub purpose: SetupPurpose,
    pub user_handle: Uuid,
    pub passkey: &'a StoredPasskey,
    pub session: &'a SessionRecord,
    pub recovery_code_hashes: &'a [SecretHash],
    pub now: DateTime<Utc>,
}

#[async_trait]
pub trait PasskeyRepository: Send + Sync {
    async fn owner_handle(&self) -> Result<Option<Uuid>, AuthError>;

    async fn list_passkeys(&self) -> Result<Vec<StoredPasskey>, AuthError>;

    async fn setup_token_purpose(
        &self,
        token_hash: SecretHash,
        now: DateTime<Utc>,
    ) -> Result<Option<SetupPurpose>, AuthError>;

    /// Commits every durable effect of initial/recovery registration together.
    async fn complete_setup_registration(
        &self,
        registration: SetupRegistration<'_>,
    ) -> Result<bool, AuthError>;

    async fn complete_authentication(
        &self,
        credential_id: &[u8],
        passkey_json: &str,
        session: &SessionRecord,
        now: DateTime<Utc>,
    ) -> Result<bool, AuthError>;

    async fn add_passkey(
        &self,
        passkey: &StoredPasskey,
        now: DateTime<Utc>,
    ) -> Result<(), AuthError>;

    async fn remove_passkey(&self, credential_id: &[u8]) -> Result<bool, AuthError>;
}
