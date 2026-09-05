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
