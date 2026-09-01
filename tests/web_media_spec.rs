use std::{io::Cursor, sync::Arc};

use axum::{
    body::Body,
    http::{Method, Request, StatusCode, header},
};
use chrono::Utc;
use http_body_util::BodyExt;
use image::{DynamicImage, ImageBuffer, ImageFormat, Rgb};
use simple_blog::{
    application::{
        auth::AuthService,
        content::{ContentService, SaveIntent},
        ports::{MediaRepository as _, SiteRepository},
        site::SiteService,
    },
    config::{Config, ConfigSources, Overrides},
    domain::{
        content::{ContentDraft, ContentKind, Publication, Slug},
        media::MediaAsset,
    },
    infrastructure::{
        entropy::SystemEntropy, markdown::ComrakMarkdownRenderer, media::LocalMediaService,
        sqlite::SqliteRepository,
    },
    web::{AppState, router},
};
use tower::ServiceExt;

fn png() -> Vec<u8> {
    let image = DynamicImage::ImageRgb8(ImageBuffer::from_pixel(640, 360, Rgb([30, 90, 180])));
    let mut cursor = Cursor::new(Vec::new());
    image.write_to(&mut cursor, ImageFormat::Png).unwrap();
    cursor.into_inner()
}

fn png_larger_than_axum_default_limit() -> Vec<u8> {
    let mut state = 0x9e37_79b9_u32;
    let image = ImageBuffer::from_fn(900, 900, |_, _| {
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        Rgb(state.to_le_bytes()[..3].try_into().unwrap())
    });
    let mut cursor = Cursor::new(Vec::new());
    DynamicImage::ImageRgb8(image)
        .write_to(&mut cursor, ImageFormat::Png)
        .unwrap();
    let bytes = cursor.into_inner();
    assert!(bytes.len() > 2 * 1024 * 1024);
    bytes
}

async fn response_body(response: axum::response::Response) -> Vec<u8> {
    response
        .into_body()
        .collect()
        .await
        .unwrap()
        .to_bytes()
        .to_vec()
}

struct CoverHarness {
    _temp: tempfile::TempDir,
    state: AppState,
    asset: MediaAsset,
}

async fn cover_harness() -> CoverHarness {
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
    let media = LocalMediaService::new(
        config.media_dir(),
        repository.clone(),
        config.max_upload_bytes,
    );
    let asset = media
        .store("cover.png", png(), "Blue & calm", "", Utc::now())
        .await
        .unwrap();
    let site = SiteService::new(repository.clone());
    let mut settings = repository.site_settings().await.unwrap();
    settings.logo_media_id = Some(asset.id.to_string());
    settings.favicon_media_id = Some(asset.id.to_string());
    site.update(settings, Vec::new(), Utc::now()).await.unwrap();
    ContentService::new(
        repository.clone(),
        Arc::new(ComrakMarkdownRenderer::default()),
    )
    .create(
        ContentDraft {
            kind: ContentKind::Post,
            title: "With cover".into(),
            slug: Slug::parse("with-cover").unwrap(),
            summary: "summary".into(),
            body_markdown: "body".into(),
            tags: vec![],
            cover_media_id: Some(asset.id.to_string()),
            seo_title: None,
            seo_description: None,
            publication: Publication::Public {
                publish_at: Utc::now(),
            },
        },
        SaveIntent::Explicit,
        Utc::now(),
    )
    .await
    .unwrap();
    CoverHarness {
        _temp: temp,
        state: AppState::new(config, repository).unwrap(),
        asset,
    }
}

