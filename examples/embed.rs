// The smallest honest embedding of the Core: resolve a configuration, open the
// SQLite repository, build the application state, publish the first release,
// and serve the router. `cargo run --example embed` creates ./data-embed and
// then blocks until the process is interrupted.
use std::{error::Error, net::SocketAddr, path::PathBuf, sync::Arc};

use simple_blog::{
    config::{Config, ConfigSources, Overrides},
    infrastructure::sqlite::SqliteRepository,
    web::{AppState, router},
};
use tokio::{net::TcpListener, sync::watch};

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    // `Config::resolve` takes its sources explicitly, so an embedder decides
    // what the process environment is allowed to say. `Config::load` is the
    // binary's entry point: it reads the environment and data/config.toml.
    let config = Config::resolve(ConfigSources {
        cli: Overrides {
            data_dir: Some(PathBuf::from("./data-embed")),
            public_url: Some("http://localhost:8080".to_owned()),
            ..Overrides::default()
        },
        ..ConfigSources::default()
    })?;
    let bind = config.bind;

    // `connect` creates the database file and applies pending migrations.
    let repository = Arc::new(SqliteRepository::connect(&config.database_path()).await?);
    let state = AppState::new(config, repository)?;

    // A release store that fails at boot is not fatal: the scheduler retries
    // and the dashboard reports the site as pending.
    if let Err(error) = state.publish_now().await {
        eprintln!("initial release deferred: {error}");
    }

    let (shutdown, shutdown_rx) = watch::channel(false);
    let scheduler = state.clone();
    let publishing = tokio::spawn(async move {
        scheduler.run_publication_scheduler(shutdown_rx).await;
    });

    // Public handlers read the peer address, so the router has to be served
    // with connection info attached.
    let listener = TcpListener::bind(bind).await?;
    let served = axum::serve(
        listener,
        router(state).into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(interrupted())
    .await;

    // The scheduler is told to stop and awaited whether or not the server
    // ended well, so a failed serve does not leave a task publishing behind it.
    let _sent = shutdown.send(true);
    publishing.await?;
    served?;
    Ok(())
}

/// Ctrl-C ends the server so that the shutdown after it runs, instead of being
/// skipped by an exiting process. `simple-blog serve` also waits for SIGTERM.
async fn interrupted() {
    tokio::signal::ctrl_c()
        .await
        .expect("failed to install Ctrl+C handler");
}
