use std::sync::{Arc, Mutex};

use axum::{
    body::Body,
    http::{Request, StatusCode, header},
};
use chrono::{DateTime, Duration, TimeZone, Utc};
use http_body_util::BodyExt;
use simple_blog::{
    application::{
        content::{ContentService, SaveIntent},
        ports::Clock,
    },
    config::{Config, ConfigSources, Overrides},
    domain::content::{ContentDraft, ContentKind, Publication, Slug},
    infrastructure::{markdown::ComrakMarkdownRenderer, sqlite::SqliteRepository},
    web::{AppState, router},
};
use tower::ServiceExt;

struct Harness {
    _temp: tempfile::TempDir,
    repository: Arc<SqliteRepository>,
    service: ContentService,
    state: AppState,
}

impl Harness {
    async fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let config = Config::resolve(ConfigSources {
            cli: Overrides {
                data_dir: Some(temp.path().to_path_buf()),
                public_url: Some("http://localhost:8080".into()),
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
        let service = ContentService::new(
            repository.clone(),
            Arc::new(ComrakMarkdownRenderer::default()),
        );
        let state = AppState::new(config, repository.clone()).unwrap();
        Self {
            _temp: temp,
            repository,
            service,
            state,
        }
    }

    async fn request(&self, path: &str) -> axum::response::Response {
        router(self.state.clone())
            .oneshot(
                Request::builder()
                    .uri(path)
                    .header(header::HOST, "localhost:8080")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap()
    }

    async fn new_with_clock(clock: Arc<dyn Clock>) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let config = Config::resolve(ConfigSources {
            cli: Overrides {
                data_dir: Some(temp.path().to_path_buf()),
                public_url: Some("http://localhost:8080".into()),
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
        let service = ContentService::new(
            repository.clone(),
            Arc::new(ComrakMarkdownRenderer::default()),
        );
        let state = AppState::new_with_clock(config, repository.clone(), clock).unwrap();
        Self {
            _temp: temp,
            repository,
            service,
            state,
        }
    }
}

#[derive(Clone)]
struct TestClock(Arc<Mutex<DateTime<Utc>>>);

impl TestClock {
    fn new(now: DateTime<Utc>) -> Self {
        Self(Arc::new(Mutex::new(now)))
    }

    fn set(&self, now: DateTime<Utc>) {
        *self.0.lock().unwrap() = now;
    }
}

impl Clock for TestClock {
    fn now(&self) -> DateTime<Utc> {
        *self.0.lock().unwrap()
    }
}

fn draft(title: &str, slug: &str, publication: Publication) -> ContentDraft {
    ContentDraft {
        kind: ContentKind::Post,
        title: title.into(),
        slug: Slug::parse(slug).unwrap(),
        summary: format!("Summary for {title}"),
        body_markdown: format!("# {title}\n\nReadable body."),
        tags: vec!["Rust".into()],
        cover_media_id: None,
        seo_title: None,
        seo_description: None,
        publication,
    }
}

async fn body_text(response: axum::response::Response) -> String {
    String::from_utf8(
        response
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes()
            .to_vec(),
    )
    .unwrap()
}

#[tokio::test]
async fn home_is_server_rendered_and_never_leaks_drafts_or_javascript() {
    let harness = Harness::new().await;
    let now = Utc::now() - Duration::seconds(1);
    harness
        .service
        .create(
            draft(
                "Public essay",
                "public-essay",
                Publication::Public { publish_at: now },
            ),
            SaveIntent::Explicit,
            now,
        )
        .await
        .unwrap();
    harness
        .service
        .create(
            draft("Private draft", "private", Publication::Draft),
            SaveIntent::Explicit,
            now,
        )
        .await
        .unwrap();

    let response = harness.request("/").await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_text(response).await;
    assert!(body.contains("Public essay"));
    assert!(!body.contains("Private draft"));
    assert!(!body.contains("<script"));
    assert!(body.contains("rel=\"canonical\" href=\"http://localhost:8080/\""));
    assert!(body.contains("class=\"skip-link\""));
}

#[tokio::test]
async fn scheduled_visibility_uses_an_injected_clock_at_the_exact_boundary() {
    let before = Utc.with_ymd_and_hms(2026, 8, 30, 12, 0, 0).unwrap();
    let publish_at = before + Duration::minutes(1);
    let clock = TestClock::new(before);
    let harness = Harness::new_with_clock(Arc::new(clock.clone())).await;
    harness
        .service
        .create(
            draft(
                "Boundary post",
                "boundary-post",
                Publication::Public { publish_at },
            ),
            SaveIntent::Explicit,
            before,
        )
        .await
        .unwrap();

    assert_eq!(
        harness.request("/boundary-post/").await.status(),
        StatusCode::NOT_FOUND
    );
    clock.set(publish_at);
    assert_eq!(
        harness.request("/boundary-post/").await.status(),
        StatusCode::OK
    );
}

#[tokio::test]
async fn permalink_is_canonical_and_supports_conditional_get() {
    let harness = Harness::new().await;
    let now = Utc::now();
    harness
        .service
        .create(
            draft(
                "Canonical",
                "canonical",
                Publication::Public { publish_at: now },
            ),
            SaveIntent::Explicit,
            now,
        )
        .await
        .unwrap();

    let redirect = harness.request("/canonical").await;
    assert_eq!(redirect.status(), StatusCode::PERMANENT_REDIRECT);
    assert_eq!(redirect.headers()[header::LOCATION], "/canonical/");

    let response = harness.request("/canonical/").await;
    assert_eq!(response.status(), StatusCode::OK);
    let etag = response.headers()[header::ETAG].clone();
    assert!(response.headers().contains_key(header::LAST_MODIFIED));

    let not_modified = router(harness.state.clone())
        .oneshot(
            Request::builder()
                .uri("/canonical/")
                .header(header::HOST, "localhost:8080")
                .header(header::IF_NONE_MATCH, etag)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(not_modified.status(), StatusCode::NOT_MODIFIED);
}

#[tokio::test]
async fn old_slugs_redirect_once_to_the_current_permalink() {
    let harness = Harness::new().await;
    let now = Utc::now();
    let created = harness
        .service
        .create(
            draft("Moved", "before", Publication::Public { publish_at: now }),
            SaveIntent::Explicit,
            now,
        )
        .await
        .unwrap();
    let mut update = created.to_draft();
    update.slug = Slug::parse("after").unwrap();
    harness
        .service
        .update(
            created.id,
            created.version,
            update,
            SaveIntent::Explicit,
            now,
        )
        .await
        .unwrap();

    let response = harness.request("/before/").await;
    assert_eq!(response.status(), StatusCode::MOVED_PERMANENTLY);
    assert_eq!(response.headers()[header::LOCATION], "/after/");
}

#[tokio::test]
async fn archive_tag_feed_and_sitemap_share_the_publication_policy() {
    let harness = Harness::new().await;
    let now = Utc::now();
    harness
        .service
        .create(
            draft(
                "In feeds",
                "in-feeds",
                Publication::Public { publish_at: now },
            ),
            SaveIntent::Explicit,
            now,
        )
        .await
        .unwrap();
    harness
        .service
        .create(
            draft(
                "Not yet",
                "not-yet",
                Publication::Public {
                    publish_at: now + Duration::days(1),
                },
            ),
            SaveIntent::Explicit,
            now,
        )
        .await
        .unwrap();

    for path in ["/archive/", "/tag/rust/", "/feed.xml", "/sitemap.xml"] {
        let response = harness.request(path).await;
        assert_eq!(response.status(), StatusCode::OK, "{path}");
        let body = body_text(response).await;
        let (published, future) = if path == "/sitemap.xml" {
            ("in-feeds", "not-yet")
        } else {
            ("In feeds", "Not yet")
        };
        assert!(body.contains(published), "{path}");
        assert!(!body.contains(future), "{path}");
    }
}

#[tokio::test]
async fn host_header_is_validated_but_health_remains_probeable() {
    let harness = Harness::new().await;
    let bad_host = router(harness.state.clone())
        .oneshot(
            Request::builder()
                .uri("/")
                .header(header::HOST, "attacker.example")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(bad_host.status(), StatusCode::BAD_REQUEST);

    let health = router(harness.state.clone())
        .oneshot(
            Request::builder()
                .uri("/healthz")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(health.status(), StatusCode::OK);
    assert_eq!(body_text(health).await, "ok\n");
}

#[tokio::test]
async fn every_response_has_a_server_generated_correlation_id_and_safe_failures() {
    let harness = Harness::new().await;
    harness.repository.close().await;
    let response = router(harness.state.clone())
        .oneshot(
            Request::builder()
                .uri("/")
                .header(header::HOST, "localhost:8080")
                .header("x-request-id", "client-controlled")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    let request_id = response.headers()["x-request-id"].to_str().unwrap();
    assert_ne!(request_id, "client-controlled");
    assert!(uuid::Uuid::parse_str(request_id).is_ok());
    assert_eq!(body_text(response).await, "Internal Server Error");
}
