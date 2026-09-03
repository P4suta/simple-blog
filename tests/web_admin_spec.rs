use std::sync::{Arc, Mutex};

use axum::{
    body::Body,
    http::{Method, Request, StatusCode, header},
};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{DateTime, Duration, TimeZone, Utc};
use http_body_util::BodyExt;
use simple_blog::{
    application::{
        auth::AuthService,
        auth::PasskeyAccountService,
        content::{ContentService, SaveIntent},
        ports::{Clock, ContentRepository, SiteRepository},
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

impl Harness {
    async fn new() -> Self {
        Self::build(None).await
    }

    async fn new_with_clock(clock: Arc<dyn Clock>) -> Self {
        Self::build(Some(clock)).await
    }

    async fn build(clock: Option<Arc<dyn Clock>>) -> Self {
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
        let state = match clock {
            Some(clock) => AppState::new_with_clock(config, repository.clone(), clock).unwrap(),
            None => AppState::new(config, repository.clone()).unwrap(),
        };
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
    assert!(public.contains("https:&#x2f;&#x2f;www.rust-lang.org&#x2f;"));
    assert!(public.contains("lang=\"en\""));
    assert!(public.contains("&#x2f;assets&#x2f;site.css?v="));

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
    Slug::parse(slug).is_ok_and(|slug| slug.is_timestamped())
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
    assert!(html.contains("name=\"publish_at\""));
    assert!(html.contains("type=\"datetime-local\""));
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

#[tokio::test]
async fn admin_page_titles_and_brand_follow_the_site_locale() {
    let harness = Harness::new().await;
    let (cookie, csrf) = harness.session_cookie().await;
    let page = |path: &'static str, cookie: Option<String>| {
        let harness = &harness;
        async move {
            let response = harness
                .send(Method::GET, path, None, Body::empty(), cookie.as_deref())
                .await;
            assert_eq!(response.status(), StatusCode::OK, "{path}");
            text(response).await
        }
    };

    assert!(
        page("/admin/", Some(cookie.clone()))
            .await
            .contains("<title>Dashboard — Simple Blog Admin</title>")
    );
    assert!(
        page("/admin/content/new/", Some(cookie.clone()))
            .await
            .contains("<title>New piece — Simple Blog Admin</title>")
    );
    assert!(
        page("/admin/settings/", Some(cookie.clone()))
            .await
            .contains("<title>Settings — Simple Blog Admin</title>")
    );
    assert!(
        page("/admin/login/", None)
            .await
            .contains("<title>Log in — Simple Blog Admin</title>")
    );

    let switched = harness
        .send(
            Method::POST,
            "/admin/settings/",
            Some("application/x-www-form-urlencoded"),
            serde_urlencoded::to_string([
                ("csrf", csrf.as_str()),
                ("site_title", "野帳"),
                ("site_description", ""),
                ("locale", "ja"),
                ("logo_media_id", ""),
                ("favicon_media_id", ""),
                ("custom_css", ""),
                ("navigation", ""),
            ])
            .unwrap(),
            Some(&cookie),
        )
        .await;
    assert_eq!(switched.status(), StatusCode::SEE_OTHER);

    assert!(
        page("/admin/", Some(cookie.clone()))
            .await
            .contains("<title>ダッシュボード — Simple Blog 管理</title>")
    );
    assert!(
        page("/admin/login/", None)
            .await
            .contains("<title>ログイン — Simple Blog 管理</title>")
    );
}

#[tokio::test]
async fn dashboard_lists_pieces_as_real_links_inside_a_list() {
    let harness = Harness::new().await;
    let (cookie, _csrf) = harness.session_cookie().await;

    let empty = harness
        .send(Method::GET, "/admin/", None, Body::empty(), Some(&cookie))
        .await;
    let empty = text(empty).await;
    assert!(empty.contains("class=\"content-empty\""));
    assert!(empty.contains("Nothing here yet. Write the first piece."));
    assert!(!empty.contains("<ul class=\"content-table\">"));

    harness
        .contents
        .create(draft("listed"), SaveIntent::Explicit, Utc::now())
        .await
        .unwrap();
    let listed = harness
        .send(Method::GET, "/admin/", None, Body::empty(), Some(&cookie))
        .await;
    let listed = text(listed).await;
    assert!(listed.contains("<ul class=\"content-table\">"));
    assert!(listed.contains("<li><a href=\"/admin/content/"));
    // An explicit ARIA role on an anchor would hide that it is a link.
    assert!(!listed.contains("role=\"listitem\""));
    assert!(!listed.contains("role=\"list\""));
}

#[tokio::test]
async fn taken_slug_conflict_is_plain_text_while_version_conflict_is_the_html_page() {
    let harness = Harness::new().await;
    let now = Utc::now();
    harness
        .contents
        .create(draft("taken"), SaveIntent::Explicit, now)
        .await
        .unwrap();
    let second = harness
        .contents
        .create(draft("second"), SaveIntent::Explicit, now)
        .await
        .unwrap();
    let (cookie, csrf) = harness.session_cookie().await;
    let path = format!("/admin/content/{}/", second.id);

    let taken = harness
        .send(
            Method::POST,
            &path,
            Some("application/x-www-form-urlencoded"),
            form(&csrf, "Second", "taken", Some(second.version), "autosave"),
            Some(&cookie),
        )
        .await;
    assert_eq!(taken.status(), StatusCode::CONFLICT);
    let content_type = taken.headers()[header::CONTENT_TYPE]
        .to_str()
        .unwrap()
        .to_owned();
    assert!(content_type.starts_with("text/plain"), "{content_type}");
    assert!(text(taken).await.contains("slug is already used: taken"));

    let stale = harness
        .send(
            Method::POST,
            &path,
            Some("application/x-www-form-urlencoded"),
            form(
                &csrf,
                "Second",
                "second",
                Some(second.version + 5),
                "autosave",
            ),
            Some(&cookie),
        )
        .await;
    assert_eq!(stale.status(), StatusCode::CONFLICT);
    let content_type = stale.headers()[header::CONTENT_TYPE]
        .to_str()
        .unwrap()
        .to_owned();
    assert!(content_type.starts_with("text/html"), "{content_type}");
}
fn scheduled_form(
    csrf: &str,
    title: &str,
    slug: &str,
    version: Option<i64>,
    publish_at: &str,
) -> String {
    let mut fields = vec![
        ("csrf", csrf.to_owned()),
        ("kind", "post".into()),
        ("title", title.into()),
        ("slug", slug.into()),
        ("summary", String::new()),
        ("body_markdown", "# body".into()),
        ("tags", String::new()),
        ("status", "public".into()),
        ("publish_at", publish_at.into()),
        ("seo_title", String::new()),
        ("seo_description", String::new()),
        ("intent", "explicit".into()),
    ];
    if let Some(version) = version {
        fields.push(("version", version.to_string()));
    }
    serde_urlencoded::to_string(fields).unwrap()
}

#[tokio::test]
#[expect(clippy::too_many_lines, reason = "one trash lifecycle, end to end")]
async fn trash_restore_and_permanent_delete_are_owner_actions_with_csrf() {
    let harness = Harness::new().await;
    let now = Utc::now();
    let created = harness
        .contents
        .create(
            ContentDraft {
                publication: Publication::Public { publish_at: now },
                ..draft("disposable")
            },
            SaveIntent::Explicit,
            now,
        )
        .await
        .unwrap();
    let (cookie, csrf) = harness.session_cookie().await;
    let trash_form = |csrf: &str, version: i64| {
        serde_urlencoded::to_string([("csrf", csrf), ("version", &version.to_string())]).unwrap()
    };
    let csrf_form = |csrf: &str| serde_urlencoded::to_string([("csrf", csrf)]).unwrap();
    let post = |path: String, body: String| {
        let harness = &harness;
        let cookie = cookie.clone();
        async move {
            harness
                .send(
                    Method::POST,
                    &path,
                    Some("application/x-www-form-urlencoded"),
                    body,
                    Some(&cookie),
                )
                .await
        }
    };
    let trash_path = format!("/admin/content/{}/trash/", created.id);
    let restore_path = format!("/admin/content/{}/restore/", created.id);
    let delete_path = format!("/admin/content/{}/delete/", created.id);

    for path in [&trash_path, &restore_path, &delete_path] {
        let forbidden = post(path.clone(), trash_form("wrong", created.version)).await;
        assert_eq!(forbidden.status(), StatusCode::FORBIDDEN, "{path}");
    }

    let stale = post(trash_path.clone(), trash_form(&csrf, created.version + 3)).await;
    assert_eq!(stale.status(), StatusCode::CONFLICT);

    harness.state.publish_now().await.unwrap();
    let live = harness
        .send(Method::GET, "/disposable/", None, Body::empty(), None)
        .await;
    assert_eq!(live.status(), StatusCode::OK);

    let trashed = post(trash_path.clone(), trash_form(&csrf, created.version)).await;
    assert_eq!(trashed.status(), StatusCode::SEE_OTHER);
    assert_eq!(trashed.headers()[header::LOCATION], "/admin/?status=trash");

    let withdrawn = harness
        .send(Method::GET, "/disposable/", None, Body::empty(), None)
        .await;
    assert_eq!(withdrawn.status(), StatusCode::NOT_FOUND);

    let editor = harness
        .send(
            Method::GET,
            &format!("/admin/content/{}/edit/", created.id),
            None,
            Body::empty(),
            Some(&cookie),
        )
        .await;
    let editor = text(editor).await;
    assert!(editor.contains("data-trashed=\"true\""));
    assert!(!editor.contains("name=\"status\" value=\"public\""));
    assert!(!editor.contains("name=\"status\" value=\"draft\""));
    assert!(editor.contains(&restore_path));
    assert!(editor.contains(&delete_path));
    assert!(editor.contains("This piece is in the trash."));

    let edit_attempt = post(
        format!("/admin/content/{}/", created.id),
        form(
            &csrf,
            "Edited while trashed",
            "disposable",
            Some(created.version + 1),
            "autosave",
        ),
    )
    .await;
    assert_eq!(edit_attempt.status(), StatusCode::UNPROCESSABLE_ENTITY);

    let restored = post(restore_path.clone(), csrf_form(&csrf)).await;
    assert_eq!(restored.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        restored.headers()[header::LOCATION],
        format!("/admin/content/{}/edit/", created.id)
    );
    let back = harness
        .send(Method::GET, "/disposable/", None, Body::empty(), None)
        .await;
    assert_eq!(back.status(), StatusCode::OK);

    let refused = post(delete_path.clone(), csrf_form(&csrf)).await;
    assert_eq!(
        refused.status(),
        StatusCode::NOT_FOUND,
        "live content must never be deleted permanently"
    );

    let current = harness
        .repository
        .find_by_id(created.id)
        .await
        .unwrap()
        .unwrap();
    let trashed = post(trash_path.clone(), trash_form(&csrf, current.version)).await;
    assert_eq!(trashed.status(), StatusCode::SEE_OTHER);
    let deleted = post(delete_path.clone(), csrf_form(&csrf)).await;
    assert_eq!(deleted.status(), StatusCode::SEE_OTHER);
    assert_eq!(deleted.headers()[header::LOCATION], "/admin/?status=trash");
    assert!(
        harness
            .repository
            .find_by_id(created.id)
            .await
            .unwrap()
            .is_none()
    );
    let gone = post(delete_path, csrf_form(&csrf)).await;
    assert_eq!(gone.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn scheduling_through_the_editor_form_flips_visibility_exactly_at_the_boundary() {
    let clock = TestClock::new(Utc.with_ymd_and_hms(2026, 9, 3, 12, 0, 0).unwrap());
    let harness = Harness::new_with_clock(Arc::new(clock.clone())).await;
    let (cookie, csrf) = harness.session_cookie().await;

    let created = harness
        .send(
            Method::POST,
            "/admin/content/",
            Some("application/x-www-form-urlencoded"),
            scheduled_form(&csrf, "Later", "later", None, "2026-09-03T12:01:00Z"),
            Some(&cookie),
        )
        .await;
    assert_eq!(created.status(), StatusCode::SEE_OTHER);

    let dashboard = harness
        .send(Method::GET, "/admin/", None, Body::empty(), Some(&cookie))
        .await;
    let dashboard = text(dashboard).await;
    assert!(dashboard.contains("status-scheduled"), "{dashboard}");

    let hidden = harness
        .send(Method::GET, "/later/", None, Body::empty(), None)
        .await;
    assert_eq!(hidden.status(), StatusCode::NOT_FOUND);

    clock.set(Utc.with_ymd_and_hms(2026, 9, 3, 12, 0, 59).unwrap());
    harness.state.publish_now().await.unwrap();
    let still_hidden = harness
        .send(Method::GET, "/later/", None, Body::empty(), None)
        .await;
    assert_eq!(still_hidden.status(), StatusCode::NOT_FOUND);

    clock.set(Utc.with_ymd_and_hms(2026, 9, 3, 12, 1, 0).unwrap());
    harness.state.publish_now().await.unwrap();
    let visible = harness
        .send(Method::GET, "/later/", None, Body::empty(), None)
        .await;
    assert_eq!(visible.status(), StatusCode::OK);

    let dashboard = harness
        .send(Method::GET, "/admin/", None, Body::empty(), Some(&cookie))
        .await;
    let dashboard = text(dashboard).await;
    assert!(dashboard.contains("status-public"));
    assert!(!dashboard.contains("status-scheduled"));
}

#[tokio::test]
async fn publish_dates_accept_naive_utc_input_and_reject_garbage() {
    let clock = TestClock::new(Utc.with_ymd_and_hms(2026, 9, 3, 12, 0, 0).unwrap());
    let harness = Harness::new_with_clock(Arc::new(clock.clone())).await;
    let (cookie, csrf) = harness.session_cookie().await;

    let created = harness
        .send(
            Method::POST,
            "/admin/content/",
            Some("application/x-www-form-urlencoded"),
            scheduled_form(&csrf, "Naive", "naive", None, "2026-09-03T12:01"),
            Some(&cookie),
        )
        .await;
    assert_eq!(created.status(), StatusCode::SEE_OTHER);
    let stored = harness
        .repository
        .find_public_by_slug(
            &Slug::parse("naive").unwrap(),
            Utc.with_ymd_and_hms(2026, 9, 3, 12, 1, 0).unwrap(),
        )
        .await
        .unwrap()
        .expect("scheduled entry exists");
    assert_eq!(
        stored.publication,
        Publication::Public {
            publish_at: Utc.with_ymd_and_hms(2026, 9, 3, 12, 1, 0).unwrap()
        }
    );

    let rejected = harness
        .send(
            Method::POST,
            "/admin/content/",
            Some("application/x-www-form-urlencoded"),
            scheduled_form(&csrf, "Garbage", "garbage", None, "next tuesday"),
            Some(&cookie),
        )
        .await;
    assert_eq!(rejected.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert!(text(rejected).await.contains("ISO 8601"));
}

#[tokio::test]
#[expect(
    clippy::too_many_lines,
    reason = "same-minute, moved, and draft cases in one scenario"
)]
async fn editing_the_publish_date_moves_a_post_but_same_minute_values_keep_it() {
    let clock = TestClock::new(Utc.with_ymd_and_hms(2026, 9, 3, 12, 0, 37).unwrap());
    let harness = Harness::new_with_clock(Arc::new(clock.clone())).await;
    let (cookie, csrf) = harness.session_cookie().await;
    let published_at = Utc.with_ymd_and_hms(2026, 9, 3, 12, 0, 37).unwrap();
    let created = harness
        .contents
        .create(
            ContentDraft {
                publication: Publication::Public {
                    publish_at: published_at,
                },
                ..draft("dated")
            },
            SaveIntent::Explicit,
            published_at,
        )
        .await
        .unwrap();
    let path = format!("/admin/content/{}/", created.id);
    let revision_before = harness.state.publish_now().await.unwrap().public_revision;

    let autosave = |csrf: &str, version: i64, publish_at: &str| {
        serde_urlencoded::to_string([
            ("csrf", csrf),
            ("kind", "post"),
            ("title", "Original title"),
            ("slug", "dated"),
            ("summary", "summary"),
            ("body_markdown", "body"),
            ("tags", ""),
            ("publish_at", publish_at),
            ("intent", "autosave"),
            ("version", &version.to_string()),
        ])
        .unwrap()
    };

    // The control only carries minutes; round-tripping it must not re-date.
    let same_minute = harness
        .send(
            Method::POST,
            &path,
            Some("application/x-www-form-urlencoded"),
            autosave(&csrf, created.version, "2026-09-03T12:00"),
            Some(&cookie),
        )
        .await;
    assert_eq!(same_minute.status(), StatusCode::OK);
    let body = text(same_minute).await;
    assert!(body.contains("\"status\":\"public\""), "{body}");
    assert!(
        body.contains("\"publish_at\":\"2026-09-03T12:00:37Z\""),
        "{body}"
    );
    assert_eq!(
        harness.state.publish_now().await.unwrap().public_revision,
        revision_before + 1,
        "an autosave still records a revision, but the date is unchanged"
    );

    let current = harness
        .repository
        .find_by_id(created.id)
        .await
        .unwrap()
        .unwrap();
    let moved = harness
        .send(
            Method::POST,
            &path,
            Some("application/x-www-form-urlencoded"),
            autosave(&csrf, current.version, "2026-09-02T09:30:00Z"),
            Some(&cookie),
        )
        .await;
    assert_eq!(moved.status(), StatusCode::OK);
    let current = harness
        .repository
        .find_by_id(created.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        current.publication,
        Publication::Public {
            publish_at: Utc.with_ymd_and_hms(2026, 9, 2, 9, 30, 0).unwrap()
        }
    );

    // A draft never takes a date from an autosave: publishing is explicit.
    let draft_piece = harness
        .contents
        .create(draft("undated"), SaveIntent::Explicit, published_at)
        .await
        .unwrap();
    let kept_draft = harness
        .send(
            Method::POST,
            &format!("/admin/content/{}/", draft_piece.id),
            Some("application/x-www-form-urlencoded"),
            serde_urlencoded::to_string([
                ("csrf", csrf.as_str()),
                ("kind", "post"),
                ("title", "Original title"),
                ("slug", "undated"),
                ("summary", "summary"),
                ("body_markdown", "body"),
                ("tags", ""),
                ("publish_at", "2026-12-24T18:00:00Z"),
                ("intent", "autosave"),
                ("version", &draft_piece.version.to_string()),
            ])
            .unwrap(),
            Some(&cookie),
        )
        .await;
    assert_eq!(kept_draft.status(), StatusCode::OK);
    assert!(text(kept_draft).await.contains("\"status\":\"draft\""));
}

#[tokio::test]
async fn logout_revokes_the_session_and_clears_both_cookies() {
    let harness = Harness::new().await;
    let (cookie, csrf) = harness.session_cookie().await;

    let dashboard = harness
        .send(Method::GET, "/admin/", None, Body::empty(), Some(&cookie))
        .await;
    assert!(text(dashboard).await.contains("action=\"/admin/logout/\""));
    let settings = harness
        .send(
            Method::GET,
            "/admin/settings/",
            None,
            Body::empty(),
            Some(&cookie),
        )
        .await;
    assert!(text(settings).await.contains("action=\"/admin/logout/\""));

    let forbidden = harness
        .send(
            Method::POST,
            "/admin/logout/",
            Some("application/x-www-form-urlencoded"),
            serde_urlencoded::to_string([("csrf", "wrong")]).unwrap(),
            Some(&cookie),
        )
        .await;
    assert_eq!(forbidden.status(), StatusCode::FORBIDDEN);

    let logged_out = harness
        .send(
            Method::POST,
            "/admin/logout/",
            Some("application/x-www-form-urlencoded"),
            serde_urlencoded::to_string([("csrf", csrf.as_str())]).unwrap(),
            Some(&cookie),
        )
        .await;
    assert_eq!(logged_out.status(), StatusCode::SEE_OTHER);
    assert_eq!(logged_out.headers()[header::LOCATION], "/admin/login/");
    let cleared: Vec<String> = logged_out
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .map(|value| value.to_str().unwrap().to_owned())
        .collect();
    assert_eq!(cleared.len(), 2, "{cleared:?}");
    assert!(cleared.iter().any(|value| value.starts_with("sb_session=;")
        && value.contains("Max-Age=0")
        && value.contains("HttpOnly")));
    assert!(
        cleared
            .iter()
            .any(|value| value.starts_with("sb_csrf=;") && value.contains("Max-Age=0"))
    );

    let after = harness
        .send(Method::GET, "/admin/", None, Body::empty(), Some(&cookie))
        .await;
    assert_eq!(after.status(), StatusCode::SEE_OTHER);
    assert_eq!(after.headers()[header::LOCATION], "/admin/login/");
}

#[tokio::test]
async fn login_remembers_the_admin_page_that_was_requested() {
    let harness = Harness::new().await;

    let settings = harness
        .send(Method::GET, "/admin/settings/", None, Body::empty(), None)
        .await;
    assert_eq!(settings.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        settings.headers()[header::LOCATION],
        "/admin/login/?next=/admin/settings/"
    );

    let login = harness
        .send(
            Method::GET,
            "/admin/login/?next=/admin/settings/",
            None,
            Body::empty(),
            None,
        )
        .await;
    // minijinja escapes `/` inside attributes; browsers decode it back.
    assert!(
        text(login)
            .await
            .contains("data-next=\"&#x2f;admin&#x2f;settings&#x2f;\"")
    );

    for hostile in [
        "https://evil.example/admin/",
        "//evil.example/admin/",
        "/archive/",
        "/admin/../secret",
        "/admin/settings/?x=1",
    ] {
        let login = harness
            .send(
                Method::GET,
                &format!("/admin/login/?next={}", percent_encode(hostile)),
                None,
                Body::empty(),
                None,
            )
            .await;
        assert!(
            text(login)
                .await
                .contains("data-next=\"&#x2f;admin&#x2f;\""),
            "{hostile} must fall back to the dashboard"
        );
    }

    let dashboard = harness
        .send(Method::GET, "/admin/", None, Body::empty(), None)
        .await;
    assert_eq!(
        dashboard.headers()[header::LOCATION],
        "/admin/login/",
        "the dashboard itself needs no next parameter"
    );
}

fn percent_encode(value: &str) -> String {
    serde_urlencoded::to_string([("next", value)])
        .unwrap()
        .trim_start_matches("next=")
        .to_owned()
}
#[tokio::test]
async fn expected_failures_render_a_page_for_browsers_and_text_for_scripts() {
    let harness = Harness::new().await;
    let (cookie, csrf) = harness.session_cookie().await;
    harness
        .contents
        .create(draft("held"), SaveIntent::Explicit, Utc::now())
        .await
        .unwrap();
    let body = form(&csrf, "Clash", "held", None, "explicit");

    let plain = harness
        .send(
            Method::POST,
            "/admin/content/",
            Some("application/x-www-form-urlencoded"),
            body.clone(),
            Some(&cookie),
        )
        .await;
    assert_eq!(plain.status(), StatusCode::CONFLICT);
    assert!(
        plain.headers()[header::CONTENT_TYPE]
            .to_str()
            .unwrap()
            .starts_with("text/plain")
    );

    let page = router(harness.state.clone())
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/admin/content/")
                .header(header::HOST, "localhost:8080")
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .header(header::ACCEPT, "text/html,application/xhtml+xml")
                .header(header::COOKIE, &cookie)
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(page.status(), StatusCode::CONFLICT);
    assert!(
        page.headers()[header::CONTENT_TYPE]
            .to_str()
            .unwrap()
            .starts_with("text/html")
    );
    let html = text(page).await;
    assert!(html.contains("Something changed in the meantime"));
    assert!(html.contains("slug is already used: held"));
    assert!(html.contains("href=\"/admin/\""));
    assert!(html.contains("<title>Something needs attention"));

    let missing = router(harness.state.clone())
        .oneshot(
            Request::builder()
                .uri("/admin/content/999999/edit/")
                .header(header::HOST, "localhost:8080")
                .header(header::ACCEPT, "text/html")
                .header(header::COOKIE, &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);
    assert!(text(missing).await.contains("Nothing here"));
}

#[tokio::test]
async fn editor_exposes_counters_shortcut_hints_confirmations_and_error_labels() {
    let harness = Harness::new().await;
    let (cookie, _csrf) = harness.session_cookie().await;
    let published = harness
        .contents
        .create(
            ContentDraft {
                publication: Publication::Public {
                    publish_at: Utc::now(),
                },
                ..draft("polished")
            },
            SaveIntent::Explicit,
            Utc::now(),
        )
        .await
        .unwrap();

    let page = harness
        .send(
            Method::GET,
            &format!("/admin/content/{}/edit/", published.id),
            None,
            Body::empty(),
            Some(&cookie),
        )
        .await;
    let html = text(page).await;
    for marker in [
        "data-count",
        "data-shortcuts",
        "data-msg-saved-at",
        "data-msg-count",
        "data-msg-slug-invalid",
        "data-msg-error-server",
        "data-msg-error-session",
        "data-msg-error-offline",
        "data-msg-upload-failed",
        "data-msg-saved-pending",
        "<dialog class=\"editor-drawer\"",
        "<details class=\"editor-confirm\" data-unpublish",
        "Take this off the public site?",
        "data-publish-at",
        "name=\"alt_text\"",
        "data-cover-alt-form",
    ] {
        assert!(html.contains(marker), "missing {marker}");
    }
    assert!(!html.contains("data-drawer-backdrop"));
}

#[tokio::test]
#[expect(
    clippy::too_many_lines,
    reason = "every dashboard filter in one scenario"
)]
async fn dashboard_filters_by_status_and_text_and_shows_dates() {
    let clock = TestClock::new(Utc.with_ymd_and_hms(2026, 9, 3, 12, 0, 0).unwrap());
    let now = clock.now();
    let harness = Harness::new_with_clock(Arc::new(clock)).await;
    let (cookie, _csrf) = harness.session_cookie().await;
    let make = |title: &str, slug: &str, publication: Publication| ContentDraft {
        title: title.into(),
        publication,
        ..draft(slug)
    };
    harness
        .contents
        .create(
            make("Alpha", "alpha", Publication::Draft),
            SaveIntent::Explicit,
            now,
        )
        .await
        .unwrap();
    harness
        .contents
        .create(
            make(
                "Beta",
                "beta",
                Publication::Public {
                    publish_at: now - Duration::seconds(1),
                },
            ),
            SaveIntent::Explicit,
            now,
        )
        .await
        .unwrap();
    harness
        .contents
        .create(
            make(
                "Gamma",
                "gamma",
                Publication::Public {
                    publish_at: now + Duration::hours(1),
                },
            ),
            SaveIntent::Explicit,
            now,
        )
        .await
        .unwrap();
    let delta = harness
        .contents
        .create(
            make("Delta", "delta", Publication::Draft),
            SaveIntent::Explicit,
            now,
        )
        .await
        .unwrap();
    harness
        .repository
        .move_to_trash(delta.id, delta.version, now)
        .await
        .unwrap();

    let page = |query: &'static str| {
        let harness = &harness;
        let cookie = cookie.clone();
        async move {
            let response = harness
                .send(
                    Method::GET,
                    &format!("/admin/{query}"),
                    None,
                    Body::empty(),
                    Some(&cookie),
                )
                .await;
            assert_eq!(response.status(), StatusCode::OK, "{query}");
            text(response).await
        }
    };

    let all = page("").await;
    assert!(all.contains("Alpha") && all.contains("Beta") && all.contains("Gamma"));
    assert!(
        !all.contains("Delta"),
        "the trash stays out of the main list"
    );
    assert!(all.contains("status-scheduled"));
    assert!(all.contains("<time datetime=\"2026-09-03T12:00:00Z\" data-local-time>"));
    assert!(all.contains("<time datetime=\"2026-09-03T13:00:00Z\" data-local-time>"));
    assert!(all.contains("aria-current=\"page\""));
    assert!(all.contains("href=\"/admin/?status=trash\""));

    let drafts = page("?status=draft").await;
    assert!(drafts.contains("Alpha"));
    assert!(!drafts.contains("Beta") && !drafts.contains("Gamma") && !drafts.contains("Delta"));

    let scheduled = page("?status=scheduled").await;
    assert!(scheduled.contains("Gamma") && !scheduled.contains("Beta"));

    let public = page("?status=public").await;
    assert!(public.contains("Beta") && !public.contains("Gamma"));

    let searched = page("?q=bet").await;
    assert!(searched.contains("Beta") && !searched.contains("Alpha"));
    assert!(searched.contains("value=\"bet\""));

    let trash = page("?status=trash").await;
    assert!(trash.contains("Delta"));
    assert!(!trash.contains("Alpha"));
    assert!(trash.contains(&format!("/admin/content/{}/restore/", delta.id)));
    assert!(trash.contains(&format!("/admin/content/{}/delete/", delta.id)));
    assert!(trash.contains("status-trashed"));
    assert!(trash.contains("Delete forever"));

    let nothing = page("?status=draft&q=zzz").await;
    assert!(nothing.contains("Nothing matches this filter."));

    let bogus = page("?status=bogus").await;
    assert!(bogus.contains("Alpha") && bogus.contains("Beta"));

    harness
        .repository
        .delete_permanently(delta.id)
        .await
        .unwrap();
    let empty_trash = page("?status=trash").await;
    assert!(empty_trash.contains("The trash is empty."));
}
#[tokio::test]
#[expect(
    clippy::too_many_lines,
    reason = "revision diff and theme reset share one settings fixture"
)]
async fn revision_page_shows_a_line_diff_and_restoring_the_default_theme_needs_csrf() {
    let harness = Harness::new().await;
    let now = Utc::now();
    let created = harness
        .contents
        .create(
            ContentDraft {
                body_markdown: "# Title\n\nfirst line\nsecond line\n".into(),
                ..draft("history")
            },
            SaveIntent::Explicit,
            now,
        )
        .await
        .unwrap();
    let mut edited = created.to_draft();
    edited.body_markdown = "# Title\n\nfirst line\nchanged line\n".into();
    edited.title = "Renamed".into();
    harness
        .contents
        .update(
            created.id,
            created.version,
            edited,
            SaveIntent::Explicit,
            now,
        )
        .await
        .unwrap();
    let revisions = harness.repository.list_revisions(created.id).await.unwrap();
    let oldest = revisions.iter().map(|revision| revision.id).min().unwrap();
    let (cookie, csrf) = harness.session_cookie().await;

    let page = harness
        .send(
            Method::GET,
            &format!("/admin/content/{}/revisions/{oldest}/", created.id),
            None,
            Body::empty(),
            Some(&cookie),
        )
        .await;
    assert_eq!(page.status(), StatusCode::OK);
    let html = text(page).await;
    assert!(html.contains("<pre class=\"diff\""));
    assert!(html.contains("<del>second line</del>"));
    assert!(html.contains("<ins>changed line</ins>"));
    assert!(html.contains("<span>first line</span>"));
    assert!(html.contains("<del>Original title</del> <ins>Renamed</ins>"));

    let settings = harness
        .send(
            Method::GET,
            "/admin/settings/",
            None,
            Body::empty(),
            Some(&cookie),
        )
        .await;
    assert!(
        text(settings)
            .await
            .contains("action=\"/admin/settings/theme/reset/\"")
    );

    let settings_form = serde_urlencoded::to_string([
        ("csrf", csrf.as_str()),
        ("site_title", "Field Notes"),
        ("site_description", ""),
        ("locale", "en"),
        ("logo_media_id", ""),
        ("favicon_media_id", ""),
        ("custom_css", "body { color: teal; }"),
        ("navigation", "Archive | /archive/"),
    ])
    .unwrap();
    let saved = harness
        .send(
            Method::POST,
            "/admin/settings/",
            Some("application/x-www-form-urlencoded"),
            settings_form,
            Some(&cookie),
        )
        .await;
    assert_eq!(saved.status(), StatusCode::SEE_OTHER);

    let forbidden = harness
        .send(
            Method::POST,
            "/admin/settings/theme/reset/",
            Some("application/x-www-form-urlencoded"),
            serde_urlencoded::to_string([("csrf", "wrong")]).unwrap(),
            Some(&cookie),
        )
        .await;
    assert_eq!(forbidden.status(), StatusCode::FORBIDDEN);

    let reset = harness
        .send(
            Method::POST,
            "/admin/settings/theme/reset/",
            Some("application/x-www-form-urlencoded"),
            serde_urlencoded::to_string([("csrf", csrf.as_str())]).unwrap(),
            Some(&cookie),
        )
        .await;
    assert_eq!(reset.status(), StatusCode::SEE_OTHER);
    assert_eq!(reset.headers()[header::LOCATION], "/admin/settings/");
    let stylesheet = harness
        .send(Method::GET, "/assets/site.css", None, Body::empty(), None)
        .await;
    assert_eq!(
        text(stylesheet).await,
        include_str!("../static/default-theme.css")
    );
    let navigation = harness.repository.navigation().await.unwrap();
    assert_eq!(navigation.len(), 1, "navigation survives a theme reset");
}

