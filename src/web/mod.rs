mod admin;
mod observability;
mod public;
mod security;

use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use axum::{
    Router,
    extract::{DefaultBodyLimit, Request, State},
    http::{HeaderValue, Method, StatusCode, header},
    middleware::{self, Next},
    response::{Html, IntoResponse, Response},
    routing::{get, post},
};
use serde::Serialize;
use thiserror::Error;
use tokio::sync::{Mutex, Notify, watch};
use tower_http::{
    catch_panic::CatchPanicLayer, compression::CompressionLayer, limit::RequestBodyLimitLayer,
};

use crate::{
    application::{
        auth::{AuthRateLimiter, AuthService, PasskeyAccountService},
        content::ContentService,
        ports::{
            AuthError, Clock, ContentRepository, EngagementRepository, LikeRepository,
            MediaRepository, MediaRepositoryError, RepositoryError, RevisionMediaReferences,
            SiteRepository,
        },
        preview::PreviewLinkService,
        publication::{
            PublicationOutcome, PublicationService, PublicationServiceError, RetrySchedule,
            SiteState, publication_delay,
        },
        site::SiteService,
        site_compiler::{SiteCompiler, SiteCompilerError},
        templates::{TemplateError, Templates},
    },
    config::Config,
    domain::media::MediaId,
    i18n::{TranslationError, Translations},
    infrastructure::{
        clock::SystemClock,
        entropy::SystemEntropy,
        markdown::ComrakMarkdownRenderer,
        media::{LocalMediaService, MediaError},
        sqlite::SqliteRepository,
        webauthn::{PasskeyCeremony, PasskeyError},
    },
    operations::{BackupCadence, BackupService, OperationError},
    release::{FilesystemReleaseStore, ReleaseError, ReleaseReader, ReleaseStore},
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
    pub(crate) preview_links: PreviewLinkService,
    pub(crate) webauthn: Arc<PasskeyCeremony>,
    pub(crate) media_repository: Arc<dyn MediaRepository>,
    pub(crate) revision_media: Arc<dyn RevisionMediaReferences>,
    pub(crate) media_service: LocalMediaService,
    pub(crate) translations: Arc<Translations>,
    pub(crate) clock: Arc<dyn Clock>,
    pub(crate) likes: Arc<dyn LikeRepository>,
    pub(crate) like_rate_limiter: AuthRateLimiter,
    pub(crate) engagement: Arc<dyn EngagementRepository>,
    pub(crate) release_store: Arc<FilesystemReleaseStore>,
    publication: Arc<PublicationService<SqliteRepository, FilesystemReleaseStore>>,
    publication_lock: Arc<Mutex<()>>,
    repository: Arc<SqliteRepository>,
    publication_wakeup: Arc<Notify>,
    /// Raised when the last build failed after a committed change, so the
    /// scheduler keeps retrying and the dashboard can say so.
    site_stale: Arc<AtomicBool>,
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
        let entropy = Arc::new(SystemEntropy);
        let auth = AuthService::new(repository.clone(), entropy.clone());
        let accounts = PasskeyAccountService::new(repository.clone(), entropy.clone());
        let preview_links = PreviewLinkService::new(repository.clone(), entropy);
        let site_service = SiteService::new(repository.clone());
        let webauthn = Arc::new(PasskeyCeremony::new(&config.public_url, "Simple Blog")?);
        let content: Arc<dyn ContentRepository> = repository.clone();
        let site: Arc<dyn SiteRepository> = repository.clone();
        let media_repository: Arc<dyn MediaRepository> = repository.clone();
        let revision_media: Arc<dyn RevisionMediaReferences> = repository.clone();
        let likes: Arc<dyn LikeRepository> = repository.clone();
        let engagement: Arc<dyn EngagementRepository> = repository.clone();
        let release_store = Arc::new(FilesystemReleaseStore::new(config.release_dir()));
        let publication = Arc::new(PublicationService::new(
            repository.clone(),
            release_store.clone(),
            SiteCompiler::embedded()?,
            config.public_url.as_str(),
        )?);
        let media_service = LocalMediaService::new(
            config.media_dir(),
            repository.clone(),
            config.max_upload_bytes,
        );
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
            preview_links,
            webauthn,
            media_repository,
            repository,
            revision_media,
            media_service,
            translations: Arc::new(Translations::embedded()?),
            clock,
            likes,
            like_rate_limiter: AuthRateLimiter::new(30, chrono::Duration::minutes(1)),
            engagement,
            release_store,
            publication,
            publication_lock: Arc::new(Mutex::new(())),
            publication_wakeup: Arc::new(Notify::new()),
            site_stale: Arc::new(AtomicBool::new(false)),
        })
    }

    pub async fn publish_now(&self) -> Result<PublicationOutcome, PublicationServiceError> {
        let _guard = self.publication_lock.lock().await;
        let outcome = self.publication.publish(self.clock.now()).await;
        self.site_stale.store(outcome.is_err(), Ordering::Release);
        self.publication_wakeup.notify_waiters();
        outcome
    }

    /// Publishes after a transaction has already committed. The change is
    /// durable either way; a failed build only defers its appearance, so this
    /// never turns a successful save into an error. The scheduler is woken to
    /// retry with backoff.
    pub(crate) async fn publish_after_commit(&self, trigger: &'static str) -> SiteState {
        match self.publish_now().await {
            Ok(_) => SiteState::Current,
            Err(error) => {
                tracing::error!(
                    event = "publication.deferred",
                    error_code = error.code(),
                    phase = error.phase(),
                    trigger,
                    error = %error
                );
                self.publication_wakeup.notify_one();
                SiteState::Pending
            }
        }
    }

    /// Whether the active release shows the latest committed public state.
    pub(crate) async fn site_state(&self) -> Result<SiteState, WebError> {
        if self.site_stale.load(Ordering::Acquire) {
            return Ok(SiteState::Pending);
        }
        let revision = self.publication.publication_state().await?.revision;
        let Some(active) = self.release_store.active().await? else {
            return Ok(if revision == 0 {
                SiteState::Current
            } else {
                SiteState::Pending
            });
        };
        let manifest = self.release_store.manifest(&active.id).await?;
        Ok(if manifest.public_revision == revision {
            SiteState::Current
        } else {
            SiteState::Pending
        })
    }

    /// Writes a complete archive under `data/backups/` and trims the folder
    /// to the configured number of generations. The settings page and the
    /// backup scheduler both come through here.
    pub async fn create_backup(&self) -> Result<std::path::PathBuf, OperationError> {
        let archive =
            BackupService::create(&self.config, &self.repository, None, self.clock.now()).await?;
        // Retention zero only switches the scheduler off; an archive the
        // owner asked for is never the one that gets pruned.
        if self.config.backup_retention > 0 {
            let removed = BackupService::prune(&self.config, self.config.backup_retention)?;
            if !removed.is_empty() {
                tracing::info!(event = "backup.pruned", removed = removed.len());
            }
        }
        Ok(archive)
    }

    pub async fn run_backup_scheduler(&self, shutdown: watch::Receiver<bool>) {
        self.run_backup_scheduler_with(shutdown, BackupCadence::DEFAULT)
            .await;
    }

    /// Keeps a daily archive without anyone remembering to. Failures are
    /// logged with a stable code and the next attempt waits the usual
    /// interval; nothing here can affect serving the site.
    pub async fn run_backup_scheduler_with(
        &self,
        mut shutdown: watch::Receiver<bool>,
        cadence: BackupCadence,
    ) {
        if self.config.backup_retention == 0 {
            tracing::info!(event = "backup.scheduler.disabled");
            return;
        }
        tracing::info!(
            event = "backup.scheduler.started",
            every_ms = cadence.every.as_millis(),
            retention = self.config.backup_retention
        );
        let mut delay = cadence.initial;
        loop {
            if sleep_or_shutdown(delay, &mut shutdown).await {
                break;
            }
            match self.create_backup().await {
                Ok(archive) => tracing::info!(
                    event = "backup.scheduled.created",
                    path = %archive.display()
                ),
                Err(error) => tracing::error!(
                    event = "backup.scheduled.failed",
                    error_code = "backup_scheduled_failed",
                    error = %error
                ),
            }
            delay = cadence.every;
        }
        tracing::info!(event = "backup.scheduler.stopped");
    }

    pub async fn run_publication_scheduler(&self, shutdown: watch::Receiver<bool>) {
        self.run_publication_scheduler_with(shutdown, RetrySchedule::DEFAULT)
            .await;
    }

    pub async fn run_publication_scheduler_with(
        &self,
        mut shutdown: watch::Receiver<bool>,
        schedule: RetrySchedule,
    ) {
        const MAXIMUM_IDLE: Duration = Duration::from_secs(60);

        tracing::info!(event = "publication.scheduler.started");
        let mut failures = 0_u32;
        loop {
            if *shutdown.borrow() {
                break;
            }
            let delay = self
                .scheduler_tick(schedule, MAXIMUM_IDLE, &mut failures)
                .await;
            if delay.is_zero() {
                continue;
            }
            tracing::debug!(
                event = "publication.scheduler.waiting",
                delay_ms = delay.as_millis()
            );
            if scheduler_wait(delay, &self.publication_wakeup, &mut shutdown).await {
                break;
            }
        }
        tracing::info!(event = "publication.scheduler.stopped");
    }

    /// One scheduler pass: publishes when a boundary is due or the site is
    /// stale, and answers how long to wait before looking again. Zero means
    /// "look again now" after a successful build.
    async fn scheduler_tick(
        &self,
        schedule: RetrySchedule,
        maximum_idle: Duration,
        failures: &mut u32,
    ) -> Duration {
        let now = self.clock.now();
        let due = match self.publication.publication_state().await {
            Ok(state) => publication_delay(state, now, maximum_idle),
            Err(error) => {
                *failures += 1;
                let retry = schedule.delay(*failures - 1);
                tracing::error!(
                    event = "publication.scheduler.state_failed",
                    error_code = "publication_scheduler_state_failed",
                    retry_ms = retry.as_millis(),
                    error = %error
                );
                return retry;
            }
        };
        if !due.is_zero() && !self.site_stale.load(Ordering::Acquire) {
            return due;
        }
        match self.publish_now().await {
            Ok(_) => {
                *failures = 0;
                Duration::ZERO
            }
            Err(error) => {
                *failures += 1;
                let retry = schedule.delay(*failures - 1);
                tracing::error!(
                    event = "publication.scheduler.publish_failed",
                    error_code = error.code(),
                    phase = error.phase(),
                    attempt = *failures,
                    retry_ms = retry.as_millis(),
                    error = %error
                );
                retry
            }
        }
    }

    /// Renders an admin template with the translation map for the site's
    /// locale injected as `t`.
    async fn render_admin_string(
        &self,
        template: &str,
        context: impl Serialize,
    ) -> Result<String, WebError> {
        let locale = self.site.site_settings().await?.locale;
        Ok(self.templates.render(
            template,
            WithTranslations {
                t: self.translations.for_locale(locale),
                lang: locale.as_str(),
                asset_version: admin::admin_asset_version(),
                cancel_label: self.translations.text(locale, "admin.cancel"),
                context,
            },
        )?)
    }

    async fn render_admin(
        &self,
        template: &str,
        context: impl Serialize,
    ) -> Result<Response, WebError> {
        Ok(Html(self.render_admin_string(template, context).await?).into_response())
    }

    fn secure_cookies(&self) -> bool {
        self.config.public_url.scheme() == "https"
    }

    fn compiler(&self) -> &SiteCompiler {
        self.publication.compiler()
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

/// Sleeps for `delay` unless shutdown arrives first; answers whether to stop.
async fn sleep_or_shutdown(delay: Duration, shutdown: &mut watch::Receiver<bool>) -> bool {
    tokio::select! {
        () = tokio::time::sleep(delay) => *shutdown.borrow(),
        changed = shutdown.changed() => changed.is_err() || *shutdown.borrow(),
    }
}

async fn scheduler_wait(
    delay: Duration,
    wakeup: &Notify,
    shutdown: &mut watch::Receiver<bool>,
) -> bool {
    tokio::select! {
        () = tokio::time::sleep(delay) => false,
        () = wakeup.notified() => false,
        changed = shutdown.changed() => changed.is_err() || *shutdown.borrow(),
    }
}

#[derive(Serialize)]
struct WithTranslations<'a, C> {
    t: &'a std::collections::HashMap<String, String>,
    lang: &'static str,
    /// Cache-busting fingerprint of the admin stylesheet and bundle.
    asset_version: &'static str,
    /// The word the script puts on every confirmation dialog's cancel button.
    cancel_label: String,
    #[serde(flatten)]
    context: C,
}

