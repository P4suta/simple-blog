#[tokio::main]
async fn main() {
    if let Err(error) = simple_blog::observability::init_tracing() {
        eprintln!("could not initialize diagnostics: {error}");
        std::process::exit(simple_blog::cli_report::UNUSABLE);
    }
    simple_blog::observability::install_panic_hook();

    if let Err(error) = simple_blog::cli::run().await {
        tracing::error!(
            event = "cli.command.failed",
            error_kind = "command",
            error = %format!("{error:#}"),
            "command failed"
        );
        // The trace is the machine's account and is always emitted. The human
        // account stands down when stderr has been declared machine-readable,
        // so `SIMPLE_BLOG_LOG_FORMAT=json` keeps its promise even on failure.
        if !machine_readable_diagnostics() {
            let mut stderr = std::io::stderr().lock();
            let _rendered = simple_blog::cli_report::render(&error, &mut stderr);
        }
        std::process::exit(simple_blog::cli_report::FAILURE);
    }
}

/// `init_tracing` has already rejected any value other than `pretty` or `json`.
fn machine_readable_diagnostics() -> bool {
    std::env::var("SIMPLE_BLOG_LOG_FORMAT").is_ok_and(|format| format.eq_ignore_ascii_case("json"))
}
