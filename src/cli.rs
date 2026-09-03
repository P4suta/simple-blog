use std::{net::SocketAddr, path::PathBuf, sync::Arc};

use anyhow::{Context, Result, anyhow, bail};
use chrono::Utc;
use clap::{Parser, Subcommand};
use tokio::net::TcpListener;
use url::Url;

use crate::{
    application::{auth::AuthService, ports::PasskeyRepository},
    config::{Config, Overrides},
    domain::auth::SetupPurpose,
    infrastructure::{entropy::SystemEntropy, sqlite::SqliteRepository},
    materialize::ReleaseMaterializer,
    operations::{
        BackupService, Doctor, Exporter, Importer, MigrationCoordinator, PortableMigrationService,
        RestoreService,
    },
    portable::PortableArchive,
    web::{AppState, router},
};

#[derive(Debug, Parser)]
#[command(name = "simple-blog", version, about)]
pub struct Cli {
    #[arg(long, global = true, value_name = "DIRECTORY")]
    data_dir: Option<PathBuf>,
    #[arg(long, global = true, value_name = "ADDRESS")]
    bind: Option<String>,
    #[arg(long, global = true, value_name = "URL")]
    public_url: Option<String>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    Init,
    Build {
        #[arg(long, value_name = "DIRECTORY")]
        output: Option<PathBuf>,
    },
    Serve,
    Backup {
        #[arg(long, value_name = "ARCHIVE")]
        output: Option<PathBuf>,
    },
    Restore {
        archive: PathBuf,
        #[arg(long)]
        force: bool,
    },
    Export {
        #[arg(long, value_name = "DIRECTORY")]
        output: Option<PathBuf>,
    },
    /// Reads Markdown files (an `export` directory, or plain files under
    /// posts/ and pages/) into this site.
    Import {
        directory: PathBuf,
        /// Replace pieces whose slug already exists instead of skipping them.
        #[arg(long)]
        force: bool,
    },
    Migrate {
        #[command(subcommand)]
        command: MigrateCommand,
    },
    Doctor {
        #[arg(long)]
        json: bool,
    },
    Owner {
        #[command(subcommand)]
        command: OwnerCommand,
    },
}

#[derive(Debug, Subcommand)]
enum OwnerCommand {
    Recover,
}

#[derive(Debug, Subcommand)]
enum MigrateCommand {
    Export {
        #[arg(long, value_name = "ARCHIVE")]
        output: Option<PathBuf>,
    },
    Import {
        archive: PathBuf,
        #[arg(long)]
        force: bool,
    },
}

impl Cli {
    pub async fn run(self) -> Result<()> {
        let overrides = self.overrides();
        match self.command {
            Command::Init => init(overrides).await,
            Command::Build { output } => build(overrides, output).await,
            Command::Serve => serve(overrides).await,
            Command::Backup { output } => backup(overrides, output).await,
            Command::Restore { archive, force } => {
                let data_dir = data_dir(&overrides);
                RestoreService::restore(&archive, &data_dir, force)
                    .await
                    .with_context(|| format!("could not restore {}", archive.display()))?;
                println!("restored {}", data_dir.display());
                Ok(())
            }
            Command::Export { output } => export(overrides, output).await,
            Command::Import { directory, force } => import(overrides, directory, force).await,
            Command::Migrate { command } => match command {
                MigrateCommand::Export { output } => migrate_export(overrides, output).await,
                MigrateCommand::Import { archive, force } => {
                    migrate_import(overrides, archive, force).await
                }
            },
            Command::Doctor { json } => doctor(overrides, json).await,
            Command::Owner { command } => match command {
                OwnerCommand::Recover => owner_recover(overrides).await,
            },
        }
    }

    fn overrides(&self) -> Overrides {
        Overrides {
            data_dir: self.data_dir.clone(),
            bind: self.bind.clone(),
            public_url: self.public_url.clone(),
            ..Overrides::default()
        }
    }
}

pub async fn run() -> Result<()> {
    let cli = Cli::parse();
    tracing::info!(
        event = "cli.command.started",
        diagnostics_schema = 1_u8,
        version = env!("CARGO_PKG_VERSION"),
        command = cli.command.name(),
        "command started"
    );
    cli.run().await
}

