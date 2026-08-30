use std::sync::Arc;

use chrono::{Duration, TimeZone, Utc};
use simple_blog::{
    application::{
        content::{ContentService, SaveIntent},
        ports::{ContentRepository, RepositoryError},
    },
    domain::content::{ContentDraft, ContentKind, Publication, Slug},
    infrastructure::{markdown::ComrakMarkdownRenderer, sqlite::SqliteRepository},
};
use tempfile::TempDir;

async fn harness() -> (TempDir, Arc<SqliteRepository>, ContentService) {
    let temp = tempfile::tempdir().expect("temp directory");
    let repository = Arc::new(
        SqliteRepository::connect(&temp.path().join("blog.sqlite3"))
            .await
            .expect("database"),
    );
    let service = ContentService::new(
        repository.clone(),
        Arc::new(ComrakMarkdownRenderer::default()),
    );
    (temp, repository, service)
}

fn post(slug: &str, publication: Publication) -> ContentDraft {
    ContentDraft {
        kind: ContentKind::Post,
        title: "A carefully written post".into(),
        slug: Slug::parse(slug).unwrap(),
        summary: "A short summary".into(),
        body_markdown: "# Heading\n\nBody **copy**.".into(),
        tags: vec!["Rust".into(), "Writing Tools".into()],
        cover_media_id: None,
        seo_title: None,
        seo_description: None,
        publication,
    }
}

#[tokio::test]
async fn migration_enables_safe_sqlite_pragmas() {
    let (_temp, repository, _service) = harness().await;

    let pragmas = repository.pragmas().await.expect("pragmas");
    assert!(pragmas.foreign_keys);
    assert_eq!(pragmas.journal_mode.to_ascii_lowercase(), "wal");
    assert!(pragmas.busy_timeout_ms >= 5_000);
}

#[tokio::test]
async fn canonical_markdown_and_safe_html_are_saved_together() {
    let (_temp, repository, service) = harness().await;
    let now = Utc.with_ymd_and_hms(2026, 8, 30, 12, 0, 0).unwrap();

    let created = service
        .create(
            post("first-post", Publication::Public { publish_at: now }),
            SaveIntent::Explicit,
            now,
        )
        .await
        .expect("create post");

    assert_eq!(created.version, 1);
    assert_eq!(created.body_markdown, "# Heading\n\nBody **copy**.");
    assert!(created.body_html.contains("<strong>copy</strong>"));
    assert_eq!(created.tags[0].slug.as_str(), "rust");
    assert_eq!(created.tags[1].slug.as_str(), "writing-tools");

    let public = repository
        .find_public_by_slug(&Slug::parse("first-post").unwrap(), now)
        .await
        .expect("query")
        .expect("visible post");
    assert_eq!(public.id, created.id);
}

#[tokio::test]
async fn malformed_cover_media_identity_is_rejected_before_storage() {
    let (_temp, _repository, service) = harness().await;
    let mut draft = post("invalid-cover", Publication::Draft);
    draft.cover_media_id = Some("../../config.toml".into());
    let error = service
        .create(draft, SaveIntent::Explicit, Utc::now())
        .await
        .unwrap_err();
    assert!(matches!(error, RepositoryError::Validation(_)));
}

#[tokio::test]
async fn scheduled_posts_are_selected_by_request_time() {
    let (_temp, repository, service) = harness().await;
    let now = Utc.with_ymd_and_hms(2026, 8, 30, 12, 0, 0).unwrap();
    let publish_at = now + Duration::minutes(10);
    service
        .create(
            post("scheduled", Publication::Public { publish_at }),
            SaveIntent::Explicit,
            now,
        )
        .await
        .unwrap();

    assert!(
        repository
            .find_public_by_slug(&Slug::parse("scheduled").unwrap(), now)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        repository
            .find_public_by_slug(&Slug::parse("scheduled").unwrap(), publish_at)
            .await
            .unwrap()
            .is_some()
    );
}

#[tokio::test]
async fn stale_autosave_returns_conflict_without_overwriting() {
    let (_temp, repository, service) = harness().await;
    let now = Utc::now();
    let created = service
        .create(
            post("locking", Publication::Draft),
            SaveIntent::Explicit,
            now,
        )
        .await
        .unwrap();
    let mut first_tab = created.to_draft();
    first_tab.title = "Saved in the first tab".into();
    service
        .update(
            created.id,
            created.version,
            first_tab,
            SaveIntent::Autosave,
            now,
        )
        .await
        .unwrap();

    let mut second_tab = created.to_draft();
    second_tab.title = "Stale second tab".into();
    let error = service
        .update(
            created.id,
            created.version,
            second_tab,
            SaveIntent::Autosave,
            now,
        )
        .await
        .expect_err("must conflict");
    assert!(matches!(error, RepositoryError::Conflict { .. }));

    let stored = repository.find_by_id(created.id).await.unwrap().unwrap();
    assert_eq!(stored.title, "Saved in the first tab");
}

