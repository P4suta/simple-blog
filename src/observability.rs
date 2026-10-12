//! Tracing setup, the panic hook, and the stable diagnostic codes every
//! failure is traced with.

use std::{backtrace::Backtrace, future::Future, time::Instant};

use thiserror::Error;
use tracing::Instrument;
use tracing_subscriber::{EnvFilter, util::SubscriberInitExt};

tokio::task_local! { static OPERATION_ID: uuid::Uuid; }

pub async fn request_scope<T>(id: uuid::Uuid, future: impl Future<Output = T>) -> T {
    OPERATION_ID.scope(id, future).await
}

#[must_use]
pub fn current_operation_id() -> Option<uuid::Uuid> {
    OPERATION_ID.try_with(|id| *id).ok()
}

/// Explicit task-local identity survives awaits and never trusts inbound headers.
pub async fn operation<T, E>(
    name: &'static str,
    future: impl Future<Output = Result<T, E>>,
) -> Result<T, E> {
    let id = uuid::Uuid::new_v4();
    let parent = current_operation_id().map(|id| id.to_string());
    let span = tracing::info_span!("operation", operation = name, operation_id = %id,
        parent_operation_id = parent.as_deref(), elapsed_ms = tracing::field::Empty);
    let started = Instant::now();
    let guard_span = span.clone();
    OPERATION_ID
        .scope(
            id,
            async move {
                let mut guard = OperationGuard {
                    span: guard_span,
                    finished: false,
                };
                tracing::info!(event = "operation.started");
                let result = future.await;
                tracing::Span::current().record(
                    "elapsed_ms",
                    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
                );
                if result.is_ok() {
                    tracing::info!(event = "operation.completed");
                } else {
                    tracing::error!(event = "operation.failed");
                }
                guard.finished = true;
                result
            }
            .instrument(span),
        )
        .await
}

struct OperationGuard {
    span: tracing::Span,
    finished: bool,
}
impl Drop for OperationGuard {
    fn drop(&mut self) {
        if !self.finished {
            self.span
                .in_scope(|| tracing::warn!(event = "operation.cancelled"));
        }
    }
}

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
        // A person reading a failure needs the failure, not a thread id and
        // a source location. The JSON format keeps both: they are a contract.
        LogFormat::Pretty => tracing_subscriber::fmt()
            .with_env_filter(filter)
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
    pub const VIEWS_RECORD_FAILED: &str = "views.record_failed";
    pub const SETUP_TIMEZONE_NOT_ADOPTED: &str = "setup.timezone.not_adopted";
    pub const DATABASE_MIGRATION_FAILED: &str = "database.migration.failed";
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
        codes::VIEWS_RECORD_FAILED,
        codes::SETUP_TIMEZONE_NOT_ADOPTED,
        codes::DATABASE_MIGRATION_FAILED,
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

/// Collects what a piece of work told the operator, so a test can assert on
/// the events themselves rather than on a return value that carries none.
#[cfg(test)]
pub(crate) mod capture {
    use std::{
        io::Write,
        sync::{Arc, Mutex, PoisonError},
    };

    #[derive(Clone, Default)]
    pub struct Traces(Arc<Mutex<Vec<u8>>>);

    impl Traces {
        pub fn text(&self) -> String {
            let bytes = self.0.lock().unwrap_or_else(PoisonError::into_inner);
            String::from_utf8_lossy(&bytes).into_owned()
        }
    }

    impl Write for Traces {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl<'writer> tracing_subscriber::fmt::MakeWriter<'writer> for Traces {
        type Writer = Self;

        fn make_writer(&'writer self) -> Self::Writer {
            self.clone()
        }
    }

    /// Captures every event on this thread until the guard is dropped.
    pub fn traces() -> (Traces, tracing::subscriber::DefaultGuard) {
        let traces = Traces::default();
        let subscriber = tracing_subscriber::fmt()
            .without_time()
            .with_ansi(false)
            .with_target(false)
            .with_writer(traces.clone())
            .finish();
        let guard = tracing::subscriber::set_default(subscriber);
        (traces, guard)
    }
}
