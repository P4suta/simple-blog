use std::sync::Arc;

use axum::{
    body::Body,
    http::{Method, Request, StatusCode, header},
};
use chrono::{Duration, Utc};
use http_body_util::BodyExt;
use simple_blog::{
    application::{
        content::{ContentService, SaveIntent},
        ports::LikeRepository,
    },
    config::{Config, ConfigSources, Overrides},
    domain::content::{Content, ContentDraft, ContentKind, Publication, Slug},
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

    async fn seed(&self, slug: &str, publication: Publication) -> Content {
        self.service
            .create(
                ContentDraft {
                    kind: ContentKind::Post,
                    title: format!("Post {slug}"),
                    slug: Slug::parse(slug).unwrap(),
                    summary: "Summary".into(),
                    body_markdown: "# Body".into(),
                    tags: Vec::new(),
                    cover_media_id: None,
                    seo_title: None,
                    seo_description: None,
                    publication,
                },
                SaveIntent::Explicit,
                Utc::now(),
            )
            .await
            .unwrap()
    }

    async fn get(&self, path: &str) -> axum::response::Response {
        self.state.publish_now().await.unwrap();
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

    async fn post(&self, path: &str, content_type: &str, body: &str) -> axum::response::Response {
        router(self.state.clone())
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(path)
                    .header(header::HOST, "localhost:8080")
                    .header(header::CONTENT_TYPE, content_type)
                    .body(Body::from(body.to_owned()))
                    .unwrap(),
            )
            .await
            .unwrap()
    }
}

#[tokio::test]
async fn likes_toggle_up_and_down_and_never_go_negative() {
    let harness = Harness::new().await;
    let publish_at = Utc::now() - Duration::seconds(1);
    let content = harness
        .seed("liked-post", Publication::Public { publish_at })
        .await;
    let path = format!("/likes/{}", content.id.as_i64());
    let count = || async {
        harness
            .repository
            .like_count(content.id, Utc::now())
            .await
            .unwrap()
    };

    // Totals are owner-facing only: the toggle answers 204 with no body and
    // the count is observable solely through the repository (the dashboard).
    let one = harness
        .post(&path, "application/json", r#"{"op":"like"}"#)
        .await;
    assert_eq!(one.status(), StatusCode::NO_CONTENT);
    assert_eq!(count().await, 1);
    // The server does not deduplicate; the client's localStorage does.
    harness
        .post(&path, "application/json", r#"{"op":"like"}"#)
        .await;
    assert_eq!(count().await, 2);

    harness
        .post(&path, "application/json", r#"{"op":"unlike"}"#)
        .await;
    assert_eq!(count().await, 1);
    harness
        .post(&path, "application/json", r#"{"op":"unlike"}"#)
        .await;
    harness
        .post(&path, "application/json", r#"{"op":"unlike"}"#)
        .await;
    assert_eq!(count().await, 0, "unliking at zero must not go negative");
}

#[tokio::test]
async fn invisible_content_cannot_be_probed_through_likes() {
    let harness = Harness::new().await;
    let hidden = harness.seed("secret-draft", Publication::Draft).await;
    let scheduled = harness
        .seed(
            "future-post",
            Publication::Public {
                publish_at: Utc::now() + Duration::hours(1),
            },
        )
        .await;

    for content in [&hidden, &scheduled] {
        let path = format!("/likes/{}", content.id.as_i64());
        let response = harness
            .post(&path, "application/json", r#"{"op":"like"}"#)
            .await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }
    let missing = harness
        .post("/likes/999999", "application/json", r#"{"op":"like"}"#)
        .await;
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn form_encoded_posts_are_rejected_as_a_csrf_guard() {
    let harness = Harness::new().await;
    let content = harness
        .seed(
            "guarded-post",
            Publication::Public {
                publish_at: Utc::now() - Duration::seconds(1),
            },
        )
        .await;
    let path = format!("/likes/{}", content.id.as_i64());

    let form = harness
        .post(&path, "application/x-www-form-urlencoded", "op=like")
        .await;
    assert_eq!(form.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);

    let bad_op = harness
        .post(&path, "application/json", r#"{"op":"boost"}"#)
        .await;
    assert_eq!(bad_op.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn article_pages_carry_the_like_button_and_home_stays_script_free() {
    let harness = Harness::new().await;
    let content = harness
        .seed(
            "essay",
            Publication::Public {
                publish_at: Utc::now() - Duration::seconds(1),
            },
        )
        .await;

    let page = harness.get("/essay/").await;
    assert_eq!(page.status(), StatusCode::OK);
    let html = String::from_utf8(
        page.into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes()
            .to_vec(),
    )
    .unwrap();
    assert!(html.contains(&format!("data-content-id=\"{}\"", content.id.as_i64())));
    assert!(html.contains("/assets/like.js?v="));

    let script = harness.get("/assets/like.js").await;
    assert_eq!(script.status(), StatusCode::OK);
    assert!(
        script.headers()[header::CACHE_CONTROL]
            .to_str()
            .unwrap()
            .contains("immutable")
    );

    // The home page carries no inline scripts and no like machinery.
    let home = harness.get("/").await;
    let home = String::from_utf8(
        home.into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes()
            .to_vec(),
    )
    .unwrap();
    assert!(!home.contains("<script>"));
    assert!(!home.contains("like.js"));
}
