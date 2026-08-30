#[tokio::main]
async fn main() {
    if let Err(error) = simple_blog::observability::init_tracing() {
        eprintln!("could not initialize diagnostics: {error}");
        std::process::exit(2);
    }
    simple_blog::observability::install_panic_hook();

    if let Err(error) = simple_blog::cli::run().await {
        tracing::error!(
            event = "cli.command.failed",
            error_kind = "command",
            error = %format!("{error:#}"),
            "command failed"
        );
        std::process::exit(1);
    }
}
