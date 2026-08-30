use std::{io::Write, net::SocketAddr, path::PathBuf, sync::Arc};

use anyhow::{Context, Result, anyhow, bail};
use chrono::Utc;
use clap::{Parser, Subcommand};
use tokio::net::TcpListener;
use url::Url;
use uuid::Uuid;

use crate::{
    application::{auth::AuthService, ports::PasskeyRepository},
    config::{Config, ConfigFile, Overrides},
    domain::auth::SetupPurpose,
    infrastructure::sqlite::SqliteRepository,
    operations::{BackupService, Doctor, Exporter, MigrationCoordinator, RestoreService},
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

impl Cli {
    pub async fn run(self) -> Result<()> {
        let overrides = self.overrides();
        match self.command {
            Command::Init => init(overrides).await,
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
            Self::Serve => "serve",
            Self::Backup { .. } => "backup",
            Self::Restore { .. } => "restore",
            Self::Export { .. } => "export",
            Self::Doctor { .. } => "doctor",
            Self::Owner { .. } => "owner",
        }
    }
}

async fn init(overrides: Overrides) -> Result<()> {
    let config = Config::load(overrides).context("could not load configuration")?;
    std::fs::create_dir_all(config.media_dir()).context("could not create media directory")?;
    std::fs::create_dir_all(config.backup_dir()).context("could not create backup directory")?;
    write_config(&config)?;
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
    let token = AuthService::new(repository)
        .issue_setup_token(SetupPurpose::Initial, Utc::now())
        .await
        .context("could not issue setup token")?;
    println!("{}", setup_url(&config.public_url, token.expose())?);
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
    let app = router(AppState::new(config, repository).context("could not build web application")?);
    let listener = TcpListener::bind(bind)
        .await
        .with_context(|| format!("could not bind {bind}"))?;
    tracing::info!(%bind, "simple-blog is listening");
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal())
    .await
    .context("web server failed")
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
    let token = AuthService::new(repository)
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

fn write_config(config: &Config) -> Result<()> {
    let file = ConfigFile {
        data_dir: None,
        bind: Some(config.bind.to_string()),
        public_url: Some(config.public_url.to_string()),
        trusted_proxies: Some(config.trusted_proxies.clone()),
        max_upload_bytes: Some(config.max_upload_bytes),
    };
    let contents = toml::to_string_pretty(&file).context("could not serialize configuration")?;
    let path = config.data_dir.join("config.toml");
    let temporary = config
        .data_dir
        .join(format!(".config-{}.toml", Uuid::new_v4()));
    let result = (|| -> Result<()> {
        let mut output = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .context("could not create configuration")?;
        output
            .write_all(contents.as_bytes())
            .context("could not write configuration")?;
        output.sync_all().context("could not sync configuration")?;
        std::fs::rename(&temporary, &path).context("could not install configuration")?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
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