// ---- Publication failures never masquerade as save failures -----------------

impl Harness {
    /// Makes every object write fail deterministically on Windows and Linux:
    /// `releases/objects` becomes a regular file, so `create_dir_all` errors
    /// while reading the active pointer still reports "no release".
    fn break_release_store(&self) {
        let dir = self._temp.path().join("releases");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("objects"), b"not a directory").unwrap();
    }

    fn heal_release_store(&self) {
        std::fs::remove_file(self._temp.path().join("releases").join("objects")).unwrap();
    }

    fn release_store(&self) -> simple_blog::release::FilesystemReleaseStore {
        simple_blog::release::FilesystemReleaseStore::new(self._temp.path().join("releases"))
    }

    async fn post_json(&self, path: &str, body: String, cookie: &str) -> axum::response::Response {
        let request = Request::builder()
            .method(Method::POST)
            .uri(path)
            .header(header::HOST, "localhost:8080")
            .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
            .header(header::ACCEPT, "application/json")
            .header(header::COOKIE, cookie)
            .body(Body::from(body))
            .unwrap();
        router(self.state.clone()).oneshot(request).await.unwrap()
    }
}

async fn json(response: axum::response::Response) -> serde_json::Value {
    serde_json::from_str(&text(response).await).unwrap()
}

