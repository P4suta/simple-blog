mod admin;
mod observability;
mod public;
mod security;
mod templates;

use std::sync::Arc;

use axum::{
    Router,
    extract::{DefaultBodyLimit, Request, State},
    http::{HeaderValue, StatusCode, header},
    middleware::{self, Next},
    response::{Html, IntoResponse, Response},
    routing::{get, post},
};
use serde::Serialize;
use thiserror::Error;
use tower_http::{
    catch_panic::CatchPanicLayer, compression::CompressionLayer, limit::RequestBodyLimitLayer,
};

use crate::{
    application::{
        auth::{AuthRateLimiter, AuthService, PasskeyAccountService},
        content::ContentService,
        ports::{
            AuthError, Clock, ContentRepository, MediaRepository, MediaRepositoryError,
            RepositoryError, SiteRepository,
        },
        site::SiteService,
    },
    config::Config,
    domain::{
        media::MediaId,
        theme::{ThemeAssets, ThemeContext},
    },
    infrastructure::{
        clock::SystemClock,
        markdown::ComrakMarkdownRenderer,
        media::{LocalMediaService, MediaError},
        sqlite::SqliteRepository,
        webauthn::{PasskeyCeremony, PasskeyError},
    },
    web::{
        public::meta,
        templates::{TemplateError, Templates},
    },
};

#[derive(Clone)]
pub struct AppState {
    pub(crate) config: Arc<Config>,
    pub(crate) content: Arc<dyn ContentRepository>,
    pub(crate) site: Arc<dyn SiteRepository>,
    pub(crate) site_service: SiteService,
    pub(crate) templates: Templates,
    pub(crate) content_service: ContentService,
    pub(crate) auth: AuthService,
    pub(crate) auth_rate_limiter: AuthRateLimiter,
    pub(crate) accounts: PasskeyAccountService,
    pub(crate) webauthn: Arc<PasskeyCeremony>,
    pub(crate) media_repository: Arc<dyn MediaRepository>,
    pub(crate) media_service: LocalMediaService,
    pub(crate) clock: Arc<dyn Clock>,
}

impl AppState {
    pub fn new(config: Config, repository: Arc<SqliteRepository>) -> Result<Self, AppBuildError> {
        Self::new_with_clock(config, repository, Arc::new(SystemClock))
    }

    pub fn new_with_clock(
        config: Config,
        repository: Arc<SqliteRepository>,
        clock: Arc<dyn Clock>,
    ) -> Result<Self, AppBuildError> {
        let content_service = ContentService::new(
            repository.clone(),
            Arc::new(ComrakMarkdownRenderer::default()),
        );
        let auth = AuthService::new(repository.clone());
        let accounts = PasskeyAccountService::new(repository.clone());
        let site_service = SiteService::new(repository.clone());
        let webauthn = Arc::new(PasskeyCeremony::new(&config.public_url, "Simple Blog")?);
        let content: Arc<dyn ContentRepository> = repository.clone();
        let site: Arc<dyn SiteRepository> = repository.clone();
        let media_repository: Arc<dyn MediaRepository> = repository.clone();
        let media_service =
            LocalMediaService::new(config.media_dir(), repository, config.max_upload_bytes);
        Ok(Self {
            config: Arc::new(config),
            content,
            site,
            site_service,
            templates: Templates::embedded()?,
            content_service,
            auth,
            auth_rate_limiter: AuthRateLimiter::authentication_default(),
            accounts,
            webauthn,
            media_repository,
            media_service,
            clock,
        })
    }

    async fn theme_context<T: Serialize>(
        &self,
        path: &str,
        title: MetaTitle,
        description: Option<String>,
        og_type: &str,
        page: T,
    ) -> Result<ThemeContext<T>, WebError> {
        let site = self.site.site_settings().await?;
        let navigation = self.site.navigation().await?;
        let logo_url = self.theme_media_url(site.logo_media_id.as_deref()).await?;
        let favicon_url = if site.favicon_media_id == site.logo_media_id {
            logo_url.clone()
        } else {
            self.theme_media_url(site.favicon_media_id.as_deref())
                .await?
        };
        let canonical_url = self.absolute_url(path)?;
        let title = match title {
            MetaTitle::Site => None,
            MetaTitle::Page(title) => Some(format!("{title} — {}", site.site_title)),
            MetaTitle::Override(title) => Some(title),
        };
        let meta = meta(&site, canonical_url, title, description, og_type);
        Ok(ThemeContext {
            site,
            assets: ThemeAssets {
                logo_url,
                favicon_url,
            },
            navigation,
            meta,
            page,
        })
    }