impl Command {
    const fn name(&self) -> &'static str {
        match self {
            Self::Init => "init",
            Self::Build { .. } => "build",
            Self::Serve => "serve",
            Self::Backup { .. } => "backup",
            Self::Restore { .. } => "restore",
            Self::Export { .. } => "export",
            Self::Import { .. } => "import",
            Self::Migrate { .. } => "migrate",
            Self::Doctor { .. } => "doctor",
            Self::Owner { .. } => "owner",
        }
    }
}

async fn init(overrides: Overrides) -> Result<()> {
    let config = Config::load(overrides).context("could not load configuration")?;
    std::fs::create_dir_all(config.media_dir()).context("could not create media directory")?;
    std::fs::create_dir_all(config.backup_dir()).context("could not create backup directory")?;
    std::fs::create_dir_all(config.release_dir()).context("could not create release directory")?;
    config.persist().context("could not write configuration")?;
    let repository = Arc::new(
        open_database(&config)
            .await
            .context("could not initialize SQLite")?,
    );
    if repository
        .owner_handle()
        .await
        .context("could not inspect owner state")?
        .is_some()
    {
        println!("initialized {}", config.data_dir.display());
        return Ok(());
    }
    let token = AuthService::new(repository, Arc::new(SystemEntropy))
        .issue_setup_token(SetupPurpose::Initial, Utc::now())
        .await
        .context("could not issue setup token")?;
    println!(
        "Initialized {}.\nNo owner passkey is registered yet. Open this link within 15 minutes to register one:\n{}\nThen start the site with `simple-blog serve`; it prints a fresh link if this one expires.",
        config.data_dir.display(),
        setup_url(&config.public_url, token.expose())?
    );
    Ok(())
}

async fn build(overrides: Overrides, output: Option<PathBuf>) -> Result<()> {
    let config = Config::load(overrides).context("could not load configuration")?;
    ensure_initialized(&config)?;
    let repository = Arc::new(
        open_database(&config)
            .await
            .context("could not open SQLite")?,
    );
    let state = AppState::new(config, repository).context("could not build publication core")?;
    let outcome = state
        .publish_now()
        .await
        .context("could not build public release")?;
    let verification = state
        .release_store
        .verify_active()
        .await
        .context("could not verify active public release")?;
    let materialized = if let Some(output) = &output {
        Some(
            ReleaseMaterializer::new(state.release_store.clone())
                .materialize(output)
                .await
                .with_context(|| format!("could not materialize {}", output.display()))?,
        )
    } else {
        None
    };
    println!(
        "{}",
        serde_json::to_string(&serde_json::json!({
            "release_id": outcome.release_id.as_str(),
            "public_revision": outcome.public_revision,
            "disposition": outcome.disposition,
            "routes": outcome.route_count,
            "objects": verification.object_count,
            "bytes": verification.total_bytes,
            "materialized_to": output.as_ref().map(|path| path.display().to_string()),
            "materialized_assets": materialized.as_ref().map(|report| report.asset_count),
            "materialized_redirects": materialized.as_ref().map(|report| report.redirect_count),
        }))?
    );
    Ok(())
}