#[tokio::test]
async fn a_failed_publish_after_a_committed_save_answers_success_with_a_pending_site() {
    use simple_blog::{
        application::ports::PublicSnapshotRepository,
        release::{ReleaseReader, ReleaseStore},
    };

    let harness = Harness::new().await;
    let (cookie, csrf) = harness.session_cookie().await;
    harness.break_release_store();

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
    let created = json(created).await;
    assert_eq!(created["site"], "pending");
    let id = created["id"].as_i64().unwrap();
    let version = created["version"].as_i64().unwrap();
    assert!(
        harness
            .repository
            .list_all_content()
            .await
            .unwrap()
            .iter()
            .any(|content| content.slug.as_str() == "typing"),
        "the save must have committed even though publishing failed"
    );

    let updated = harness
        .send(
            Method::POST,
            &format!("/admin/content/{id}/"),
            Some("application/x-www-form-urlencoded"),
            form(&csrf, "Typing more", "typing", Some(version), "autosave"),
            Some(&cookie),
        )
        .await;
    assert_eq!(updated.status(), StatusCode::OK);
    assert_eq!(json(updated).await["site"], "pending");
    assert!(harness.release_store().active().await.unwrap().is_none());

    let dashboard = text(
        harness
            .send(Method::GET, "/admin/", None, Body::empty(), Some(&cookie))
            .await,
    )
    .await;
    assert!(dashboard.contains("class=\"publish-banner\" role=\"status\""));
    assert!(dashboard.contains("action=\"/admin/publish/\""));

    let forbidden = harness
        .send(
            Method::POST,
            "/admin/publish/",
            Some("application/x-www-form-urlencoded"),
            serde_urlencoded::to_string([("csrf", "wrong")]).unwrap(),
            Some(&cookie),
        )
        .await;
    assert_eq!(forbidden.status(), StatusCode::FORBIDDEN);

    harness.heal_release_store();
    let published = harness
        .send(
            Method::POST,
            "/admin/publish/",
            Some("application/x-www-form-urlencoded"),
            serde_urlencoded::to_string([("csrf", csrf.as_str())]).unwrap(),
            Some(&cookie),
        )
        .await;
    assert_eq!(published.status(), StatusCode::SEE_OTHER);
    assert_eq!(published.headers()[header::LOCATION], "/admin/");

    let dashboard = text(
        harness
            .send(Method::GET, "/admin/", None, Body::empty(), Some(&cookie))
            .await,
    )
    .await;
    assert!(!dashboard.contains("publish-banner"));
    let store = harness.release_store();
    let active = store.active().await.unwrap().expect("a release is active");
    let manifest = store.manifest(&active.id).await.unwrap();
    let state = harness.repository.publication_state().await.unwrap();
    assert_eq!(manifest.public_revision, state.revision);
}

