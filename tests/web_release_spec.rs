use std::sync::Arc;

use axum::{
    body::Body,
    http::{Request, StatusCode, header},
};
use chrono::{DateTime, TimeZone, Utc};
use http_body_util::BodyExt;
use simple_blog::{
    application::{
        content::{ContentService, SaveIntent},
        ports::{Clock, EngagementRepository},
    },
    config::{Config, ConfigSources, Overrides},
    domain::content::{ContentDraft, ContentId, ContentKind, Publication, Slug},
    infrastructure::{markdown::ComrakMarkdownRenderer, sqlite::SqliteRepository},
    release::{FilesystemReleaseStore, ReleaseReader, ReleaseStore},
    web::{AppState, router},
};
use tower::ServiceExt;

#[derive(Clone)]
struct FixedClock(DateTime<Utc>);

impl Clock for FixedClock {
    fn now(&self) -> DateTime<Utc> {
        self.0
    }
}

struct Harness {
    _temp: tempfile::TempDir,
    config: Config,
    repository: Arc<SqliteRepository>,
    content: ContentService,
    state: AppState,
    now: DateTime<Utc>,
}

impl Harness {
    async fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let now = Utc.with_ymd_and_hms(2026, 9, 2, 12, 34, 56).unwrap();
        let config = Config::resolve(ConfigSources {
            cli: Overrides {
                data_dir: Some(temp.path().to_owned()),
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
        let content = ContentService::new(
            repository.clone(),
            Arc::new(ComrakMarkdownRenderer::default()),
        );
        let state = AppState::new_with_clock(
            config.clone(),
            repository.clone(),
            Arc::new(FixedClock(now)),
        )
        .unwrap();
        Self {
            _temp: temp,
            config,
            repository,
            content,
            state,
            now,
        }
    }

    async fn create_and_publish(&self) -> ContentId {
        let content = self
            .content
            .create(
                ContentDraft {
                    kind: ContentKind::Post,
                    title: "A static story".into(),
                    slug: Slug::parse("static-story").unwrap(),
                    summary: "Compiled once".into(),
                    body_markdown: "# Durable output".into(),
                    tags: Vec::new(),
                    cover_media_id: None,
                    seo_title: None,
                    seo_description: None,
                    publication: Publication::Public {
                        publish_at: self.now,
                    },
                },
                SaveIntent::Explicit,
                self.now,
            )
            .await
            .unwrap();
        self.state.publish_now().await.unwrap();
        content.id
    }

    async fn request(&self, path: &str) -> axum::response::Response {
        self.request_with(path, None, None).await
    }

    async fn request_with(
        &self,
        path: &str,
        etag: Option<&str>,
        user_agent: Option<&str>,
    ) -> axum::response::Response {
        let mut builder = Request::builder()
            .uri(path)
            .header(header::HOST, "localhost:8080");
        if let Some(etag) = etag {
            builder = builder.header(header::IF_NONE_MATCH, etag);
        }
        if let Some(user_agent) = user_agent {
            builder = builder.header(header::USER_AGENT, user_agent);
        }
        router(self.state.clone())
            .oneshot(builder.body(Body::empty()).unwrap())
            .await
            .unwrap()
    }
}

async fn body(response: axum::response::Response) -> Vec<u8> {
    response
        .into_body()
        .collect()
        .await
        .unwrap()
        .to_bytes()
        .to_vec()
}

#[tokio::test]
async fn native_adapter_reports_an_unpublished_site_as_temporarily_unavailable() {
    let harness = Harness::new().await;

    let response = harness.request("/").await;

    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(response.headers()[header::RETRY_AFTER], "5");
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
}

#[tokio::test]
async fn native_adapter_preserves_release_status_headers_body_and_conditionals() {
    let harness = Harness::new().await;
    let content_id = harness.create_and_publish().await;

    let response = harness.request("/static-story/").await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()[header::CONTENT_TYPE],
        "text/html; charset=utf-8"
    );
    assert_eq!(
        response.headers()[header::CACHE_CONTROL],
        "public, max-age=0, must-revalidate"
    );
    assert_eq!(
        response.headers()[header::LAST_MODIFIED],
        "Wed, 02 Sep 2026 12:34:56 GMT"
    );
    let etag = response.headers()[header::ETAG]
        .to_str()
        .unwrap()
        .to_owned();
    assert!(etag.starts_with("\"blake3-"));
    assert_eq!(etag.len(), 73);
    let release_id = response.headers()["x-simple-blog-release"]
        .to_str()
        .unwrap();
    assert_eq!(release_id.len(), 64);
    assert!(
        String::from_utf8(body(response).await)
            .unwrap()
            .contains("Durable output")
    );

    let not_modified = harness
        .request_with("/static-story/", Some(&etag), None)
        .await;
    assert_eq!(not_modified.status(), StatusCode::NOT_MODIFIED);
    assert_eq!(not_modified.headers()[header::ETAG], etag);
    assert!(body(not_modified).await.is_empty());

    let totals = harness.repository.engagement_totals().await.unwrap();
    assert_eq!(totals[&content_id].views, 2);
}

#[tokio::test]
async fn native_adapter_serves_manifest_redirects_assets_and_release_owned_404() {
    let harness = Harness::new().await;
    harness.create_and_publish().await;

    let redirect = harness.request("/static-story").await;
    assert_eq!(redirect.status(), StatusCode::PERMANENT_REDIRECT);
    assert_eq!(redirect.headers()[header::LOCATION], "/static-story/");
    assert_eq!(redirect.headers()["x-simple-blog-release"].len(), 64);

    let script = harness.request("/assets/search.js").await;
    assert_eq!(script.status(), StatusCode::OK);
    assert_eq!(
        script.headers()[header::CACHE_CONTROL],
        "public, max-age=31536000, immutable"
    );

    let missing = harness.request("/not/a/real/route").await;
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);
    assert!(
        String::from_utf8(body(missing).await)
            .unwrap()
            .contains("Page not found")
    );
}

#[tokio::test]
async fn native_adapter_fails_closed_with_a_diagnostic_response_on_corruption() {
    let harness = Harness::new().await;
    harness.create_and_publish().await;
    let store = FilesystemReleaseStore::new(harness.config.release_dir());
    let active = store.active().await.unwrap().unwrap();
    let manifest = store.manifest(&active.id).await.unwrap();
    let object_id = manifest.routes["/static-story/"].object_id().unwrap();
    std::fs::write(store.root().join("objects").join(object_id), b"corrupt").unwrap();

    let response = harness.request("/static-story/").await;

    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(body(response).await, b"Internal Server Error");
}