#[tokio::test]
async fn explicit_saves_and_pruned_autosaves_form_a_revision_timeline() {
    let (_temp, repository, service) = harness().await;
    let start = Utc::now();
    let mut content = service
        .create(
            post("history", Publication::Draft),
            SaveIntent::Explicit,
            start,
        )
        .await
        .unwrap();

    for number in 0..55 {
        let mut draft = content.to_draft();
        draft.body_markdown = format!("autosave {number}");
        content = service
            .update(
                content.id,
                content.version,
                draft,
                SaveIntent::Autosave,
                start + Duration::seconds(number),
            )
            .await
            .unwrap();
    }
    let mut draft = content.to_draft();
    draft.body_markdown = "explicit checkpoint".into();
    service
        .update(
            content.id,
            content.version,
            draft,
            SaveIntent::Explicit,
            start + Duration::minutes(2),
        )
        .await
        .unwrap();

    let revisions = repository.list_revisions(content.id).await.unwrap();
    assert_eq!(
        revisions
            .iter()
            .filter(|r| r.intent == SaveIntent::Autosave)
            .count(),
        50
    );
    assert_eq!(
        revisions
            .iter()
            .filter(|r| r.intent == SaveIntent::Explicit)
            .count(),
        2
    );
}

#[tokio::test]
async fn changing_slug_atomically_creates_a_redirect_to_the_current_slug() {
    let (_temp, repository, service) = harness().await;
    let now = Utc::now();
    let created = service
        .create(
            post("old-address", Publication::Public { publish_at: now }),
            SaveIntent::Explicit,
            now,
        )
        .await
        .unwrap();
    let mut draft = created.to_draft();
    draft.slug = Slug::parse("new-address").unwrap();
    service
        .update(
            created.id,
            created.version,
            draft,
            SaveIntent::Explicit,
            now,
        )
        .await
        .unwrap();

    let target = repository
        .resolve_redirect(&Slug::parse("old-address").unwrap())
        .await
        .unwrap();
    assert_eq!(target.as_ref().map(Slug::as_str), Some("new-address"));
}

#[tokio::test]
async fn restoring_a_revision_is_an_explicit_optimistically_locked_save() {
    let (_temp, repository, service) = harness().await;
    let now = Utc::now();
    let first = service
        .create(
            post("restorable", Publication::Draft),
            SaveIntent::Explicit,
            now,
        )
        .await
        .unwrap();
    let first_revision = repository.list_revisions(first.id).await.unwrap()[0].id;
    let mut changed = first.to_draft();
    changed.title = "Changed title".into();
    changed.body_markdown = "Changed body".into();
    let changed = service
        .update(
            first.id,
            first.version,
            changed,
            SaveIntent::Explicit,
            now + Duration::seconds(1),
        )
        .await
        .unwrap();

    let restored = service
        .restore_revision(
            first.id,
            first_revision,
            changed.version,
            now + Duration::seconds(2),
        )
        .await
        .unwrap();
    assert_eq!(restored.title, first.title);
    assert_eq!(restored.body_markdown, first.body_markdown);
    assert_eq!(restored.version, changed.version + 1);
    assert_eq!(
        repository.list_revisions(first.id).await.unwrap()[0].intent,
        SaveIntent::Explicit
    );

    let error = service
        .restore_revision(
            first.id,
            first_revision,
            changed.version,
            now + Duration::seconds(3),
        )
        .await
        .unwrap_err();
    assert!(matches!(error, RepositoryError::Conflict { .. }));
}

#[tokio::test]
async fn reverting_to_an_own_historical_slug_keeps_only_the_other_slug_as_a_redirect() {
    let (_temp, repository, service) = harness().await;
    let now = Utc::now();
    let first = service
        .create(
            post("first-address", Publication::Public { publish_at: now }),
            SaveIntent::Explicit,
            now,
        )
        .await
        .unwrap();
    let mut second = first.to_draft();
    second.slug = Slug::parse("second-address").unwrap();
    let second = service
        .update(first.id, first.version, second, SaveIntent::Explicit, now)
        .await
        .unwrap();
    let mut reverted = second.to_draft();
    reverted.slug = Slug::parse("first-address").unwrap();
    service
        .update(
            second.id,
            second.version,
            reverted,
            SaveIntent::Explicit,
            now,
        )
        .await
        .unwrap();

    assert_eq!(
        repository
            .resolve_redirect(&Slug::parse("second-address").unwrap())
            .await
            .unwrap(),
        Some(Slug::parse("first-address").unwrap())
    );
    assert_eq!(
        repository
            .resolve_redirect(&Slug::parse("first-address").unwrap())
            .await
            .unwrap(),
        None
    );
}

#[tokio::test]
async fn active_and_historical_slugs_are_globally_unique() {
    let (_temp, _repository, service) = harness().await;
    let now = Utc::now();
    let first = service
        .create(
            post("unique", Publication::Draft),
            SaveIntent::Explicit,
            now,
        )
        .await
        .unwrap();
    let duplicate = service
        .create(
            post("unique", Publication::Draft),
            SaveIntent::Explicit,
            now,
        )
        .await;
    assert!(matches!(duplicate, Err(RepositoryError::SlugTaken(_))));

    let mut renamed = first.to_draft();
    renamed.slug = Slug::parse("renamed").unwrap();
    service
        .update(first.id, first.version, renamed, SaveIntent::Explicit, now)
        .await
        .unwrap();
    let historical_duplicate = service
        .create(
            post("unique", Publication::Draft),
            SaveIntent::Explicit,
            now,
        )
        .await;
    assert!(matches!(
        historical_duplicate,
        Err(RepositoryError::SlugTaken(_))
    ));
}