#[tokio::test]
async fn other_committed_admin_actions_never_fail_when_publication_fails() {
    let harness = Harness::new().await;
    let (cookie, csrf) = harness.session_cookie().await;
    let content = harness
        .contents
        .create(
            ContentDraft {
                publication: Publication::Public {
                    publish_at: Utc::now(),
                },
                ..draft("resilient")
            },
            SaveIntent::Explicit,
            Utc::now(),
        )
        .await
        .unwrap();
    harness.break_release_store();

    let trashed = harness
        .post_json(
            &format!("/admin/content/{}/trash/", content.id),
            serde_urlencoded::to_string([
                ("csrf", csrf.as_str()),
                ("version", &content.version.to_string()),
            ])
            .unwrap(),
            &cookie,
        )
        .await;
    assert_eq!(trashed.status(), StatusCode::OK);
    let body = json(trashed).await;
    assert_eq!(body["ok"], true);
    assert_eq!(body["site"], "pending");

    let restored = harness
        .post_json(
            &format!("/admin/content/{}/restore/", content.id),
            serde_urlencoded::to_string([("csrf", csrf.as_str())]).unwrap(),
            &cookie,
        )
        .await;
    assert_eq!(restored.status(), StatusCode::OK);
    assert_eq!(json(restored).await["site"], "pending");

    let settings = harness
        .post_json(
            "/admin/settings/",
            serde_urlencoded::to_string([
                ("csrf", csrf.as_str()),
                ("site_title", "Field Notes"),
                ("site_description", ""),
                ("locale", "en"),
                ("logo_media_id", ""),
                ("favicon_media_id", ""),
                ("custom_css", "body { margin: 0; }"),
                ("navigation", ""),
            ])
            .unwrap(),
            &cookie,
        )
        .await;
    assert_eq!(settings.status(), StatusCode::OK);
    assert_eq!(json(settings).await["site"], "pending");

    let reset = harness
        .post_json(
            "/admin/settings/theme/reset/",
            serde_urlencoded::to_string([("csrf", csrf.as_str())]).unwrap(),
            &cookie,
        )
        .await;
    assert_eq!(reset.status(), StatusCode::OK);
    assert_eq!(json(reset).await["site"], "pending");

    let current = harness
        .repository
        .find_by_id(content.id)
        .await
        .unwrap()
        .unwrap();
    let revision = harness
        .repository
        .list_revisions(content.id)
        .await
        .unwrap()
        .into_iter()
        .next()
        .unwrap();
    let restored_revision = harness
        .send(
            Method::POST,
            &format!(
                "/admin/content/{}/revisions/{}/restore/",
                content.id, revision.id
            ),
            Some("application/x-www-form-urlencoded"),
            serde_urlencoded::to_string([
                ("csrf", csrf.as_str()),
                ("version", &current.version.to_string()),
            ])
            .unwrap(),
            Some(&cookie),
        )
        .await;
    assert_eq!(restored_revision.status(), StatusCode::SEE_OTHER);
}