    fn render_html(&self, template: &str, context: impl Serialize) -> Result<Response, WebError> {
        Ok(Html(self.templates.render(template, context)?).into_response())
    }

    fn absolute_url(&self, path: &str) -> Result<String, WebError> {
        self.config
            .public_url
            .join(path.trim_start_matches('/'))
            .map(|url| url.to_string())
            .map_err(|error| WebError::Internal(error.to_string()))
    }

    fn secure_cookies(&self) -> bool {
        self.config.public_url.scheme() == "https"
    }

    async fn theme_media_url(&self, id: Option<&str>) -> Result<Option<String>, WebError> {
        let Some(id) = id else {
            return Ok(None);
        };
        let id = MediaId::parse(id)
            .map_err(|error| WebError::Internal(format!("invalid theme media ID: {error}")))?;
        self.media_repository
            .find_media(&id)
            .await
            .map_err(WebError::media_repository)
            .map(|asset| asset.map(|asset| format!("/media/{}", asset.original_filename)))
    }
}

pub(crate) enum MetaTitle {
    Site,
    Page(String),
    Override(String),
}

const MULTIPART_ENVELOPE_BYTES: usize = 64 * 1024;

pub fn router(state: AppState) -> Router {
    let upload_envelope = state
        .config
        .max_upload_bytes
        .saturating_add(MULTIPART_ENVELOPE_BYTES);
    Router::new()
        .route("/", get(public::home))
        .route("/archive", get(public::canonical_archive))
        .route("/archive/", get(public::archive))
        .route("/tag/{slug}", get(public::canonical_tag))
        .route("/tag/{slug}/", get(public::tag))
        .route("/feed.xml", get(public::feed))
        .route("/sitemap.xml", get(public::sitemap))
        .route("/robots.txt", get(public::robots))
        .route("/healthz", get(public::health))
        .route("/assets/theme.css", get(public::theme_css))
        .route("/media/{filename}", get(public::media_file))
        .route(
            "/admin",
            get(|| async {
                (
                    StatusCode::PERMANENT_REDIRECT,
                    [(header::LOCATION, "/admin/")],
                )
            }),
        )
        .route("/admin/", get(admin::dashboard))
        .route("/admin/login/", get(admin::login_page))
        .route("/admin/setup/", get(admin::setup_page))
        .route("/admin/security/", get(admin::security_page))
        .route(
            "/admin/security/recovery-codes/",
            post(admin::regenerate_recovery_codes),
        )
        .route(
            "/admin/security/passkeys/remove/",
            post(admin::remove_passkey),
        )
        .route("/admin/content/new/", get(admin::new_content))
        .route("/admin/content/", post(admin::create_content))
        .route("/admin/preview/", post(admin::preview_markdown))
        .route("/admin/content/{id}/edit/", get(admin::edit_content))
        .route("/admin/content/{id}/", post(admin::update_content))
        .route(
            "/admin/content/{id}/revisions/{revision_id}/",
            get(admin::revision_page),
        )
        .route(
            "/admin/content/{id}/revisions/{revision_id}/restore/",
            post(admin::restore_revision),
        )
        .route(
            "/admin/settings/",
            get(admin::settings_page).post(admin::update_settings),
        )
        .route(
            "/admin/media/",
            post(admin::upload_media).layer(DefaultBodyLimit::max(upload_envelope)),
        )
        .route("/admin/auth/setup/start", post(admin::setup_start))
        .route("/admin/auth/setup/finish", post(admin::setup_finish))
        .route("/admin/auth/login/start", post(admin::login_start))
        .route("/admin/auth/login/finish", post(admin::login_finish))
        .route("/admin/auth/recovery", post(admin::recovery_login))
        .route("/admin/auth/passkeys/start", post(admin::passkey_add_start))
        .route(
            "/admin/auth/passkeys/finish",
            post(admin::passkey_add_finish),
        )
        .route("/admin/assets/admin.css", get(admin::admin_css))
        .route("/admin/assets/admin.js", get(admin::admin_js))
        .route("/{slug}", get(public::canonical_content))
        .route("/{slug}/", get(public::content))
        .fallback(public::not_found)
        .with_state(state.clone())
        .layer(CompressionLayer::new())
        .layer(RequestBodyLimitLayer::new(upload_envelope))
        .layer(CatchPanicLayer::custom(observability::panic_response))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            security::auth_rate_limit,
        ))
        .layer(middleware::from_fn_with_state(state, perimeter))
        .layer(middleware::from_fn(observability::request_trace))
}

