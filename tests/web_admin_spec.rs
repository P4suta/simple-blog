use std::sync::Arc;

use axum::{
    body::Body,
    http::{Method, Request, StatusCode, header},
};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{Duration, Utc};
use http_body_util::BodyExt;
use simple_blog::{
    application::{
        auth::AuthService,
        auth::PasskeyAccountService,
        content::{ContentService, SaveIntent},
        ports::ContentRepository,
    },
    config::{Config, ConfigSources, Overrides},
    domain::{
        auth::{SetupPurpose, StoredPasskey},
        content::{ContentDraft, ContentId, ContentKind, Publication, Slug},
    },
    infrastructure::{
        entropy::SystemEntropy, markdown::ComrakMarkdownRenderer, sqlite::SqliteRepository,
    },
    web::{AppState, router},
};
use tower::ServiceExt;

struct Harness {
    _temp: tempfile::TempDir,
    repository: Arc<SqliteRepository>,
    contents: ContentService,
    auth: AuthService,
    accounts: PasskeyAccountService,
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
        let contents = ContentService::new(
            repository.clone(),
            Arc::new(ComrakMarkdownRenderer::default()),
        );
        let entropy = Arc::new(SystemEntropy);
        let auth = AuthService::new(repository.clone(), entropy.clone());
        let accounts = PasskeyAccountService::new(repository.clone(), entropy);
        let state = AppState::new(config, repository.clone()).unwrap();
        Self {
            _temp: temp,
            repository,
            contents,
            auth,
            accounts,
            state,
        }
    }

    async fn send(
        &self,
        method: Method,
        path: &str,
        content_type: Option<&str>,
        body: impl Into<Body>,
        cookie: Option<&str>,
    ) -> axum::response::Response {
        let mut builder = Request::builder()
            .method(method)
            .uri(path)
            .header(header::HOST, "localhost:8080");
        if let Some(content_type) = content_type {
            builder = builder.header(header::CONTENT_TYPE, content_type);
        }
        if let Some(cookie) = cookie {
            builder = builder.header(header::COOKIE, cookie);
        }
        router(self.state.clone())
            .oneshot(builder.body(body.into()).unwrap())
            .await
            .unwrap()
    }

    async fn session_cookie(&self) -> (String, String) {
        let session = self.auth.create_session(Utc::now()).await.unwrap();
        (
            format!(
                "sb_session={}; sb_csrf={}",
                session.session.expose(),
                session.csrf.expose()
            ),
            session.csrf.expose().to_owned(),
        )
    }
}

impl Harness {
    async fn registered_owner(&self) -> (String, String) {
        let now = Utc::now();
        let token = self
            .auth
            .issue_setup_token(SetupPurpose::Initial, now)
            .await
            .unwrap();
        let completed = self
            .accounts
            .complete_setup_registration(
                token.expose(),
                SetupPurpose::Initial,
                uuid::Uuid::new_v4(),
                StoredPasskey {
                    credential_id: vec![9, 8, 7],
                    name: "First key".into(),
                    passkey_json: "{}".into(),
                },
                now,
            )
            .await
            .unwrap()
            .unwrap();
        (
            format!(
                "sb_session={}; sb_csrf={}",
                completed.session.session.expose(),
                completed.session.csrf.expose()
            ),
            completed.session.csrf.expose().to_owned(),
        )
    }
}