#[tokio::test]
async fn the_scheduler_retries_a_deferred_publication_until_the_store_recovers() {
    use simple_blog::{application::publication::RetrySchedule, release::ReleaseStore};

    let harness = Harness::new().await;
    let (cookie, csrf) = harness.session_cookie().await;
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    let scheduler_state = harness.state.clone();
    let scheduler = tokio::spawn(async move {
        scheduler_state
            .run_publication_scheduler_with(
                shutdown_rx,
                RetrySchedule {
                    initial: std::time::Duration::from_millis(20),
                    second: std::time::Duration::from_millis(40),
                    cap: std::time::Duration::from_millis(80),
                },
            )
            .await;
    });

    harness.break_release_store();
    let created = harness
        .send(
            Method::POST,
            "/admin/content/",
            Some("application/x-www-form-urlencoded"),
            form(&csrf, "Deferred", "deferred", None, "autosave"),
            Some(&cookie),
        )
        .await;
    assert_eq!(created.status(), StatusCode::CREATED);
    assert_eq!(json(created).await["site"], "pending");
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    assert!(harness.release_store().active().await.unwrap().is_none());

    harness.heal_release_store();
    let store = harness.release_store();
    let mut healed = false;
    for _ in 0..100 {
        if store.active().await.unwrap().is_some() {
            healed = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    shutdown_tx.send(true).unwrap();
    scheduler.await.unwrap();
    assert!(healed, "the scheduler must publish once the store recovers");
}

// ---- Admin assets and session lifetime ------------------------------------

#[tokio::test]
async fn admin_assets_are_fingerprinted_and_immutable() {
    let harness = Harness::new().await;
    let login = text(
        harness
            .send(Method::GET, "/admin/login/", None, Body::empty(), None)
            .await,
    )
    .await;
    let css_at = login
        .find("/admin/assets/admin.css?v=")
        .expect("stylesheet link carries a version");
    let version = &login[css_at + "/admin/assets/admin.css?v=".len()..][..16];
    assert!(version.chars().all(|c| c.is_ascii_hexdigit()), "{version}");
    assert!(login.contains(&format!("/admin/assets/admin.js?v={version}")));

    for path in [
        format!("/admin/assets/admin.css?v={version}"),
        format!("/admin/assets/admin.js?v={version}"),
    ] {
        let asset = harness
            .send(Method::GET, &path, None, Body::empty(), None)
            .await;
        assert_eq!(asset.status(), StatusCode::OK);
        let cache = asset.headers()[header::CACHE_CONTROL].to_str().unwrap();
        assert!(cache.contains("immutable"), "{path}: {cache}");
        assert!(cache.contains("max-age=31536000"), "{path}: {cache}");
    }
}

#[tokio::test]
async fn sessions_extend_on_use_without_changing_csrf() {
    let start = Utc::now();
    let clock = TestClock::new(start);
    let harness = Harness::new_with_clock(Arc::new(clock.clone())).await;
    let (cookie, csrf) = harness.session_cookie().await;
    let session_token = cookie
        .split(';')
        .find_map(|part| part.trim().strip_prefix("sb_session="))
        .unwrap()
        .to_owned();

    // Fresh sessions are left alone: no cookie churn on every page.
    clock.set(start + Duration::hours(1));
    let fresh = harness
        .send(Method::GET, "/admin/", None, Body::empty(), Some(&cookie))
        .await;
    assert_eq!(fresh.status(), StatusCode::OK);
    assert_eq!(
        fresh.headers().get_all(header::SET_COOKIE).iter().count(),
        0
    );

    // After a day of use the same tokens are re-issued for another week.
    clock.set(start + Duration::days(2));
    let renewed = harness
        .send(Method::GET, "/admin/", None, Body::empty(), Some(&cookie))
        .await;
    assert_eq!(renewed.status(), StatusCode::OK);
    let cookies: Vec<&str> = renewed
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .map(|value| value.to_str().unwrap())
        .collect();
    assert_eq!(cookies.len(), 2, "{cookies:?}");
    assert!(
        cookies
            .iter()
            .any(|c| c.starts_with(&format!("sb_session={session_token};"))
                && c.contains("Max-Age=604800"))
    );
    assert!(
        cookies
            .iter()
            .any(|c| c.starts_with(&format!("sb_csrf={csrf};")) && c.contains("Max-Age=604800"))
    );

    // Without the extension the session would have died on day seven.
    clock.set(start + Duration::days(8));
    let still_valid = harness
        .send(Method::GET, "/admin/", None, Body::empty(), Some(&cookie))
        .await;
    assert_eq!(still_valid.status(), StatusCode::OK);

    // Neglect still ends it: nothing renewed it after day eight.
    clock.set(start + Duration::days(15) + Duration::hours(1));
    let expired = harness
        .send(Method::GET, "/admin/", None, Body::empty(), Some(&cookie))
        .await;
    assert_eq!(expired.status(), StatusCode::SEE_OTHER);
}

// ---- Site zone and author ---------------------------------------------------

fn settings_form_with(csrf: &str, timezone: &str, author_name: &str, custom_css: &str) -> String {
    serde_urlencoded::to_string([
        ("csrf", csrf),
        ("site_title", "Field Notes"),
        ("site_description", ""),
        ("locale", "en"),
        ("logo_media_id", ""),
        ("favicon_media_id", ""),
        ("custom_css", custom_css),
        ("navigation", ""),
        ("timezone", timezone),
        ("author_name", author_name),
    ])
    .unwrap()
}

#[tokio::test]
async fn settings_form_round_trips_timezone_and_author_and_rejects_unknown_zones() {
    let harness = Harness::new().await;
    let (cookie, csrf) = harness.session_cookie().await;
    let page = text(
        harness
            .send(
                Method::GET,
                "/admin/settings/",
                None,
                Body::empty(),
                Some(&cookie),
            )
            .await,
    )
    .await;
    assert!(page.contains("name=\"timezone\""));
    assert!(page.contains("<optgroup label=\"Asia\">"));
    // minijinja escapes the slash inside attribute values.
    assert!(page.contains("value=\"Asia&#x2f;Tokyo\""));
    assert!(page.contains("name=\"author_name\""));

    let saved = harness
        .send(
            Method::POST,
            "/admin/settings/",
            Some("application/x-www-form-urlencoded"),
            settings_form_with(&csrf, "Asia/Tokyo", "Ryo", "body {}"),
            Some(&cookie),
        )
        .await;
    assert_eq!(saved.status(), StatusCode::SEE_OTHER);
    let stored = harness.repository.site_settings().await.unwrap();
    assert_eq!(stored.timezone, "Asia/Tokyo");
    assert_eq!(stored.author_name, "Ryo");

    harness
        .contents
        .create(
            ContentDraft {
                publication: Publication::Public {
                    publish_at: Utc::now(),
                },
                ..draft("tokyo-time")
            },
            SaveIntent::Explicit,
            Utc::now(),
        )
        .await
        .unwrap();
    harness.state.publish_now().await.unwrap();
    let article = text(
        harness
            .send(Method::GET, "/tokyo-time/", None, Body::empty(), None)
            .await,
    )
    .await;
    assert!(
        article.contains("+09:00\""),
        "public dates carry the site offset"
    );
    let editor = text(
        harness
            .send(
                Method::GET,
                "/admin/content/new/",
                None,
                Body::empty(),
                Some(&cookie),
            )
            .await,
    )
    .await;
    assert!(editor.contains("data-site-zone=\"Asia&#x2f;Tokyo\""));
    assert!(editor.contains("data-site-zone-hint"));

    let rejected = harness
        .send(
            Method::POST,
            "/admin/settings/",
            Some("application/x-www-form-urlencoded"),
            settings_form_with(&csrf, "Mars/Olympus", "Ryo", "body {}"),
            Some(&cookie),
        )
        .await;
    assert_eq!(rejected.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        harness.repository.site_settings().await.unwrap().timezone,
        "Asia/Tokyo"
    );
}

#[tokio::test]
async fn theme_reset_keeps_a_one_slot_undo() {
    let harness = Harness::new().await;
    let (cookie, csrf) = harness.session_cookie().await;
    let post = |path: &'static str, body: String| {
        harness.send(
            Method::POST,
            path,
            Some("application/x-www-form-urlencoded"),
            body,
            Some(&cookie),
        )
    };
    let csrf_only = || serde_urlencoded::to_string([("csrf", csrf.as_str())]).unwrap();

    assert_eq!(
        post(
            "/admin/settings/",
            settings_form_with(&csrf, "UTC", "", "body { color: teal; }"),
        )
        .await
        .status(),
        StatusCode::SEE_OTHER
    );
    let nothing_to_undo = post("/admin/settings/theme/undo/", csrf_only()).await;
    assert_eq!(nothing_to_undo.status(), StatusCode::NOT_FOUND);

    assert_eq!(
        post("/admin/settings/theme/reset/", csrf_only())
            .await
            .status(),
        StatusCode::SEE_OTHER
    );
    let stored = harness.repository.site_settings().await.unwrap();
    assert_eq!(
        stored.custom_css_backup.as_deref(),
        Some("body { color: teal; }")
    );
    assert_eq!(
        stored.custom_css,
        include_str!("../static/default-theme.css")
    );

    // A plain settings save keeps the backup around.
    assert_eq!(
        post(
            "/admin/settings/",
            settings_form_with(&csrf, "UTC", "", ".a { margin: 0; }"),
        )
        .await
        .status(),
        StatusCode::SEE_OTHER
    );
    assert_eq!(
        harness
            .repository
            .site_settings()
            .await
            .unwrap()
            .custom_css_backup
            .as_deref(),
        Some("body { color: teal; }")
    );

    let undone = post("/admin/settings/theme/undo/", csrf_only()).await;
    assert_eq!(undone.status(), StatusCode::SEE_OTHER);
    assert_eq!(undone.headers()[header::LOCATION], "/admin/settings/");
    let stored = harness.repository.site_settings().await.unwrap();
    assert_eq!(stored.custom_css, "body { color: teal; }");
    assert_eq!(stored.custom_css_backup, None);
    let stylesheet = text(
        harness
            .send(Method::GET, "/assets/site.css", None, Body::empty(), None)
            .await,
    )
    .await;
    assert_eq!(stylesheet, "body { color: teal; }");
}

// ---- Preview through the public theme, and shareable links -----------------

#[tokio::test]
#[expect(
    clippy::too_many_lines,
    reason = "the preview, its framing policy and its assets in one scenario"
)]
async fn owner_preview_renders_the_current_draft_through_the_public_theme() {
    let harness = Harness::new().await;
    let (cookie, _csrf) = harness.session_cookie().await;
    let piece = harness
        .contents
        .create(
            ContentDraft {
                body_markdown: "First draft".into(),
                ..draft("previewed")
            },
            SaveIntent::Explicit,
            Utc::now(),
        )
        .await
        .unwrap();
    let path = format!("/admin/content/{}/preview/", piece.id);

    let anonymous = harness
        .send(Method::GET, &path, None, Body::empty(), None)
        .await;
    assert_eq!(anonymous.status(), StatusCode::SEE_OTHER);

    let preview = harness
        .send(Method::GET, &path, None, Body::empty(), Some(&cookie))
        .await;
    assert_eq!(preview.status(), StatusCode::OK);
    assert!(
        preview.headers()[header::CONTENT_TYPE]
            .to_str()
            .unwrap()
            .starts_with("text/html")
    );
    let csp = preview.headers()[header::CONTENT_SECURITY_POLICY]
        .to_str()
        .unwrap()
        .to_owned();
    assert!(csp.contains("frame-ancestors 'self'"), "{csp}");
    assert_eq!(preview.headers()[header::CACHE_CONTROL], "no-store");
    let html = text(preview).await;
    assert!(html.contains("<article class=\"prose-shell\""));
    assert!(html.contains("<h1 itemprop=\"headline\">Original title</h1>"));
    assert!(html.contains("First draft"));
    assert!(html.contains("name=\"robots\" content=\"noindex\""));
    assert!(html.contains("&#x2f;admin&#x2f;assets&#x2f;theme.css?v="));
    assert!(!html.contains("like.js"));

    // The dashboard keeps forbidding framing entirely.
    let dashboard = harness
        .send(Method::GET, "/admin/", None, Body::empty(), Some(&cookie))
        .await;
    assert!(
        dashboard.headers()[header::CONTENT_SECURITY_POLICY]
            .to_str()
            .unwrap()
            .contains("frame-ancestors 'none'")
    );

    // A newer save shows immediately: nothing here is a compiled release.
    harness
        .contents
        .update(
            piece.id,
            piece.version,
            ContentDraft {
                body_markdown: "Second draft".into(),
                ..draft("previewed")
            },
            SaveIntent::Autosave,
            Utc::now(),
        )
        .await
        .unwrap();
    let refreshed = text(
        harness
            .send(Method::GET, &path, None, Body::empty(), Some(&cookie))
            .await,
    )
    .await;
    assert!(refreshed.contains("Second draft"));

    let css = harness
        .send(
            Method::GET,
            "/admin/assets/theme.css",
            None,
            Body::empty(),
            None,
        )
        .await;
    assert_eq!(css.status(), StatusCode::OK);
    assert!(
        css.headers()[header::CONTENT_TYPE]
            .to_str()
            .unwrap()
            .starts_with("text/css")
    );
    assert_eq!(css.headers()[header::CACHE_CONTROL], "no-store");
    assert_eq!(
        text(css).await,
        harness.repository.site_settings().await.unwrap().custom_css
    );
    let prefs = harness
        .send(
            Method::GET,
            "/admin/assets/prefs.js",
            None,
            Body::empty(),
            None,
        )
        .await;
    assert_eq!(prefs.status(), StatusCode::OK);
}

