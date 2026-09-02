use std::str::FromStr;

use axum::{
    Form, Json,
    extract::{Multipart, Path, Query, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{DateTime, Duration, NaiveDateTime, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;
use webauthn_rs::prelude::{PublicKeyCredential, RegisterPublicKeyCredential};

use crate::{
    application::{content::SaveIntent, ports::RepositoryError, site::DEFAULT_THEME_CSS},
    domain::diff::{DiffLine, diff_lines},
    domain::{
        auth::{SessionIdentity, SessionSecrets, SetupPurpose, StoredPasskey},
        content::{Content, ContentDraft, ContentId, ContentKind, Publication, Slug},
        media::MediaId,
        theme::{Locale, NavigationItem, SiteSettings},
    },
    web::{AppState, WebError},
};

#[derive(Serialize)]
struct DashboardContext {
    csrf: String,
    contents: Vec<DashboardItem>,
    /// The active filter key: `all`, `draft`, `scheduled`, `public`, or `trash`.
    filter: &'static str,
    q: String,
    /// `q` percent-encoded for use inside the filter links.
    q_query: String,
    /// Which empty-state sentence applies when `contents` is empty.
    empty_key: &'static str,
}

#[derive(Serialize)]
struct DashboardItem {
    id: i64,
    title: String,
    slug: String,
    status: &'static str,
    views: u64,
    likes: u64,
    updated_at: String,
    publish_at: Option<String>,
    deleted_at: Option<String>,
    trashed: bool,
}

#[derive(Deserialize)]
pub struct DashboardQuery {
    #[serde(default)]
    status: String,
    #[serde(default)]
    q: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DashboardFilter {
    All,
    Draft,
    Scheduled,
    Public,
    Trash,
}

impl DashboardFilter {
    /// An unknown value shows everything rather than an error: a stale link
    /// should never strand the writer.
    fn parse(value: &str) -> Self {
        match value {
            "draft" => Self::Draft,
            "scheduled" => Self::Scheduled,
            "public" => Self::Public,
            "trash" => Self::Trash,
            _ => Self::All,
        }
    }

    const fn as_str(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::Draft => "draft",
            Self::Scheduled => "scheduled",
            Self::Public => "public",
            Self::Trash => "trash",
        }
    }

    fn admits(self, status: &str) -> bool {
        match self {
            Self::All => status != "trashed",
            Self::Trash => status == "trashed",
            other => status == other.as_str(),
        }
    }
}

/// Case-insensitive match on the title or slug; an empty query admits all.
fn dashboard_matches(query: &str, title: &str, slug: &str) -> bool {
    let query = query.trim().to_lowercase();
    query.is_empty()
        || title.to_lowercase().contains(&query)
        || slug.to_lowercase().contains(&query)
}

#[derive(Serialize)]
struct EditorContext {
    title: String,
    csrf: String,
    action: String,
    content_id: Option<i64>,
    version: Option<i64>,
    content: EditorContent,
    cover_url: Option<String>,
    /// The cover's stored alternative text, editable from the drawer.
    cover_alt_text: String,
    revisions: Vec<RevisionItem>,
    trashed: bool,
}

#[derive(Serialize)]
struct RevisionItem {
    id: i64,
    intent: &'static str,
    created_at: String,
}

#[derive(Serialize)]
struct EditorContent {
    kind: &'static str,
    title: String,
    slug: String,
    summary: String,
    body_markdown: String,
    tags: String,
    /// `draft`, `scheduled`, `public`, or `trashed`.
    status: &'static str,
    /// RFC 3339 publication instant, or empty for a draft.
    publish_at: String,
    /// The same instant shaped for a `datetime-local` control, in UTC; the
    /// browser script re-expresses it in the writer's own zone.
    publish_at_input: String,
    seo_title: String,
    seo_description: String,
    cover_media_id: String,
}

#[derive(Serialize)]
struct ConflictContext {
    csrf: String,
    current: ConflictVersion,
    submitted: ConflictVersion,
}

#[derive(Serialize)]
struct ConflictVersion {
    id: i64,
    title: String,
    body_markdown: String,
}

#[derive(Serialize)]
struct SettingsContext {
    csrf: String,
    settings: SiteSettings,
    navigation: String,
    logo_url: Option<String>,
    favicon_url: Option<String>,
    passkeys: Vec<PasskeyView>,
    reauth_ok: bool,
}

#[derive(Serialize)]
struct PasskeyView {
    name: String,
    credential_id: String,
}

#[derive(Serialize)]
struct RecoveryCodesContext<'a> {
    csrf: String,
    recovery_codes: Vec<&'a str>,
}

#[derive(Serialize)]
struct RevisionContext {
    csrf: String,
    content_id: i64,
    revision_id: i64,
    expected_version: i64,
    current: RevisionVersion,
    revision: RevisionVersion,
    diff: RevisionDiffContext,
}

#[derive(Serialize)]
struct RevisionVersion {
    title: String,
    body_markdown: String,
    version: i64,
}

#[derive(Serialize)]
struct RevisionDiffContext {
    /// Lines of the current text compared against the selected revision:
    /// what a restore would remove reads as `added`, what it would put back
    /// as `removed`.
    lines: Vec<DiffLine>,
    title_changed: bool,
}

#[derive(Deserialize)]
pub struct ContentForm {
    csrf: String,
    kind: String,
    title: String,
    slug: String,
    #[serde(default)]
    summary: String,
    body_markdown: String,
    #[serde(default)]
    tags: String,
    #[serde(default)]
    status: String,
    #[serde(default)]
    publish_at: String,
    #[serde(default)]
    seo_title: String,
    #[serde(default)]
    seo_description: String,
    #[serde(default)]
    cover_media_id: String,
    #[serde(default)]
    intent: String,
    version: Option<i64>,
}

#[derive(Deserialize)]
pub struct TrashForm {
    csrf: String,
    version: i64,
}

#[derive(Deserialize)]
pub struct MediaAltForm {
    csrf: String,
    #[serde(default)]
    alt_text: String,
}

#[derive(Deserialize)]
pub struct SiteSettingsForm {
    csrf: String,
    site_title: String,
    #[serde(default)]
    site_description: String,
    locale: String,
    #[serde(default)]
    logo_media_id: String,
    #[serde(default)]
    favicon_media_id: String,
    #[serde(default)]
    custom_css: String,
    #[serde(default)]
    navigation: String,
}

struct AdminIdentity {
    session: SessionIdentity,
    csrf: String,
}

#[derive(Deserialize)]
pub struct SetupPageQuery {
    token: String,
}

#[derive(Deserialize)]
pub struct LoginPageQuery {
    #[serde(default)]
    next: String,
}

#[derive(Deserialize)]
pub struct SetupStartRequest {
    token: String,
}

#[derive(Deserialize)]
pub struct SetupFinishRequest {
    token: String,
    flow_id: Uuid,
    #[serde(default = "default_passkey_name")]
    name: String,
    credential: RegisterPublicKeyCredential,
}

#[derive(Deserialize)]
pub struct LoginFinishRequest {
    flow_id: Uuid,
    credential: PublicKeyCredential,
}

#[derive(Deserialize)]
pub struct CsrfRequest {
    csrf: String,
}

#[derive(Deserialize)]
pub struct PasskeyAddFinishRequest {
    csrf: String,
    flow_id: Uuid,
    #[serde(default = "default_passkey_name")]
    name: String,
    credential: RegisterPublicKeyCredential,
}

#[derive(Deserialize)]
pub struct CsrfForm {
    csrf: String,
}

#[derive(Deserialize)]
pub struct RecoveryLoginForm {
    code: String,
}

#[derive(Deserialize)]
pub struct RemovePasskeyForm {
    csrf: String,
    credential_id: String,
}

#[derive(Deserialize)]
pub struct RestoreRevisionForm {
    csrf: String,
    version: i64,
}

#[derive(Deserialize)]
pub struct PreviewRequest {
    csrf: String,
    markdown: String,
}

pub async fn dashboard(
    State(state): State<AppState>,
    Query(query): Query<DashboardQuery>,
    headers: HeaderMap,
) -> Result<Response, WebError> {
    let identity = match authenticate(&state, &headers, None).await? {
        Ok(identity) => identity,
        Err(response) => return Ok(response),
    };
    let now = state.clock.now();
    let filter = DashboardFilter::parse(&query.status);
    let everything = state.content.list_all_content().await?;
    let any_content = everything.iter().any(|content| !content.is_trashed());
    let totals = state.engagement.engagement_totals().await?;
    let stamp = |at: DateTime<Utc>| at.to_rfc3339_opts(SecondsFormat::Secs, true);
    let contents: Vec<DashboardItem> = everything
        .into_iter()
        .filter(|content| filter.admits(content_status(content, now)))
        .filter(|content| dashboard_matches(&query.q, &content.title, content.slug.as_str()))
        .map(|content| {
            let engagement = totals.get(&content.id).copied().unwrap_or_default();
            DashboardItem {
                id: content.id.as_i64(),
                status: content_status(&content, now),
                updated_at: stamp(content.updated_at),
                publish_at: content.publication.publish_at().map(stamp),
                deleted_at: content.deleted_at.map(stamp),
                trashed: content.is_trashed(),
                title: content.title,
                slug: content.slug.to_string(),
                views: engagement.views,
                likes: engagement.likes,
            }
        })
        .collect();
    let empty_key = match filter {
        DashboardFilter::Trash => "dashboard.empty_trash",
        DashboardFilter::All if !any_content && query.q.trim().is_empty() => "dashboard.empty",
        _ => "dashboard.empty_filtered",
    };
    state
        .render_admin(
            "admin/dashboard.html",
            DashboardContext {
                csrf: identity.csrf,
                contents,
                filter: filter.as_str(),
                q_query: percent_encode_query(query.q.trim()),
                q: query.q.trim().to_owned(),
                empty_key,
            },
        )
        .await
}

pub async fn new_content(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Response, WebError> {
    let identity = match authenticate(&state, &headers, None).await? {
        Ok(identity) => identity,
        Err(response) => return Ok(response),
    };
    state
        .render_admin(
            "admin/editor.html",
            EditorContext {
                title: "New".into(),
                csrf: identity.csrf,
                action: "/admin/content/".into(),
                content_id: None,
                version: None,
                content: EditorContent::empty(state.clock.now()),
                cover_url: None,
                cover_alt_text: String::new(),
                revisions: Vec::new(),
                trashed: false,
            },
        )
        .await
}

pub async fn settings_page(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Response, WebError> {
    let identity = match authenticate(&state, &headers, None).await? {
        Ok(identity) => identity,
        Err(response) => return Ok(response),
    };
    let settings = state.site.site_settings().await?;
    let navigation = state
        .site
        .navigation()
        .await?
        .into_iter()
        .map(|item| format!("{} | {}", item.label, item.destination))
        .collect::<Vec<_>>()
        .join("\n");
    let logo_url = state
        .theme_media_url(settings.logo_media_id.as_deref())
        .await?;
    let favicon_url = state
        .theme_media_url(settings.favicon_media_id.as_deref())
        .await?;
    let passkeys = state
        .accounts
        .passkeys()
        .await
        .map_err(WebError::auth)?
        .into_iter()
        .map(|passkey| PasskeyView {
            name: passkey.name,
            credential_id: URL_SAFE_NO_PAD.encode(passkey.credential_id),
        })
        .collect();
    let reauth_ok = recently_reauthenticated(&identity, state.clock.now());
    state
        .render_admin(
            "admin/settings.html",
            SettingsContext {
                csrf: identity.csrf,
                settings,
                navigation,
                logo_url,
                favicon_url,
                passkeys,
                reauth_ok,
            },
        )
        .await
}

/// Puts the stock stylesheet back. The owner's edits are gone afterwards,
/// which is why the page asks twice before posting here.
pub async fn reset_theme(
    State(state): State<AppState>,
    headers: HeaderMap,
    Form(form): Form<CsrfForm>,
) -> Result<Response, WebError> {
    if let Err(response) = authenticate(&state, &headers, Some(&form.csrf)).await? {
        return Ok(response);
    }
    let mut settings = state.site.site_settings().await?;
    settings.custom_css = DEFAULT_THEME_CSS.to_owned();
    let navigation = state.site.navigation().await?;
    match state
        .site_service
        .update(settings, navigation, state.clock.now())
        .await
    {
        Ok(()) => {
            state.publish_now().await?;
            if wants_json(&headers) {
                Ok(Json(json!({ "ok": true })).into_response())
            } else {
                Ok(redirect(StatusCode::SEE_OTHER, "/admin/settings/"))
            }
        }
        Err(error) => application_error(&state, &headers, error).await,
    }
}

pub async fn update_settings(
    State(state): State<AppState>,
    headers: HeaderMap,
    Form(form): Form<SiteSettingsForm>,
) -> Result<Response, WebError> {
    if let Err(response) = authenticate(&state, &headers, Some(&form.csrf)).await? {
        return Ok(response);
    }
    let (settings, navigation) = match form.into_configuration() {
        Ok(configuration) => configuration,
        Err(message) => {
            return failure(&state, &headers, StatusCode::UNPROCESSABLE_ENTITY, &message).await;
        }
    };
    match state
        .site_service
        .update(settings, navigation, state.clock.now())
        .await
    {
        Ok(()) => {
            state.publish_now().await?;
            run_media_gc(&state).await;
            if wants_json(&headers) {
                Ok(Json(json!({ "ok": true })).into_response())
            } else {
                Ok(redirect(StatusCode::SEE_OTHER, "/admin/settings/"))
            }
        }
        Err(error) => application_error(&state, &headers, error).await,
    }
}

pub async fn edit_content(
    State(state): State<AppState>,
    Path(raw_id): Path<i64>,
    headers: HeaderMap,
) -> Result<Response, WebError> {
    let identity = match authenticate(&state, &headers, None).await? {
        Ok(identity) => identity,
        Err(response) => return Ok(response),
    };
    let Some(content) = state
        .content
        .find_by_id(ContentId::from_i64(raw_id))
        .await?
    else {
        return failure(
            &state,
            &headers,
            StatusCode::NOT_FOUND,
            "content does not exist",
        )
        .await;
    };
    let revisions = state
        .content
        .list_revisions(content.id)
        .await?
        .into_iter()
        .map(|revision| RevisionItem {
            id: revision.id,
            intent: revision.intent.as_str(),
            created_at: revision
                .created_at
                .to_rfc3339_opts(SecondsFormat::Secs, true),
        })
        .collect();
    let cover_url = state
        .theme_media_url(content.cover_media_id.as_deref())
        .await?;
    let cover_alt_text = match content.cover_media_id.as_deref().map(MediaId::parse) {
        Some(Ok(id)) => state
            .media_repository
            .find_media(&id)
            .await
            .map_err(WebError::media_repository)?
            .map(|asset| asset.alt_text)
            .unwrap_or_default(),
        _ => String::new(),
    };
    state
        .render_admin(
            "admin/editor.html",
            EditorContext {
                title: content.title.clone(),
                csrf: identity.csrf,
                action: format!("/admin/content/{raw_id}/"),
                content_id: Some(raw_id),
                version: Some(content.version),
                trashed: content.is_trashed(),
                content: EditorContent::from_content(&content, state.clock.now()),
                cover_url,
                cover_alt_text,
                revisions,
            },
        )
        .await
}

/// Alternative text lives on the media record and is baked into every page
/// that shows the image, so a change republishes the site.
pub async fn update_media(
    State(state): State<AppState>,
    Path(raw_id): Path<String>,
    headers: HeaderMap,
    Form(form): Form<MediaAltForm>,
) -> Result<Response, WebError> {
    if let Err(response) = authenticate(&state, &headers, Some(&form.csrf)).await? {
        return Ok(response);
    }
    let Ok(id) = MediaId::parse(&raw_id) else {
        return failure(
            &state,
            &headers,
            StatusCode::NOT_FOUND,
            "media does not exist",
        )
        .await;
    };
    match state
        .media_service
        .update_alt_text(&id, &form.alt_text, state.clock.now())
        .await
    {
        Ok(true) => {
            state.publish_now().await?;
            if wants_json(&headers) {
                Ok(Json(json!({ "ok": true, "alt_text": form.alt_text.trim() })).into_response())
            } else {
                Ok(redirect(StatusCode::SEE_OTHER, "/admin/"))
            }
        }
        Ok(false) => {
            failure(
                &state,
                &headers,
                StatusCode::NOT_FOUND,
                "media does not exist",
            )
            .await
        }
        Err(crate::infrastructure::media::MediaError::InvalidMetadata(message)) => {
            failure(&state, &headers, StatusCode::UNPROCESSABLE_ENTITY, &message).await
        }
        Err(error) => Err(WebError::media(error)),
    }
}

pub async fn trash_content(
    State(state): State<AppState>,
    Path(raw_id): Path<i64>,
    headers: HeaderMap,
    Form(form): Form<TrashForm>,
) -> Result<Response, WebError> {
    if let Err(response) = authenticate(&state, &headers, Some(&form.csrf)).await? {
        return Ok(response);
    }
    let now = state.clock.now();
    match state
        .content_service
        .move_to_trash(ContentId::from_i64(raw_id), form.version, now)
        .await
    {
        Ok(_) => {
            // The route is withdrawn by the next release; media stays
            // referenced by the trashed piece, so no garbage collection.
            state.publish_now().await?;
            if wants_json(&headers) {
                Ok(Json(json!({ "ok": true })).into_response())
            } else {
                Ok(redirect(StatusCode::SEE_OTHER, "/admin/?status=trash"))
            }
        }
        Err(RepositoryError::Conflict { .. }) => {
            failure(
                &state,
                &headers,
                StatusCode::CONFLICT,
                "content changed after this page was opened",
            )
            .await
        }
        Err(RepositoryError::NotFound) => {
            failure(
                &state,
                &headers,
                StatusCode::NOT_FOUND,
                "content does not exist",
            )
            .await
        }
        Err(error) => application_error(&state, &headers, error).await,
    }
}

pub async fn restore_content(
    State(state): State<AppState>,
    Path(raw_id): Path<i64>,
    headers: HeaderMap,
    Form(form): Form<CsrfForm>,
) -> Result<Response, WebError> {
    if let Err(response) = authenticate(&state, &headers, Some(&form.csrf)).await? {
        return Ok(response);
    }
    match state
        .content_service
        .restore_from_trash(ContentId::from_i64(raw_id), state.clock.now())
        .await
    {
        Ok(_) => {
            state.publish_now().await?;
            if wants_json(&headers) {
                Ok(Json(json!({ "ok": true })).into_response())
            } else {
                Ok(redirect(
                    StatusCode::SEE_OTHER,
                    &format!("/admin/content/{raw_id}/edit/"),
                ))
            }
        }
        Err(RepositoryError::NotFound) => {
            failure(
                &state,
                &headers,
                StatusCode::NOT_FOUND,
                "content does not exist",
            )
            .await
        }
        Err(error) => application_error(&state, &headers, error).await,
    }
}

pub async fn delete_content(
    State(state): State<AppState>,
    Path(raw_id): Path<i64>,
    headers: HeaderMap,
    Form(form): Form<CsrfForm>,
) -> Result<Response, WebError> {
    if let Err(response) = authenticate(&state, &headers, Some(&form.csrf)).await? {
        return Ok(response);
    }
    match state
        .content_service
        .delete_permanently(ContentId::from_i64(raw_id))
        .await
    {
        Ok(()) => {
            // A trashed piece was already outside the release; only its media
            // references disappear now.
            run_media_gc(&state).await;
            if wants_json(&headers) {
                Ok(Json(json!({ "ok": true })).into_response())
            } else {
                Ok(redirect(StatusCode::SEE_OTHER, "/admin/?status=trash"))
            }
        }
        Err(RepositoryError::NotFound) => {
            failure(
                &state,
                &headers,
                StatusCode::NOT_FOUND,
                "content does not exist",
            )
            .await
        }
        Err(error) => application_error(&state, &headers, error).await,
    }
}

pub async fn revision_page(
    State(state): State<AppState>,
    Path((raw_id, revision_id)): Path<(i64, i64)>,
    headers: HeaderMap,
) -> Result<Response, WebError> {
    let identity = match authenticate(&state, &headers, None).await? {
        Ok(identity) => identity,
        Err(response) => return Ok(response),
    };
    let id = ContentId::from_i64(raw_id);
    let Some(current) = state.content.find_by_id(id).await? else {
        return failure(
            &state,
            &headers,
            StatusCode::NOT_FOUND,
            "content does not exist",
        )
        .await;
    };
    let Some(revision) = state.content.find_revision(id, revision_id).await? else {
        return failure(
            &state,
            &headers,
            StatusCode::NOT_FOUND,
            "content does not exist",
        )
        .await;
    };
    state
        .render_admin(
            "admin/revision.html",
            RevisionContext {
                csrf: identity.csrf,
                content_id: raw_id,
                revision_id,
                expected_version: current.version,
                diff: RevisionDiffContext {
                    lines: diff_lines(&revision.snapshot.body_markdown, &current.body_markdown),
                    title_changed: revision.snapshot.title != current.title,
                },
                current: RevisionVersion {
                    title: current.title,
                    body_markdown: current.body_markdown,
                    version: current.version,
                },
                revision: RevisionVersion {
                    title: revision.snapshot.title,
                    body_markdown: revision.snapshot.body_markdown,
                    version: revision.snapshot.version,
                },
            },
        )
        .await
}

pub async fn restore_revision(
    State(state): State<AppState>,
    Path((raw_id, revision_id)): Path<(i64, i64)>,
    headers: HeaderMap,
    Form(form): Form<RestoreRevisionForm>,
) -> Result<Response, WebError> {
    if let Err(response) = authenticate(&state, &headers, Some(&form.csrf)).await? {
        return Ok(response);
    }
    match state
        .content_service
        .restore_revision(
            ContentId::from_i64(raw_id),
            revision_id,
            form.version,
            state.clock.now(),
        )
        .await
    {
        Ok(_) => {
            state.publish_now().await?;
            run_media_gc(&state).await;
            Ok(redirect(
                StatusCode::SEE_OTHER,
                &format!("/admin/content/{raw_id}/edit/"),
            ))
        }
        Err(RepositoryError::Conflict { .. }) => {
            failure(
                &state,
                &headers,
                StatusCode::CONFLICT,
                "content changed after this restore page was opened",
            )
            .await
        }
        Err(error) => application_error(&state, &headers, error).await,
    }
}

pub async fn preview_markdown(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<PreviewRequest>,
) -> Result<Response, WebError> {
    if let Err(response) = authenticate(&state, &headers, Some(&request.csrf)).await? {
        return Ok(response);
    }
    match state.content_service.preview(&request.markdown) {
        Ok(rendered) => Ok(Json(json!({ "html": rendered.html })).into_response()),
        Err(RepositoryError::Validation(message)) => {
            Ok((StatusCode::UNPROCESSABLE_ENTITY, message).into_response())
        }
        Err(error) => application_error(&state, &headers, error).await,
    }
}

pub async fn create_content(
    State(state): State<AppState>,
    headers: HeaderMap,
    Form(form): Form<ContentForm>,
) -> Result<Response, WebError> {
    if let Err(response) = authenticate(&state, &headers, Some(&form.csrf)).await? {
        return Ok(response);
    }
    let now = state.clock.now();
    let (draft, intent) = match form.to_draft(now, None) {
        Ok(value) => value,
        Err(error) => {
            return failure(&state, &headers, StatusCode::UNPROCESSABLE_ENTITY, &error).await;
        }
    };
    let mut result = state
        .content_service
        .create(draft.clone(), intent, now)
        .await;
    // A generated slug can collide when two pieces are created within the same
    // minute; retry once at second resolution before surfacing the conflict.
    if matches!(result, Err(RepositoryError::SlugTaken(_))) && draft.slug.is_timestamped() {
        let retry = ContentDraft {
            slug: Slug::timestamped_precise(now),
            ..draft
        };
        result = state.content_service.create(retry, intent, now).await;
    }
    match result {
        Ok(content) => {
            state.publish_now().await?;
            run_media_gc(&state).await;
            if intent == SaveIntent::Autosave || wants_json(&headers) {
                Ok((StatusCode::CREATED, Json(save_response(&content, now))).into_response())
            } else {
                Ok(redirect(
                    StatusCode::SEE_OTHER,
                    &format!("/admin/content/{}/edit/", content.id),
                ))
            }
        }
        Err(error) => application_error(&state, &headers, error).await,
    }
}

/// Runs after any successful save: media that nothing current references is
/// removed immediately. Failure never fails the save that triggered it.
async fn run_media_gc(state: &AppState) {
    let result = async {
        let contents = state
            .content
            .list_all_content()
            .await
            .map_err(|error| error.to_string())?;
        let settings = state
            .site
            .site_settings()
            .await
            .map_err(|error| error.to_string())?;
        let referenced = crate::application::media_gc::referenced_media_ids(&contents, &settings);
        state
            .media_service
            .collect_garbage(&referenced)
            .await
            .map_err(|error| error.to_string())
    }
    .await;
    match result {
        Ok(0) => {}
        Ok(removed) => tracing::info!(event = "media.gc.removed", removed),
        Err(error) => tracing::warn!(event = "media.gc.failed", error),
    }
}

pub async fn update_content(
    State(state): State<AppState>,
    Path(raw_id): Path<i64>,
    headers: HeaderMap,
    Form(form): Form<ContentForm>,
) -> Result<Response, WebError> {
    let identity = match authenticate(&state, &headers, Some(&form.csrf)).await? {
        Ok(identity) => identity,
        Err(response) => return Ok(response),
    };
    let Some(expected_version) = form.version else {
        return failure(
            &state,
            &headers,
            StatusCode::BAD_REQUEST,
            "missing content version",
        )
        .await;
    };
    let now = state.clock.now();
    let id = ContentId::from_i64(raw_id);
    let Some(existing) = state.content.find_by_id(id).await? else {
        return failure(
            &state,
            &headers,
            StatusCode::NOT_FOUND,
            "content does not exist",
        )
        .await;
    };
    let (draft, intent) = match form.to_draft(now, Some(&existing)) {
        Ok(value) => value,
        Err(error) => {
            return failure(&state, &headers, StatusCode::UNPROCESSABLE_ENTITY, &error).await;
        }
    };
    let submitted_title = draft.title.clone();
    let submitted_body = draft.body_markdown.clone();
    match state
        .content_service
        .update(id, expected_version, draft, intent, now)
        .await
    {
        Ok(content) => {
            state.publish_now().await?;
            run_media_gc(&state).await;
            if intent == SaveIntent::Autosave || wants_json(&headers) {
                Ok(Json(save_response(&content, now)).into_response())
            } else {
                Ok(redirect(
                    StatusCode::SEE_OTHER,
                    &format!("/admin/content/{raw_id}/edit/"),
                ))
            }
        }
        Err(RepositoryError::Conflict { .. }) => {
            let Some(current) = state.content.find_by_id(id).await? else {
                return failure(
                    &state,
                    &headers,
                    StatusCode::NOT_FOUND,
                    "content does not exist",
                )
                .await;
            };
            let html = state
                .render_admin_string(
                    "admin/conflict.html",
                    ConflictContext {
                        csrf: identity.csrf,
                        current: ConflictVersion {
                            id: current.id.as_i64(),
                            title: current.title,
                            body_markdown: current.body_markdown,
                        },
                        submitted: ConflictVersion {
                            id: raw_id,
                            title: submitted_title,
                            body_markdown: submitted_body,
                        },
                    },
                )
                .await?;
            Ok((StatusCode::CONFLICT, axum::response::Html(html)).into_response())
        }
        Err(error) => application_error(&state, &headers, error).await,
    }
}

pub async fn login_page(
    State(state): State<AppState>,
    Query(query): Query<LoginPageQuery>,
) -> Result<Response, WebError> {
    state
        .render_admin(
            "admin/login.html",
            json!({ "next": safe_admin_path(&query.next).unwrap_or("/admin/") }),
        )
        .await
}

pub async fn logout(
    State(state): State<AppState>,
    headers: HeaderMap,
    Form(form): Form<CsrfForm>,
) -> Result<Response, WebError> {
    if let Err(response) = authenticate(&state, &headers, Some(&form.csrf)).await? {
        return Ok(response);
    }
    // `authenticate` has just proven the cookie exists and names a live session.
    let Some(token) = cookie(&headers, "sb_session") else {
        return Ok(login_redirect(None));
    };
    let revoked = state.auth.logout(token).await.map_err(WebError::auth)?;
    tracing::info!(event = "auth.session.logged_out", revoked);
    let mut response = redirect(StatusCode::SEE_OTHER, "/admin/login/");
    clear_auth_cookies(&mut response, state.secure_cookies())?;
    Ok(response)
}

pub async fn setup_page(
    State(state): State<AppState>,
    Query(query): Query<SetupPageQuery>,
    headers: HeaderMap,
) -> Result<Response, WebError> {
    if state
        .accounts
        .setup_context(&query.token, state.clock.now())
        .await
        .map_err(WebError::auth)?
        .is_none()
    {
        return failure(
            &state,
            &headers,
            StatusCode::GONE,
            "This setup link is invalid or expired.",
        )
        .await;
    }
    state
        .render_admin("admin/setup.html", json!({ "token": query.token }))
        .await
}

pub async fn setup_start(
    State(state): State<AppState>,
    Json(request): Json<SetupStartRequest>,
) -> Result<Response, WebError> {
    let Some(context) = state
        .accounts
        .setup_context(&request.token, state.clock.now())
        .await
        .map_err(WebError::auth)?
    else {
        return Ok(json_error(StatusCode::UNAUTHORIZED, "invalid setup token"));
    };
    let start = state
        .webauthn
        .start_registration(
            context.user_handle,
            &context.excluded_credentials,
            state.clock.now(),
        )
        .map_err(WebError::passkey)?;
    Ok(Json(json!({ "flow_id": start.flow_id, "options": start.public })).into_response())
}

pub async fn setup_finish(
    State(state): State<AppState>,
    Json(request): Json<SetupFinishRequest>,
) -> Result<Response, WebError> {
    let Some(context) = state
        .accounts
        .setup_context(&request.token, state.clock.now())
        .await
        .map_err(WebError::auth)?
    else {
        return Ok(json_error(StatusCode::UNAUTHORIZED, "invalid setup token"));
    };
    let registered = state
        .webauthn
        .finish_registration(request.flow_id, &request.credential, state.clock.now())
        .map_err(WebError::passkey)?;
    // Recovery must register under the persisted owner handle. Initial setup has
    // no owner yet: the authoritative handle is the one minted at setup start and
    // carried in the server-side ceremony state (setup_context mints a fresh
    // handle per call, so comparing against it here would always fail).
    if context.purpose == SetupPurpose::Recovery && registered.user_handle != context.user_handle {
        return Ok(json_error(
            StatusCode::UNAUTHORIZED,
            "registration mismatch",
        ));
    }
    let name = clean_passkey_name(&request.name);
    let Some(completed) = state
        .accounts
        .complete_setup_registration(
            &request.token,
            context.purpose,
            registered.user_handle,
            StoredPasskey {
                credential_id: registered.credential_id,
                name,
                passkey_json: registered.passkey_json,
            },
            state.clock.now(),
        )
        .await
        .map_err(WebError::auth)?
    else {
        return Ok(json_error(
            StatusCode::UNAUTHORIZED,
            "setup token was consumed",
        ));
    };
    let recovery_codes: Vec<_> = completed
        .recovery_codes
        .iter()
        .map(crate::domain::auth::SecretToken::expose)
        .collect();
    let mut response =
        Json(json!({ "ok": true, "recovery_codes": recovery_codes })).into_response();
    set_auth_cookies(&mut response, &completed.session, state.secure_cookies())?;
    Ok(response)
}

pub async fn login_start(State(state): State<AppState>) -> Result<Response, WebError> {
    let passkeys = state.accounts.passkeys().await.map_err(WebError::auth)?;
    let json: Vec<_> = passkeys
        .into_iter()
        .map(|passkey| passkey.passkey_json)
        .collect();
    let start = state
        .webauthn
        .start_authentication(&json, state.clock.now())
        .map_err(WebError::passkey)?;
    Ok(Json(json!({ "flow_id": start.flow_id, "options": start.public })).into_response())
}

pub async fn login_finish(
    State(state): State<AppState>,
    Json(request): Json<LoginFinishRequest>,
) -> Result<Response, WebError> {
    let authenticated = state
        .webauthn
        .finish_authentication(request.flow_id, &request.credential, state.clock.now())
        .map_err(WebError::passkey)?;
    let Some(session) = state
        .accounts
        .complete_authentication(
            &authenticated.credential_id,
            &authenticated.passkey_json,
            state.clock.now(),
        )
        .await
        .map_err(WebError::auth)?
    else {
        return Ok(json_error(
            StatusCode::UNAUTHORIZED,
            "passkey no longer exists",
        ));
    };
    let mut response = Json(json!({ "ok": true })).into_response();
    set_auth_cookies(&mut response, &session, state.secure_cookies())?;
    Ok(response)
}

pub async fn recovery_login(
    State(state): State<AppState>,
    Form(form): Form<RecoveryLoginForm>,
) -> Result<Response, WebError> {
    let Some(session) = state
        .auth
        .recover_session(form.code.trim(), state.clock.now())
        .await
        .map_err(WebError::auth)?
    else {
        return Ok((StatusCode::UNAUTHORIZED, "invalid recovery code").into_response());
    };
    let mut response = redirect(StatusCode::SEE_OTHER, "/admin/settings/");
    set_auth_cookies(&mut response, &session, state.secure_cookies())?;
    Ok(response)
}

pub async fn passkey_add_start(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<CsrfRequest>,
) -> Result<Response, WebError> {
    let identity = match authenticate(&state, &headers, Some(&request.csrf)).await? {
        Ok(identity) => identity,
        Err(response) => return Ok(response),
    };
    if !recently_reauthenticated(&identity, state.clock.now()) {
        return Ok(json_error(
            StatusCode::UNAUTHORIZED,
            "reauthentication required",
        ));
    }
    let Some(context) = state
        .accounts
        .owner_registration_context()
        .await
        .map_err(WebError::auth)?
    else {
        return Ok(json_error(StatusCode::CONFLICT, "owner is not configured"));
    };
    let start = state
        .webauthn
        .start_registration(
            context.user_handle,
            &context.excluded_credentials,
            state.clock.now(),
        )
        .map_err(WebError::passkey)?;
    Ok(Json(json!({ "flow_id": start.flow_id, "options": start.public })).into_response())
}

pub async fn passkey_add_finish(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<PasskeyAddFinishRequest>,
) -> Result<Response, WebError> {
    let identity = match authenticate(&state, &headers, Some(&request.csrf)).await? {
        Ok(identity) => identity,
        Err(response) => return Ok(response),
    };
    if !recently_reauthenticated(&identity, state.clock.now()) {
        return Ok(json_error(
            StatusCode::UNAUTHORIZED,
            "reauthentication required",
        ));
    }
    let Some(context) = state
        .accounts
        .owner_registration_context()
        .await
        .map_err(WebError::auth)?
    else {
        return Ok(json_error(StatusCode::CONFLICT, "owner is not configured"));
    };
    let registered = state
        .webauthn
        .finish_registration(request.flow_id, &request.credential, state.clock.now())
        .map_err(WebError::passkey)?;
    if registered.user_handle != context.user_handle {
        return Ok(json_error(
            StatusCode::UNAUTHORIZED,
            "registration mismatch",
        ));
    }
    state
        .accounts
        .add_passkey(
            &StoredPasskey {
                credential_id: registered.credential_id,
                name: clean_passkey_name(&request.name),
                passkey_json: registered.passkey_json,
            },
            state.clock.now(),
        )
        .await
        .map_err(WebError::auth)?;
    Ok(Json(json!({ "ok": true })).into_response())
}

pub async fn regenerate_recovery_codes(
    State(state): State<AppState>,
    headers: HeaderMap,
    Form(form): Form<CsrfForm>,
) -> Result<Response, WebError> {
    let identity = match authenticate(&state, &headers, Some(&form.csrf)).await? {
        Ok(identity) => identity,
        Err(response) => return Ok(response),
    };
    if !recently_reauthenticated(&identity, state.clock.now()) {
        return failure(
            &state,
            &headers,
            StatusCode::UNAUTHORIZED,
            "reauthentication required",
        )
        .await;
    }
    let recovery_codes = state
        .auth
        .replace_recovery_codes(state.clock.now())
        .await
        .map_err(WebError::auth)?;
    let recovery_codes = recovery_codes
        .iter()
        .map(crate::domain::auth::SecretToken::expose)
        .collect();
    state
        .render_admin(
            "admin/recovery_codes.html",
            RecoveryCodesContext {
                csrf: identity.csrf,
                recovery_codes,
            },
        )
        .await
}

pub async fn remove_passkey(
    State(state): State<AppState>,
    headers: HeaderMap,
    Form(form): Form<RemovePasskeyForm>,
) -> Result<Response, WebError> {
    let identity = match authenticate(&state, &headers, Some(&form.csrf)).await? {
        Ok(identity) => identity,
        Err(response) => return Ok(response),
    };
    if !recently_reauthenticated(&identity, state.clock.now()) {
        return failure(
            &state,
            &headers,
            StatusCode::UNAUTHORIZED,
            "reauthentication required",
        )
        .await;
    }
    let Ok(credential_id) = URL_SAFE_NO_PAD.decode(&form.credential_id) else {
        return failure(
            &state,
            &headers,
            StatusCode::BAD_REQUEST,
            "invalid credential ID",
        )
        .await;
    };
    if state
        .accounts
        .remove_passkey(&credential_id)
        .await
        .map_err(WebError::auth)?
    {
        Ok(redirect(StatusCode::SEE_OTHER, "/admin/settings/"))
    } else {
        failure(
            &state,
            &headers,
            StatusCode::CONFLICT,
            "the last Passkey cannot be removed",
        )
        .await
    }
}

pub async fn upload_media(
    State(state): State<AppState>,
    headers: HeaderMap,
    mut multipart: Multipart,
) -> Result<Response, WebError> {
    if let Err(response) = authenticate(&state, &headers, None).await? {
        return Ok(response);
    }
    let mut csrf = None;
    let mut alt_text = String::new();
    let mut caption = String::new();
    let mut upload = None;
    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|error| WebError::Internal(error.to_string()))?
    {
        let name = field.name().unwrap_or_default().to_owned();
        match name.as_str() {
            "csrf" => {
                csrf = Some(
                    field
                        .text()
                        .await
                        .map_err(|error| WebError::Internal(error.to_string()))?,
                );
            }
            "alt_text" => {
                alt_text = field
                    .text()
                    .await
                    .map_err(|error| WebError::Internal(error.to_string()))?;
            }
            "caption" => {
                caption = field
                    .text()
                    .await
                    .map_err(|error| WebError::Internal(error.to_string()))?;
            }
            "file" => {
                let filename = field.file_name().unwrap_or("upload").to_owned();
                let bytes = field
                    .bytes()
                    .await
                    .map_err(|error| WebError::Internal(error.to_string()))?;
                upload = Some((filename, bytes.to_vec()));
            }
            _ => {}
        }
    }
    let Some(csrf) = csrf else {
        return Ok(StatusCode::FORBIDDEN.into_response());
    };
    if let Err(response) = authenticate(&state, &headers, Some(&csrf)).await? {
        return Ok(response);
    }
    let Some((filename, bytes)) = upload else {
        return Ok((StatusCode::BAD_REQUEST, "missing image file").into_response());
    };
    let asset = match state
        .media_service
        .store(&filename, bytes, &alt_text, &caption, state.clock.now())
        .await
    {
        Ok(asset) => asset,
        Err(crate::infrastructure::media::MediaError::TooLarge { .. }) => {
            return Ok(
                (StatusCode::PAYLOAD_TOO_LARGE, "image exceeds upload limit").into_response(),
            );
        }
        Err(
            crate::infrastructure::media::MediaError::UnsupportedType
            | crate::infrastructure::media::MediaError::InvalidImage(_)
            | crate::infrastructure::media::MediaError::PixelLimit,
        ) => {
            return Ok((StatusCode::UNPROCESSABLE_ENTITY, "invalid image").into_response());
        }
        Err(error) => return Err(WebError::media(error)),
    };
    Ok((
        StatusCode::CREATED,
        Json(json!({
            "id": asset.id,
            "url": format!("/media/{}", asset.original_filename),
            "alt_text": asset.alt_text,
            "width": asset.width,
            "height": asset.height,
            "variants": asset.variants,
        })),
    )
        .into_response())
}

pub async fn admin_css() -> Response {
    asset_response(
        include_str!("../../static/admin.css"),
        "text/css; charset=utf-8",
    )
}

pub async fn admin_js() -> Response {
    asset_response(
        include_str!(concat!(env!("OUT_DIR"), "/admin.js")),
        "text/javascript; charset=utf-8",
    )
}

impl ContentForm {
    fn to_draft(
        &self,
        now: DateTime<Utc>,
        current: Option<&Content>,
    ) -> Result<(ContentDraft, SaveIntent), String> {
        let kind = ContentKind::from_str(&self.kind).map_err(str::to_owned)?;
        let slug = if self.slug.trim().is_empty() {
            current.map_or_else(|| Slug::timestamped(now), |content| content.slug.clone())
        } else {
            Slug::parse(&self.slug).map_err(|error| error.to_string())?
        };
        let requested = parse_publish_at(&self.publish_at)?;
        let publication = publication_for(&self.status, requested, current, now)?;
        let intent = if self.intent == "autosave" {
            SaveIntent::Autosave
        } else {
            SaveIntent::Explicit
        };
        Ok((
            ContentDraft {
                kind,
                title: self.title.clone(),
                slug,
                summary: self.summary.clone(),
                body_markdown: self.body_markdown.clone(),
                tags: self.tags.split(',').map(str::to_owned).collect(),
                cover_media_id: optional_text(&self.cover_media_id),
                seo_title: Some(self.seo_title.clone()),
                seo_description: Some(self.seo_description.clone()),
                publication,
            },
            intent,
        ))
    }
}

impl SiteSettingsForm {
    fn into_configuration(self) -> Result<(SiteSettings, Vec<NavigationItem>), String> {
        let locale = match self.locale.as_str() {
            "en" => Locale::En,
            "ja" => Locale::Ja,
            "zh" => Locale::Zh,
            _ => return Err("unknown locale".into()),
        };
        let navigation = self
            .navigation
            .lines()
            .enumerate()
            .filter(|(_, line)| !line.trim().is_empty())
            .map(|(position, line)| {
                let (label, destination) = line
                    .split_once('|')
                    .ok_or_else(|| format!("navigation line {} must contain |", position + 1))?;
                let destination = destination.trim().to_owned();
                Ok(NavigationItem {
                    id: 0,
                    label: label.trim().to_owned(),
                    is_external: destination.starts_with("https://")
                        || destination.starts_with("http://"),
                    destination,
                    position: u16::try_from(position)
                        .map_err(|_| "too many navigation items".to_owned())?,
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
        Ok((
            SiteSettings {
                site_title: self.site_title,
                site_description: self.site_description,
                locale,
                logo_media_id: optional_text(&self.logo_media_id),
                favicon_media_id: optional_text(&self.favicon_media_id),
                custom_css: self.custom_css,
            },
            navigation,
        ))
    }
}

fn optional_text(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_owned())
}

impl EditorContent {
    fn empty(now: DateTime<Utc>) -> Self {
        Self {
            kind: "post",
            title: String::new(),
            slug: Slug::timestamped(now).into(),
            summary: String::new(),
            body_markdown: String::new(),
            tags: String::new(),
            status: "draft",
            publish_at: String::new(),
            publish_at_input: String::new(),
            seo_title: String::new(),
            seo_description: String::new(),
            cover_media_id: String::new(),
        }
    }

    fn from_content(content: &Content, now: DateTime<Utc>) -> Self {
        let publish_at = content.publication.publish_at();
        Self {
            kind: content.kind.as_str(),
            title: content.title.clone(),
            slug: content.slug.to_string(),
            summary: content.summary.clone(),
            body_markdown: content.body_markdown.clone(),
            tags: content
                .tags
                .iter()
                .map(|tag| tag.name.as_str())
                .collect::<Vec<_>>()
                .join(", "),
            status: content_status(content, now),
            publish_at: publish_at
                .map(|at| at.to_rfc3339_opts(SecondsFormat::Secs, true))
                .unwrap_or_default(),
            publish_at_input: publish_at
                .map(|at| at.format("%Y-%m-%dT%H:%M").to_string())
                .unwrap_or_default(),
            seo_title: content.seo_title.clone().unwrap_or_default(),
            seo_description: content.seo_description.clone().unwrap_or_default(),
            cover_media_id: content.cover_media_id.clone().unwrap_or_default(),
        }
    }
}

async fn authenticate(
    state: &AppState,
    headers: &HeaderMap,
    csrf: Option<&str>,
) -> Result<Result<AdminIdentity, Response>, WebError> {
    // Only a page the browser navigated to is worth returning to after login;
    // form posts and JSON calls fall back to the dashboard.
    let next = csrf.is_none().then(|| requested_path(headers)).flatten();
    let Some(session_token) = cookie(headers, "sb_session") else {
        return Ok(Err(login_redirect(next.as_deref())));
    };
    let Some(csrf_cookie) = cookie(headers, "sb_csrf") else {
        return Ok(Err(login_redirect(next.as_deref())));
    };
    let Some(identity) = state
        .auth
        .authenticate(session_token, state.clock.now())
        .await
        .map_err(WebError::auth)?
    else {
        return Ok(Err(login_redirect(next.as_deref())));
    };
    if let Some(presented) = csrf
        && (presented != csrf_cookie || !state.auth.verify_csrf(&identity, presented))
    {
        return Ok(Err(StatusCode::FORBIDDEN.into_response()));
    }
    Ok(Ok(AdminIdentity {
        session: identity,
        csrf: csrf_cookie.to_owned(),
    }))
}

fn recently_reauthenticated(identity: &AdminIdentity, now: DateTime<Utc>) -> bool {
    identity
        .session
        .was_reauthenticated_within(now, Duration::minutes(5))
}

fn wants_json(headers: &HeaderMap) -> bool {
    headers
        .get(header::ACCEPT)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|accept| accept.contains("application/json"))
}

fn cookie<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers
        .get(header::COOKIE)?
        .to_str()
        .ok()?
        .split(';')
        .filter_map(|part| part.trim().split_once('='))
        .find_map(|(key, value)| (key == name).then_some(value))
}

/// The writer-facing state of one piece: `trashed`, `draft`, `scheduled`
/// (public with a future instant), or `public`.
fn content_status(content: &Content, now: DateTime<Utc>) -> &'static str {
    if content.is_trashed() {
        "trashed"
    } else if content.publication.is_scheduled_at(now) {
        "scheduled"
    } else {
        match content.publication {
            Publication::Draft => "draft",
            Publication::Public { .. } => "public",
        }
    }
}

fn save_response(content: &Content, now: DateTime<Utc>) -> serde_json::Value {
    json!({
        "id": content.id.as_i64(),
        "version": content.version,
        "slug": content.slug.as_str(),
        "status": content_status(content, now),
        "publish_at": content
            .publication
            .publish_at()
            .map(|at| at.to_rfc3339_opts(SecondsFormat::Secs, true)),
    })
}

/// Accepts the RFC 3339 instant the browser script sends, or the naive
/// `datetime-local` value a JavaScript-free form submits, which the editor
/// labels as UTC. Empty means "no explicit instant".
fn parse_publish_at(value: &str) -> Result<Option<DateTime<Utc>>, String> {
    let value = value.trim();
    if value.is_empty() {
        return Ok(None);
    }
    if let Ok(parsed) = DateTime::parse_from_rfc3339(value) {
        return Ok(Some(parsed.with_timezone(&Utc)));
    }
    for format in ["%Y-%m-%dT%H:%M:%S", "%Y-%m-%dT%H:%M"] {
        if let Ok(naive) = NaiveDateTime::parse_from_str(value, format) {
            return Ok(Some(naive.and_utc()));
        }
    }
    Err("publish date must be an ISO 8601 date-time".into())
}

/// The `datetime-local` control has minute resolution, so a value that merely
/// round-tripped through it must not re-date a piece or bump a revision.
const fn same_minute(left: DateTime<Utc>, right: DateTime<Utc>) -> bool {
    left.timestamp().div_euclid(60) == right.timestamp().div_euclid(60)
}

/// An absent status means "keep what the content already is": saves that are
/// not an explicit Publish/Unpublish never change publication, though a public
/// piece may be moved along the timeline. Publishing with an instant
/// schedules; publishing without one keeps an existing instant or uses now.
fn publication_for(
    status: &str,
    requested: Option<DateTime<Utc>>,
    current: Option<&Content>,
    now: DateTime<Utc>,
) -> Result<Publication, String> {
    let current = current.map(|content| &content.publication);
    match status {
        "" => Ok(match (requested, current) {
            (Some(date), Some(&Publication::Public { publish_at })) => Publication::Public {
                publish_at: if same_minute(date, publish_at) {
                    publish_at
                } else {
                    date
                },
            },
            (_, Some(publication)) => publication.clone(),
            (_, None) => Publication::Draft,
        }),
        "draft" => Ok(Publication::Draft),
        "public" => Ok(Publication::Public {
            publish_at: match (requested, current) {
                (Some(date), Some(&Publication::Public { publish_at }))
                    if same_minute(date, publish_at) =>
                {
                    publish_at
                }
                (Some(date), _) => date,
                (None, Some(&Publication::Public { publish_at })) => publish_at,
                (None, _) => now,
            },
        }),
        _ => Err("unknown publication status".into()),
    }
}

async fn application_error(
    state: &AppState,
    headers: &HeaderMap,
    error: RepositoryError,
) -> Result<Response, WebError> {
    match error {
        RepositoryError::Validation(message) => {
            failure(state, headers, StatusCode::UNPROCESSABLE_ENTITY, &message).await
        }
        RepositoryError::SlugTaken(slug) => {
            failure(
                state,
                headers,
                StatusCode::CONFLICT,
                &format!("slug is already used: {slug}"),
            )
            .await
        }
        other => Err(WebError::Repository(other)),
    }
}

/// A browser navigation gets a real page for an expected failure instead of
/// a bare sentence on white; scripts and JSON callers keep the plain text
/// they parse today.
fn wants_html(headers: &HeaderMap) -> bool {
    headers
        .get(header::ACCEPT)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|accept| accept.contains("text/html") && !accept.contains("application/json"))
}

async fn failure(
    state: &AppState,
    headers: &HeaderMap,
    status: StatusCode,
    message: &str,
) -> Result<Response, WebError> {
    if !wants_html(headers) {
        return Ok((status, message.to_owned()).into_response());
    }
    let heading_key = match status {
        StatusCode::UNPROCESSABLE_ENTITY | StatusCode::BAD_REQUEST => "admin.error_invalid_heading",
        StatusCode::CONFLICT => "admin.error_conflict_heading",
        StatusCode::FORBIDDEN | StatusCode::UNAUTHORIZED => "admin.error_forbidden_heading",
        StatusCode::NOT_FOUND | StatusCode::GONE => "admin.error_not_found_heading",
        _ => "admin.error_heading",
    };
    let html = state
        .render_admin_string(
            "admin/error.html",
            json!({
                "status": status.as_u16(),
                "heading_key": heading_key,
                "message": message,
                "csrf": "",
            }),
        )
        .await?;
    Ok((status, axum::response::Html(html)).into_response())
}

fn set_auth_cookies(
    response: &mut Response,
    secrets: &SessionSecrets,
    secure: bool,
) -> Result<(), WebError> {
    let secure = if secure { "; Secure" } else { "" };
    let session = format!(
        "sb_session={}; Path=/admin; HttpOnly; SameSite=Strict; Max-Age=604800{secure}",
        secrets.session.expose()
    );
    let csrf = format!(
        "sb_csrf={}; Path=/admin; SameSite=Strict; Max-Age=604800{secure}",
        secrets.csrf.expose()
    );
    response.headers_mut().append(
        header::SET_COOKIE,
        HeaderValue::from_str(&session).map_err(WebError::header)?,
    );
    response.headers_mut().append(
        header::SET_COOKIE,
        HeaderValue::from_str(&csrf).map_err(WebError::header)?,
    );
    Ok(())
}

fn login_redirect(next: Option<&str>) -> Response {
    match next.and_then(safe_admin_path) {
        Some(next) if next != "/admin/" => redirect(
            StatusCode::SEE_OTHER,
            &format!("/admin/login/?next={}", percent_encode_path(next)),
        ),
        _ => redirect(StatusCode::SEE_OTHER, "/admin/login/"),
    }
}

/// The admin path the browser asked for, carried as the login `next` target.
/// Axum strips nothing here, so the original path is what the proxy or
/// browser sent; only same-site admin pages are ever accepted.
fn requested_path(headers: &HeaderMap) -> Option<String> {
    headers
        .get(crate::web::REQUESTED_PATH_HEADER)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned)
}

/// Accepts only an absolute admin path without a scheme, host, query, or
/// fragment, so `next` can never send the writer off-site after login.
fn safe_admin_path(candidate: &str) -> Option<&str> {
    let valid = candidate.starts_with("/admin/")
        && !candidate.starts_with("//")
        && !candidate.contains(['\\', '?', '#', '\n', '\r'])
        && candidate.len() <= 512
        && candidate
            .split('/')
            .all(|segment| !matches!(segment, "." | ".."));
    valid.then_some(candidate)
}

/// Percent-encodes a query value: everything but unreserved characters.
fn percent_encode_query(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(char::from(byte));
            }
            b' ' => encoded.push('+'),
            _ => encoded.push_str(&format!("%{byte:02X}")),
        }
    }
    encoded
}