async fn perimeter(State(state): State<AppState>, request: Request, next: Next) -> Response {
    let path = request.uri().path().to_owned();
    let private = path.starts_with("/admin/") && !path.starts_with("/admin/assets/");
    if path != "/healthz" && !valid_host(&state.config, request.headers()) {
        return security_headers(
            StatusCode::BAD_REQUEST.into_response(),
            state.secure_cookies(),
            private,
        );
    }
    security_headers(next.run(request).await, state.secure_cookies(), private)
}

fn valid_host(config: &Config, headers: &axum::http::HeaderMap) -> bool {
    let Some(actual) = headers
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
    else {
        return false;
    };
    let Some(host) = config.public_url.host_str() else {
        return false;
    };
    let expected = config
        .public_url
        .port()
        .map_or_else(|| host.to_owned(), |port| format!("{host}:{port}"));
    actual.eq_ignore_ascii_case(&expected)
}

fn security_headers(mut response: Response, secure: bool, private: bool) -> Response {
    let headers = response.headers_mut();
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("strict-origin-when-cross-origin"),
    );
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(
            "default-src 'self'; img-src 'self' data:; style-src 'self' 'unsafe-inline'; script-src 'self'; object-src 'none'; base-uri 'none'; frame-ancestors 'none'; form-action 'self'",
        ),
    );
    headers.insert(
        header::HeaderName::from_static("permissions-policy"),
        HeaderValue::from_static("camera=(), microphone=(), geolocation=(), payment=()"),
    );
    if secure {
        headers.insert(
            header::STRICT_TRANSPORT_SECURITY,
            HeaderValue::from_static("max-age=31536000"),
        );
    }
    if private {
        headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
        headers.insert(header::PRAGMA, HeaderValue::from_static("no-cache"));
    }
    response
}

#[derive(Debug, Error)]
pub enum AppBuildError {
    #[error(transparent)]
    Template(#[from] TemplateError),
    #[error(transparent)]
    Passkey(#[from] PasskeyError),
}

#[derive(Debug, Error)]
pub enum WebError {
    #[error(transparent)]
    Repository(#[from] RepositoryError),
    #[error(transparent)]
    Template(#[from] TemplateError),
    #[error(transparent)]
    Auth(#[from] AuthError),
    #[error(transparent)]
    Passkey(#[from] PasskeyError),
    #[error(transparent)]
    Media(#[from] MediaError),
    #[error(transparent)]
    MediaRepository(#[from] MediaRepositoryError),
    #[error("internal web error: {0}")]
    Internal(String),
}

impl WebError {
    pub(super) fn header(error: impl std::fmt::Display) -> Self {
        Self::Internal(error.to_string())
    }

    pub(super) const fn auth(error: AuthError) -> Self {
        Self::Auth(error)
    }

    pub(super) const fn passkey(error: PasskeyError) -> Self {
        Self::Passkey(error)
    }

    pub(super) const fn media(error: MediaError) -> Self {
        Self::Media(error)
    }

    pub(super) const fn media_repository(error: MediaRepositoryError) -> Self {
        Self::MediaRepository(error)
    }

    const fn diagnostic_code(&self) -> &'static str {
        match self {
            Self::Repository(RepositoryError::Conflict { .. }) => "repository.conflict",
            Self::Repository(RepositoryError::SlugTaken(_)) => "repository.slug_taken",
            Self::Repository(RepositoryError::NotFound) => "repository.not_found",
            Self::Repository(RepositoryError::Validation(_)) => "repository.validation",
            Self::Repository(RepositoryError::Storage(_)) => "repository.storage",
            Self::Template(_) => "template.render",
            Self::Auth(_) => "auth.storage",
            Self::Passkey(_) => "auth.passkey",
            Self::Media(_) => "media.processing",
            Self::MediaRepository(_) => "media.storage",
            Self::Internal(_) => "web.internal",
        }
    }
}

impl IntoResponse for WebError {
    fn into_response(self) -> Response {
        tracing::error!(
            event = "http.request.failed",
            error_code = self.diagnostic_code(),
            error = %self,
            "request failed"
        );
        (StatusCode::INTERNAL_SERVER_ERROR, "Internal Server Error").into_response()
    }
}