/// A session issued at a chosen instant, for scenarios driven by the test clock.
async fn session_at(harness: &Harness, at: DateTime<Utc>) -> (String, String) {
    let session = harness.auth.create_session(at).await.unwrap();
    (
        format!(
            "sb_session={}; sb_csrf={}",
            session.session.expose(),
            session.csrf.expose()
        ),
        session.csrf.expose().to_owned(),
    )
}

#[tokio::test]
#[expect(
    clippy::too_many_lines,
    reason = "the whole life of a preview link in one scenario"
)]
async fn share_links_are_hashed_capabilities_with_a_seven_day_ttl() {
    let start = Utc::now();
    let clock = TestClock::new(start);
    let harness = Harness::new_with_clock(Arc::new(clock.clone())).await;
    let (cookie, csrf) = session_at(&harness, start).await;
    let piece = harness
        .contents
        .create(draft("shared"), SaveIntent::Explicit, start)
        .await
        .unwrap();
    let share_path = format!("/admin/content/{}/share/", piece.id);

    let forbidden = harness
        .send(
            Method::POST,
            &share_path,
            Some("application/x-www-form-urlencoded"),
            serde_urlencoded::to_string([("csrf", "wrong")]).unwrap(),
            Some(&cookie),
        )
        .await;
    assert_eq!(forbidden.status(), StatusCode::FORBIDDEN);

    let issued = harness
        .post_json(
            &share_path,
            serde_urlencoded::to_string([("csrf", csrf.as_str())]).unwrap(),
            &cookie,
        )
        .await;
    assert_eq!(issued.status(), StatusCode::OK);
    let link = json(issued).await;
    let url = link["url"].as_str().unwrap().to_owned();
    let token = url
        .strip_prefix("/admin/share/")
        .and_then(|rest| rest.strip_suffix('/'))
        .expect("a share path");
    assert_eq!(token.len(), 43, "{url}");
    assert_eq!(
        link["expires_at"].as_str().unwrap(),
        (start + Duration::days(7)).to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
    );

    // Anyone holding the link reads the draft; no session, no index.
    let shared = harness
        .send(Method::GET, &url, None, Body::empty(), None)
        .await;
    assert_eq!(shared.status(), StatusCode::OK);
    let html = text(shared).await;
    assert!(html.contains("<h1 itemprop=\"headline\">Original title</h1>"));
    assert!(html.contains("name=\"robots\" content=\"noindex\""));

    let garbage = harness
        .send(
            Method::GET,
            "/admin/share/not-a-token/",
            None,
            Body::empty(),
            None,
        )
        .await;
    assert_eq!(garbage.status(), StatusCode::NOT_FOUND);
    assert!(text(garbage).await.contains("expired"));

    clock.set(start + Duration::days(8));
    let expired = harness
        .send(Method::GET, &url, None, Body::empty(), None)
        .await;
    assert_eq!(expired.status(), StatusCode::NOT_FOUND);

    // Revocation ends every link of the piece at once.
    let (cookie, csrf) = session_at(&harness, start + Duration::days(8)).await;
    let second = json(
        harness
            .post_json(
                &share_path,
                serde_urlencoded::to_string([("csrf", csrf.as_str())]).unwrap(),
                &cookie,
            )
            .await,
    )
    .await;
    let second_url = second["url"].as_str().unwrap().to_owned();
    assert_eq!(
        harness
            .send(Method::GET, &second_url, None, Body::empty(), None)
            .await
            .status(),
        StatusCode::OK
    );
    let revoked = harness
        .post_json(
            &format!("/admin/content/{}/share/revoke/", piece.id),
            serde_urlencoded::to_string([("csrf", csrf.as_str())]).unwrap(),
            &cookie,
        )
        .await;
    assert_eq!(revoked.status(), StatusCode::OK);
    assert_eq!(json(revoked).await["ok"], true);
    assert_eq!(
        harness
            .send(Method::GET, &second_url, None, Body::empty(), None)
            .await
            .status(),
        StatusCode::NOT_FOUND
    );

    // Deleting the piece permanently takes its links with it.
    let third = json(
        harness
            .post_json(
                &share_path,
                serde_urlencoded::to_string([("csrf", csrf.as_str())]).unwrap(),
                &cookie,
            )
            .await,
    )
    .await;
    let third_url = third["url"].as_str().unwrap().to_owned();
    let current = harness
        .repository
        .find_by_id(piece.id)
        .await
        .unwrap()
        .unwrap();
    harness
        .repository
        .move_to_trash(piece.id, current.version, start + Duration::days(8))
        .await
        .unwrap();
    harness
        .repository
        .delete_permanently(piece.id)
        .await
        .unwrap();
    assert_eq!(
        harness
            .send(Method::GET, &third_url, None, Body::empty(), None)
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn editor_exposes_the_preview_frame_and_share_controls() {
    let harness = Harness::new().await;
    let (cookie, _csrf) = harness.session_cookie().await;
    let piece = harness
        .contents
        .create(draft("framed"), SaveIntent::Explicit, Utc::now())
        .await
        .unwrap();
    let editor = text(
        harness
            .send(
                Method::GET,
                &format!("/admin/content/{}/edit/", piece.id),
                None,
                Body::empty(),
                Some(&cookie),
            )
            .await,
    )
    .await;
    for marker in [
        "data-preview-frame",
        &format!("data-preview-url=\"/admin/content/{}/preview/\"", piece.id),
        "data-share-form",
        &format!("action=\"/admin/content/{}/share/\"", piece.id),
        &format!("action=\"/admin/content/{}/share/revoke/\"", piece.id),
        "data-msg-share-copied",
        "data-msg-share-expires",
    ] {
        assert!(editor.contains(marker), "missing {marker}");
    }
    assert!(!editor.contains("data-preview-output"));

    let fresh = text(
        harness
            .send(
                Method::GET,
                "/admin/content/new/",
                None,
                Body::empty(),
                Some(&cookie),
            )
            .await,
    )
    .await;
    assert!(fresh.contains("data-preview-frame"));
    assert!(!fresh.contains("data-preview-url"));
    assert!(fresh.contains("data-preview-note"));
    assert!(!fresh.contains("data-share-form"));
}

#[tokio::test]
async fn editor_exposes_local_draft_markers_and_the_server_timestamp() {
    let harness = Harness::new().await;
    let (cookie, _csrf) = harness.session_cookie().await;
    let piece = harness
        .contents
        .create(draft("kept"), SaveIntent::Explicit, Utc::now())
        .await
        .unwrap();
    let editor = text(
        harness
            .send(
                Method::GET,
                &format!("/admin/content/{}/edit/", piece.id),
                None,
                Body::empty(),
                Some(&cookie),
            )
            .await,
    )
    .await;
    let stamp = piece
        .updated_at
        .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    for marker in [
        format!("data-updated-at=\"{stamp}\""),
        format!("data-content-id=\"{}\"", piece.id),
        "class=\"local-draft-bar\" data-local-draft role=\"status\"".to_owned(),
        "data-local-draft-restore".to_owned(),
        "data-local-draft-discard".to_owned(),
        "data-msg-local-draft".to_owned(),
        "data-msg-local-restore".to_owned(),
        "data-msg-local-discard".to_owned(),
    ] {
        assert!(editor.contains(&marker), "missing {marker}");
    }

    let fresh = text(
        harness
            .send(
                Method::GET,
                "/admin/content/new/",
                None,
                Body::empty(),
                Some(&cookie),
            )
            .await,
    )
    .await;
    assert!(fresh.contains("data-updated-at=\"\""));
    assert!(!fresh.contains("data-content-id="));
}

#[tokio::test]
async fn editor_offers_focus_mode_and_a_fuller_shortcut_legend() {
    let harness = Harness::new().await;
    let (cookie, _csrf) = harness.session_cookie().await;
    let editor = text(
        harness
            .send(
                Method::GET,
                "/admin/content/new/",
                None,
                Body::empty(),
                Some(&cookie),
            )
            .await,
    )
    .await;
    for marker in [
        "data-focus-toggle",
        "data-msg-focus",
        "data-msg-focus-exit",
        "data-msg-uploading",
        "data-shortcuts",
    ] {
        assert!(editor.contains(marker), "missing {marker}");
    }
    let legend_at = editor.find("data-msg-shortcuts=\"").unwrap() + "data-msg-shortcuts=\"".len();
    let legend = &editor[legend_at..editor[legend_at..].find('"').unwrap() + legend_at];
    for key in ["{mod}+B", "{mod}+I", "{mod}+K", "{mod}+S"] {
        assert!(legend.contains(key), "legend lacks {key}: {legend}");
    }
}
