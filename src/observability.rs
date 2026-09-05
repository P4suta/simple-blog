use std::backtrace::Backtrace;

use thiserror::Error;
use tracing_subscriber::{EnvFilter, util::SubscriberInitExt};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LogFormat {
    Pretty,
    Json,
}

#[derive(Debug, Error)]
pub enum DiagnosticsError {
    #[error("invalid RUST_LOG filter: {0}")]
    Filter(String),
    #[error("SIMPLE_BLOG_LOG_FORMAT must be `pretty` or `json`, got {0:?}")]
    Format(String),
    #[error("could not install tracing subscriber: {0}")]
    Install(String),
}

pub fn init_tracing() -> Result<(), DiagnosticsError> {
    let filter = std::env::var("RUST_LOG").unwrap_or_else(|_| "simple_blog=info".into());
    let filter =
        EnvFilter::try_new(filter).map_err(|error| DiagnosticsError::Filter(error.to_string()))?;
    let format = match std::env::var("SIMPLE_BLOG_LOG_FORMAT") {
        Ok(value) if value.eq_ignore_ascii_case("json") => LogFormat::Json,
        Ok(value) if value.eq_ignore_ascii_case("pretty") => LogFormat::Pretty,
        Ok(value) => return Err(DiagnosticsError::Format(value)),
        Err(std::env::VarError::NotPresent) => LogFormat::Pretty,
        Err(error) => return Err(DiagnosticsError::Format(error.to_string())),
    };

    match format {
        LogFormat::Pretty => tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_thread_ids(true)
            .with_thread_names(true)
            .with_file(true)
            .with_line_number(true)
            .with_writer(std::io::stderr)
            .finish()
            .try_init(),
        LogFormat::Json => tracing_subscriber::fmt()
            .json()
            .with_current_span(true)
            .with_span_list(true)
            .with_env_filter(filter)
            .with_thread_ids(true)
            .with_thread_names(true)
            .with_file(true)
            .with_line_number(true)
            .with_writer(std::io::stderr)
            .finish()
            .try_init(),
    }
    .map_err(|error| DiagnosticsError::Install(error.to_string()))
}

/// Every stable `error_code` the native adapter can emit, in one place.
///
/// The codes are a compatibility contract: `contracts/diagnostics-v1.json`
/// lists them for every adapter and `docs/diagnostics.md` explains each. An
/// emitter names one of these constants, never a literal, so a code cannot
/// be misspelled or invented in passing; a new one is added here, to the
/// contract, and to the catalogue in the same change.
pub mod codes {
    pub const REPOSITORY_CONFLICT: &str = "repository.conflict";
    pub const REPOSITORY_SLUG_TAKEN: &str = "repository.slug_taken";
    pub const REPOSITORY_NOT_FOUND: &str = "repository.not_found";
    pub const REPOSITORY_VALIDATION: &str = "repository.validation";
    pub const REPOSITORY_STORAGE: &str = "repository.storage";
    pub const TEMPLATE_RENDER: &str = "template.render";
    pub const AUTH_STORAGE: &str = "auth.storage";
    pub const AUTH_PASSKEY: &str = "auth.passkey";
    pub const MEDIA_PROCESSING: &str = "media.processing";
    pub const MEDIA_STORAGE: &str = "media.storage";
    pub const PUBLICATION_BUILD: &str = "publication.build";
    pub const SITE_COMPILE: &str = "site.compile";
    pub const RELEASE_INTEGRITY: &str = "release.integrity";
    pub const RELEASE_NOT_FOUND: &str = "release.not_found";
    pub const RELEASE_READ: &str = "release.read";
    pub const WEB_INTERNAL: &str = "web.internal";
    pub const SECURITY_RATE_LIMITED: &str = "security.rate_limited";
    pub const PUBLICATION_REPOSITORY_FAILED: &str = "publication_repository_failed";
    pub const PUBLICATION_COMPILE_FAILED: &str = "publication_compile_failed";
    pub const PUBLICATION_RELEASE_STORE_FAILED: &str = "publication_release_store_failed";
    pub const PUBLICATION_SCHEDULER_STATE_FAILED: &str = "publication_scheduler_state_failed";
    pub const RELEASE_OBJECT_STORE_FAILED: &str = "release_object_store_failed";
    pub const RELEASE_MANIFEST_STORE_FAILED: &str = "release_manifest_store_failed";
    pub const RELEASE_ACTIVATION_FAILED: &str = "release_activation_failed";
    pub const BACKUP_SCHEDULED_FAILED: &str = "backup_scheduled_failed";
}

/// Every code in [`codes`], so the contract can be checked against the
/// binary rather than against a reader's memory.
#[must_use]
pub const fn diagnostic_codes() -> &'static [&'static str] {
    &[
        codes::REPOSITORY_CONFLICT,
        codes::REPOSITORY_SLUG_TAKEN,
        codes::REPOSITORY_NOT_FOUND,
        codes::REPOSITORY_VALIDATION,
        codes::REPOSITORY_STORAGE,
        codes::TEMPLATE_RENDER,
        codes::AUTH_STORAGE,
        codes::AUTH_PASSKEY,
        codes::MEDIA_PROCESSING,
        codes::MEDIA_STORAGE,
        codes::PUBLICATION_BUILD,
        codes::SITE_COMPILE,
        codes::RELEASE_INTEGRITY,
        codes::RELEASE_NOT_FOUND,
        codes::RELEASE_READ,
        codes::WEB_INTERNAL,
        codes::SECURITY_RATE_LIMITED,
        codes::PUBLICATION_REPOSITORY_FAILED,
        codes::PUBLICATION_COMPILE_FAILED,
        codes::PUBLICATION_RELEASE_STORE_FAILED,
        codes::PUBLICATION_SCHEDULER_STATE_FAILED,
        codes::RELEASE_OBJECT_STORE_FAILED,
        codes::RELEASE_MANIFEST_STORE_FAILED,
        codes::RELEASE_ACTIVATION_FAILED,
        codes::BACKUP_SCHEDULED_FAILED,
    ]
}

pub fn install_panic_hook() {
    std::panic::set_hook(Box::new(|info| {
        let (file, line, column) = info.location().map_or(("unknown", 0, 0), |location| {
            (location.file(), location.line(), location.column())
        });
        tracing::error!(
            event = "runtime.panic",
            panic_file = file,
            panic_line = line,
            panic_column = column,
            backtrace = %Backtrace::force_capture(),
            "panic captured"
        );
    }));
}
