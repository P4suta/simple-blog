//! Development-only fixture binary. Production routes have no test bypasses.
#[path = "support/faults.rs"]
mod faults;
use std::{
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Instant,
};
use tracing_subscriber::{Layer, layer::SubscriberExt};

use anyhow::{Context, Result, bail};
use chrono::{DateTime, TimeZone, Utc};
use simple_blog::{
    application::{
        auth::{AuthService, PasskeyAccountService},
        content::{ContentService, SaveIntent},
        ports::{Clock, SearchRepository, SiteRepository},
    },
    config::{Config, ConfigSources, Overrides},
    domain::{
        auth::{SetupPurpose, StoredPasskey},
        content::{ContentDraft, ContentKind, Publication, Slug},
        search::parse_query,
        theme::Locale,
    },
    infrastructure::{
        entropy::SystemEntropy, markdown::ComrakMarkdownRenderer, sqlite::SqliteRepository,
    },
    operations::{BackupService, RestoreService},
    web::AppState,
};

#[tokio::main]
async fn main() -> Result<()> {
    let arguments = std::env::args().skip(1).collect::<Vec<_>>();
    match arguments.first().map(String::as_str) {
        Some("run") if arguments.len() > 2 => {
            use clap::Parser;
            tracing::subscriber::set_global_default(
                tracing_subscriber::registry()
                    .with(faults::Faults(arguments[1].clone().into()))
                    .with(tracing_subscriber::EnvFilter::new("simple_blog=debug"))
                    .with(
                        tracing_subscriber::fmt::layer()
                            .json()
                            .with_writer(std::io::stderr),
                    ),
            )?;
            let cli = simple_blog::cli::Cli::try_parse_from(
                std::iter::once("simple-blog".to_owned()).chain(arguments[2..].iter().cloned()),
            )?;
            Box::pin(cli.run()).await
        }
        Some("seed" | "seed-unowned") if arguments.len() == 3 => {
            seed(
                Path::new(&arguments[1]),
                &arguments[2],
                arguments[0] == "seed",
            )
            .await
        }
        Some("benchmark") => benchmark().await,
        _ => bail!("usage: verification_fixture seed DIRECTORY ORIGIN | benchmark"),
    }
}

fn config(data: &Path, origin: &str) -> Result<Config> {
    Ok(Config::resolve(ConfigSources {
        cli: Overrides {
            data_dir: Some(data.into()),
            public_url: Some(origin.into()),
            backup_retention: Some(0),
            ..Overrides::default()
        },
        ..ConfigSources::default()
    })?)
}

async fn prepare(
    data: &Path,
    origin: &str,
    at: DateTime<Utc>,
) -> Result<(Config, Arc<SqliteRepository>)> {
    let config = config(data, origin)?;
    for path in [
        &config.data_dir,
        &config.media_dir(),
        &config.backup_dir(),
        &config.release_dir(),
    ] {
        std::fs::create_dir_all(path)?;
    }
    config.persist()?;
    let repository = Arc::new(SqliteRepository::connect(&config.database_path()).await?);
    let mut settings = repository.site_settings().await?;
    settings.locale = Locale::En;
    repository
        .save_configuration(&settings, &repository.navigation().await?, at)
        .await?;
    Ok((config, repository))
}

async fn seed(data: &Path, origin: &str, owned: bool) -> Result<()> {
    if data.join("simple-blog.sqlite3").exists() {
        bail!("seed requires an empty disposable installation");
    }
    let (config, repository) = prepare(data, origin, Utc::now()).await?;
    let entropy = Arc::new(SystemEntropy);
    let auth = AuthService::new(repository.clone(), entropy.clone());
    let token = auth
        .issue_setup_token(SetupPurpose::Initial, Utc::now())
        .await?;
    let credentials = if owned {
        let completed = PasskeyAccountService::new(repository.clone(), entropy)
            .complete_setup_registration(
                token.expose(),
                SetupPurpose::Initial,
                uuid::Uuid::new_v4(),
                StoredPasskey {
                    credential_id: vec![1, 2, 3],
                    name: "Disposable fixture".into(),
                    passkey_json: "{}".into(),
                },
                Utc::now(),
            )
            .await?
            .context("fixture owner registration")?;
        serde_json::json!({"recovery_codes": completed.recovery_codes.iter().map(simple_blog::domain::auth::SecretToken::expose).collect::<Vec<_>>() })
    } else {
        serde_json::json!({"recovery_codes":[],"setup_token":token.expose()})
    };
    let contents = ContentService::new(
        repository.clone(),
        Arc::new(ComrakMarkdownRenderer::default()),
    );
    contents
        .create(draft(0, Utc::now()), SaveIntent::Explicit, Utc::now())
        .await?;
    AppState::new(config, repository.clone())?
        .publish_now()
        .await?;
    // This stdout is a private pipe consumed by the test harness, never a log artifact.
    println!("{credentials}");
    repository.close().await;
    Ok(())
}