async fn serve(overrides: Overrides) -> Result<()> {
    let config = Config::load(overrides).context("could not load configuration")?;
    let bind = config.bind;
    let repository = Arc::new(
        open_database(&config)
            .await
            .context("could not open SQLite")?,
    );
    let public_url = config.public_url.clone();
    if repository
        .owner_handle()
        .await
        .context("could not inspect owner state")?
        .is_none()
    {
        // A fresh installation started with `serve` alone must still be
        // claimable: print the same one-time setup link `init` would.
        let token = AuthService::new(repository.clone(), Arc::new(SystemEntropy))
            .issue_setup_token(SetupPurpose::Initial, Utc::now())
            .await
            .context("could not issue setup token")?;
        println!(
            "No owner passkey is registered yet. Open this link within 15 minutes to register one:\n{}",
            setup_url(&public_url, token.expose())?
        );
    }
    let state = AppState::new(config, repository).context("could not build web application")?;
    // A broken release store at boot is not fatal: the scheduler keeps
    // retrying with backoff, and the dashboard says the site is pending.
    match state.publish_now().await {
        Ok(initial) => tracing::info!(
            event = "server.initial_release.ready",
            release_id = %initial.release_id,
            public_revision = initial.public_revision,
            disposition = ?initial.disposition
        ),
        Err(error) => tracing::error!(
            event = "server.initial_release.deferred",
            error_code = error.code(),
            phase = error.phase(),
            error = %error
        ),
    }
    let app = router(state.clone());
    let listener = TcpListener::bind(bind)
        .await
        .with_context(|| format!("could not bind {bind}"))?;
    tracing::info!(%bind, "simple-blog is listening");
    println!("Site:  {public_url}\nAdmin: {public_url}admin/");
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    let backup_rx = shutdown_tx.subscribe();
    let backup_state = state.clone();
    let backups = tokio::spawn(async move {
        backup_state.run_backup_scheduler(backup_rx).await;
    });
    let scheduler = tokio::spawn(async move {
        state.run_publication_scheduler(shutdown_rx).await;
    });
    let signal_tx = shutdown_tx.clone();
    let server = axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(async move {
        shutdown_signal().await;
        let _sent = signal_tx.send(true);
    })
    .await;
    let _sent = shutdown_tx.send(true);
    scheduler
        .await
        .context("publication scheduler task failed")?;
    backups.await.context("backup scheduler task failed")?;
    server.context("web server failed")
}

async fn backup(overrides: Overrides, output: Option<PathBuf>) -> Result<()> {
    let config = Config::load(overrides).context("could not load configuration")?;
    ensure_initialized(&config)?;
    let repository = open_database(&config)
        .await
        .context("could not open SQLite")?;
    let archive = BackupService::create(&config, &repository, output, Utc::now())
        .await
        .context("could not create backup")?;
    println!("{}", archive.display());
    Ok(())
}

async fn import(overrides: Overrides, directory: PathBuf, force: bool) -> Result<()> {
    let config = Config::load(overrides).context("could not load configuration")?;
    ensure_initialized(&config)?;
    let repository = Arc::new(
        open_database(&config)
            .await
            .context("could not open SQLite")?,
    );
    let report = Importer::import(&config, &repository, &directory, force, Utc::now())
        .await
        .with_context(|| format!("could not import {}", directory.display()))?;
    println!(
        "imported {} piece(s), {} media file(s)",
        report.imported.len(),
        report.media
    );
    for slug in &report.imported {
        println!("  /{slug}/");
    }
    if !report.skipped.is_empty() {
        println!("skipped {}:", report.skipped.len());
        for (file, reason) in &report.skipped {
            println!("  {file}: {reason}");
        }
    }
    // Everything imported is visible only once a release carries it. The
    // pieces are already saved at this point, so a publishing problem is
    // reported as such rather than as a failed import.
    let state = AppState::new(config, repository)
        .context("the import is saved, but the site could not be prepared for publishing")?;
    state
        .publish_now()
        .await
        .context("the import is saved, but the site could not be published")?;
    Ok(())
}

async fn export(overrides: Overrides, output: Option<PathBuf>) -> Result<()> {
    let config = Config::load(overrides).context("could not load configuration")?;
    ensure_initialized(&config)?;
    let repository = open_database(&config)
        .await
        .context("could not open SQLite")?;
    let output = output.unwrap_or_else(|| {
        PathBuf::from(format!(
            "simple-blog-export-{}",
            Utc::now().format("%Y%m%d-%H%M%S")
        ))
    });
    let output = Exporter::export(&config, &repository, &output, Utc::now())
        .await
        .context("could not export content")?;
    println!("{}", output.display());
    Ok(())
}

async fn migrate_export(overrides: Overrides, output: Option<PathBuf>) -> Result<()> {
    let config = Config::load(overrides).context("could not load configuration")?;
    ensure_initialized(&config)?;
    let repository = open_database(&config)
        .await
        .context("could not open SQLite")?;
    let output = output.unwrap_or_else(|| {
        PathBuf::from(format!(
            "simple-blog-{}.simple-blog",
            Utc::now().format("%Y%m%d-%H%M%S")
        ))
    });
    let report = PortableMigrationService::export(&config, &repository, &output, Utc::now())
        .await
        .with_context(|| format!("could not create portable archive {}", output.display()))?;
    println!(
        "{}",
        serde_json::to_string(&serde_json::json!({
            "archive": output.display().to_string(),
            "archive_id": report.archive_id,
            "entries": report.entry_count,
        }))?
    );
    Ok(())
}

