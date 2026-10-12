//! Small capability-oriented ports keep external libraries out of the domain.

use std::collections::HashSet;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use thiserror::Error;

use crate::application::site_compiler::SiteSnapshotV1;
use crate::domain::auth::{
    PreviewLinkRecord, SecretHash, SessionRecord, SetupPurpose, StoredPasskey,
};
use crate::domain::content::{
    Content, ContentDraft, ContentId, ContentKind, ContentRevision, SaveIntent, Slug, Tag,
};
use crate::domain::media::{MediaAsset, MediaId};
use crate::domain::search::SearchTerms;
use crate::domain::theme::{NavigationItem, SettingsRevision, SiteSettings};
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

    /// Every piece, trashed ones included: the trash is still durable state
    /// (media it references must survive garbage collection, and the
    /// dashboard lists it). Callers that need only live content filter on
    /// [`Content::is_trashed`].
    async fn list_all_content(&self) -> Result<Vec<Content>, RepositoryError>;

    /// The chronologically adjacent public posts (older, newer) around one
    /// post, for prev/next navigation. Pages take no part in the chain.
    async fn neighbor_posts(
        &self,
        id: ContentId,
        publish_at: DateTime<Utc>,
        now: DateTime<Utc>,
    ) -> Result<(Option<ContentLink>, Option<ContentLink>), RepositoryError>;

    /// Moves content to the trash: it leaves every public query and the
    /// publication clock, but keeps its slug reserved and its media
    /// referenced. `expected_version` guards against a concurrent editor tab.
    /// Already trashed content is returned unchanged.
    async fn move_to_trash(
        &self,
        id: ContentId,
        expected_version: i64,
        now: DateTime<Utc>,
    ) -> Result<Content, RepositoryError>;

    /// Returns trashed content to exactly the publication state it had
    /// before. Live content is returned unchanged.
    async fn restore_from_trash(
        &self,
        id: ContentId,
        now: DateTime<Utc>,
    ) -> Result<Content, RepositoryError>;

    /// Hard-deletes content that is already in the trash; revisions, tags,
    /// redirects, engagement and the search row go with it. Live or absent
    /// content answers `NotFound`, so a stale page can never destroy
    /// restored work.
    async fn delete_permanently(&self, id: ContentId) -> Result<(), RepositoryError>;

    /// Every tag in use, most used first, then by name.
    async fn list_tag_usage(&self) -> Result<Vec<TagUsage>, RepositoryError>;
}

/// A tag and how many pieces carry it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TagUsage {
    pub name: String,
    pub count: u64,
}

/// Media identities still mentioned by stored revision snapshots, so a sweep
/// keeps what history may restore.
#[async_trait]
pub trait RevisionMediaReferences: Send + Sync {
    async fn revision_media_ids(&self) -> Result<HashSet<String>, RepositoryError>;
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
    ) -> Result<std::collections::HashMap<ContentId, Engagement>, RepositoryError>;
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
    /// Saves settings and navigation together and keeps the saved state as
    /// a revision (the newest fifty; an unchanged save adds none). The very
    /// first save also keeps the state it replaces.
    async fn save_configuration(
        &self,
        settings: &SiteSettings,
        navigation: &[NavigationItem],
        now: DateTime<Utc>,
    ) -> Result<(), RepositoryError>;

    /// Every kept state of the settings and navigation, newest first.
    async fn list_settings_revisions(&self) -> Result<Vec<SettingsRevision>, RepositoryError>;

    /// One kept state, or `None` when it was pruned or never existed.
    async fn find_settings_revision(
        &self,
        id: i64,
    ) -> Result<Option<SettingsRevision>, RepositoryError>;

    /// Every historical address and the piece it leads to, oldest address first.
    async fn list_redirects(&self) -> Result<Vec<RedirectEntry>, RepositoryError>;

    /// Points an old address (one imported from elsewhere, say) at a piece.
    /// `SlugTaken` when the address is active or already historical,
    /// `NotFound` when the piece does not exist. Advances the public revision.
    async fn add_redirect(
        &self,
        old_slug: &Slug,
        content_id: ContentId,
        now: DateTime<Utc>,
    ) -> Result<(), RepositoryError>;

    /// Forgets an old address; `false` when it was not known.
    async fn remove_redirect(
        &self,
        old_slug: &Slug,
        now: DateTime<Utc>,
    ) -> Result<bool, RepositoryError>;
}

/// One historical address with the piece it currently leads to.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RedirectEntry {
    pub old_slug: Slug,
    pub content_id: ContentId,
    pub slug: Slug,
    pub title: String,
    pub created_at: DateTime<Utc>,
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

    /// Replaces the alternative text of one asset; `false` when it does not
    /// exist. Alternative text is rendered into released pages, so the
    /// implementation advances the public revision in the same transaction.
    async fn update_media_alt_text(
        &self,
        id: &MediaId,
        alt_text: &str,
        now: DateTime<Utc>,
    ) -> Result<bool, MediaRepositoryError>;
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

    /// Ends one session immediately; `false` when no such session existed.
    async fn revoke_session(&self, token_hash: SecretHash) -> Result<bool, AuthError>;

    /// Moves a live session's expiry forward without changing its tokens;
    /// `false` when the session is unknown or already expired.
    async fn extend_session(
        &self,
        token_hash: SecretHash,
        expires_at: DateTime<Utc>,
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

/// Bearer capabilities that open one unpublished piece to whoever holds them.
#[async_trait]
pub trait PreviewLinkRepository: Send + Sync {
    async fn store_preview_link(&self, link: &PreviewLinkRecord) -> Result<(), AuthError>;

    /// The piece a live link opens; `None` when unknown or expired.
    async fn find_preview_link(
        &self,
        token_hash: SecretHash,
        now: DateTime<Utc>,
    ) -> Result<Option<ContentId>, AuthError>;

    /// Ends every link of one piece; answers how many existed.
    async fn revoke_preview_links(&self, content_id: ContentId) -> Result<u64, AuthError>;
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
