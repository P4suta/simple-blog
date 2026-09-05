use std::{collections::BTreeMap, str::FromStr};

use axum::{
    Form, Json,
    body::Body,
    extract::{Multipart, Path, Query, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{DateTime, Duration, LocalResult, NaiveDateTime, SecondsFormat, TimeZone, Utc};
use chrono_tz::Tz;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio_util::io::ReaderStream;
use uuid::Uuid;
use webauthn_rs::prelude::{PublicKeyCredential, RegisterPublicKeyCredential};

use crate::{
    application::{
        auth::AuthService, content::SaveIntent, ports::RepositoryError, publication::SiteState,
        site::DEFAULT_THEME_CSS, site_compiler::PreviewAssets,
    },
    domain::diff::{DiffLine, diff_lines},
    domain::{
        auth::{SecretToken, SessionIdentity, SessionSecrets, SetupPurpose, StoredPasskey},
        content::{Content, ContentDraft, ContentId, ContentKind, Publication, Slug},
        media::MediaId,
        theme::{Locale, NavigationItem, SiteSettings, TimezoneGroup, timezone_choices},
    },
    i18n::Translations,
    web::{AppState, EMBEDDABLE_CSP, WebError},
};

const ADMIN_PREFS_JS: &str = include_str!("../../static/prefs.js");
const ADMIN_ARTICLE_JS: &str = include_str!("../../static/article.js");

/// Rows per dashboard page: enough to scan, few enough to render instantly.
const DASHBOARD_PAGE_SIZE: usize = 50;

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
    /// The active release lags behind a committed change; a retry is pending.
    site_pending: bool,
    /// The active sort key: `updated`, `published`, or `title`.
    sort: &'static str,
    /// Query-string suffix carrying a non-default sort, for the filter links.
    sort_query: String,
    page: usize,
    page_count: usize,
    page_of: String,
    prev_url: Option<String>,
    next_url: Option<String>,
    /// How many pieces each filter would show for the same search.
    counts: std::collections::BTreeMap<&'static str, usize>,
    /// The trash filter is showing something that can be emptied.
    can_empty_trash: bool,
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
    #[serde(default)]
    sort: String,
    #[serde(default)]
    page: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DashboardSort {
    Updated,
    Published,
    Title,
}

impl DashboardSort {
    fn parse(value: &str) -> Self {
        match value {
            "published" => Self::Published,
            "title" => Self::Title,
            _ => Self::Updated,
        }
    }

    const fn as_str(self) -> &'static str {
        match self {
            Self::Updated => "updated",
            Self::Published => "published",
            Self::Title => "title",
        }
    }

    /// Orders pieces for the dashboard; every order falls back to the most
    /// recent edit so the list is stable.
    fn apply(self, items: &mut [Content]) {
        match self {
            Self::Updated => items.sort_by_key(|item| std::cmp::Reverse(item.updated_at)),
            Self::Published => items.sort_by(|a, b| {
                b.publication
                    .publish_at()
                    .cmp(&a.publication.publish_at())
                    .then(b.updated_at.cmp(&a.updated_at))
            }),
            Self::Title => items.sort_by(|a, b| {
                a.title
                    .to_lowercase()
                    .cmp(&b.title.to_lowercase())
                    .then(b.updated_at.cmp(&a.updated_at))
            }),
        }
    }
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

/// Case-insensitive match on the title, slug, summary or a tag; an empty
/// query admits all.
fn dashboard_matches(query: &str, content: &Content) -> bool {
    let query = query.trim().to_lowercase();
    query.is_empty()
        || content.title.to_lowercase().contains(&query)
        || content.slug.as_str().contains(&query)
        || content.summary.to_lowercase().contains(&query)
        || content
            .tags
            .iter()
            .any(|tag| tag.name.to_lowercase().contains(&query))
}

/// `/admin/?status=…&q=…&sort=…&page=N` with only the parts that matter.
fn dashboard_url(filter: &str, q_query: &str, sort: DashboardSort, page: usize) -> String {
    let mut url = format!("/admin/?status={filter}");
    if !q_query.is_empty() {
        url.push_str("&q=");
        url.push_str(q_query);
    }
    if sort != DashboardSort::Updated {
        url.push_str("&sort=");
        url.push_str(sort.as_str());
    }
    if page > 1 {
        url.push_str(&format!("&page={page}"));
    }
    url
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
    /// The site's zone: the scheduling control reads and writes its clock,
    /// and the name stands next to it so a writer elsewhere is never in doubt.
    site_zone: String,
    /// The public origin, so the drawer can show the full address live.
    site_origin: String,
    /// What each key binding does, in the site's language, keyed the way the
    /// script's shortcut table names them.
    shortcut_labels: BTreeMap<String, String>,
}

/// The localized descriptions of the editor's key bindings. The bindings
/// themselves live in the script, which builds both the keymap and the help
/// from one table; this only supplies the words.
fn shortcut_labels(translations: &Translations, locale: Locale) -> BTreeMap<String, String> {
    translations
        .for_locale(locale)
        .iter()
        .filter(|(key, _)| key.starts_with("editor.shortcut_"))
        .map(|(key, label)| (key.clone(), label.clone()))
        .collect()
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
    /// The server's last save, RFC 3339, so the browser can tell whether its
    /// own unsaved copy is newer. Empty for a piece that was never saved.
    updated_at: String,
    /// The address still follows the title; the field stays empty and shows
    /// the current one as a placeholder.
    slug_auto: bool,
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
struct RedirectView {
    old_slug: String,
    slug: String,
    title: String,
    created_at: String,
}

#[derive(Serialize)]
struct RedirectTarget {
    id: i64,
    title: String,
    slug: String,
}

#[derive(Deserialize)]
pub struct RedirectForm {
    csrf: String,
    #[serde(default)]
    old_slug: String,
    #[serde(default)]
    content_id: i64,
}

#[derive(Deserialize)]
pub struct RemoveRedirectForm {
    csrf: String,
    #[serde(default)]
    old_slug: String,
}

#[derive(Serialize)]
struct SettingsContext {
    csrf: String,
    settings: SiteSettings,
    timezones: Vec<TimezoneGroup>,
    redirects: Vec<RedirectView>,
    /// Pieces a manual redirect may point at, for the picker.
    redirect_targets: Vec<RedirectTarget>,
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
    #[serde(default)]
    timezone: String,
    #[serde(default)]
    author_name: String,
}

struct AdminIdentity {
    session: SessionIdentity,
    csrf: String,
    /// Set when the session was just extended: the page must re-send both
    /// cookies so the browser's copies live as long as the server's.
    refreshed: Option<SessionSecrets>,
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
    /// The browser's zone, adopted once for a site still on the UTC default.
    #[serde(default)]
    timezone: Option<String>,
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
    let sort = DashboardSort::parse(&query.sort);
    let q = query.q.trim().to_owned();
    let everything = state.content.list_all_content().await?;
    let any_content = everything.iter().any(|content| !content.is_trashed());
    let totals = state.engagement.engagement_totals().await?;

    let matching = everything
        .into_iter()
        .filter(|content| dashboard_matches(&q, content))
        .collect::<Vec<_>>();
    let counts = filter_counts(&matching, now);
    let mut shown = matching
        .into_iter()
        .filter(|content| filter.admits(content_status(content, now)))
        .collect::<Vec<_>>();
    sort.apply(&mut shown);

    let page_count = shown.len().div_ceil(DASHBOARD_PAGE_SIZE).max(1);
    let page = query.page.clamp(1, page_count);
    let start = (page - 1) * DASHBOARD_PAGE_SIZE;
    let end = (start + DASHBOARD_PAGE_SIZE).min(shown.len());
    let contents = dashboard_items(shown.drain(start..end), &totals, now);
    let empty_key = match filter {
        DashboardFilter::Trash => "dashboard.empty_trash",
        DashboardFilter::All if !any_content && q.is_empty() => "dashboard.empty",
        _ => "dashboard.empty_filtered",
    };
    let site_pending = state.site_state().await? == SiteState::Pending;
    let locale = state.site.site_settings().await?.locale;
    let q_query = percent_encode_query(&q);
    let can_empty_trash = filter == DashboardFilter::Trash && !contents.is_empty();
    let page = state
        .render_admin(
            "admin/dashboard.html",
            DashboardContext {
                csrf: identity.csrf.clone(),
                contents,
                filter: filter.as_str(),
                sort: sort.as_str(),
                sort_query: if sort == DashboardSort::Updated {
                    String::new()
                } else {
                    format!("&sort={}", sort.as_str())
                },
                page_of: state.translations.format(
                    locale,
                    "dashboard.page_of",
                    &[
                        ("number", &page.to_string()),
                        ("count", &page_count.to_string()),
                    ],
                ),
                prev_url: (page > 1)
                    .then(|| dashboard_url(filter.as_str(), &q_query, sort, page - 1)),
                next_url: (page < page_count)
                    .then(|| dashboard_url(filter.as_str(), &q_query, sort, page + 1)),
                page,
                page_count,
                q_query,
                q,
                empty_key,
                site_pending,
                counts,
                can_empty_trash,
            },
        )
        .await?;
    with_session_refresh(page, &identity, state.secure_cookies())
}

/// How many matching pieces each filter tab would show.
fn filter_counts(
    matching: &[Content],
    now: DateTime<Utc>,
) -> std::collections::BTreeMap<&'static str, usize> {
    [
        DashboardFilter::All,
        DashboardFilter::Draft,
        DashboardFilter::Scheduled,
        DashboardFilter::Public,
        DashboardFilter::Trash,
    ]
    .into_iter()
    .map(|candidate| {
        let count = matching
            .iter()
            .filter(|content| candidate.admits(content_status(content, now)))
            .count();
        (candidate.as_str(), count)
    })
    .collect()
}

/// One dashboard row per piece, stamps in RFC 3339 for the local-time script.
fn dashboard_items(
    contents: impl Iterator<Item = Content>,
    totals: &std::collections::HashMap<ContentId, crate::application::ports::Engagement>,
    now: DateTime<Utc>,
) -> Vec<DashboardItem> {
    let stamp = |at: DateTime<Utc>| at.to_rfc3339_opts(SecondsFormat::Secs, true);
    contents
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
        .collect()
}

/// Deletes every trashed piece for good, in one deliberate step.
pub async fn empty_trash(
    State(state): State<AppState>,
    headers: HeaderMap,
    Form(form): Form<CsrfForm>,
) -> Result<Response, WebError> {
    if let Err(response) = authenticate(&state, &headers, Some(&form.csrf)).await? {
        return Ok(response);
    }
    let trashed = state
        .content
        .list_all_content()
        .await?
        .into_iter()
        .filter(Content::is_trashed)
        .map(|content| content.id)
        .collect::<Vec<_>>();
    let mut deleted = 0_usize;
    for id in trashed {
        match state.content_service.delete_permanently(id).await {
            Ok(()) => deleted += 1,
            // Restored between the page load and the click: not ours to delete.
            Err(RepositoryError::NotFound) => {}
            Err(error) => return application_error(&state, &headers, error).await,
        }
    }
    run_media_gc(&state).await;
    if wants_json(&headers) {
        Ok(Json(json!({ "ok": true, "deleted": deleted })).into_response())
    } else {
        Ok(redirect(StatusCode::SEE_OTHER, "/admin/?status=trash"))
    }
}

/// Rebuilds the public site on request, typically from the dashboard banner
/// after a deferred publication.
pub async fn publish_site(
    State(state): State<AppState>,
    headers: HeaderMap,
    Form(form): Form<CsrfForm>,
) -> Result<Response, WebError> {
    if let Err(response) = authenticate(&state, &headers, Some(&form.csrf)).await? {
        return Ok(response);
    }
    let site = state.publish_after_commit("manual").await;
    if wants_json(&headers) {
        Ok(Json(json!({ "ok": site == SiteState::Current, "site": site })).into_response())
    } else {
        Ok(redirect(StatusCode::SEE_OTHER, "/admin/"))
    }
}

pub async fn new_content(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Response, WebError> {
    let identity = match authenticate(&state, &headers, None).await? {
        Ok(identity) => identity,
        Err(response) => return Ok(response),
    };
    let site_settings = state.site.site_settings().await?;
    let page = state
        .render_admin(
            "admin/editor.html",
            EditorContext {
                title: "New".into(),
                csrf: identity.csrf.clone(),
                action: "/admin/content/".into(),
                content_id: None,
                version: None,
                content: EditorContent::empty(state.clock.now()),
                cover_url: None,
                cover_alt_text: String::new(),
                revisions: Vec::new(),
                trashed: false,
                site_zone: site_settings.timezone.clone(),
                site_origin: preview_origin(&state).to_owned(),
                shortcut_labels: shortcut_labels(&state.translations, site_settings.locale),
            },
        )
        .await?;
    with_session_refresh(page, &identity, state.secure_cookies())
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
    let page = state
        .render_admin(
            "admin/settings.html",
            SettingsContext {
                csrf: identity.csrf.clone(),
                timezones: timezone_choices_including(&settings.timezone),
                redirects: state
                    .site
                    .list_redirects()
                    .await?
                    .into_iter()
                    .map(|entry| RedirectView {
                        old_slug: entry.old_slug.to_string(),
                        slug: entry.slug.to_string(),
                        title: entry.title,
                        created_at: entry.created_at.to_rfc3339_opts(SecondsFormat::Secs, true),
                    })
                    .collect(),
                redirect_targets: state
                    .content
                    .list_all_content()
                    .await?
                    .into_iter()
                    .filter(|content| !content.is_trashed())
                    .map(|content| RedirectTarget {
                        id: content.id.as_i64(),
                        title: content.title,
                        slug: content.slug.to_string(),
                    })
                    .collect(),
                settings,
                navigation,
                logo_url,
                favicon_url,
                passkeys,
                reauth_ok,
            },
        )
        .await?;
    with_session_refresh(page, &identity, state.secure_cookies())
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
    if settings.custom_css != DEFAULT_THEME_CSS {
        settings.custom_css_backup = Some(std::mem::take(&mut settings.custom_css));
    }
    settings.custom_css = DEFAULT_THEME_CSS.to_owned();
    let navigation = state.site.navigation().await?;
    match state
        .site_service
        .update(settings, navigation, state.clock.now())
        .await
    {
        Ok(()) => {
            let site = state.publish_after_commit("theme_reset").await;
            if wants_json(&headers) {
                Ok(Json(json!({ "ok": true, "site": site })).into_response())
            } else {
                Ok(redirect(StatusCode::SEE_OTHER, "/admin/settings/"))
            }
        }
        Err(error) => application_error(&state, &headers, error).await,
    }
}

/// Brings back the stylesheet a reset replaced. The slot holds one
/// stylesheet, so this answers 404 once it has been used or never filled.
pub async fn undo_theme_reset(
    State(state): State<AppState>,
    headers: HeaderMap,
    Form(form): Form<CsrfForm>,
) -> Result<Response, WebError> {
    if let Err(response) = authenticate(&state, &headers, Some(&form.csrf)).await? {
        return Ok(response);
    }
    let mut settings = state.site.site_settings().await?;
    let Some(previous) = settings.custom_css_backup.take() else {
        return failure(
            &state,
            &headers,
            StatusCode::NOT_FOUND,
            "there is no stylesheet to bring back",
        )
        .await;
    };
    settings.custom_css = previous;
    let navigation = state.site.navigation().await?;
    match state
        .site_service
        .update(settings, navigation, state.clock.now())
        .await
    {
        Ok(()) => {
            let site = state.publish_after_commit("theme_undo").await;
            if wants_json(&headers) {
                Ok(Json(json!({ "ok": true, "site": site })).into_response())
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
    let (mut settings, navigation) = match form.into_configuration() {
        Ok(configuration) => configuration,
        Err(message) => {
            return failure(&state, &headers, StatusCode::UNPROCESSABLE_ENTITY, &message).await;
        }
    };
    // The form never carries the theme backup; an ordinary save keeps it.
    settings.custom_css_backup = state.site.site_settings().await?.custom_css_backup;
    match state
        .site_service
        .update(settings, navigation, state.clock.now())
        .await
    {
        Ok(()) => {
            let site = state.publish_after_commit("settings").await;
            run_media_gc(&state).await;
            if wants_json(&headers) {
                Ok(Json(json!({ "ok": true, "site": site })).into_response())
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
    let site_settings = state.site.site_settings().await?;
    let page = state
        .render_admin(
            "admin/editor.html",
            EditorContext {
                title: content.title.clone(),
                csrf: identity.csrf.clone(),
                action: format!("/admin/content/{raw_id}/"),
                content_id: Some(raw_id),
                version: Some(content.version),
                trashed: content.is_trashed(),
                site_zone: site_settings.timezone.clone(),
                site_origin: preview_origin(&state).to_owned(),
                shortcut_labels: shortcut_labels(&state.translations, site_settings.locale),
                content: EditorContent::from_content(
                    &content,
                    state.clock.now(),
                    site_settings.time_zone(),
                ),
                cover_url,
                cover_alt_text,
                revisions,
            },
        )
        .await?;
    with_session_refresh(page, &identity, state.secure_cookies())
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
            let site = state.publish_after_commit("media_alt_text").await;
            if wants_json(&headers) {
                Ok(
                    Json(json!({ "ok": true, "site": site, "alt_text": form.alt_text.trim() }))
                        .into_response(),
                )
            } else {
                Ok(redirect(StatusCode::SEE_OTHER, "/admin/media/"))
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
            let site = state.publish_after_commit("trash").await;
            if wants_json(&headers) {
                Ok(Json(json!({ "ok": true, "site": site })).into_response())
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
            let site = state.publish_after_commit("restore").await;
            if wants_json(&headers) {
                Ok(Json(json!({ "ok": true, "site": site })).into_response())
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
    let page = state
        .render_admin(
            "admin/revision.html",
            RevisionContext {
                csrf: identity.csrf.clone(),
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
        .await?;
    with_session_refresh(page, &identity, state.secure_cookies())
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
            let _site = state.publish_after_commit("restore_revision").await;
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

pub async fn create_content(
    State(state): State<AppState>,
    headers: HeaderMap,
    Form(form): Form<ContentForm>,
) -> Result<Response, WebError> {
    if let Err(response) = authenticate(&state, &headers, Some(&form.csrf)).await? {
        return Ok(response);
    }
    let now = state.clock.now();
    let zone = state.site.site_settings().await?.time_zone();
    let (draft, intent) = match form.to_draft(now, None, zone) {
        Ok(value) => value,
        Err(error) => {
            return failure(&state, &headers, StatusCode::UNPROCESSABLE_ENTITY, &error).await;
        }
    };
    let automatic = form.slug.trim().is_empty();
    let result = with_slug_alternatives(automatic, draft, now, |draft| {
        state.content_service.create(draft, intent, now)
    })
    .await;
    match result {
        Ok(content) => {
            let site = state.publish_after_commit("create").await;
            if intent == SaveIntent::Explicit {
                run_media_gc(&state).await;
            }
            if intent == SaveIntent::Autosave || wants_json(&headers) {
                Ok((
                    StatusCode::CREATED,
                    Json(save_response(&content, now, site)),
                )
                    .into_response())
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

/// Runs after deliberate moments (an explicit save, a permanent delete, a
/// settings change, a restore): media that neither current content nor any
/// stored revision references is removed. Autosaves never sweep, so a
/// half-inserted image survives the typing pause. Failure never fails the
/// action that triggered it.
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
        let revisions = state
            .revision_media
            .revision_media_ids()
            .await
            .map_err(|error| error.to_string())?;
        let referenced =
            crate::application::media_gc::gc_survivors(&contents, &settings, &revisions);
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
    let zone = state.site.site_settings().await?.time_zone();
    let (draft, intent) = match form.to_draft(now, Some(&existing), zone) {
        Ok(value) => value,
        Err(error) => {
            return failure(&state, &headers, StatusCode::UNPROCESSABLE_ENTITY, &error).await;
        }
    };
    let submitted_title = draft.title.clone();
    let submitted_body = draft.body_markdown.clone();
    let automatic = form.slug.trim().is_empty() && draft.slug != existing.slug;
    let result = with_slug_alternatives(automatic, draft, now, |draft| {
        state
            .content_service
            .update(id, expected_version, draft, intent, now)
    })
    .await;
    match result {
        Ok(content) => {
            let site = state.publish_after_commit("update").await;
            if intent == SaveIntent::Explicit {
                run_media_gc(&state).await;
            }
            if intent == SaveIntent::Autosave || wants_json(&headers) {
                Ok(Json(save_response(&content, now, site)).into_response())
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
    // A brand-new site takes the browser's zone so its dates are right from
    // the first post; a failure here must never spoil a finished ceremony.
    if context.purpose == SetupPurpose::Initial
        && let Some(zone) = request.timezone.as_deref()
        && let Err(error) = state
            .site_service
            .adopt_timezone_once(zone, state.clock.now())
            .await
    {
        tracing::warn!(event = "setup.timezone.not_adopted", error = %error);
    }
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

/// Creates a backup archive on disk and streams it to the browser. A POST
/// because it writes a file, and a fresh reauthentication because the
/// archive holds every session hash and passkey record the site has.
pub async fn download_backup(
    State(state): State<AppState>,
    headers: HeaderMap,
    Form(form): Form<CsrfForm>,
) -> Result<Response, WebError> {
    let identity = match authenticate(&state, &headers, Some(&form.csrf)).await? {
        Ok(identity) => identity,
        Err(response) => return Ok(response),
    };
    if !recently_reauthenticated(&identity, state.clock.now()) {
        return Ok(redirect(
            StatusCode::SEE_OTHER,
            "/admin/login/?next=/admin/settings/",
        ));
    }
    let archive = state
        .create_backup()
        .await
        .map_err(|error| WebError::Internal(format!("backup failed: {error}")))?;
    let file = tokio::fs::File::open(&archive)
        .await
        .map_err(|error| WebError::Internal(format!("backup unreadable: {error}")))?;
    let length = file
        .metadata()
        .await
        .map_err(|error| WebError::Internal(format!("backup unreadable: {error}")))?
        .len();
    let filename = archive
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("simple-blog.tar.zst");
    let disposition = HeaderValue::from_str(&format!("attachment; filename=\"{filename}\""))
        .map_err(|error| WebError::Internal(error.to_string()))?;
    let mut response = Body::from_stream(ReaderStream::new(file)).into_response();
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/zstd"),
    );
    headers.insert(header::CONTENT_LENGTH, HeaderValue::from(length));
    headers.insert(header::CONTENT_DISPOSITION, disposition);
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
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

const ADMIN_CSS: &str = include_str!("../../static/admin.css");
const ADMIN_JS: &str = include_str!(concat!(env!("OUT_DIR"), "/admin.js"));

/// One fingerprint for both admin assets: their URLs carry it, so an
/// upgraded server never runs against a browser's day-old bundle.
pub fn admin_asset_version() -> &'static str {
    static VERSION: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| {
        let mut hasher = blake3::Hasher::new();
        hasher.update(ADMIN_CSS.as_bytes());
        hasher.update(ADMIN_JS.as_bytes());
        hasher.finalize().to_hex()[..16].to_owned()
    });
    VERSION.as_str()
}

pub async fn admin_css() -> Response {
    asset_response(ADMIN_CSS, "text/css; charset=utf-8")
}

pub async fn admin_js() -> Response {
    asset_response(ADMIN_JS, "text/javascript; charset=utf-8")
}

impl ContentForm {
    fn to_draft(
        &self,
        now: DateTime<Utc>,
        current: Option<&Content>,
        zone: Tz,
    ) -> Result<(ContentDraft, SaveIntent), String> {
        let kind = ContentKind::from_str(&self.kind).map_err(str::to_owned)?;
        // An empty slug means "from the title": always for a new piece, and
        // for a draft on every save; a published address never moves on its own.
        let slug = if self.slug.trim().is_empty() {
            match current {
                Some(content) if content.publication.publish_at().is_some() => content.slug.clone(),
                _ => Slug::from_title(&self.title, now),
            }
        } else {
            Slug::parse(&self.slug).map_err(|error| error.to_string())?
        };
        let requested = parse_publish_at(&self.publish_at, zone)?;
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
                timezone: optional_text(&self.timezone).unwrap_or_else(|| "UTC".into()),
                author_name: self.author_name,
                custom_css_backup: None,
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
            updated_at: String::new(),
            slug_auto: true,
        }
    }

    /// `zone` is the site's: the scheduling control shows the site's clock,
    /// which is what "a minute on the clock in the site's own time" means.
    fn from_content(content: &Content, now: DateTime<Utc>, zone: Tz) -> Self {
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
                .map(|at| at.with_timezone(&zone).format("%Y-%m-%dT%H:%M").to_string())
                .unwrap_or_default(),
            seo_title: content.seo_title.clone().unwrap_or_default(),
            seo_description: content.seo_description.clone().unwrap_or_default(),
            cover_media_id: content.cover_media_id.clone().unwrap_or_default(),
            updated_at: content
                .updated_at
                .to_rfc3339_opts(SecondsFormat::Secs, true),
            slug_auto: slug_follows_title(content, now),
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
    // Only page loads renew: a form post has no page to carry new cookies
    // home, and the tokens never change anyway.
    let now = state.clock.now();
    let refreshed = if csrf.is_none() && AuthService::needs_renewal(&identity, now) {
        state
            .auth
            .extend_session(session_token, now)
            .await
            .map_err(WebError::auth)?
            .map(|_| SessionSecrets {
                session: SecretToken::new(session_token.to_owned()),
                csrf: SecretToken::new(csrf_cookie.to_owned()),
            })
    } else {
        None
    };
    Ok(Ok(AdminIdentity {
        session: identity,
        csrf: csrf_cookie.to_owned(),
        refreshed,
    }))
}

/// Attaches renewed session cookies to a page response when the login was
/// just extended; a no-op otherwise.
fn with_session_refresh(
    mut response: Response,
    identity: &AdminIdentity,
    secure: bool,
) -> Result<Response, WebError> {
    if let Some(secrets) = &identity.refreshed {
        set_auth_cookies(&mut response, secrets, secure)?;
    }
    Ok(response)
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

/// Whether the address still follows the title: a draft whose slug is the
/// one its title (or the clock) would produce. Published addresses are fixed.
fn slug_follows_title(content: &Content, now: DateTime<Utc>) -> bool {
    content.publication.publish_at().is_none()
        && (content.slug.is_timestamped() || content.slug == Slug::from_title(&content.title, now))
}

/// The addresses to try after a collision: numbered variants of a title
/// slug, or the second-resolution stamp for a timestamped one.
fn slug_alternatives(slug: &Slug, now: DateTime<Utc>) -> Vec<Slug> {
    if slug.is_timestamped() {
        vec![Slug::timestamped_precise(now)]
    } else {
        (2..=6).map(|n| slug.numbered(n)).collect()
    }
}

/// Saves a draft, and when its automatically chosen address collides (a
/// second piece with the same title, two pieces in the same minute) tries
/// the alternatives before surfacing the conflict. A hand-typed address is
/// never rewritten.
async fn with_slug_alternatives<F, Fut>(
    automatic: bool,
    draft: ContentDraft,
    now: DateTime<Utc>,
    save: F,
) -> Result<Content, RepositoryError>
where
    F: Fn(ContentDraft) -> Fut,
    Fut: std::future::Future<Output = Result<Content, RepositoryError>>,
{
    let mut result = save(draft.clone()).await;
    if !automatic {
        return result;
    }
    for candidate in slug_alternatives(&draft.slug, now) {
        if !matches!(result, Err(RepositoryError::SlugTaken(_))) {
            break;
        }
        result = save(ContentDraft {
            slug: candidate,
            ..draft.clone()
        })
        .await;
    }
    result
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

fn save_response(content: &Content, now: DateTime<Utc>, site: SiteState) -> serde_json::Value {
    json!({
        "id": content.id.as_i64(),
        "version": content.version,
        "slug": content.slug.as_str(),
        "slug_auto": slug_follows_title(content, now),
        "status": content_status(content, now),
        "site": site,
        "publish_at": content
            .publication
            .publish_at()
            .map(|at| at.to_rfc3339_opts(SecondsFormat::Secs, true)),
    })
}

/// Accepts the RFC 3339 instant the browser script sends, or the naive
/// `datetime-local` value a JavaScript-free form submits, which the editor
/// labels as UTC. Empty means "no explicit instant".
/// A publish date as the form sends it. An instant with an offset is taken
/// as it is; a bare wall-clock time is read on the site's clock (`zone`),
/// because that is the clock the writer was shown. When a zone's clocks jump
/// and a minute happens twice, the first one wins; when a minute is skipped,
/// the piece appears as soon as the clock reaches it.
fn parse_publish_at(value: &str, zone: Tz) -> Result<Option<DateTime<Utc>>, String> {
    let value = value.trim();
    if value.is_empty() {
        return Ok(None);
    }
    if let Ok(parsed) = DateTime::parse_from_rfc3339(value) {
        return Ok(Some(parsed.with_timezone(&Utc)));
    }
    for format in ["%Y-%m-%dT%H:%M:%S", "%Y-%m-%dT%H:%M"] {
        if let Ok(naive) = NaiveDateTime::parse_from_str(value, format) {
            let instant = match zone.from_local_datetime(&naive) {
                LocalResult::Single(at) | LocalResult::Ambiguous(at, _) => at,
                LocalResult::None => zone
                    .from_local_datetime(&(naive + Duration::hours(1)))
                    .earliest()
                    .ok_or_else(|| "publish date does not exist in the site's zone".to_owned())?,
            };
            return Ok(Some(instant.with_timezone(&Utc)));
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

/// English sentences the application layers produce, mapped to catalog keys
/// so a writer reads them in the site's language. Sentences with a variable
/// part keep it in `{detail}`; anything unknown is shown as it came.
const EXACT_DETAILS: &[(&str, &str)] = &[
    (
        "title must contain 1-200 characters",
        "validation.title_length",
    ),
    (
        "summary must contain at most 500 characters",
        "validation.summary_length",
    ),
    (
        "Markdown exceeds the 2 MiB limit",
        "validation.markdown_size",
    ),
    (
        "SEO title or description is too long",
        "validation.seo_length",
    ),
    (
        "tag names must contain at most 50 characters",
        "validation.tag_length",
    ),
    (
        "content may contain at most 20 tags",
        "validation.tag_count",
    ),
    (
        "media id must be a 64-character lowercase hexadecimal digest",
        "validation.media_id",
    ),
    ("logo or favicon media ID is invalid", "validation.media_id"),
    (
        "publish date must be an ISO 8601 date-time",
        "validation.publish_at",
    ),
    ("unknown publication status", "validation.status"),
    ("missing content version", "validation.version_missing"),
    ("content does not exist", "validation.not_found"),
    ("media does not exist", "validation.media_not_found"),
    ("redirect does not exist", "validation.redirect_not_found"),
    (
        "content changed after this page was opened",
        "validation.conflict",
    ),
    (
        "content changed after this restore page was opened",
        "validation.conflict",
    ),
    ("too many navigation items", "validation.navigation_count"),
    (
        "navigation may contain at most 16 items",
        "validation.navigation_count",
    ),
    (
        "navigation label must contain 1-80 characters",
        "validation.navigation_label",
    ),
    (
        "navigation destination does not match its internal or external kind",
        "validation.navigation_destination",
    ),
    ("unknown locale", "validation.locale"),
    (
        "site title must contain 1-120 characters",
        "validation.site_title",
    ),
    (
        "site description must contain at most 300 characters",
        "validation.site_description",
    ),
    (
        "custom CSS is too large or could escape its style element",
        "validation.custom_css",
    ),
    (
        "time zone must be an IANA zone name such as Asia/Tokyo",
        "validation.timezone",
    ),
    (
        "author name must contain at most 120 characters",
        "validation.author_name",
    ),
    (
        "there is no stylesheet to bring back",
        "validation.no_theme_backup",
    ),
    (
        "the image is still used by current content or the site settings",
        "validation.media_in_use",
    ),
    (
        "slug must be 1-120 lowercase ASCII characters, using only letters, digits, and interior hyphens",
        "validation.slug_shape",
    ),
];
const PREFIXED_DETAILS: &[(&str, &str)] = &[
    ("slug is already used: ", "validation.slug_taken"),
    (
        "slug is already active or historical: ",
        "validation.slug_taken",
    ),
    ("navigation line ", "validation.navigation_line"),
    ("tag ", "validation.tag_slug"),
];

fn localize_detail(translations: &Translations, locale: Locale, message: &str) -> String {
    if let Some((_, key)) = EXACT_DETAILS.iter().find(|(text, _)| *text == message) {
        return translations.text(locale, key);
    }
    for (prefix, key) in PREFIXED_DETAILS {
        if let Some(rest) = message.strip_prefix(prefix) {
            // The variable part: a slug, a line number, or a quoted tag name.
            let detail = rest
                .split_once('"')
                .and_then(|(_, after)| after.split_once('"').map(|(inner, _)| inner))
                .unwrap_or_else(|| rest.split_whitespace().next().unwrap_or(rest));
            return translations.format(locale, key, &[("detail", detail)]);
        }
    }
    message.to_owned()
}

async fn failure(
    state: &AppState,
    headers: &HeaderMap,
    status: StatusCode,
    message: &str,
) -> Result<Response, WebError> {
    let locale = state.site.site_settings().await?.locale;
    let message = localize_detail(&state.translations, locale, message);
    let message = message.as_str();
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
        HeaderValue::from_static("public, max-age=31536000, immutable"),
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

/// The current draft through the public templates and the live stylesheet:
/// the owner sees the future page, not an imitation. Framable by the editor.
pub async fn preview_content(
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
    let html = render_content_preview(&state, &content).await?;
    let mut response = preview_response(html);
    response.headers_mut().insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(EMBEDDABLE_CSP),
    );
    with_session_refresh(response, &identity, state.secure_cookies())
}

/// The home page with the stylesheet as it is being edited, for Settings.
pub async fn preview_home(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Response, WebError> {
    let identity = match authenticate(&state, &headers, None).await? {
        Ok(identity) => identity,
        Err(response) => return Ok(response),
    };
    let contents = state.content.list_all_content().await?;
    let snapshot = preview_snapshot(&state, contents).await?;
    let html = state.compiler().render_home_preview(
        &snapshot,
        preview_origin(&state),
        preview_assets(&snapshot),
    )?;
    let mut response = preview_response(html);
    response.headers_mut().insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(EMBEDDABLE_CSP),
    );
    with_session_refresh(response, &identity, state.secure_cookies())
}

/// Whoever holds a live preview link reads the piece, without a session.
pub async fn shared_preview(
    State(state): State<AppState>,
    Path(token): Path<String>,
) -> Result<Response, WebError> {
    let now = state.clock.now();
    let content = match state
        .preview_links
        .resolve(&token, now)
        .await
        .map_err(WebError::auth)?
    {
        Some(id) => state.content.find_by_id(id).await?,
        None => None,
    };
    let Some(content) = content.filter(|content| !content.is_trashed()) else {
        let locale = state.site.site_settings().await?.locale;
        let html = state
            .render_admin_string(
                "admin/error.html",
                json!({
                    "status": 404,
                    "heading_key": "admin.error_not_found_heading",
                    "message": state.translations.text(locale, "share.not_found"),
                    "csrf": "",
                }),
            )
            .await?;
        let mut response = (StatusCode::NOT_FOUND, axum::response::Html(html)).into_response();
        response
            .headers_mut()
            .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
        return Ok(response);
    };
    let html = render_content_preview(&state, &content).await?;
    Ok(preview_response(html))
}

pub async fn issue_preview_link(
    State(state): State<AppState>,
    Path(raw_id): Path<i64>,
    headers: HeaderMap,
    Form(form): Form<CsrfForm>,
) -> Result<Response, WebError> {
    if let Err(response) = authenticate(&state, &headers, Some(&form.csrf)).await? {
        return Ok(response);
    }
    let id = ContentId::from_i64(raw_id);
    let Some(content) = state
        .content
        .find_by_id(id)
        .await?
        .filter(|content| !content.is_trashed())
    else {
        return failure(
            &state,
            &headers,
            StatusCode::NOT_FOUND,
            "content does not exist",
        )
        .await;
    };
    let now = state.clock.now();
    let link = state
        .preview_links
        .issue(content.id, now)
        .await
        .map_err(WebError::auth)?;
    let path = format!("/admin/share/{}/", link.token.expose());
    let expires_at = link.expires_at.to_rfc3339_opts(SecondsFormat::Secs, true);
    if wants_json(&headers) {
        return Ok(Json(json!({ "url": path, "expires_at": expires_at })).into_response());
    }
    let locale = state.site.site_settings().await?.locale;
    let expires_note =
        state
            .translations
            .format(locale, "editor.share_expires", &[("time", &expires_at)]);
    let absolute = format!(
        "{}{}",
        state.config.public_url.as_str().trim_end_matches('/'),
        path
    );
    state
        .render_admin(
            "admin/share_link.html",
            json!({
                "csrf": form.csrf,
                "content_id": raw_id,
                "url": absolute,
                "expires_note": expires_note,
            }),
        )
        .await
}

pub async fn revoke_preview_links(
    State(state): State<AppState>,
    Path(raw_id): Path<i64>,
    headers: HeaderMap,
    Form(form): Form<CsrfForm>,
) -> Result<Response, WebError> {
    if let Err(response) = authenticate(&state, &headers, Some(&form.csrf)).await? {
        return Ok(response);
    }
    let revoked = state
        .preview_links
        .revoke(ContentId::from_i64(raw_id))
        .await
        .map_err(WebError::auth)?;
    if wants_json(&headers) {
        Ok(Json(json!({ "ok": true, "revoked": revoked })).into_response())
    } else {
        Ok(redirect(
            StatusCode::SEE_OTHER,
            &format!("/admin/content/{raw_id}/edit/"),
        ))
    }
}

/// The stylesheet as currently saved, for previews. Deliberately public and
/// uncached: the same bytes reach every reader once published, and a preview
/// link holder needs them before that.
pub async fn theme_css(State(state): State<AppState>) -> Result<Response, WebError> {
    let css = state.site.site_settings().await?.custom_css;
    let mut response = css.into_response();
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/css; charset=utf-8"),
    );
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(response)
}

pub async fn admin_prefs_js() -> Response {
    asset_response(ADMIN_PREFS_JS, "text/javascript; charset=utf-8")
}

/// The article script for previews, which may exist before any release
/// does and so cannot rely on `/assets/`.
pub async fn admin_article_js() -> Response {
    asset_response(ADMIN_ARTICLE_JS, "text/javascript; charset=utf-8")
}

async fn render_content_preview(state: &AppState, content: &Content) -> Result<String, WebError> {
    let now = state.clock.now();
    let snapshot = preview_snapshot(state, Vec::new()).await?;
    // A draft has no date yet; showing it as if published today gives the
    // header the shape the reader will eventually see.
    let shown = if content.publication.publish_at().is_none() {
        Content {
            publication: Publication::Public { publish_at: now },
            ..content.clone()
        }
    } else {
        content.clone()
    };
    Ok(state.compiler().render_content_preview(
        &snapshot,
        &shown,
        preview_origin(state),
        preview_assets(&snapshot),
    )?)
}

/// The current settings, navigation and media around the given contents, with
/// no release involved.
async fn preview_snapshot(
    state: &AppState,
    contents: Vec<Content>,
) -> Result<crate::application::site_compiler::SiteSnapshotV1, WebError> {
    Ok(crate::application::site_compiler::SiteSnapshotV1 {
        public_revision: 0,
        effective_at: state.clock.now(),
        settings: state.site.site_settings().await?,
        navigation: state.site.navigation().await?,
        contents,
        redirects: Vec::new(),
        media: state
            .media_repository
            .list_media()
            .await
            .map_err(WebError::media_repository)?,
    })
}

fn preview_origin(state: &AppState) -> &str {
    state.config.public_url.as_str().trim_end_matches('/')
}

fn preview_assets(snapshot: &crate::application::site_compiler::SiteSnapshotV1) -> PreviewAssets {
    PreviewAssets {
        css_url: format!(
            "/admin/assets/theme.css?v={}",
            &blake3::hash(snapshot.settings.custom_css.as_bytes()).to_hex()[..8]
        ),
        prefs_js_url: format!("/admin/assets/prefs.js?v={}", admin_asset_version()),
        article_js_url: format!("/admin/assets/article.js?v={}", admin_asset_version()),
    }
}

fn preview_response(html: String) -> Response {
    let mut response = axum::response::Html(html).into_response();
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

/// Existing tags for the editor's suggestions, most used first.
pub async fn list_tags(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Response, WebError> {
    if let Err(response) = authenticate(&state, &headers, None).await? {
        return Ok(response);
    }
    let tags = state
        .content
        .list_tag_usage()
        .await?
        .into_iter()
        .map(|tag| json!({ "name": tag.name, "count": tag.count }))
        .collect::<Vec<_>>();
    Ok(Json(tags).into_response())
}

/// Points an address readers may still hold (one carried over from another
/// platform, say) at a piece of this site.
pub async fn add_redirect(
    State(state): State<AppState>,
    headers: HeaderMap,
    Form(form): Form<RedirectForm>,
) -> Result<Response, WebError> {
    if let Err(response) = authenticate(&state, &headers, Some(&form.csrf)).await? {
        return Ok(response);
    }
    let old_slug = match Slug::parse(form.old_slug.trim()) {
        Ok(slug) => slug,
        Err(error) => {
            return failure(
                &state,
                &headers,
                StatusCode::UNPROCESSABLE_ENTITY,
                &error.to_string(),
            )
            .await;
        }
    };
    match state
        .site
        .add_redirect(
            &old_slug,
            ContentId::from_i64(form.content_id),
            state.clock.now(),
        )
        .await
    {
        Ok(()) => {
            let site = state.publish_after_commit("redirect_add").await;
            if wants_json(&headers) {
                Ok(Json(json!({ "ok": true, "site": site })).into_response())
            } else {
                Ok(redirect(
                    StatusCode::SEE_OTHER,
                    "/admin/settings/#redirects",
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

pub async fn remove_redirect(
    State(state): State<AppState>,
    headers: HeaderMap,
    Form(form): Form<RemoveRedirectForm>,
) -> Result<Response, WebError> {
    if let Err(response) = authenticate(&state, &headers, Some(&form.csrf)).await? {
        return Ok(response);
    }
    let Ok(old_slug) = Slug::parse(form.old_slug.trim()) else {
        return failure(
            &state,
            &headers,
            StatusCode::NOT_FOUND,
            "redirect does not exist",
        )
        .await;
    };
    if !state
        .site
        .remove_redirect(&old_slug, state.clock.now())
        .await?
    {
        return failure(
            &state,
            &headers,
            StatusCode::NOT_FOUND,
            "redirect does not exist",
        )
        .await;
    }
    let site = state.publish_after_commit("redirect_remove").await;
    if wants_json(&headers) {
        Ok(Json(json!({ "ok": true, "site": site })).into_response())
    } else {
        Ok(redirect(
            StatusCode::SEE_OTHER,
            "/admin/settings/#redirects",
        ))
    }
}

#[derive(Serialize)]
struct MediaLibraryContext {
    csrf: String,
    items: Vec<MediaItem>,
}

#[derive(Serialize)]
struct MediaItem {
    id: String,
    url: String,
    thumb_url: String,
    alt_text: String,
    width: u32,
    height: u32,
    size_label: String,
    created_at: String,
    /// `used`, `settings`, `history` or `unused`, for styling.
    usage_key: &'static str,
    usage_label: String,
    deletable: bool,
    /// The Markdown that places the image, ready to paste.
    markdown: String,
}

/// Every uploaded image, newest first, with what uses it and what can go.
pub async fn media_library(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Response, WebError> {
    let identity = match authenticate(&state, &headers, None).await? {
        Ok(identity) => identity,
        Err(response) => return Ok(response),
    };
    let usage = current_media_usage(&state).await?;
    let locale = state.site.site_settings().await?.locale;
    let assets = state
        .media_repository
        .list_media()
        .await
        .map_err(WebError::media_repository)?;
    let items = assets
        .iter()
        .map(|asset| media_item(asset, usage.get(asset.id.as_str()).copied(), &state, locale))
        .collect();
    let page = state
        .render_admin(
            "admin/media.html",
            MediaLibraryContext {
                csrf: identity.csrf.clone(),
                items,
            },
        )
        .await?;
    with_session_refresh(page, &identity, state.secure_cookies())
}

/// Removes an image nothing current shows. History may still mention it,
/// which the page says before the writer confirms.
pub async fn delete_media(
    State(state): State<AppState>,
    Path(raw_id): Path<String>,
    headers: HeaderMap,
    Form(form): Form<CsrfForm>,
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
    let usage = current_media_usage(&state).await?;
    if usage
        .get(id.as_str())
        .is_some_and(|usage| usage.is_current())
    {
        return failure(
            &state,
            &headers,
            StatusCode::CONFLICT,
            "the image is still used by current content or the site settings",
        )
        .await;
    }
    if !state
        .media_service
        .delete_asset(&id)
        .await
        .map_err(WebError::media)?
    {
        return failure(
            &state,
            &headers,
            StatusCode::NOT_FOUND,
            "media does not exist",
        )
        .await;
    }
    if wants_json(&headers) {
        Ok(Json(json!({ "ok": true })).into_response())
    } else {
        Ok(redirect(StatusCode::SEE_OTHER, "/admin/media/"))
    }
}

async fn current_media_usage(
    state: &AppState,
) -> Result<std::collections::HashMap<String, crate::application::media_gc::MediaUsage>, WebError> {
    let contents = state.content.list_all_content().await?;
    let settings = state.site.site_settings().await?;
    let revisions = state.revision_media.revision_media_ids().await?;
    Ok(crate::application::media_gc::media_usage(
        &contents, &settings, &revisions,
    ))
}

fn media_item(
    asset: &crate::domain::media::MediaAsset,
    usage: Option<crate::application::media_gc::MediaUsage>,
    state: &AppState,
    locale: Locale,
) -> MediaItem {
    let usage = usage.unwrap_or_default();
    let (usage_key, usage_label) = if usage.pieces > 0 {
        (
            "used",
            state.translations.format(
                locale,
                "media.used_by",
                &[("count", &usage.pieces.to_string())],
            ),
        )
    } else if usage.settings {
        (
            "settings",
            state.translations.text(locale, "media.used_by_settings"),
        )
    } else if usage.history_only {
        (
            "history",
            state.translations.text(locale, "media.history_only"),
        )
    } else {
        ("unused", state.translations.text(locale, "media.unused"))
    };
    let url = format!("/media/{}", asset.original_filename);
    MediaItem {
        id: asset.id.to_string(),
        thumb_url: asset.variants.first().map_or_else(
            || url.clone(),
            |variant| format!("/media/{}", variant.filename),
        ),
        markdown: format!("![{}]({url})", asset.alt_text),
        url,
        alt_text: asset.alt_text.clone(),
        width: asset.width,
        height: asset.height,
        size_label: size_label(asset.byte_size),
        created_at: asset.created_at.to_rfc3339_opts(SecondsFormat::Secs, true),
        usage_key,
        usage_label,
        deletable: !usage.is_current(),
    }
}

/// `12 KB`, `3.4 MB`: enough precision to notice an accidental 20 MB scan.
fn size_label(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    if bytes >= MB {
        format!("{}.{} MB", bytes / MB, (bytes % MB) * 10 / MB)
    } else {
        format!("{} KB", bytes.div_ceil(KB))
    }
}

/// The picker always contains the stored zone, even a legacy alias that the
/// curated regions leave out, so the select never loses its selection.
fn timezone_choices_including(stored: &str) -> Vec<TimezoneGroup> {
    let mut groups = timezone_choices();
    if !groups
        .iter()
        .any(|group| group.zones.iter().any(|zone| zone == stored))
    {
        groups.push(TimezoneGroup {
            region: "Other",
            zones: vec![stored.to_owned()],
        });
    }
    groups
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;

    fn matching_content(title: &str, slug: &str) -> Content {
        Content {
            id: ContentId::from_i64(1),
            kind: ContentKind::Post,
            title: title.into(),
            slug: Slug::parse(slug).unwrap(),
            summary: String::new(),
            body_markdown: String::new(),
            body_html: String::new(),
            tags: Vec::new(),
            cover_media_id: None,
            seo_title: None,
            seo_description: None,
            publication: Publication::Draft,
            version: 1,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            deleted_at: None,
        }
    }

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
    fn parse_publish_at_reads_bare_times_on_the_site_clock_and_rejects_garbage() {
        let expected = Utc.with_ymd_and_hms(2026, 9, 3, 12, 1, 0).unwrap();
        let utc = Tz::UTC;
        assert_eq!(parse_publish_at("", utc), Ok(None));
        assert_eq!(parse_publish_at("   ", utc), Ok(None));
        assert_eq!(
            parse_publish_at("2026-09-03T12:01:00Z", utc),
            Ok(Some(expected))
        );
        assert_eq!(
            parse_publish_at("2026-09-03T21:01:00+09:00", utc),
            Ok(Some(expected))
        );
        assert_eq!(
            parse_publish_at("2026-09-03T12:01", utc),
            Ok(Some(expected))
        );
        assert_eq!(
            parse_publish_at("2026-09-03T12:01:00", utc),
            Ok(Some(expected))
        );
        assert!(parse_publish_at("next tuesday", utc).is_err());
        assert!(parse_publish_at("2026-13-40T99:99", utc).is_err());

        // The site's clock: 21:01 in Tokyo is 12:01 UTC, and an explicit
        // offset is never reinterpreted.
        let tokyo: Tz = "Asia/Tokyo".parse().unwrap();
        assert_eq!(
            parse_publish_at("2026-09-03T21:01", tokyo),
            Ok(Some(expected))
        );
        assert_eq!(
            parse_publish_at("2026-09-03T12:01:00Z", tokyo),
            Ok(Some(expected))
        );

        // Clock changes: the repeated hour takes its first reading, and a
        // skipped minute becomes the moment the clock reaches it.
        let berlin: Tz = "Europe/Berlin".parse().unwrap();
        assert_eq!(
            parse_publish_at("2026-10-25T02:30", berlin),
            Ok(Some(Utc.with_ymd_and_hms(2026, 10, 25, 0, 30, 0).unwrap()))
        );
        assert_eq!(
            parse_publish_at("2026-03-29T02:30", berlin),
            Ok(Some(Utc.with_ymd_and_hms(2026, 3, 29, 1, 30, 0).unwrap()))
        );
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
        assert!(dashboard_matches(
            "",
            &matching_content("Anything", "anything")
        ));
        assert!(dashboard_matches("BET", &matching_content("Beta", "b")));
        assert!(dashboard_matches(
            "second",
            &matching_content("Two", "the-second-piece")
        ));
        assert!(!dashboard_matches("zzz", &matching_content("Two", "two")));
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