fn percent_encode_path(path: &str) -> String {
    let mut encoded = String::with_capacity(path.len());
    for byte in path.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => {
                encoded.push(char::from(byte));
            }
            _ => encoded.push_str(&format!("%{byte:02X}")),
        }
    }
    encoded
}

fn clear_auth_cookies(response: &mut Response, secure: bool) -> Result<(), WebError> {
    let secure = if secure { "; Secure" } else { "" };
    for cookie in [
        format!("sb_session=; Path=/admin; HttpOnly; SameSite=Strict; Max-Age=0{secure}"),
        format!("sb_csrf=; Path=/admin; SameSite=Strict; Max-Age=0{secure}"),
    ] {
        response.headers_mut().append(
            header::SET_COOKIE,
            HeaderValue::from_str(&cookie).map_err(WebError::header)?,
        );
    }
    Ok(())
}

fn redirect(status: StatusCode, location: &str) -> Response {
    let mut response = status.into_response();
    if let Ok(value) = HeaderValue::from_str(location) {
        response.headers_mut().insert(header::LOCATION, value);
    }
    response
}

fn json_error(status: StatusCode, message: &str) -> Response {
    (status, Json(json!({ "error": message }))).into_response()
}

fn asset_response(body: &'static str, content_type: &'static str) -> Response {
    let mut response = body.into_response();
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("public, max-age=86400"),
    );
    response
}

