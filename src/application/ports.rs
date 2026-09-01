//! Small capability-oriented ports keep external libraries out of the domain.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use thiserror::Error;

use crate::application::site_compiler::SiteSnapshotV1;
use crate::domain::auth::{SecretHash, SessionRecord, SetupPurpose, StoredPasskey};
use crate::domain::content::{
    Content, ContentDraft, ContentId, ContentKind, ContentRevision, SaveIntent, Slug, Tag,
};
use crate::domain::media::{MediaAsset, MediaId};
use crate::domain::search::SearchTerms;
use crate::domain::theme::{NavigationItem, SiteSettings};
use crate::portable::PortableSiteV1;
use uuid::Uuid;

pub trait Clock: Send + Sync {
    fn now(&self) -> DateTime<Utc>;
}

#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
#[error("operating system entropy is unavailable")]
pub struct EntropyError;

pub trait EntropySource: Send + Sync {
    fn fill(&self, destination: &mut [u8]) -> Result<(), EntropyError>;
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

    /// The chronologically adjacent public posts (older, newer) around one
    /// post, for prev/next navigation. Pages take no part in the chain.
    async fn neighbor_posts(
        &self,
        id: ContentId,
        publish_at: DateTime<Utc>,
        now: DateTime<Utc>,
    ) -> Result<(Option<ContentLink>, Option<ContentLink>), RepositoryError>;
}

/// A minimal reference to another content item.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ContentLink {
    pub id: ContentId,
    pub slug: Slug,
    pub title: String,
}

/// One search result: normalized display text plus what the page needs.
#[derive(Clone, Debug)]
pub struct SearchHit {
    pub slug: Slug,
    pub title: String,
    pub body: String,
    pub kind: ContentKind,
    pub publish_at: DateTime<Utc>,
}

/// Full-text search over publicly visible content.
#[async_trait]
pub trait SearchRepository: Send + Sync {
    async fn search(
        &self,
        terms: &SearchTerms,
        now: DateTime<Utc>,
        limit: u32,
    ) -> Result<Vec<SearchHit>, RepositoryError>;
}

/// Per-content engagement totals, shown only to the owner.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Engagement {
    pub likes: u64,
    pub views: u64,
}

#[async_trait]
pub trait EngagementRepository: Send + Sync {
    /// Records one page view; failures must never fail the page render.
    async fn record_view(&self, id: ContentId) -> Result<(), RepositoryError>;

    /// Likes and views per content id, for the dashboard.
    async fn engagement_totals(
        &self,
    ) -> Result<std::collections::HashMap<i64, Engagement>, RepositoryError>;
}

/// Anonymous like counters for public content. Every operation is gated on the
/// content being publicly visible at `now`, so drafts cannot be probed by id.
#[async_trait]
pub trait LikeRepository: Send + Sync {
    /// Increments and returns the new count; `NotFound` if not publicly visible.
    async fn add_like(&self, id: ContentId, now: DateTime<Utc>) -> Result<u64, RepositoryError>;

    /// Decrements (never below zero) and returns the new count; `NotFound` if
    /// not publicly visible.
    async fn remove_like(&self, id: ContentId, now: DateTime<Utc>) -> Result<u64, RepositoryError>;

    /// Returns the current count; `NotFound` if not publicly visible.
    async fn like_count(&self, id: ContentId, now: DateTime<Utc>) -> Result<u64, RepositoryError>;
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PublicationState {
    pub revision: u64,
    pub next_publish_at: Option<DateTime<Utc>>,
}

/// One consistent source snapshot and its durable publication clock.
#[async_trait]
pub trait PublicSnapshotRepository: Send + Sync {
    async fn publication_state(&self) -> Result<PublicationState, RepositoryError>;

    /// Advances exactly once when one or more scheduled entries have become
    /// visible, and recomputes the next durable boundary in the same transaction.
    async fn advance_publication_clock(&self, now: DateTime<Utc>) -> Result<bool, RepositoryError>;

    /// Reads settings, navigation, visible content, redirects, media and the
    /// matching public revision from one database snapshot.
    async fn public_snapshot(
        &self,
        effective_at: DateTime<Utc>,
    ) -> Result<SiteSnapshotV1, RepositoryError>;
}

/// Complete, host-neutral durable state used to leave or enter a host.
///
/// Importers must rebuild every derived HTML field with the destination's
/// renderer, invalidate ephemeral authentication capabilities, and commit the
/// database replacement atomically.
#[async_trait]
pub trait PortableRepository: Send + Sync {
    /// Reads every portable table from one consistent database snapshot.
    async fn portable_site(
        &self,
        canonical_origin: &str,
        exported_at: DateTime<Utc>,
    ) -> Result<PortableSiteV1, RepositoryError>;

    /// Replaces all portable tables in one transaction.
    async fn replace_portable_site(
        &self,
        site: &PortableSiteV1,
        markdown: &dyn MarkdownRenderer,
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
    async fn delete_media(&self, id: &MediaId) -> Result<(), MediaRepositoryError>;
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum AuthError {
    #[error("authentication storage failure: {0}")]
    Storage(String),
    #[error("authentication entropy is unavailable")]
    EntropyUnavailable,
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
