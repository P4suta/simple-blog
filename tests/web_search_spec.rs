//! End-to-end search over the public site, exercised the way Japanese (and
//! mixed-script) queries actually arrive.

use std::sync::Arc;

use axum::{
    body::Body,
    http::{Request, StatusCode, header},
};
use chrono::{Duration, Utc};
use http_body_util::BodyExt;
use simple_blog::{
    application::{
        content::{ContentService, SaveIntent},
        ports::{ContentRepository, SearchRepository},
        static_search::StaticSearchIndexV1,
    },
    config::{Config, ConfigSources, Overrides},
    domain::{
        content::{ContentDraft, ContentKind, Publication, Slug},
        search::parse_query,
    },
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

    async fn seed(&self, title: &str, slug: &str, body: &str, publication: Publication) {
        self.service
            .create(
                ContentDraft {
                    kind: ContentKind::Post,
                    title: title.into(),
                    slug: Slug::parse(slug).unwrap(),
                    summary: String::new(),
                    body_markdown: body.into(),
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
            .unwrap();
    }

    async fn search(&self, query: &str) -> Vec<String> {
        self.state.publish_now().await.unwrap();
        let response = router(self.state.clone())
            .oneshot(
                Request::builder()
                    .uri("/assets/search-index.json")
                    .header(header::HOST, "localhost:8080")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        let index = StaticSearchIndexV1::from_bytes(&bytes).unwrap();
        index
            .search(query, 50)
            .into_iter()
            .map(|document| document.slug.clone())
            .collect()
    }

    async fn search_page(&self, query: &str) -> String {
        self.state.publish_now().await.unwrap();
        let encoded: String = url::form_urlencoded::byte_serialize(query.as_bytes()).collect();
        let response = router(self.state.clone())
            .oneshot(
                Request::builder()
                    .uri(format!("/search/?q={encoded}"))
                    .header(header::HOST, "localhost:8080")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
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
}

fn public_now() -> Publication {
    Publication::Public {
        publish_at: Utc::now() - Duration::seconds(1),
    }
}

#[tokio::test]
async fn two_character_kanji_queries_find_content() {
    let harness = Harness::new().await;
    harness
        .seed(
            "旅の記録",
            "tokyo-trip",
            "# 東京の休日\n\n朝から都心を歩いた。",
            public_now(),
        )
        .await;
    harness
        .seed("別の話", "other", "京都のカフェの話。", public_now())
        .await;

    // 「東京」is two characters — the class of query a trigram-only index
    // silently fails on. The LIKE path must carry it.
    assert_eq!(harness.search("東京").await, ["tokyo-trip"]);
}

#[tokio::test]
async fn katakana_and_hiragana_queries_meet_through_folding() {
    let harness = Harness::new().await;
    harness
        .seed(
            "サーバー再構築",
            "server-rebuild",
            "自宅サーバーを立て直した。",
            public_now(),
        )
        .await;

    // Hiragana input, katakana content.
    assert_eq!(harness.search("さーばー").await, ["server-rebuild"]);
    // And the reverse spelling still works.
    assert_eq!(harness.search("サーバー").await, ["server-rebuild"]);
}

#[tokio::test]
async fn width_variants_and_ascii_case_are_transparent() {
    let harness = Harness::new().await;
    harness
        .seed(
            "Rust移行メモ",
            "rust-migration",
            "RustでCMSを書いた。",
            public_now(),
        )
        .await;

    // Full-width input from a Japanese IME.
    assert_eq!(harness.search("Ｒｕｓｔ").await, ["rust-migration"]);
    // Case-insensitive ASCII.
    assert_eq!(harness.search("RUST").await, ["rust-migration"]);
}

#[tokio::test]
async fn mixed_script_terms_combine_as_and() {
    let harness = Harness::new().await;
    harness
        .seed(
            "移行の記録",
            "with-both",
            "東京の会社でRustを書いている。",
            public_now(),
        )
        .await;
    harness
        .seed("東京だけ", "tokyo-only", "東京の話。", public_now())
        .await;

    // A 2-char CJK term (LIKE) and a 4-char Latin term (FTS) must both hold.
    assert_eq!(harness.search("東京 rust").await, ["with-both"]);
}

#[tokio::test]
async fn drafts_and_scheduled_posts_never_surface() {
    let harness = Harness::new().await;
    harness
        .seed(
            "秘密の下書き",
            "secret",
            "非公開の東京メモ。",
            Publication::Draft,
        )
        .await;
    harness
        .seed(
            "予約済み",
            "scheduled",
            "未来の東京の話。",
            Publication::Public {
                publish_at: Utc::now() + Duration::hours(1),
            },
        )
        .await;

    assert!(harness.search("東京").await.is_empty());
}

#[tokio::test]
async fn title_matches_rank_above_body_matches() {
    let harness = Harness::new().await;
    harness
        .seed(
            "本文だけの記録",
            "body-hit",
            "検索エンジンを自作した長い記録。",
            Publication::Public {
                publish_at: Utc::now() - Duration::seconds(1),
            },
        )
        .await;
    harness
        .seed(
            "検索エンジン自作記",
            "title-hit",
            "まとめ。",
            Publication::Public {
                publish_at: Utc::now() - Duration::hours(1),
            },
        )
        .await;

    // The title-hit post is older, so recency alone would rank it second;
    // bm25 title weighting must pull it first.
    assert_eq!(
        harness.search("検索エンジン").await,
        ["title-hit", "body-hit"]
    );
}

#[tokio::test]
async fn edits_reindex_and_search_follows_the_current_text() {
    let harness = Harness::new().await;
    harness
        .seed("初稿", "evolving", "最初は紅茶の話だった。", public_now())
        .await;
    let content = harness
        .repository
        .find_public_by_slug(&Slug::parse("evolving").unwrap(), Utc::now())
        .await
        .unwrap()
        .unwrap();
    harness
        .service
        .update(
            content.id,
            content.version,
            ContentDraft {
                body_markdown: "いまは珈琲の話。".into(),
                ..content.to_draft()
            },
            SaveIntent::Explicit,
            Utc::now(),
        )
        .await
        .unwrap();

    assert_eq!(harness.search("珈琲").await, ["evolving"]);
    assert!(harness.search("紅茶").await.is_empty());
}

#[tokio::test]
async fn static_search_page_is_query_independent_and_client_semantics_are_tested_separately() {
    let harness = Harness::new().await;
    harness
        .seed("普通の記事", "normal", "内容。", public_now())
        .await;

    // Empty query: the form renders, nothing is claimed to be "not found".
    let html = harness.search_page("").await;
    assert!(html.contains("search-form"));
    assert!(html.contains("data-static-search"));
    assert!(html.contains("data-search-results"));
    assert!(html.contains("data-search-results"));
    assert!(html.contains("data-search-results aria-label=\"Search\" hidden"));
    assert!(html.contains("data-index=\"&#x2f;assets&#x2f;search-index.json?v="));

    // The release resolver intentionally discards the query string. Hostile
    // query handling is exercised by frontend/search.test.cjs, which executes
    // the client search implementation rather than this immutable HTML asset.
    for hostile in ["\"OR\" (", "100%", "_", "a* NOT b", "<script>"] {
        assert_eq!(
            harness.search_page(hostile).await,
            html,
            "query {hostile:?}"
        );
    }
}

#[tokio::test]
async fn sqlite_search_clamps_untrusted_repository_limits() {
    let harness = Harness::new().await;
    for id in 1..=101 {
        harness
            .seed(
                &format!("Needle {id}"),
                &format!("needle-{id}"),
                "needle",
                public_now(),
            )
            .await;
    }

    let hits = harness
        .repository
        .search(&parse_query("needle"), Utc::now(), u32::MAX)
        .await
        .unwrap();

    assert_eq!(hits.len(), 100);
}