const MULTIPART_ENVELOPE_BYTES: usize = 64 * 1024;

pub fn router(state: AppState) -> Router {
    let upload_envelope = state
        .config
        .max_upload_bytes
        .saturating_add(MULTIPART_ENVELOPE_BYTES);
    Router::new()
        .route("/healthz", get(public::health))
        .route("/likes/{id}", post(public::like_toggle))
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
        .route("/admin/publish/", post(admin::publish_site))
        .route("/admin/trash/empty/", post(admin::empty_trash))
        .route("/admin/preview/home/", get(admin::preview_home))
        .route("/admin/tags/", get(admin::list_tags))
        .route("/admin/share/{token}/", get(admin::shared_preview))
        .route("/admin/assets/theme.css", get(admin::theme_css))
        .route("/admin/assets/prefs.js", get(admin::admin_prefs_js))
        .route("/admin/assets/article.js", get(admin::admin_article_js))
        .route("/admin/login/", get(admin::login_page))
        .route("/admin/logout/", post(admin::logout))
        .route("/admin/setup/", get(admin::setup_page))
        .route(
            "/admin/settings/recovery-codes/",
            post(admin::regenerate_recovery_codes),
        )
        .route("/admin/settings/theme/reset/", post(admin::reset_theme))
        .route("/admin/backup/", post(admin::download_backup))
        .route("/admin/settings/theme/undo/", post(admin::undo_theme_reset))
        .route("/admin/redirects/", post(admin::add_redirect))
        .route("/admin/redirects/remove/", post(admin::remove_redirect))
        .route(
            "/admin/settings/passkeys/remove/",
            post(admin::remove_passkey),
        )
        .route("/admin/content/new/", get(admin::new_content))
        .route("/admin/content/", post(admin::create_content))
        .route("/admin/content/{id}/edit/", get(admin::edit_content))
        .route("/admin/content/{id}/preview/", get(admin::preview_content))
        .route(
            "/admin/content/{id}/share/",
            post(admin::issue_preview_link),
        )
        .route(
            "/admin/content/{id}/share/revoke/",
            post(admin::revoke_preview_links),
        )
        .route("/admin/content/{id}/", post(admin::update_content))
        .route("/admin/content/{id}/trash/", post(admin::trash_content))
        .route("/admin/content/{id}/restore/", post(admin::restore_content))
        .route("/admin/content/{id}/delete/", post(admin::delete_content))
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
            get(admin::media_library)
                .post(admin::upload_media)
                .layer(DefaultBodyLimit::max(upload_envelope)),
        )
        .route("/admin/media/{id}/", post(admin::update_media))
        .route("/admin/media/{id}/delete/", post(admin::delete_media))
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
        .fallback(public::release_site)
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