async fn get(state: &AppState, path: &str) -> axum::response::Response {
    state.publish_now().await.unwrap();
    router(state.clone())
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

#[tokio::test]
async fn registered_media_is_served_immutably_and_cover_uses_srcset() {
    let harness = cover_harness().await;
    let asset = &harness.asset;
    let page = get(&harness.state, "/with-cover/").await;
    let html = String::from_utf8(response_body(page).await).unwrap();
    assert!(html.contains("<picture"));
    assert!(html.contains("srcset="));
    assert!(html.contains("Blue &amp; calm"));
    assert!(html.contains(&asset.variants[0].filename));

    let home = get(&harness.state, "/").await;
    let home = String::from_utf8(response_body(home).await).unwrap();
    assert!(home.contains(&format!(
        "rel=\"icon\" href=\"/media/{}\"",
        asset.original_filename
    )));
    assert!(home.contains(&format!(
        "class=\"site-logo\" src=\"/media/{}\"",
        asset.original_filename
    )));

    let media_path = format!("/media/{}", asset.original_filename);
    let media_response = get(&harness.state, &media_path).await;
    assert_eq!(media_response.status(), StatusCode::OK);
    assert_eq!(media_response.headers()[header::CONTENT_TYPE], "image/webp");
    assert!(
        media_response.headers()[header::CACHE_CONTROL]
            .to_str()
            .unwrap()
            .contains("immutable")
    );

    let unknown = get(&harness.state, "/media/config.toml").await;
    assert_eq!(unknown.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn multipart_upload_requires_session_and_csrf_then_returns_media_json() {
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
    let auth = AuthService::new(repository.clone(), Arc::new(SystemEntropy));
    let session = auth.create_session(Utc::now()).await.unwrap();
    let cookie = format!(
        "sb_session={}; sb_csrf={}",
        session.session.expose(),
        session.csrf.expose()
    );
    let boundary = "simple-blog-test-boundary";
    let image = png_larger_than_axum_default_limit();
    let mut body = Vec::new();
    body.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"csrf\"\r\n\r\n{}\r\n",
            session.csrf.expose()
        )
        .as_bytes(),
    );
    body.extend_from_slice(format!("--{boundary}\r\nContent-Disposition: form-data; name=\"alt_text\"\r\n\r\nUploaded cover\r\n").as_bytes());
    body.extend_from_slice(format!("--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"cover.png\"\r\nContent-Type: image/png\r\n\r\n").as_bytes());
    body.extend_from_slice(&image);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    let state = AppState::new(config, repository).unwrap();

    let unauthenticated = router(state.clone())
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/admin/media/")
                .header(header::HOST, "localhost:8080")
                .header(
                    header::CONTENT_TYPE,
                    format!("multipart/form-data; boundary={boundary}"),
                )
                .body(Body::from(body.clone()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(unauthenticated.status(), StatusCode::SEE_OTHER);

    let uploaded = router(state)
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/admin/media/")
                .header(header::HOST, "localhost:8080")
                .header(header::COOKIE, cookie)
                .header(
                    header::CONTENT_TYPE,
                    format!("multipart/form-data; boundary={boundary}"),
                )
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(uploaded.status(), StatusCode::CREATED);
    let json: serde_json::Value = serde_json::from_slice(&response_body(uploaded).await).unwrap();
    assert_eq!(json["alt_text"], "Uploaded cover");
    assert!(json["url"].as_str().unwrap().starts_with("/media/"));
}

#[tokio::test]
async fn transport_rejects_a_body_beyond_the_configured_upload_envelope() {
    let temp = tempfile::tempdir().unwrap();
    let config = Config::resolve(ConfigSources {
        cli: Overrides {
            data_dir: Some(temp.path().to_path_buf()),
            public_url: Some("http://localhost:8080".into()),
            max_upload_bytes: Some(1024),
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
    let state = AppState::new(config, repository).unwrap();
    let oversized = vec![0_u8; 128 * 1024];

    let response = router(state)
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/healthz")
                .header(header::HOST, "localhost:8080")
                .header(header::CONTENT_LENGTH, oversized.len())
                .body(Body::from(oversized))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(
        response.headers()[header::X_CONTENT_TYPE_OPTIONS],
        "nosniff"
    );
}

#[tokio::test]
#[expect(
    clippy::too_many_lines,
    reason = "walks one story end-to-end: two uploads, a reference edit, and both outcomes"
)]
async fn media_loses_its_last_reference_and_is_deleted_immediately() {
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
    let media = LocalMediaService::new(
        config.media_dir(),
        repository.clone(),
        config.max_upload_bytes,
    );
    let body_asset = media
        .store("inline.png", png(), "Inline", "", Utc::now())
        .await
        .unwrap();
    let logo_image = DynamicImage::ImageRgb8(ImageBuffer::from_pixel(320, 180, Rgb([200, 40, 40])));
    let mut cursor = Cursor::new(Vec::new());
    logo_image.write_to(&mut cursor, ImageFormat::Png).unwrap();
    let logo_asset = media
        .store("logo.png", cursor.into_inner(), "Logo", "", Utc::now())
        .await
        .unwrap();

    let site = SiteService::new(repository.clone());
    let mut settings = repository.site_settings().await.unwrap();
    settings.logo_media_id = Some(logo_asset.id.to_string());
    site.update(settings, Vec::new(), Utc::now()).await.unwrap();

    let content = ContentService::new(
        repository.clone(),
        Arc::new(ComrakMarkdownRenderer::default()),
    )
    .create(
        ContentDraft {
            kind: ContentKind::Post,
            title: "Referencing".into(),
            slug: Slug::parse("referencing").unwrap(),
            summary: String::new(),
            body_markdown: format!("![Inline](/media/{})", body_asset.original_filename),
            tags: vec![],
            cover_media_id: None,
            seo_title: None,
            seo_description: None,
            publication: Publication::Draft,
        },
        SaveIntent::Explicit,
        Utc::now(),
    )
    .await
    .unwrap();

    let auth = AuthService::new(repository.clone(), Arc::new(SystemEntropy));
    let session = auth.create_session(Utc::now()).await.unwrap();
    let cookie = format!(
        "sb_session={}; sb_csrf={}",
        session.session.expose(),
        session.csrf.expose()
    );
    let form = serde_urlencoded::to_string([
        ("csrf", session.csrf.expose()),
        ("kind", "post"),
        ("title", "Referencing"),
        ("slug", "referencing"),
        ("body_markdown", "no more image"),
        ("status", "draft"),
        ("intent", "autosave"),
        ("version", &content.version.to_string()),
    ])
    .unwrap();
    let state = AppState::new(config.clone(), repository.clone()).unwrap();
    let saved = router(state.clone())
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!("/admin/content/{}/", content.id.as_i64()))
                .header(header::HOST, "localhost:8080")
                .header(header::COOKIE, &cookie)
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(form))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(saved.status(), StatusCode::OK);

    assert!(
        repository
            .find_media(&body_asset.id)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        !config
            .media_dir()
            .join(&body_asset.original_filename)
            .exists()
    );
    for variant in &body_asset.variants {
        assert!(!config.media_dir().join(&variant.filename).exists());
    }
    let gone = get(&state, &format!("/media/{}", body_asset.original_filename)).await;
    assert_eq!(gone.status(), StatusCode::NOT_FOUND);

    // The logo keeps its settings reference and survives the sweep.
    assert!(
        repository
            .find_media(&logo_asset.id)
            .await
            .unwrap()
            .is_some()
    );
    assert!(
        config
            .media_dir()
            .join(&logo_asset.original_filename)
            .exists()
    );
}
