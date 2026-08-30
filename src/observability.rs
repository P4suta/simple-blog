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
