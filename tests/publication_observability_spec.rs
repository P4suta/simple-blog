use std::{
    io::Write,
    sync::{Arc, Mutex},
};

use chrono::{TimeZone, Utc};
use simple_blog::{
    application::{
        content::{ContentService, SaveIntent},
        publication::PublicationService,
        site_compiler::SiteCompiler,
    },
    domain::content::{ContentDraft, ContentKind, Publication, Slug},
    infrastructure::{markdown::ComrakMarkdownRenderer, sqlite::SqliteRepository},
    release::FilesystemReleaseStore,
};

#[derive(Clone, Default)]
struct TraceBuffer(Arc<Mutex<Vec<u8>>>);

impl Write for TraceBuffer {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0
            .lock()
            .map_err(|error| std::io::Error::other(error.to_string()))?
            .extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'writer> tracing_subscriber::fmt::MakeWriter<'writer> for TraceBuffer {
    type Writer = Self;

    fn make_writer(&'writer self) -> Self::Writer {
        self.clone()
    }
}

#[tokio::test(flavor = "current_thread")]
async fn publication_trace_has_a_build_id_revision_release_and_phase_events() {
    let temp = tempfile::tempdir().unwrap();
    let repository = Arc::new(
        SqliteRepository::connect(&temp.path().join("simple-blog.sqlite3"))
            .await
            .unwrap(),
    );
    let content = ContentService::new(
        repository.clone(),
        Arc::new(ComrakMarkdownRenderer::default()),
    );
    let store = Arc::new(FilesystemReleaseStore::new(temp.path().join("releases")));
    let publication = PublicationService::new(
        repository,
        store,
        SiteCompiler::embedded().unwrap(),
        "https://writing.example",
    )
    .unwrap();
    let now = Utc.with_ymd_and_hms(2026, 9, 2, 12, 0, 0).unwrap();
    content
        .create(
            ContentDraft {
                kind: ContentKind::Post,
                title: "Traced".into(),
                slug: Slug::parse("traced").unwrap(),
                summary: String::new(),
                body_markdown: "# Traced".into(),
                tags: Vec::new(),
                cover_media_id: None,
                seo_title: None,
                seo_description: None,
                publication: Publication::Public { publish_at: now },
            },
            SaveIntent::Explicit,
            now,
        )
        .await
        .unwrap();
    let traces = TraceBuffer::default();
    let subscriber = tracing_subscriber::fmt()
        .json()
        .without_time()
        .with_current_span(true)
        .with_span_list(true)
        .with_writer(traces.clone())
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);

    let outcome = publication.publish(now).await.unwrap();

    let output = String::from_utf8(traces.0.lock().unwrap().clone()).unwrap();
    let events = output
        .lines()
        .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
        .collect::<Vec<_>>();
    for name in [
        "publication.build.started",
        "publication.snapshot.loaded",
        "site.compile.completed",
        "release.publish.completed",
        "publication.build.completed",
    ] {
        assert!(
            events.iter().any(|event| event["fields"]["event"] == name),
            "missing {name}: {output}"
        );
    }
    let completed = events
        .iter()
        .find(|event| event["fields"]["event"] == "publication.build.completed")
        .unwrap();
    assert_eq!(
        completed["fields"]["release_id"],
        outcome.release_id.as_str()
    );
    assert_eq!(completed["fields"]["public_revision"], 1);
    assert!(uuid::Uuid::parse_str(completed["span"]["build_id"].as_str().unwrap()).is_ok());
}

#[tokio::test(flavor = "current_thread")]
async fn deferred_publication_retains_its_request_parent_until_a_successful_retry() {
    use simple_blog::{
        config::{Config, ConfigSources, Overrides},
        observability::request_scope,
        web::AppState,
    };
    let temp = tempfile::tempdir().unwrap();
    let config = Config::resolve(ConfigSources {
        cli: Overrides {
            data_dir: Some(temp.path().into()),
            ..Overrides::default()
        },
        ..ConfigSources::default()
    })
    .unwrap();
    let repository = Arc::new(
        SqliteRepository::connect(&config.database_path())
            .await
            .unwrap(),
    );
    // A real filesystem obstruction makes publication fail before activation.
    std::fs::write(config.release_dir(), b"synthetic obstruction").unwrap();
    let state = AppState::new(config.clone(), repository.clone()).unwrap();
    let traces = TraceBuffer::default();
    let subscriber = tracing_subscriber::fmt()
        .json()
        .without_time()
        .with_current_span(true)
        .with_span_list(true)
        .with_writer(traces.clone())
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);
    let parent = uuid::Uuid::new_v4();
    assert!(request_scope(parent, state.publish_now()).await.is_err());
    std::fs::remove_file(config.release_dir()).unwrap();
    state.publish_now().await.unwrap();
    state.publish_now().await.unwrap();
    let output = String::from_utf8(traces.0.lock().unwrap().clone()).unwrap();
    let started = output
        .lines()
        .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
        .filter(|event| event["fields"]["event"] == "operation.started")
        .collect::<Vec<_>>();
    assert_eq!(started.len(), 3);
    assert_eq!(
        started[0]["span"]["parent_operation_id"],
        parent.to_string()
    );
    assert_eq!(
        started[1]["span"]["parent_operation_id"],
        parent.to_string()
    );
    assert!(started[2]["span"]["parent_operation_id"].is_null());
    repository.close().await;
}
