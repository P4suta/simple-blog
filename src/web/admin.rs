use std::str::FromStr;

use axum::{
    Form, Json,
    extract::{Multipart, Path, Query, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{DateTime, Duration, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;
use webauthn_rs::prelude::{PublicKeyCredential, RegisterPublicKeyCredential};

use crate::{
    application::{content::SaveIntent, ports::RepositoryError},
    domain::{
        auth::{SessionIdentity, SessionSecrets, SetupPurpose, StoredPasskey},
        content::{Content, ContentDraft, ContentId, ContentKind, Publication, Slug},
        theme::{Locale, NavigationItem, SiteSettings},
    },
    web::{AppState, WebError},
};

#[derive(Serialize)]
struct DashboardContext {
    title: &'static str,
    csrf: String,
    contents: Vec<DashboardItem>,
}

#[derive(Serialize)]
struct DashboardItem {
    id: i64,
    title: String,
    slug: String,
    status: &'static str,
    views: u64,
    likes: u64,
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
    revisions: Vec<RevisionItem>,
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
    status: &'static str,
    seo_title: String,
    seo_description: String,
    cover_media_id: String,
}

#[derive(Serialize)]
struct ConflictContext {
    title: &'static str,
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
    title: &'static str,
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
    title: &'static str,
    csrf: String,
    recovery_codes: Vec<&'a str>,
}

#[derive(Serialize)]
struct RevisionContext {
    title: &'static str,
    csrf: String,
    content_id: i64,
    revision_id: i64,
    expected_version: i64,
    current: RevisionVersion,
    revision: RevisionVersion,
}

#[derive(Serialize)]
struct RevisionVersion {
    title: String,
    body_markdown: String,
    version: i64,
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
    headers: HeaderMap,
) -> Result<Response, WebError> {
    let identity = match authenticate(&state, &headers, None).await? {
        Ok(identity) => identity,
        Err(response) => return Ok(response),
    };
    let contents = state.content.list_all_content().await?;
    let totals = state.engagement.engagement_totals().await?;
    let contents = contents
        .into_iter()
        .map(|content| {
            let engagement = totals
                .get(&content.id.as_i64())
                .copied()
                .unwrap_or_default();
            DashboardItem {
                id: content.id.as_i64(),
                title: content.title,
                slug: content.slug.to_string(),
                status: publication_status(&content.publication),
                views: engagement.views,
                likes: engagement.likes,
            }
        })
        .collect();
    state
        .render_admin(
            "admin/dashboard.html",
            DashboardContext {
                title: "Dashboard",
                csrf: identity.csrf,
                contents,
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
                revisions: Vec::new(),
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
                title: "Settings",
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
        Err(message) => return Ok((StatusCode::UNPROCESSABLE_ENTITY, message).into_response()),
    };
    match state
        .site_service
        .update(settings, navigation, state.clock.now())
        .await
    {
        Ok(()) => {
            state.publish_now().await?;
            run_media_gc(&state).await;
            let wants_json = headers
                .get(header::ACCEPT)
                .and_then(|accept| accept.to_str().ok())
                .is_some_and(|accept| accept.contains("application/json"));
            if wants_json {
                Ok(Json(json!({ "ok": true })).into_response())
            } else {
                Ok(redirect(StatusCode::SEE_OTHER, "/admin/settings/"))
            }
        }
        Err(error) => application_error(error),
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
        return Ok(StatusCode::NOT_FOUND.into_response());
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
    state
        .render_admin(
            "admin/editor.html",
            EditorContext {
                title: content.title.clone(),
                csrf: identity.csrf,
                action: format!("/admin/content/{raw_id}/"),
                content_id: Some(raw_id),
                version: Some(content.version),
                content: EditorContent::from_content(&content),
                cover_url,
                revisions,
            },
        )
        .await
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
        return Ok(StatusCode::NOT_FOUND.into_response());
    };
    let Some(revision) = state.content.find_revision(id, revision_id).await? else {
        return Ok(StatusCode::NOT_FOUND.into_response());
    };
    state
        .render_admin(
            "admin/revision.html",
            RevisionContext {
                title: "Revision",
                csrf: identity.csrf,
                content_id: raw_id,
                revision_id,
                expected_version: current.version,
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
        Err(RepositoryError::Conflict { .. }) => Ok((
            StatusCode::CONFLICT,
            "content changed after this restore page was opened",
        )
            .into_response()),
        Err(error) => application_error(error),
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
        Err(error) => application_error(error),
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
        Err(error) => return Ok((StatusCode::UNPROCESSABLE_ENTITY, error).into_response()),
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
                Ok((
                    StatusCode::CREATED,
                    Json(json!({
                        "id": content.id.as_i64(),
                        "version": content.version,
                        "slug": content.slug.as_str(),
                    })),
                )
                    .into_response())
            } else {
                Ok(redirect(
                    StatusCode::SEE_OTHER,
                    &format!("/admin/content/{}/edit/", content.id),
                ))
            }
        }
        Err(error) => application_error(error),
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
        return Ok((StatusCode::BAD_REQUEST, "missing content version").into_response());
    };
    let now = state.clock.now();
    let id = ContentId::from_i64(raw_id);
    let Some(existing) = state.content.find_by_id(id).await? else {
        return Ok(StatusCode::NOT_FOUND.into_response());
    };
    let (draft, intent) = match form.to_draft(now, Some(&existing)) {
        Ok(value) => value,
        Err(error) => return Ok((StatusCode::UNPROCESSABLE_ENTITY, error).into_response()),
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
                Ok(Json(json!({
                    "id": content.id.as_i64(),
                    "version": content.version,
                    "slug": content.slug.as_str(),
                }))
                .into_response())
            } else {
                Ok(redirect(
                    StatusCode::SEE_OTHER,
                    &format!("/admin/content/{raw_id}/edit/"),
                ))
            }
        }
        Err(RepositoryError::Conflict { .. }) => {
            let Some(current) = state.content.find_by_id(id).await? else {
                return Ok(StatusCode::NOT_FOUND.into_response());
            };
            let html = state
                .render_admin_string(
                    "admin/conflict.html",
                    ConflictContext {
                        title: "Save conflict",
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
        Err(error) => application_error(error),
    }
}

pub async fn login_page(State(state): State<AppState>) -> Result<Response, WebError> {
    state.render_admin("admin/login.html", json!({})).await
}

pub async fn setup_page(
    State(state): State<AppState>,
    Query(query): Query<SetupPageQuery>,
) -> Result<Response, WebError> {
    if state
        .accounts
        .setup_context(&query.token, state.clock.now())
        .await
        .map_err(WebError::auth)?
        .is_none()
    {
        return Ok((StatusCode::GONE, "This setup link is invalid or expired.").into_response());
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
        return Ok((StatusCode::UNAUTHORIZED, "reauthentication required").into_response());
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
                title: "Recovery codes",
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
        return Ok((StatusCode::UNAUTHORIZED, "reauthentication required").into_response());
    }
    let Ok(credential_id) = URL_SAFE_NO_PAD.decode(&form.credential_id) else {
        return Ok((StatusCode::BAD_REQUEST, "invalid credential ID").into_response());
    };
    if state
        .accounts
        .remove_passkey(&credential_id)
        .await
        .map_err(WebError::auth)?
    {
        Ok(redirect(StatusCode::SEE_OTHER, "/admin/settings/"))
    } else {
        Ok((StatusCode::CONFLICT, "the last Passkey cannot be removed").into_response())
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
        // An absent status means "keep what the content already is": saves that
        // are not an explicit Publish/Unpublish must never change publication,
        // and a re-save of public content must keep its original publish_at.
        let publication = match self.status.as_str() {
            "" => current.map_or(Publication::Draft, |content| content.publication.clone()),
            "draft" => Publication::Draft,
            "public" => match current.map(|content| &content.publication) {
                Some(&Publication::Public { publish_at }) => Publication::Public { publish_at },
                _ => Publication::Public { publish_at: now },
            },
            _ => return Err("unknown publication status".into()),
        };
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
            seo_title: String::new(),
            seo_description: String::new(),
            cover_media_id: String::new(),
        }
    }

    fn from_content(content: &Content) -> Self {
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
            status: publication_status(&content.publication),
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
    let Some(session_token) = cookie(headers, "sb_session") else {
        return Ok(Err(login_redirect()));
    };
    let Some(csrf_cookie) = cookie(headers, "sb_csrf") else {
        return Ok(Err(login_redirect()));
    };
    let Some(identity) = state
        .auth
        .authenticate(session_token, state.clock.now())
        .await
        .map_err(WebError::auth)?
    else {
        return Ok(Err(login_redirect()));
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

const fn publication_status(publication: &Publication) -> &'static str {
    match publication {
        Publication::Draft => "draft",
        Publication::Public { .. } => "public",
    }
}

fn application_error(error: RepositoryError) -> Result<Response, WebError> {
    match error {
        RepositoryError::Validation(message) => {
            Ok((StatusCode::UNPROCESSABLE_ENTITY, message).into_response())
        }
        RepositoryError::SlugTaken(slug) => Ok((
            StatusCode::CONFLICT,
            format!("slug is already used: {slug}"),
        )
            .into_response()),
        other => Err(WebError::Repository(other)),
    }
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

fn login_redirect() -> Response {
    redirect(StatusCode::SEE_OTHER, "/admin/login/")
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