fn clean_passkey_name(value: &str) -> String {
    let value = value.trim();
    if value.is_empty() {
        "Passkey".into()
    } else {
        value.chars().take(80).collect()
    }
}

fn default_passkey_name() -> String {
    "Passkey".into()
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;

    fn content_with(publication: Publication, deleted: bool) -> Content {
        let at = Utc.with_ymd_and_hms(2026, 9, 3, 12, 0, 0).unwrap();
        Content {
            id: ContentId::from_i64(1),
            kind: ContentKind::Post,
            title: "T".into(),
            slug: Slug::parse("t").unwrap(),
            summary: String::new(),
            body_markdown: String::new(),
            body_html: String::new(),
            tags: Vec::new(),
            cover_media_id: None,
            seo_title: None,
            seo_description: None,
            publication,
            version: 1,
            created_at: at,
            updated_at: at,
            deleted_at: deleted.then_some(at),
        }
    }

    #[test]
    fn parse_publish_at_accepts_rfc3339_and_naive_utc_and_rejects_garbage() {
        let expected = Utc.with_ymd_and_hms(2026, 9, 3, 12, 1, 0).unwrap();
        assert_eq!(parse_publish_at(""), Ok(None));
        assert_eq!(parse_publish_at("   "), Ok(None));
        assert_eq!(parse_publish_at("2026-09-03T12:01:00Z"), Ok(Some(expected)));
        assert_eq!(
            parse_publish_at("2026-09-03T21:01:00+09:00"),
            Ok(Some(expected))
        );
        assert_eq!(parse_publish_at("2026-09-03T12:01"), Ok(Some(expected)));
        assert_eq!(parse_publish_at("2026-09-03T12:01:00"), Ok(Some(expected)));
        assert!(parse_publish_at("next tuesday").is_err());
        assert!(parse_publish_at("2026-13-40T99:99").is_err());
    }

    #[test]
    fn publication_for_follows_the_status_and_date_table() {
        let now = Utc.with_ymd_and_hms(2026, 9, 3, 12, 0, 37).unwrap();
        let minute = Utc.with_ymd_and_hms(2026, 9, 3, 12, 0, 0).unwrap();
        let earlier = Utc.with_ymd_and_hms(2026, 9, 1, 8, 0, 0).unwrap();
        let later = Utc.with_ymd_and_hms(2026, 9, 9, 8, 0, 0).unwrap();
        let public = content_with(Publication::Public { publish_at: now }, false);
        let draft = content_with(Publication::Draft, false);

        // Absent status keeps things as they are, but may move a public piece.
        assert_eq!(publication_for("", None, None, now), Ok(Publication::Draft));
        assert_eq!(
            publication_for("", None, Some(&public), now),
            Ok(Publication::Public { publish_at: now })
        );
        assert_eq!(
            publication_for("", Some(minute), Some(&public), now),
            Ok(Publication::Public { publish_at: now }),
            "a same-minute round trip keeps the exact instant"
        );
        assert_eq!(
            publication_for("", Some(earlier), Some(&public), now),
            Ok(Publication::Public {
                publish_at: earlier
            })
        );
        assert_eq!(
            publication_for("", Some(later), Some(&draft), now),
            Ok(Publication::Draft),
            "a draft does not take a date until it is published"
        );
        assert_eq!(
            publication_for("draft", Some(later), Some(&public), now),
            Ok(Publication::Draft)
        );
        assert_eq!(
            publication_for("public", Some(later), Some(&draft), now),
            Ok(Publication::Public { publish_at: later })
        );
        assert_eq!(
            publication_for("public", Some(minute), Some(&public), now),
            Ok(Publication::Public { publish_at: now })
        );
        assert_eq!(
            publication_for("public", None, Some(&public), now),
            Ok(Publication::Public { publish_at: now })
        );
        assert_eq!(
            publication_for("public", None, Some(&draft), now),
            Ok(Publication::Public { publish_at: now })
        );
        assert!(publication_for("archived", None, None, now).is_err());
    }

    #[test]
    fn content_status_distinguishes_trash_schedule_and_visibility() {
        let now = Utc.with_ymd_and_hms(2026, 9, 3, 12, 0, 0).unwrap();
        let later = now + Duration::hours(1);
        assert_eq!(
            content_status(&content_with(Publication::Draft, false), now),
            "draft"
        );
        assert_eq!(
            content_status(
                &content_with(Publication::Public { publish_at: later }, false),
                now
            ),
            "scheduled"
        );
        assert_eq!(
            content_status(
                &content_with(Publication::Public { publish_at: now }, false),
                now
            ),
            "public"
        );
        assert_eq!(
            content_status(
                &content_with(Publication::Public { publish_at: now }, true),
                now
            ),
            "trashed"
        );
    }

    #[test]
    fn dashboard_filter_matches_status_and_query_case_insensitively() {
        assert_eq!(DashboardFilter::parse("bogus"), DashboardFilter::All);
        assert!(DashboardFilter::All.admits("draft"));
        assert!(DashboardFilter::All.admits("public"));
        assert!(!DashboardFilter::All.admits("trashed"));
        assert!(DashboardFilter::Trash.admits("trashed"));
        assert!(!DashboardFilter::Trash.admits("public"));
        assert!(DashboardFilter::Scheduled.admits("scheduled"));
        assert!(!DashboardFilter::Scheduled.admits("public"));
        assert!(dashboard_matches("", "Anything", "anything"));
        assert!(dashboard_matches("BET", "Beta", "b"));
        assert!(dashboard_matches("second", "Two", "the-second-piece"));
        assert!(!dashboard_matches("zzz", "Two", "two"));
    }

    #[test]
    fn next_targets_are_limited_to_admin_pages() {
        assert_eq!(
            safe_admin_path("/admin/settings/"),
            Some("/admin/settings/")
        );
        assert_eq!(
            safe_admin_path("/admin/content/7/edit/"),
            Some("/admin/content/7/edit/")
        );
        assert_eq!(safe_admin_path("https://evil.example/admin/"), None);
        assert_eq!(safe_admin_path("//evil.example/admin/"), None);
        assert_eq!(safe_admin_path("/archive/"), None);
        assert_eq!(safe_admin_path("/admin/../secret"), None);
        assert_eq!(safe_admin_path("/admin/settings/?x=1"), None);
        assert_eq!(safe_admin_path("/admin/a#b"), None);
        assert_eq!(
            percent_encode_path("/admin/コンテンツ/"),
            "/admin/%E3%82%B3%E3%83%B3%E3%83%86%E3%83%B3%E3%83%84/"
        );
        assert_eq!(percent_encode_query("a b&c=d/é"), "a+b%26c%3Dd%2F%C3%A9");
    }
}
