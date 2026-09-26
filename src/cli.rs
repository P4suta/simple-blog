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

/// Shown by `--help` above the command list. Nothing reflows it: clap is built
/// without `wrap_help`, so each line here is a line on the reader's terminal.
const LONG_ABOUT: &str = "\
A writing-focused, single-owner CMS: one binary, one database file, one
data directory.

Every command acts on one installation, chosen with --data-dir and
defaulting to ./data. `init` prepares one, `serve` runs it, and the rest
move, copy, inspect, and repair it.

A setting resolves in one order: the command-line flag, then the matching
SIMPLE_BLOG_ variable, then config.toml in the data directory, then the
built-in default.";

/// Shown by `--help` below the command list.
const EPILOGUE: &str = "\
Examples:
  simple-blog init
      Prepare ./data and print a one-time link for the owner passkey.

  simple-blog serve
      Run the site and the admin at the addresses it prints.

  simple-blog import ./writing --force
      Read a folder of Markdown in, replacing pieces at matching addresses.

  simple-blog backup
      Write one complete archive and print its path, ready for `restore`.

  simple-blog migrate export --output site.simple-blog
      Pack the whole site, with its history and passkeys, for another host.

Exit codes:
  0  the command succeeded
  1  the command ran and failed; stderr says what failed and what to try
  2  the command never ran: it could not be understood, or diagnostics
     could not be started

Environment:
  SIMPLE_BLOG_DATA_DIR, SIMPLE_BLOG_BIND, SIMPLE_BLOG_PUBLIC_URL
      The global flags by another name; a flag wins over a variable.
  SIMPLE_BLOG_LOG_FORMAT=json
      Write diagnostics to stderr as one JSON object per line.
  RUST_LOG
      The tracing filter. Defaults to simple_blog=info.";

#[derive(Debug, Parser)]
#[command(
    name = "simple-blog",
    version,
    about,
    long_about = LONG_ABOUT,
    after_long_help = EPILOGUE
)]
pub struct Cli {
    /// Act on the installation in this directory instead of ./data.
    #[arg(long, global = true, value_name = "DIRECTORY")]
    data_dir: Option<PathBuf>,
    /// Listen on this address instead of 127.0.0.1:8080.
    #[arg(long, global = true, value_name = "ADDRESS")]
    bind: Option<String>,
    /// Treat this origin as the site's public address.
    ///
    /// Canonical links, feeds, setup links and the passkey identity derive from it.
    #[arg(long, global = true, value_name = "URL")]
    public_url: Option<String>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Prepare a new installation and print its setup link.
    ///
    /// Writes config.toml, the database, and the media, backup and release folders.
    ///
    /// The link registers the owner passkey and is valid for fifteen minutes.
    ///
    /// Running init again rewrites config.toml; a claimed site gets no new link.
    Init,
    /// Publish a release now, without running the site.
    ///
    /// Compiles the site, verifies the release, and activates it as the visible one.
    ///
    /// Prints one JSON object describing the release, for a machine to read.
    Build {
        /// Also write the visible release into this directory as plain files.
        ///
        /// The directory must not exist, so an existing site is never overwritten.
        #[arg(long, value_name = "DIRECTORY")]
        output: Option<PathBuf>,
    },
    /// Run the site and the admin until interrupted.
    ///
    /// Prints the site and admin addresses, and a setup link while no owner exists.
    ///
    /// Publishes on schedule, and backs up ten minutes after start, then daily.
    Serve,
    /// Write one complete backup archive.
    ///
    /// It holds the database, media, releases and settings, and `restore` reads it.
    ///
    /// Prints the archive's path and nothing else, so a script can capture it.
    Backup {
        /// Write the archive here instead of in the installation's backups folder.
        #[arg(long, value_name = "ARCHIVE")]
        output: Option<PathBuf>,
    },
    /// Replace an installation with the contents of a backup.
    ///
    /// An existing installation is refused unless --force is given.
    Restore {
        /// The backup archive to read.
        archive: PathBuf,
        /// Replace the existing installation instead of refusing to touch it.
        #[arg(long)]
        force: bool,
    },
    /// Write the site's Markdown and media out as plain files.
    ///
    /// Produces posts/, pages/ and media/, which import reads back.
    ///
    /// Prints the directory's path and nothing else.
    Export {
        /// Write the export into this directory instead of a dated one here.
        #[arg(long, value_name = "DIRECTORY")]
        output: Option<PathBuf>,
    },
    /// Read Markdown files into this site.
    ///
    /// Accepts an export directory, or any folder of plain .md files.
    ///
    /// A file without front matter is titled from its first heading.
    ///
    /// Publishes afterwards, so imported pieces are visible immediately.
    Import {
        /// The directory to read.
        directory: PathBuf,
        /// Replace pieces whose slug already exists instead of skipping them.
        #[arg(long)]
        force: bool,
    },
    /// Move a whole site between conforming hosts.
    Migrate {
        #[command(subcommand)]
        command: MigrateCommand,
    },
    /// Check the installation and report what it finds.
    Doctor {
        /// Print the report as one JSON object instead of a list of checks.
        #[arg(long)]
        json: bool,
        /// Explicitly create, synchronize and remove filesystem probe files.
        #[arg(long)]
        probe_writes: bool,
    },
    /// Recover ownership of an installation.
    Owner {
        #[command(subcommand)]
        command: OwnerCommand,
    },
}

