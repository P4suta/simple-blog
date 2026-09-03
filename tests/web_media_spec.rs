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
        ports::{ContentRepository as _, MediaRepository as _, SiteRepository},
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
    repository: Arc<SqliteRepository>,
    config: Config,
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
        state: AppState::new(config.clone(), repository.clone()).unwrap(),
        asset,
        repository,
        config,
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
        "rel=\"icon\" href=\"&#x2f;media&#x2f;{}\"",
        asset.original_filename
    )));
    assert!(home.contains(&format!(
        "class=\"site-logo\" src=\"&#x2f;media&#x2f;{}\"",
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

struct GcHarness {
    _temp: tempfile::TempDir,
    config: Config,
    repository: Arc<SqliteRepository>,
    media: LocalMediaService,
    state: AppState,
    cookie: String,
    csrf: String,
}

/// A site whose logo is one asset and whose single draft references another
/// inline, plus an authenticated session for the editor routes.
async fn gc_harness() -> (
    GcHarness,
    MediaAsset,
    MediaAsset,
    simple_blog::domain::content::Content,
) {
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
    let state = AppState::new(config.clone(), repository.clone()).unwrap();
    (
        GcHarness {
            _temp: temp,
            config,
            repository,
            media,
            state,
            cookie,
            csrf: session.csrf.expose().to_owned(),
        },
        body_asset,
        logo_asset,
        content,
    )
}

impl GcHarness {
    /// Saves the piece with a body that no longer references any image.
    async fn save_without_image(&self, id: i64, version: i64, intent: &str) -> StatusCode {
        let form = serde_urlencoded::to_string([
            ("csrf", self.csrf.as_str()),
            ("kind", "post"),
            ("title", "Referencing"),
            ("slug", "referencing"),
            ("body_markdown", "no more image"),
            ("status", "draft"),
            ("intent", intent),
            ("version", &version.to_string()),
        ])
        .unwrap();
        router(self.state.clone())
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(format!("/admin/content/{id}/"))
                    .header(header::HOST, "localhost:8080")
                    .header(header::COOKIE, &self.cookie)
                    .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                    .body(Body::from(form))
                    .unwrap(),
            )
            .await
            .unwrap()
            .status()
    }

    async fn post(&self, path: &str, fields: &[(&str, &str)]) -> axum::response::Response {
        let mut form: Vec<(&str, &str)> = vec![("csrf", self.csrf.as_str())];
        form.extend_from_slice(fields);
        router(self.state.clone())
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(path)
                    .header(header::HOST, "localhost:8080")
                    .header(header::COOKIE, &self.cookie)
                    .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                    .body(Body::from(serde_urlencoded::to_string(form).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap()
    }

    async fn asset_exists(&self, asset: &MediaAsset) -> bool {
        let row = self
            .repository
            .find_media(&asset.id)
            .await
            .unwrap()
            .is_some();
        let files = std::iter::once(&asset.original_filename)
            .chain(asset.variants.iter().map(|variant| &variant.filename))
            .all(|name| self.config.media_dir().join(name).exists());
        assert_eq!(row, files, "row and files must agree for {}", asset.id);
        row
    }
}

#[tokio::test]
async fn an_image_referenced_only_by_a_revision_survives_gc_after_an_explicit_save() {
    let (harness, body_asset, logo_asset, content) = gc_harness().await;

    // The explicit save drops the only live reference, yet the revision that
    // still shows the image keeps it alive: nothing written is lost.
    let status = harness
        .save_without_image(content.id.as_i64(), content.version, "explicit")
        .await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert!(harness.asset_exists(&body_asset).await);
    assert!(harness.asset_exists(&logo_asset).await);

    // Deleting the piece permanently cascades its revisions, and the very
    // next sweep removes the now unreferenced image while the logo stays.
    let current = harness
        .repository
        .find_by_id(content.id)
        .await
        .unwrap()
        .unwrap();
    let trashed = harness
        .post(
            &format!("/admin/content/{}/trash/", content.id),
            &[("version", &current.version.to_string())],
        )
        .await;
    assert_eq!(trashed.status(), StatusCode::SEE_OTHER);
    assert!(harness.asset_exists(&body_asset).await);
    let deleted = harness
        .post(&format!("/admin/content/{}/delete/", content.id), &[])
        .await;
    assert_eq!(deleted.status(), StatusCode::SEE_OTHER);
    assert!(!harness.asset_exists(&body_asset).await);
    assert!(harness.asset_exists(&logo_asset).await);
    let gone = get(
        &harness.state,
        &format!("/media/{}", body_asset.original_filename),
    )
    .await;
    assert_eq!(gone.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn autosaves_do_not_run_media_garbage_collection() {
    let (harness, _body_asset, _logo_asset, content) = gc_harness().await;
    // Distinct bytes, or content addressing would merge it with the inline
    // image that history still references.
    let orphan_image =
        DynamicImage::ImageRgb8(ImageBuffer::from_pixel(200, 100, Rgb([20, 60, 200])));
    let mut cursor = Cursor::new(Vec::new());
    orphan_image
        .write_to(&mut cursor, ImageFormat::Png)
        .unwrap();
    let orphan = harness
        .media
        .store("orphan.png", cursor.into_inner(), "Orphan", "", Utc::now())
        .await
        .unwrap();

    // Typing pauses save every second or so; sweeping the media directory on
    // each of them would be wasteful and would delete a half-inserted image.
    let status = harness
        .save_without_image(content.id.as_i64(), content.version, "autosave")
        .await;
    assert_eq!(status, StatusCode::OK);
    assert!(harness.asset_exists(&orphan).await);

    // An explicit save is a deliberate moment, and the orphan has no
    // reference anywhere, not even in history.
    let current = harness
        .repository
        .find_by_id(content.id)
        .await
        .unwrap()
        .unwrap();
    let status = harness
        .save_without_image(content.id.as_i64(), current.version, "explicit")
        .await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert!(!harness.asset_exists(&orphan).await);
}

#[tokio::test]
async fn variant_filenames_are_served_as_webp_and_unknown_names_are_not_served() {
    let harness = cover_harness().await;
    let variant = harness.asset.variants.first().expect("responsive variants");

    let served = get(&harness.state, &format!("/media/{}", variant.filename)).await;
    assert_eq!(served.status(), StatusCode::OK);
    assert_eq!(served.headers()[header::CONTENT_TYPE], "image/webp");
    assert_eq!(
        served.headers()[header::CACHE_CONTROL],
        "public, max-age=31536000, immutable"
    );

    let original = get(
        &harness.state,
        &format!("/media/{}", harness.asset.original_filename),
    )
    .await;
    assert_eq!(original.status(), StatusCode::OK);
    assert_eq!(
        original.headers()[header::CONTENT_TYPE],
        harness.asset.mime_type.as_str()
    );

    // The same digest under a different extension is not a registered file.
    let renamed = get(&harness.state, &format!("/media/{}.gif", harness.asset.id)).await;
    assert_eq!(renamed.status(), StatusCode::NOT_FOUND);
    let unknown = get(&harness.state, &format!("/media/{}.png", "0".repeat(64))).await;
    assert_eq!(unknown.status(), StatusCode::NOT_FOUND);
}
#[tokio::test]
async fn trashed_content_keeps_its_media_until_it_is_deleted_permanently() {
    use simple_blog::application::ports::ContentRepository;

    let harness = cover_harness().await;
    let repository = harness.repository.clone();
    let content = repository
        .list_all_content()
        .await
        .unwrap()
        .into_iter()
        .find(|content| content.slug.as_str() == "with-cover")
        .expect("cover content");
    let media_path = harness
        .config
        .media_dir()
        .join(&harness.asset.original_filename);

    // Only the piece references the asset from now on.
    let site = SiteService::new(repository.clone());
    let mut settings = repository.site_settings().await.unwrap();
    settings.logo_media_id = None;
    settings.favicon_media_id = None;
    site.update(settings, Vec::new(), Utc::now()).await.unwrap();

    let auth = AuthService::new(repository.clone(), Arc::new(SystemEntropy));
    let session = auth.create_session(Utc::now()).await.unwrap();
    let cookie = format!(
        "sb_session={}; sb_csrf={}",
        session.session.expose(),
        session.csrf.expose()
    );
    let post = |path: String, body: String| {
        let state = harness.state.clone();
        let cookie = cookie.clone();
        async move {
            router(state)
                .oneshot(
                    Request::builder()
                        .method(Method::POST)
                        .uri(path)
                        .header(header::HOST, "localhost:8080")
                        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                        .header(header::COOKIE, cookie)
                        .body(Body::from(body))
                        .unwrap(),
                )
                .await
                .unwrap()
        }
    };

    let trashed = post(
        format!("/admin/content/{}/trash/", content.id),
        format!("csrf={}&version={}", session.csrf.expose(), content.version),
    )
    .await;
    assert_eq!(trashed.status(), StatusCode::SEE_OTHER);
    assert!(
        repository
            .find_media(&harness.asset.id)
            .await
            .unwrap()
            .is_some(),
        "a trashed piece still references its cover"
    );
    assert!(media_path.exists());

    let deleted = post(
        format!("/admin/content/{}/delete/", content.id),
        format!("csrf={}", session.csrf.expose()),
    )
    .await;
    assert_eq!(deleted.status(), StatusCode::SEE_OTHER);
    assert!(
        repository
            .find_media(&harness.asset.id)
            .await
            .unwrap()
            .is_none(),
        "permanent deletion releases the cover"
    );
    assert!(!media_path.exists());
}
#[tokio::test]
async fn cover_alt_text_can_be_edited_and_reaches_the_released_page() {
    use simple_blog::application::ports::PublicSnapshotRepository;

    let harness = cover_harness().await;
    let auth = AuthService::new(harness.repository.clone(), Arc::new(SystemEntropy));
    let session = auth.create_session(Utc::now()).await.unwrap();
    let cookie = format!(
        "sb_session={}; sb_csrf={}",
        session.session.expose(),
        session.csrf.expose()
    );
    let post = |path: String, body: String, accept: &'static str| {
        let state = harness.state.clone();
        let cookie = cookie.clone();
        async move {
            router(state)
                .oneshot(
                    Request::builder()
                        .method(Method::POST)
                        .uri(path)
                        .header(header::HOST, "localhost:8080")
                        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                        .header(header::ACCEPT, accept)
                        .header(header::COOKIE, cookie)
                        .body(Body::from(body))
                        .unwrap(),
                )
                .await
                .unwrap()
        }
    };
    let path = format!("/admin/media/{}/", harness.asset.id);
    let revision_before = harness
        .repository
        .publication_state()
        .await
        .unwrap()
        .revision;

    let forbidden = post(path.clone(), "csrf=wrong&alt_text=x".into(), "*/*").await;
    assert_eq!(forbidden.status(), StatusCode::FORBIDDEN);

    let unknown = post(
        format!("/admin/media/{}/", "f".repeat(64)),
        format!("csrf={}&alt_text=Sea", session.csrf.expose()),
        "*/*",
    )
    .await;
    assert_eq!(unknown.status(), StatusCode::NOT_FOUND);

    let too_long = post(
        path.clone(),
        format!(
            "csrf={}&alt_text={}",
            session.csrf.expose(),
            "a".repeat(501)
        ),
        "*/*",
    )
    .await;
    assert_eq!(too_long.status(), StatusCode::UNPROCESSABLE_ENTITY);

    let saved = post(
        path.clone(),
        format!("csrf={}&alt_text=Calm+blue+sea", session.csrf.expose()),
        "application/json",
    )
    .await;
    assert_eq!(saved.status(), StatusCode::OK);
    let json: serde_json::Value = serde_json::from_slice(&response_body(saved).await).unwrap();
    assert_eq!(json["alt_text"], "Calm blue sea");
    assert_eq!(
        harness
            .repository
            .publication_state()
            .await
            .unwrap()
            .revision,
        revision_before + 1,
        "alternative text is part of the released pages"
    );

    let page = get(&harness.state, "/with-cover/").await;
    assert_eq!(page.status(), StatusCode::OK);
    let html = String::from_utf8(response_body(page).await).unwrap();
    assert!(html.contains("alt=\"Calm blue sea\""), "{html}");

    let redirected = post(
        path,
        format!("csrf={}&alt_text=Still+sea", session.csrf.expose()),
        "text/html",
    )
    .await;
    assert_eq!(redirected.status(), StatusCode::SEE_OTHER);
    assert_eq!(redirected.headers()[header::LOCATION], "/admin/media/");
}

#[tokio::test]
async fn cover_alt_text_edit_answers_pending_when_publication_fails() {
    let harness = cover_harness().await;
    let releases = harness.config.release_dir();
    std::fs::create_dir_all(&releases).unwrap();
    std::fs::write(releases.join("objects"), b"not a directory").unwrap();
    let auth = AuthService::new(harness.repository.clone(), Arc::new(SystemEntropy));
    let session = auth.create_session(Utc::now()).await.unwrap();
    let cookie = format!(
        "sb_session={}; sb_csrf={}",
        session.session.expose(),
        session.csrf.expose()
    );
    let form = serde_urlencoded::to_string([
        ("csrf", session.csrf.expose()),
        ("alt_text", "Calm sea at dusk"),
    ])
    .unwrap();
    let response = router(harness.state.clone())
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!("/admin/media/{}/", harness.asset.id))
                .header(header::HOST, "localhost:8080")
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .header(header::ACCEPT, "application/json")
                .header(header::COOKIE, &cookie)
                .body(Body::from(form))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: serde_json::Value = serde_json::from_slice(&response_body(response).await).unwrap();
    assert_eq!(body["ok"], true);
    assert_eq!(body["site"], "pending");
    assert_eq!(body["alt_text"], "Calm sea at dusk");
}

async fn media_page(harness: &GcHarness) -> String {
    let response = router(harness.state.clone())
        .oneshot(
            Request::builder()
                .uri("/admin/media/")
                .header(header::HOST, "localhost:8080")
                .header(header::COOKIE, &harness.cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    String::from_utf8(response_body(response).await).unwrap()
}

#[tokio::test]
async fn media_library_lists_assets_with_usage_and_deletes_only_unreferenced_ones() {
    let (harness, body_asset, logo_asset, content) = gc_harness().await;

    let anonymous = router(harness.state.clone())
        .oneshot(
            Request::builder()
                .uri("/admin/media/")
                .header(header::HOST, "localhost:8080")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(anonymous.status(), StatusCode::SEE_OTHER);

    let page = media_page(&harness).await;
    // Newest first: the logo was stored after the inline image.
    assert!(
        page.find(logo_asset.id.as_str()).unwrap() < page.find(body_asset.id.as_str()).unwrap()
    );
    let smallest = body_asset.variants.first().expect("variants");
    // minijinja escapes the slashes inside attribute values.
    assert!(page.contains(&format!("src=\"&#x2f;media&#x2f;{}\"", smallest.filename)));
    assert!(page.contains("name=\"alt_text\" value=\"Inline\""));
    assert!(page.contains("Used by 1 piece"));
    assert!(page.contains("Used by the site settings"));
    assert!(page.contains(&format!(
        "data-copy-markdown=\"![Inline](&#x2f;media&#x2f;{})\"",
        body_asset.original_filename
    )));
    assert!(page.contains("data-msg-copied"));
    assert!(
        !page.contains(&format!("/admin/media/{}/delete/", body_asset.id)),
        "a referenced asset offers no delete"
    );

    // Dropping the only live reference leaves the image to history, which
    // the page says, and only then may it be deleted.
    let status = harness
        .save_without_image(content.id.as_i64(), content.version, "explicit")
        .await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    let page = media_page(&harness).await;
    assert!(page.contains("History only"));
    assert!(page.contains(&format!("/admin/media/{}/delete/", body_asset.id)));

    let deleted = harness
        .post(&format!("/admin/media/{}/delete/", body_asset.id), &[])
        .await;
    assert_eq!(deleted.status(), StatusCode::SEE_OTHER);
    assert_eq!(deleted.headers()[header::LOCATION], "/admin/media/");
    assert!(!harness.asset_exists(&body_asset).await);
    assert!(harness.asset_exists(&logo_asset).await);
    let gone = get(
        &harness.state,
        &format!("/media/{}", body_asset.original_filename),
    )
    .await;
    assert_eq!(gone.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn media_delete_route_refuses_assets_referenced_by_current_content() {
    let (harness, body_asset, logo_asset, _content) = gc_harness().await;
    let forbidden = router(harness.state.clone())
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!("/admin/media/{}/delete/", body_asset.id))
                .header(header::HOST, "localhost:8080")
                .header(header::COOKIE, &harness.cookie)
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(
                    serde_urlencoded::to_string([("csrf", "wrong")]).unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(forbidden.status(), StatusCode::FORBIDDEN);

    let in_body = harness
        .post(&format!("/admin/media/{}/delete/", body_asset.id), &[])
        .await;
    assert_eq!(in_body.status(), StatusCode::CONFLICT);
    assert!(
        String::from_utf8(response_body(in_body).await)
            .unwrap()
            .contains("still used")
    );
    let in_settings = harness
        .post(&format!("/admin/media/{}/delete/", logo_asset.id), &[])
        .await;
    assert_eq!(in_settings.status(), StatusCode::CONFLICT);
    let unknown = harness
        .post(&format!("/admin/media/{}/delete/", "f".repeat(64)), &[])
        .await;
    assert_eq!(unknown.status(), StatusCode::NOT_FOUND);
    assert!(harness.asset_exists(&body_asset).await);
    assert!(harness.asset_exists(&logo_asset).await);
}