async fn migrate_import(mut overrides: Overrides, archive: PathBuf, force: bool) -> Result<()> {
    let package = PortableArchive::read(&archive)
        .with_context(|| format!("could not read portable archive {}", archive.display()))?;
    let origin_is_explicit =
        overrides.public_url.is_some() || std::env::var_os("SIMPLE_BLOG_PUBLIC_URL").is_some();
    let mut config =
        Config::load(overrides.clone()).context("could not load destination configuration")?;
    if !origin_is_explicit && !config.data_dir.join("config.toml").is_file() {
        overrides.public_url = Some(package.site.canonical_origin.clone());
        config = Config::load(overrides).context("could not load destination configuration")?;
    }
    let report = PortableMigrationService::import_package(&archive, package, &config, force)
        .await
        .with_context(|| format!("could not import portable archive {}", archive.display()))?;
    println!(
        "{}",
        serde_json::to_string(&serde_json::json!({
            "data_dir": config.data_dir.display().to_string(),
            "release_id": report.release_id,
            "contents": report.content_count,
            "media": report.media_count,
            "previous_data_retained_at": report
                .replaced_data_dir
                .map(|path| path.display().to_string()),
        }))?
    );
    Ok(())
}

async fn doctor(overrides: Overrides, json: bool) -> Result<()> {
    let config = Config::load(overrides).context("could not load configuration")?;
    ensure_initialized(&config)?;
    let repository = open_database(&config)
        .await
        .context("could not open SQLite")?;
    let report = Doctor::inspect(&config, &repository)
        .await
        .context("could not inspect installation")?;
    let healthy = report.is_healthy();
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "diagnostics_schema": 1,
                "application_version": env!("CARGO_PKG_VERSION"),
                "healthy": healthy,
                "checks": report.checks,
                "issues": report.issues,
            }))?
        );
    } else if healthy {
        for check in &report.checks {
            println!("[ok] {}: {}", check.name, check.detail);
        }
        println!("healthy");
    } else {
        for check in &report.checks {
            println!("[{}] {}: {}", check.status, check.name, check.detail);
        }
    }
    if healthy {
        return Ok(());
    }
    bail!("installation is unhealthy")
}

async fn owner_recover(overrides: Overrides) -> Result<()> {
    let config = Config::load(overrides).context("could not load configuration")?;
    ensure_initialized(&config)?;
    let repository = Arc::new(
        open_database(&config)
            .await
            .context("could not open SQLite")?,
    );
    if repository
        .owner_handle()
        .await
        .context("could not inspect owner state")?
        .is_none()
    {
        bail!("owner has not completed initial setup")
    }
    let token = AuthService::new(repository, Arc::new(SystemEntropy))
        .issue_setup_token(SetupPurpose::Recovery, Utc::now())
        .await
        .context("could not issue recovery token")?;
    println!("{}", setup_url(&config.public_url, token.expose())?);
    Ok(())
}

fn data_dir(overrides: &Overrides) -> PathBuf {
    overrides
        .data_dir
        .clone()
        .or_else(|| std::env::var_os("SIMPLE_BLOG_DATA_DIR").map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("./data"))
}

async fn open_database(config: &Config) -> Result<SqliteRepository> {
    MigrationCoordinator::open(config, Utc::now())
        .await
        .map(|database| database.repository)
        .context("database preparation failed")
}

fn ensure_initialized(config: &Config) -> Result<()> {
    if config.database_path().is_file() {
        Ok(())
    } else {
        Err(anyhow!(
            "installation is not initialized; run `simple-blog init`"
        ))
    }
}

fn setup_url(origin: &Url, token: &str) -> Result<Url> {
    let mut url = origin
        .join("admin/setup/")
        .context("could not construct setup URL")?;
    url.query_pairs_mut().append_pair("token", token);
    Ok(url)
}

async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install termination handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        () = ctrl_c => {},
        () = terminate => {},
    }
}