#[derive(Debug, Subcommand)]
enum OwnerCommand {
    /// Print a fresh link for registering a replacement owner passkey.
    ///
    /// The link is a short-lived secret and is never written to a trace.
    Recover,
}

#[derive(Debug, Subcommand)]
enum MigrateCommand {
    /// Pack the entire site into one .simple-blog archive.
    ///
    /// Carries history, redirects, settings, media, trash and passkeys.
    ///
    /// Prints one JSON object describing the archive.
    Export {
        /// Write the archive to this path instead of a dated one here.
        #[arg(long, value_name = "ARCHIVE")]
        output: Option<PathBuf>,
    },
    /// Unpack a .simple-blog archive into this data directory.
    ///
    /// A fresh destination adopts the archive's origin, so addresses survive.
    ///
    /// Prints one JSON object describing the imported site.
    Import {
        /// The archive to read.
        archive: PathBuf,
        /// Replace the installation, keeping the previous data for recovery.
        #[arg(long)]
        force: bool,
    },
}

impl Cli {
    pub async fn run(self) -> Result<()> {
        let overrides = self.overrides();
        // Hold across configuration loading, DB use and replacement. The OS
        // releases the lease after a crash; doctor neither locks nor recovers.
        let _lease = if matches!(self.command, Command::Doctor { .. }) {
            None
        } else {
            let destination = data_dir(&overrides);
            let lease = crate::operations::activation::InstallationLease::acquire(&destination)?;
            crate::operations::activation::recover(&destination)?;
            Some(lease)
        };
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
                println!(
                    "Restored {} from {}.",
                    data_dir.display(),
                    archive.display()
                );
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
            Command::Doctor { json, probe_writes } => doctor(overrides, json, probe_writes).await,
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
    crate::observability::operation(cli.command.name(), Box::pin(cli.run())).await
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
        println!(
            "Initialized {}. An owner passkey is already registered.",
            config.data_dir.display()
        );
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
            phase = error.phase()
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
        "Imported {} piece(s) and {} media file(s).",
        report.imported.len(),
        report.media
    );
    for slug in &report.imported {
        println!("  /{slug}/");
    }
    if !report.skipped.is_empty() {
        println!("Skipped {} file(s):", report.skipped.len());
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

async fn doctor(overrides: Overrides, json: bool, probe_writes: bool) -> Result<()> {
    let config = Config::load(overrides).context("could not load configuration")?;
    let report = Doctor::inspect_installation(&config, probe_writes).await;
    let healthy = report.is_healthy();
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "diagnostics_schema": 1,
                "application_version": env!("CARGO_PKG_VERSION"),
                "healthy": healthy,
                "inspection_scope": if probe_writes { "write_probes" } else { "read_only" },
                "limits": report.limits,
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
        bail!("cannot recover an installation that has no owner passkey yet")
    }
    let token = AuthService::new(repository, Arc::new(SystemEntropy))
        .issue_setup_token(SetupPurpose::Recovery, Utc::now())
        .await
        .context("could not issue recovery token")?;
    println!("Open this link within 15 minutes to register a replacement owner passkey:");
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