async fn text(response: axum::response::Response) -> String {
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

fn draft(slug: &str) -> ContentDraft {
    ContentDraft {
        kind: ContentKind::Post,
        title: "Original title".into(),
        slug: Slug::parse(slug).unwrap(),
        summary: "summary".into(),
        body_markdown: "body".into(),
        tags: vec![],
        cover_media_id: None,
        seo_title: None,
        seo_description: None,
        publication: Publication::Draft,
    }
}

fn form(csrf: &str, title: &str, slug: &str, version: Option<i64>, intent: &str) -> String {
    let mut fields = vec![
        ("csrf", csrf.to_owned()),
        ("kind", "post".into()),
        ("title", title.into()),
        ("slug", slug.into()),
        ("summary", "summary".into()),
        ("body_markdown", "# body".into()),
        ("tags", "Rust, CMS".into()),
        ("status", "draft".into()),
        ("publish_at", String::new()),
        ("seo_title", String::new()),
        ("seo_description", String::new()),
        ("intent", intent.into()),
    ];
    if let Some(version) = version {
        fields.push(("version", version.to_string()));
    }
    serde_urlencoded::to_string(fields).unwrap()
}

#[tokio::test]
async fn admin_pages_require_an_opaque_session() {
    let harness = Harness::new().await;
    let response = harness
        .send(Method::GET, "/admin/", None, Body::empty(), None)
        .await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(response.headers()[header::LOCATION], "/admin/login/");

    let (cookie, csrf) = harness.session_cookie().await;
    let response = harness
        .send(
            Method::GET,
            "/admin/content/new/",
            None,
            Body::empty(),
            Some(&cookie),
        )
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = text(response).await;
    assert!(body.contains("<form"));
    assert!(body.contains(&format!("value=\"{csrf}\"")));
    assert!(body.contains("name=\"body_markdown\""));
}

#[tokio::test]
async fn normal_html_form_can_create_content_but_csrf_is_mandatory() {
    let harness = Harness::new().await;
    let (cookie, csrf) = harness.session_cookie().await;
    let missing = harness
        .send(
            Method::POST,
            "/admin/content/",
            Some("application/x-www-form-urlencoded"),
            form("wrong", "Rejected", "rejected", None, "explicit"),
            Some(&cookie),
        )
        .await;
    assert_eq!(missing.status(), StatusCode::FORBIDDEN);

    let created = harness
        .send(
            Method::POST,
            "/admin/content/",
            Some("application/x-www-form-urlencoded"),
            form(
                &csrf,
                "Created without JavaScript",
                "created",
                None,
                "explicit",
            ),
            Some(&cookie),
        )
        .await;
    assert_eq!(created.status(), StatusCode::SEE_OTHER);
    assert!(
        created.headers()[header::LOCATION]
            .to_str()
            .unwrap()
            .contains("/edit/")
    );
}

#[tokio::test]
async fn stale_autosave_returns_409_with_both_versions_recoverable() {
    let harness = Harness::new().await;
    let now = Utc::now();
    let created = harness
        .contents
        .create(draft("conflict"), SaveIntent::Explicit, now)
        .await
        .unwrap();
    let (cookie, csrf) = harness.session_cookie().await;
    let path = format!("/admin/content/{}/", created.id);

    let first = harness
        .send(
            Method::POST,
            &path,
            Some("application/x-www-form-urlencoded"),
            form(
                &csrf,
                "First tab",
                "conflict",
                Some(created.version),
                "autosave",
            ),
            Some(&cookie),
        )
        .await;
    assert_eq!(first.status(), StatusCode::OK);

    let stale = harness
        .send(
            Method::POST,
            &path,
            Some("application/x-www-form-urlencoded"),
            form(
                &csrf,
                "Second tab unsaved",
                "conflict",
                Some(created.version),
                "autosave",
            ),
            Some(&cookie),
        )
        .await;
    assert_eq!(stale.status(), StatusCode::CONFLICT);
    let body = text(stale).await;
    assert!(body.contains("First tab"));
    assert!(body.contains("Second tab unsaved"));
    assert!(body.to_ascii_lowercase().contains("conflict"));
}

#[tokio::test]
async fn setup_start_returns_public_options_without_echoing_the_capability() {
    let harness = Harness::new().await;
    let token = harness
        .auth
        .issue_setup_token(SetupPurpose::Initial, Utc::now())
        .await
        .unwrap();
    let payload = serde_json::json!({ "token": token.expose() }).to_string();
    let response = harness
        .send(
            Method::POST,
            "/admin/auth/setup/start",
            Some("application/json"),
            payload,
            None,
        )
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = text(response).await;
    assert!(body.contains("publicKey"));
    assert!(body.contains("flow_id"));
    assert!(!body.contains(token.expose()));
}

#[tokio::test]
async fn settings_and_ordered_navigation_work_without_javascript_and_require_csrf() {
    let harness = Harness::new().await;
    let (cookie, csrf) = harness.session_cookie().await;
    let page = harness
        .send(
            Method::GET,
            "/admin/settings/",
            None,
            Body::empty(),
            Some(&cookie),
        )
        .await;
    assert_eq!(page.status(), StatusCode::OK);
    let page = text(page).await;
    assert!(page.contains("name=\"site_title\""));
    assert!(page.contains("name=\"navigation\""));

    let settings_form = |csrf: &str| {
        serde_urlencoded::to_string([
            ("csrf", csrf),
            ("site_title", "Field Notes"),
            ("site_description", "Writing with intent"),
            ("locale", "en"),
            ("logo_media_id", ""),
            ("favicon_media_id", ""),
            ("custom_css", ".prose { text-wrap: balance; }"),
            (
                "navigation",
                "Archive | /archive/\nRust | https://www.rust-lang.org/",
            ),
        ])
        .unwrap()
    };
    let rejected = harness
        .send(
            Method::POST,
            "/admin/settings/",
            Some("application/x-www-form-urlencoded"),
            settings_form("wrong"),
            Some(&cookie),
        )
        .await;
    assert_eq!(rejected.status(), StatusCode::FORBIDDEN);

    let saved = harness
        .send(
            Method::POST,
            "/admin/settings/",
            Some("application/x-www-form-urlencoded"),
            settings_form(&csrf),
            Some(&cookie),
        )
        .await;
    assert_eq!(saved.status(), StatusCode::SEE_OTHER);
    assert_eq!(saved.headers()[header::LOCATION], "/admin/settings/");

    let public = harness
        .send(Method::GET, "/", None, Body::empty(), None)
        .await;
    let public = text(public).await;
    assert!(public.contains("Field Notes"));
    assert!(public.contains("href=\"/archive/\""));
    assert!(public.contains("https://www.rust-lang.org/"));
    assert!(public.contains("lang=\"en\""));
    assert!(public.contains("/assets/site.css?v="));

    let stylesheet = harness
        .send(Method::GET, "/assets/site.css", None, Body::empty(), None)
        .await;
    assert_eq!(stylesheet.status(), StatusCode::OK);
    assert_eq!(text(stylesheet).await, ".prose { text-wrap: balance; }");

    harness
        .contents
        .create(
            ContentDraft {
                publication: Publication::Public {
                    publish_at: Utc::now(),
                },
                ..draft("uses-site-title")
            },
            SaveIntent::Explicit,
            Utc::now(),
        )
        .await
        .unwrap();
    harness.state.publish_now().await.unwrap();
    let article = harness
        .send(Method::GET, "/uses-site-title/", None, Body::empty(), None)
        .await;
    let article = text(article).await;
    assert!(article.contains("<title>Original title — Field Notes</title>"));
    assert!(!article.contains("Original title — Simple Blog"));
}

#[tokio::test]
async fn security_page_can_start_passkey_add_and_rotate_recovery_codes_after_recent_auth() {
    let harness = Harness::new().await;
    let (cookie, csrf) = harness.registered_owner().await;
    harness
        .accounts
        .add_passkey(
            &StoredPasskey {
                credential_id: vec![6, 5, 4],
                name: "Second key".into(),
                passkey_json: "{}".into(),
            },
            Utc::now(),
        )
        .await
        .unwrap();
    let page = harness
        .send(
            Method::GET,
            "/admin/settings/",
            None,
            Body::empty(),
            Some(&cookie),
        )
        .await;
    assert_eq!(page.status(), StatusCode::OK);
    assert_eq!(page.headers()[header::CACHE_CONTROL], "no-store");
    let page = text(page).await;
    assert!(page.contains("First key"));
    assert!(page.contains("Second key"));

    let start = harness
        .send(
            Method::POST,
            "/admin/auth/passkeys/start",
            Some("application/json"),
            serde_json::json!({ "csrf": csrf }).to_string(),
            Some(&cookie),
        )
        .await;
    assert_eq!(start.status(), StatusCode::OK);
    let start = text(start).await;
    assert!(start.contains("publicKey"));
    assert!(start.contains("flow_id"));

    let recovery = harness
        .send(
            Method::POST,
            "/admin/settings/recovery-codes/",
            Some("application/x-www-form-urlencoded"),
            serde_urlencoded::to_string([("csrf", csrf.as_str())]).unwrap(),
            Some(&cookie),
        )
        .await;
    assert_eq!(recovery.status(), StatusCode::OK);
    assert_eq!(recovery.headers()[header::CACHE_CONTROL], "no-store");
    let recovery = text(recovery).await;
    assert!(recovery.contains("Recovery codes"));
    assert!(recovery.contains("Shown only once"));

    let stale = harness
        .auth
        .create_session(Utc::now() - Duration::minutes(10))
        .await
        .unwrap();
    let stale_cookie = format!(
        "sb_session={}; sb_csrf={}",
        stale.session.expose(),
        stale.csrf.expose()
    );
    let denied = harness
        .send(
            Method::POST,
            "/admin/settings/recovery-codes/",
            Some("application/x-www-form-urlencoded"),
            serde_urlencoded::to_string([("csrf", stale.csrf.expose())]).unwrap(),
            Some(&stale_cookie),
        )
        .await;
    assert_eq!(denied.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn owner_can_remove_a_passkey_but_not_the_last_credential() {
    let harness = Harness::new().await;
    let (cookie, csrf) = harness.registered_owner().await;
    harness
        .accounts
        .add_passkey(
            &StoredPasskey {
                credential_id: vec![6, 5, 4],
                name: "Second key".into(),
                passkey_json: "{}".into(),
            },
            Utc::now(),
        )
        .await
        .unwrap();
    let remove = |credential_id: &[u8]| {
        serde_urlencoded::to_string([
            ("csrf", csrf.as_str()),
            (
                "credential_id",
                URL_SAFE_NO_PAD.encode(credential_id).as_str(),
            ),
        ])
        .unwrap()
    };
    let removed = harness
        .send(
            Method::POST,
            "/admin/settings/passkeys/remove/",
            Some("application/x-www-form-urlencoded"),
            remove(&[6, 5, 4]),
            Some(&cookie),
        )
        .await;
    assert_eq!(removed.status(), StatusCode::SEE_OTHER);
    let last = harness
        .send(
            Method::POST,
            "/admin/settings/passkeys/remove/",
            Some("application/x-www-form-urlencoded"),
            remove(&[9, 8, 7]),
            Some(&cookie),
        )
        .await;
    assert_eq!(last.status(), StatusCode::CONFLICT);
}

#[tokio::test]
async fn a_recovery_code_is_one_time_and_returns_a_fresh_authenticated_session() {
    let harness = Harness::new().await;
    let code = harness
        .auth
        .replace_recovery_codes(Utc::now())
        .await
        .unwrap()
        .remove(0);
    let form = serde_urlencoded::to_string([("code", code.expose())]).unwrap();
    let recovered = harness
        .send(
            Method::POST,
            "/admin/auth/recovery",
            Some("application/x-www-form-urlencoded"),
            form.clone(),
            None,
        )
        .await;
    assert_eq!(recovered.status(), StatusCode::SEE_OTHER);
    assert_eq!(recovered.headers()[header::LOCATION], "/admin/settings/");
    assert_eq!(
        recovered
            .headers()
            .get_all(header::SET_COOKIE)
            .iter()
            .count(),
        2
    );

    let replay = harness
        .send(
            Method::POST,
            "/admin/auth/recovery",
            Some("application/x-www-form-urlencoded"),
            form,
            None,
        )
        .await;
    assert_eq!(replay.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn revision_history_can_be_inspected_and_restored_without_overwriting_newer_work() {
    let harness = Harness::new().await;
    let now = Utc::now();
    let original = harness
        .contents
        .create(draft("revision-rescue"), SaveIntent::Explicit, now)
        .await
        .unwrap();
    let first_revision = harness
        .repository
        .list_revisions(original.id)
        .await
        .unwrap()[0]
        .id;
    let mut changed = original.to_draft();
    changed.title = "Newer title".into();
    changed.body_markdown = "Newer body".into();
    let changed = harness
        .contents
        .update(
            original.id,
            original.version,
            changed,
            SaveIntent::Explicit,
            now + Duration::seconds(1),
        )
        .await
        .unwrap();
    let (cookie, csrf) = harness.session_cookie().await;

    let edit = harness
        .send(
            Method::GET,
            &format!("/admin/content/{}/edit/", original.id),
            None,
            Body::empty(),
            Some(&cookie),
        )
        .await;
    let edit = text(edit).await;
    assert!(edit.contains("History"));
    assert!(edit.contains(&format!(
        "/admin/content/{}/revisions/{first_revision}/",
        original.id
    )));

    let revision = harness
        .send(
            Method::GET,
            &format!("/admin/content/{}/revisions/{first_revision}/", original.id),
            None,
            Body::empty(),
            Some(&cookie),
        )
        .await;
    let revision = text(revision).await;
    assert!(revision.contains("Original title"));
    assert!(revision.contains("body"));
    assert!(revision.contains("Newer body"));

    let restore_form = serde_urlencoded::to_string([
        ("csrf", csrf.clone()),
        ("version", changed.version.to_string()),
    ])
    .unwrap();
    let restore_path = format!(
        "/admin/content/{}/revisions/{first_revision}/restore/",
        original.id
    );
    let restored = harness
        .send(
            Method::POST,
            &restore_path,
            Some("application/x-www-form-urlencoded"),
            restore_form.clone(),
            Some(&cookie),
        )
        .await;
    assert_eq!(restored.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        harness
            .repository
            .find_by_id(original.id)
            .await
            .unwrap()
            .unwrap()
            .title,
        "Original title"
    );

    let stale = harness
        .send(
            Method::POST,
            &restore_path,
            Some("application/x-www-form-urlencoded"),
            restore_form,
            Some(&cookie),
        )
        .await;
    assert_eq!(stale.status(), StatusCode::CONFLICT);
}

#[tokio::test]
async fn cover_media_can_be_selected_through_the_normal_editor_form() {
    let harness = Harness::new().await;
    let media_id = "a".repeat(64);
    sqlx::query("INSERT INTO media (id, original_name, mime_type, extension, width, height, byte_size, created_at) VALUES (?, 'cover.png', 'image/png', 'png', 1, 1, 1, ?)")
        .bind(&media_id)
        .bind(Utc::now())
        .execute(harness.repository.pool())
        .await
        .unwrap();
    let (cookie, csrf) = harness.session_cookie().await;
    let body = format!(
        "{}&cover_media_id={media_id}",
        form(&csrf, "With cover", "with-cover-form", None, "explicit")
    );
    let response = harness
        .send(
            Method::POST,
            "/admin/content/",
            Some("application/x-www-form-urlencoded"),
            body,
            Some(&cookie),
        )
        .await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let content = harness
        .repository
        .list_all_content()
        .await
        .unwrap()
        .remove(0);
    assert_eq!(content.cover_media_id.as_deref(), Some(media_id.as_str()));

    let editor = harness
        .send(
            Method::GET,
            &format!("/admin/content/{}/edit/", content.id),
            None,
            Body::empty(),
            Some(&cookie),
        )
        .await;
    let editor = text(editor).await;
    assert!(editor.contains("name=\"cover_media_id\""));
    assert!(editor.contains(&format!("value=\"{media_id}\"")));
    assert!(editor.contains("data-media-drop"));
}

#[tokio::test]
async fn live_preview_uses_the_same_safe_markdown_boundary_and_requires_csrf() {
    let harness = Harness::new().await;
    let (cookie, csrf) = harness.session_cookie().await;
    let payload = serde_json::json!({
        "csrf": csrf,
        "markdown": "# Preview\n\n<script>alert(1)</script>\n\n- [x] safe"
    })
    .to_string();
    let preview = harness
        .send(
            Method::POST,
            "/admin/preview/",
            Some("application/json"),
            payload,
            Some(&cookie),
        )
        .await;
    assert_eq!(preview.status(), StatusCode::OK);
    let preview: serde_json::Value = serde_json::from_str(&text(preview).await).unwrap();
    let html = preview["html"].as_str().unwrap();
    assert!(html.contains("<h1 id=\"user-content-preview\">"));
    assert!(html.contains("type=\"checkbox\""));
    assert!(!html.contains("<script"));

    let rejected = harness
        .send(
            Method::POST,
            "/admin/preview/",
            Some("application/json"),
            serde_json::json!({ "csrf": "wrong", "markdown": "body" }).to_string(),
            Some(&cookie),
        )
        .await;
    assert_eq!(rejected.status(), StatusCode::FORBIDDEN);

    let editor = harness
        .send(
            Method::GET,
            "/admin/content/new/",
            None,
            Body::empty(),
            Some(&cookie),
        )
        .await;
    assert!(text(editor).await.contains("data-preview"));
}

#[tokio::test]
async fn first_autosave_creates_once_then_returns_identity_for_future_updates() {
    let harness = Harness::new().await;
    let (cookie, csrf) = harness.session_cookie().await;
    let created = harness
        .send(
            Method::POST,
            "/admin/content/",
            Some("application/x-www-form-urlencoded"),
            form(&csrf, "Typing", "typing", None, "autosave"),
            Some(&cookie),
        )
        .await;
    assert_eq!(created.status(), StatusCode::CREATED);
    let created: serde_json::Value = serde_json::from_str(&text(created).await).unwrap();
    let id = created["id"].as_i64().unwrap();
    let version = created["version"].as_i64().unwrap();

    let updated = harness
        .send(
            Method::POST,
            &format!("/admin/content/{id}/"),
            Some("application/x-www-form-urlencoded"),
            form(&csrf, "Still typing", "typing", Some(version), "autosave"),
            Some(&cookie),
        )
        .await;
    assert_eq!(updated.status(), StatusCode::OK);
    let contents = harness.repository.list_all_content().await.unwrap();
    assert_eq!(contents.len(), 1);
    assert_eq!(contents[0].title, "Still typing");
}

fn form_with_status(
    csrf: &str,
    title: &str,
    slug: &str,
    version: Option<i64>,
    intent: &str,
    status: Option<&str>,
) -> String {
    let mut fields = vec![
        ("csrf", csrf.to_owned()),
        ("kind", "post".into()),
        ("title", title.into()),
        ("slug", slug.into()),
        ("summary", "summary".into()),
        ("body_markdown", "# body".into()),
        ("tags", String::new()),
        ("seo_title", String::new()),
        ("seo_description", String::new()),
        ("intent", intent.into()),
    ];
    if let Some(status) = status {
        fields.push(("status", status.into()));
    }
    if let Some(version) = version {
        fields.push(("version", version.to_string()));
    }
    serde_urlencoded::to_string(fields).unwrap()
}

fn is_timestamped_slug(slug: &str) -> bool {
    let bytes = slug.as_bytes();
    (bytes.len() == 13 || bytes.len() == 15)
        && bytes[8] == b'-'
        && bytes
            .iter()
            .enumerate()
            .all(|(index, byte)| index == 8 || byte.is_ascii_digit())
}

#[tokio::test]
#[expect(
    clippy::too_many_lines,
    reason = "one publish lifecycle, kept as a single scenario"
)]
async fn autosave_without_status_never_demotes_or_moves_published_content() {
    let harness = Harness::new().await;
    let (cookie, csrf) = harness.session_cookie().await;

    // Create with no status field: a fresh piece is a draft.
    let created = harness
        .send(
            Method::POST,
            "/admin/content/",
            Some("application/x-www-form-urlencoded"),
            form_with_status(&csrf, "Piece", "piece", None, "autosave", None),
            Some(&cookie),
        )
        .await;
    assert_eq!(created.status(), StatusCode::CREATED);
    let created: serde_json::Value = serde_json::from_str(&text(created).await).unwrap();
    let id = created["id"].as_i64().unwrap();
    let content_id = ContentId::from_i64(id);
    let fetch = |harness: &Harness| {
        let repository = harness.repository.clone();
        async move { repository.find_by_id(content_id).await.unwrap().unwrap() }
    };
    assert_eq!(fetch(&harness).await.publication, Publication::Draft);

    // Publish (the editor bar button posts status=public, intent=explicit).
    let version = created["version"].as_i64().unwrap();
    let published = harness
        .send(
            Method::POST,
            &format!("/admin/content/{id}/"),
            Some("application/x-www-form-urlencoded"),
            form_with_status(
                &csrf,
                "Piece",
                "piece",
                Some(version),
                "explicit",
                Some("public"),
            ),
            Some(&cookie),
        )
        .await;
    assert_eq!(published.status(), StatusCode::SEE_OTHER);
    let after_publish = fetch(&harness).await;
    let Publication::Public { publish_at } = after_publish.publication else {
        panic!("publish must make the content public");
    };

    // A later autosave carries no status: publication and publish_at survive.
    let autosaved = harness
        .send(
            Method::POST,
            &format!("/admin/content/{id}/"),
            Some("application/x-www-form-urlencoded"),
            form_with_status(
                &csrf,
                "Piece edited",
                "piece",
                Some(after_publish.version),
                "autosave",
                None,
            ),
            Some(&cookie),
        )
        .await;
    assert_eq!(autosaved.status(), StatusCode::OK);
    let after_autosave = fetch(&harness).await;
    assert_eq!(after_autosave.title, "Piece edited");
    assert_eq!(
        after_autosave.publication,
        Publication::Public { publish_at },
        "an autosave must not demote or re-date published content"
    );

    // Re-sending status=public (e.g. a retried publish) also keeps the date.
    let republished = harness
        .send(
            Method::POST,
            &format!("/admin/content/{id}/"),
            Some("application/x-www-form-urlencoded"),
            form_with_status(
                &csrf,
                "Piece edited",
                "piece",
                Some(after_autosave.version),
                "explicit",
                Some("public"),
            ),
            Some(&cookie),
        )
        .await;
    assert_eq!(republished.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        fetch(&harness).await.publication,
        Publication::Public { publish_at }
    );

    // Unpublish returns to draft.
    let unpublished = harness
        .send(
            Method::POST,
            &format!("/admin/content/{id}/"),
            Some("application/x-www-form-urlencoded"),
            form_with_status(
                &csrf,
                "Piece edited",
                "piece",
                Some(fetch(&harness).await.version),
                "explicit",
                Some("draft"),
            ),
            Some(&cookie),
        )
        .await;
    assert_eq!(unpublished.status(), StatusCode::SEE_OTHER);
    assert_eq!(fetch(&harness).await.publication, Publication::Draft);
}

#[tokio::test]
async fn explicit_saves_answer_json_when_the_client_asks_for_it() {
    let harness = Harness::new().await;
    let (cookie, csrf) = harness.session_cookie().await;
    let request = Request::builder()
        .method(Method::POST)
        .uri("/admin/content/")
        .header(header::HOST, "localhost:8080")
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .header(header::ACCEPT, "application/json")
        .header(header::COOKIE, &cookie)
        .body(Body::from(form_with_status(
            &csrf,
            "Fetch publish",
            "fetch-publish",
            None,
            "explicit",
            Some("public"),
        )))
        .unwrap();
    let response = router(harness.state.clone())
        .oneshot(request)
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
    let body: serde_json::Value = serde_json::from_str(&text(response).await).unwrap();
    assert_eq!(body["slug"].as_str().unwrap(), "fetch-publish");
    assert!(body["id"].as_i64().is_some());
}

#[tokio::test]
async fn empty_slugs_are_generated_server_side_and_collisions_get_seconds() {
    let harness = Harness::new().await;
    let (cookie, csrf) = harness.session_cookie().await;

    let first = harness
        .send(
            Method::POST,
            "/admin/content/",
            Some("application/x-www-form-urlencoded"),
            form_with_status(&csrf, "日本語タイトル", "", None, "autosave", None),
            Some(&cookie),
        )
        .await;
    assert_eq!(first.status(), StatusCode::CREATED);
    let first: serde_json::Value = serde_json::from_str(&text(first).await).unwrap();
    let first_slug = first["slug"].as_str().unwrap().to_owned();
    assert!(
        is_timestamped_slug(&first_slug),
        "generated slug must be timestamp-shaped, got {first_slug:?}"
    );

    // A second empty-slug create in the same minute collides and retries at
    // second resolution (or lands in the next minute; either way it differs).
    let second = harness
        .send(
            Method::POST,
            "/admin/content/",
            Some("application/x-www-form-urlencoded"),
            form_with_status(&csrf, "另一篇", "", None, "autosave", None),
            Some(&cookie),
        )
        .await;
    assert_eq!(second.status(), StatusCode::CREATED);
    let second: serde_json::Value = serde_json::from_str(&text(second).await).unwrap();
    let second_slug = second["slug"].as_str().unwrap();
    assert!(is_timestamped_slug(second_slug));
    assert_ne!(second_slug, first_slug);
}

#[tokio::test]
async fn editor_offers_publish_buttons_instead_of_a_status_select() {
    let harness = Harness::new().await;
    let (cookie, _csrf) = harness.session_cookie().await;
    let page = harness
        .send(
            Method::GET,
            "/admin/content/new/",
            None,
            Body::empty(),
            Some(&cookie),
        )
        .await;
    assert_eq!(page.status(), StatusCode::OK);
    let html = text(page).await;
    assert!(html.contains("data-publish"));
    assert!(html.contains("data-unpublish"));
    assert!(!html.contains("<select name=\"status\""));
    assert!(!html.contains("name=\"publish_at\""));
    // The slug field is pre-filled with a server-generated timestamp slug.
    let slug_value = html
        .split("name=\"slug\" value=\"")
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .expect("slug input present");
    assert!(
        is_timestamped_slug(slug_value),
        "prefilled slug must be timestamp-shaped, got {slug_value:?}"
    );
}