const DEFAULT_CSP: &str = "default-src 'self'; img-src 'self' data:; style-src 'self' 'unsafe-inline'; script-src 'self'; object-src 'none'; base-uri 'none'; frame-ancestors 'none'; form-action 'self'";
/// The same policy with same-origin framing allowed, for the owner preview
/// the editor embeds.
pub(crate) const EMBEDDABLE_CSP: &str = "default-src 'self'; img-src 'self' data:; style-src 'self' 'unsafe-inline'; script-src 'self'; object-src 'none'; base-uri 'none'; frame-ancestors 'self'; form-action 'self'";

/// Set by the perimeter on admin page loads so a login can return the writer
/// to the page they asked for. Always overwritten, so a client cannot supply it.
pub(crate) const REQUESTED_PATH_HEADER: &str = "x-simple-blog-requested-path";

async fn perimeter(State(state): State<AppState>, mut request: Request, next: Next) -> Response {
    let path = request.uri().path().to_owned();
    let private = path.starts_with("/admin/") && !path.starts_with("/admin/assets/");
    if path != "/healthz" && !valid_host(&state.config, request.headers()) {
        return security_headers(
            StatusCode::BAD_REQUEST.into_response(),
            state.secure_cookies(),
            private,
        );
    }
    request.headers_mut().remove(REQUESTED_PATH_HEADER);
    if private
        && request.method() == Method::GET
        && let Ok(value) = HeaderValue::from_str(&path)
    {
        request.headers_mut().insert(REQUESTED_PATH_HEADER, value);
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
    // A handler that must be framed (the editor preview) sets its own policy
    // first; everything else forbids framing.
    headers
        .entry(header::CONTENT_SECURITY_POLICY)
        .or_insert(HeaderValue::from_static(DEFAULT_CSP));
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
    #[error(transparent)]
    Translation(#[from] TranslationError),
    #[error(transparent)]
    Compiler(#[from] SiteCompilerError),
    #[error(transparent)]
    Publication(#[from] PublicationServiceError),
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
    #[error(transparent)]
    Publication(#[from] PublicationServiceError),
    #[error(transparent)]
    Compiler(#[from] SiteCompilerError),
    #[error(transparent)]
    Release(#[from] ReleaseError),
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
            Self::Publication(_) => "publication.build",
            Self::Compiler(_) => "site.compile",
            Self::Release(ReleaseError::Integrity { .. }) => "release.integrity",
            Self::Release(ReleaseError::NotFound { .. }) => "release.not_found",
            Self::Release(_) => "release.read",
            Self::Internal(_) => "web.internal",
        }
    }
}

impl IntoResponse for WebError {
    fn into_response(self) -> Response {
        let active_release_missing = matches!(
            &self,
            Self::Release(ReleaseError::NotFound {
                kind: "active release",
                ..
            })
        );
        tracing::error!(
            event = "http.request.failed",
            error_code = self.diagnostic_code(),
            error = %self,
            "request failed"
        );
        if active_release_missing {
            let mut response =
                (StatusCode::SERVICE_UNAVAILABLE, "Service Unavailable").into_response();
            response
                .headers_mut()
                .insert(header::RETRY_AFTER, HeaderValue::from_static("5"));
            response
                .headers_mut()
                .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
            response
        } else {
            (StatusCode::INTERNAL_SERVER_ERROR, "Internal Server Error").into_response()
        }
    }
}