fn draft(index: u32, at: DateTime<Utc>) -> ContentDraft {
    ContentDraft {
        kind: ContentKind::Post,
        title: format!("Synthetic article {index}"),
        slug: Slug::parse(format!("synthetic-{index}")).expect("generated slug"),
        summary: "Synthetic verification content".into(),
        body_markdown: "# 日本語 Rust\n\nA searchable article.".repeat(8),
        tags: vec!["verification".into()],
        cover_media_id: None,
        seo_title: None,
        seo_description: None,
        publication: Publication::Public { publish_at: at },
    }
}

async fn benchmark() -> Result<()> {
    let queries = Arc::new(AtomicU64::new(0));
    tracing::subscriber::set_global_default(
        tracing_subscriber::registry().with(QueryCounter(queries.clone())),
    )?;
    let temp = tempfile::tempdir()?;
    let at = Utc.with_ymd_and_hms(2026, 9, 5, 0, 0, 0).unwrap();
    let (config, repository) =
        prepare(&temp.path().join("source"), "http://localhost:8080", at).await?;
    let contents = ContentService::new(
        repository.clone(),
        Arc::new(ComrakMarkdownRenderer::default()),
    );
    let started = Instant::now();
    let initial_queries = queries.load(Ordering::Relaxed);
    for index in 0..100 {
        contents
            .create(draft(index, at), SaveIntent::Explicit, at)
            .await?;
    }
    let render_store_ms = started.elapsed().as_millis();
    let rendered_queries = queries.load(Ordering::Relaxed);
    let state =
        AppState::new_with_clock(config.clone(), repository.clone(), Arc::new(FixedClock(at)))?;
    let started = Instant::now();
    let publication = state.publish_now().await?;
    let publish_ms = started.elapsed().as_millis();
    let published_queries = queries.load(Ordering::Relaxed);
    let terms = parse_query("日本語 Rust");
    let started = Instant::now();
    for _ in 0..100 {
        repository.search(&terms, at, 20).await?;
    }
    let search_ms = started.elapsed().as_millis();
    let searched_queries = queries.load(Ordering::Relaxed);
    if rendered_queries == initial_queries || searched_queries == published_queries {
        bail!("SQL measurement did not observe the workload; zero is not a valid measurement");
    }
    let archive = BackupService::create(&config, &repository, None, at).await?;
    repository.close().await;
    let started = Instant::now();
    RestoreService::restore(&archive, &temp.path().join("restored"), false).await?;
    println!(
        "{}",
        serde_json::json!({"schema":1,"dataset":"synthetic-100-v1", "dataset_time":at,"articles":100,
        "render_store_ms":render_store_ms,"publish_ms":publish_ms,"search_100_ms":search_ms,
        "restore_ms":started.elapsed().as_millis(),"published_routes":publication.route_count,
        "archive_bytes":std::fs::metadata(archive)?.len(),
        "db_operations":{"render_store":rendered_queries-initial_queries,"publish":published_queries-rendered_queries,"search_100":searched_queries-published_queries},
        "peak_resident_bytes":peak_memory(),"memory_scope":"whole fixture process including setup"})
    );
    Ok(())
}

struct FixedClock(DateTime<Utc>);
impl Clock for FixedClock {
    fn now(&self) -> DateTime<Utc> {
        self.0
    }
}

struct QueryCounter(Arc<AtomicU64>);
impl<S: tracing::Subscriber> Layer<S> for QueryCounter {
    fn on_event(
        &self,
        event: &tracing::Event<'_>,
        _context: tracing_subscriber::layer::Context<'_, S>,
    ) {
        if event.metadata().target() == "sqlx::query" {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
    }
}

fn peak_memory() -> Option<u64> {
    if cfg!(target_os = "linux") {
        let status = std::fs::read_to_string("/proc/self/status").ok()?;
        status
            .lines()
            .find_map(|line| line.strip_prefix("VmHWM:"))?
            .split_whitespace()
            .next()?
            .parse::<u64>()
            .ok()?
            .checked_mul(1024)
    } else if cfg!(windows) {
        let output = std::process::Command::new("powershell.exe")
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                &format!("(Get-Process -Id {}).PeakWorkingSet64", std::process::id()),
            ])
            .output()
            .ok()?;
        output.status.success().then_some(())?;
        std::str::from_utf8(&output.stdout)
            .ok()?
            .trim()
            .parse()
            .ok()
    } else {
        None
    }
}
