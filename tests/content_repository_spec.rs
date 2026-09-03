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
    // Public, so the rename below leaves a historical address behind.
    let first = service
        .create(
            post("unique", Publication::Public { publish_at: now }),
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

#[tokio::test]
async fn listing_many_pieces_keeps_every_tag_in_position_order() {
    let (_temp, repository, service) = harness().await;
    let now = Utc.with_ymd_and_hms(2026, 9, 3, 9, 0, 0).unwrap();
    for index in 0..30 {
        service
            .create(
                ContentDraft {
                    tags: vec![
                        format!("Tag {index}"),
                        "Rust".into(),
                        "Writing Tools".into(),
                    ],
                    ..post(
                        &format!("post-{index}"),
                        Publication::Public {
                            publish_at: now - Duration::minutes(index),
                        },
                    )
                },
                SaveIntent::Explicit,
                now,
            )
            .await
            .expect("create post");
    }

    let everything = repository.list_all_content().await.expect("list all");
    let public = repository.list_all_public(now).await.expect("list public");
    let by_tag = repository
        .list_public_by_tag(&Slug::parse("rust").unwrap(), now)
        .await
        .expect("list by tag");
    let recent = repository
        .list_public_posts(now, 10, 5)
        .await
        .expect("list posts");

    assert_eq!(everything.len(), 30);
    assert_eq!(public.len(), 30);
    assert_eq!(by_tag.len(), 30);
    assert_eq!(recent.len(), 10);
    assert_eq!(recent[0].slug.as_str(), "post-5");
    for content in everything
        .iter()
        .chain(&public)
        .chain(&by_tag)
        .chain(&recent)
    {
        let names: Vec<&str> = content.tags.iter().map(|tag| tag.name.as_str()).collect();
        assert!(names[0].starts_with("Tag "), "{names:?}");
        assert_eq!(&names[1..], ["Rust", "Writing Tools"], "{names:?}");
        assert_eq!(content.tags[1].slug.as_str(), "rust");
    }
}
#[tokio::test]
async fn trashed_content_leaves_every_public_query_but_keeps_its_slug() {
    use simple_blog::application::ports::{LikeRepository, SearchRepository};
    use simple_blog::domain::search::parse_query;

    let (_temp, repository, service) = harness().await;
    let now = Utc.with_ymd_and_hms(2026, 9, 3, 10, 0, 0).unwrap();
    let created = service
        .create(
            post("first-post", Publication::Public { publish_at: now }),
            SaveIntent::Explicit,
            now,
        )
        .await
        .expect("create post");
    let sibling = service
        .create(
            post(
                "second-post",
                Publication::Public {
                    publish_at: now + Duration::minutes(1),
                },
            ),
            SaveIntent::Explicit,
            now,
        )
        .await
        .expect("create sibling");

    let trashed = repository
        .move_to_trash(created.id, created.version, now + Duration::minutes(2))
        .await
        .expect("trash");
    assert_eq!(trashed.deleted_at, Some(now + Duration::minutes(2)));
    assert_eq!(trashed.version, created.version + 1);
    assert!(trashed.is_trashed());

    let later = now + Duration::minutes(3);
    let slug = Slug::parse("first-post").unwrap();
    assert!(
        repository
            .find_public_by_slug(&slug, later)
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(
        repository
            .list_public_posts(later, 10, 0)
            .await
            .unwrap()
            .len(),
        1
    );
    assert_eq!(repository.list_all_public(later).await.unwrap().len(), 1);
    assert_eq!(
        repository
            .list_public_by_tag(&Slug::parse("rust").unwrap(), later)
            .await
            .unwrap()
            .len(),
        1
    );
    assert!(
        repository
            .search(&parse_query("Heading"), later, 10)
            .await
            .unwrap()
            .iter()
            .all(|hit| hit.slug != slug)
    );
    assert_eq!(
        repository.add_like(created.id, later).await,
        Err(RepositoryError::NotFound)
    );
    let (older, newer) = repository
        .neighbor_posts(sibling.id, now + Duration::minutes(1), later)
        .await
        .unwrap();
    assert!(
        older.is_none(),
        "a trashed post must not appear as a neighbour"
    );
    assert!(newer.is_none());

    // The slug stays reserved for the trashed piece.
    let reuse = service
        .create(
            post("first-post", Publication::Draft),
            SaveIntent::Explicit,
            later,
        )
        .await;
    assert!(matches!(reuse, Err(RepositoryError::SlugTaken(_))));

    let everything = repository.list_all_content().await.unwrap();
    assert_eq!(everything.len(), 2);
    assert!(
        everything
            .iter()
            .any(simple_blog::domain::content::Content::is_trashed)
    );
}

#[tokio::test]
async fn trash_requires_the_current_version_and_refuses_edits_until_restored() {
    let (_temp, repository, service) = harness().await;
    let now = Utc.with_ymd_and_hms(2026, 9, 3, 10, 0, 0).unwrap();
    let created = service
        .create(
            post("guarded", Publication::Draft),
            SaveIntent::Explicit,
            now,
        )
        .await
        .expect("create");

    let stale = repository
        .move_to_trash(created.id, created.version + 7, now)
        .await;
    assert!(matches!(stale, Err(RepositoryError::Conflict { .. })));

    let trashed = repository
        .move_to_trash(created.id, created.version, now)
        .await
        .expect("trash");
    // Trashing twice is a harmless no-op.
    let again = repository
        .move_to_trash(created.id, trashed.version, now)
        .await
        .expect("idempotent trash");
    assert_eq!(again.version, trashed.version);

    let edit = service
        .update(
            created.id,
            trashed.version,
            created.to_draft(),
            SaveIntent::Explicit,
            now,
        )
        .await;
    match edit {
        Err(RepositoryError::Validation(message)) => assert!(message.contains("trash")),
        other => panic!("trashed content must refuse edits, got {other:?}"),
    }

    let restored = repository
        .restore_from_trash(created.id, now + Duration::minutes(1))
        .await
        .expect("restore");
    assert_eq!(restored.deleted_at, None);
    assert_eq!(restored.version, trashed.version + 1);
    let restored_again = repository
        .restore_from_trash(created.id, now + Duration::minutes(2))
        .await
        .expect("restore is idempotent");
    assert_eq!(restored_again.version, restored.version);

    let edited = service
        .update(
            created.id,
            restored.version,
            created.to_draft(),
            SaveIntent::Explicit,
            now + Duration::minutes(3),
        )
        .await
        .expect("editing works again after restore");
    assert_eq!(edited.version, restored.version + 1);
}

#[tokio::test]
async fn permanent_delete_only_applies_to_trashed_content_and_cascades() {
    let (_temp, repository, service) = harness().await;
    let now = Utc.with_ymd_and_hms(2026, 9, 3, 10, 0, 0).unwrap();
    let created = service
        .create(
            post("old-name", Publication::Public { publish_at: now }),
            SaveIntent::Explicit,
            now,
        )
        .await
        .expect("create");

    assert_eq!(
        repository.delete_permanently(created.id).await,
        Err(RepositoryError::NotFound),
        "live content must never be hard-deleted"
    );

    let mut renamed = created.to_draft();
    renamed.slug = Slug::parse("new-name").unwrap();
    let renamed = service
        .update(
            created.id,
            created.version,
            renamed,
            SaveIntent::Explicit,
            now,
        )
        .await
        .expect("rename");
    assert_eq!(
        repository
            .resolve_redirect(&Slug::parse("old-name").unwrap())
            .await
            .unwrap()
            .map(|slug| slug.to_string()),
        Some("new-name".into())
    );

    repository
        .move_to_trash(created.id, renamed.version, now)
        .await
        .expect("trash");
    repository
        .delete_permanently(created.id)
        .await
        .expect("permanent delete");

    assert!(repository.find_by_id(created.id).await.unwrap().is_none());
    assert!(
        repository
            .list_revisions(created.id)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        repository
            .resolve_redirect(&Slug::parse("old-name").unwrap())
            .await
            .unwrap()
            .is_none()
    );
    let indexed: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM search_index WHERE rowid = ?")
        .bind(created.id.as_i64())
        .fetch_one(repository.pool())
        .await
        .unwrap();
    assert_eq!(indexed, 0);
    let tagged: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM content_tags WHERE content_id = ?")
        .bind(created.id.as_i64())
        .fetch_one(repository.pool())
        .await
        .unwrap();
    assert_eq!(tagged, 0);

    // Both slugs are free again.
    service
        .create(
            post("old-name", Publication::Draft),
            SaveIntent::Explicit,
            now,
        )
        .await
        .expect("old slug is reusable");
    service
        .create(
            post("new-name", Publication::Draft),
            SaveIntent::Explicit,
            now,
        )
        .await
        .expect("new slug is reusable");
}

#[tokio::test]
async fn renaming_a_draft_leaves_no_redirect_behind() {
    let (_temp, repository, service) = harness().await;
    let now = Utc::now();
    let created = service
        .create(
            post("first-thoughts", Publication::Draft),
            SaveIntent::Explicit,
            now,
        )
        .await
        .unwrap();
    let mut draft = created.to_draft();
    draft.slug = Slug::parse("second-thoughts").unwrap();
    let renamed = service
        .update(
            created.id,
            created.version,
            draft,
            SaveIntent::Explicit,
            now,
        )
        .await
        .unwrap();
    assert_eq!(
        repository
            .resolve_redirect(&Slug::parse("first-thoughts").unwrap())
            .await
            .unwrap(),
        None,
        "nobody ever saw the old address"
    );
    // The old address is free again for another piece.
    service
        .create(
            post("first-thoughts", Publication::Draft),
            SaveIntent::Explicit,
            now,
        )
        .await
        .unwrap();

    // Once public, a rename keeps the old address working.
    let mut published = renamed.to_draft();
    published.publication = Publication::Public { publish_at: now };
    let published = service
        .update(
            renamed.id,
            renamed.version,
            published,
            SaveIntent::Explicit,
            now,
        )
        .await
        .unwrap();
    let mut moved = published.to_draft();
    moved.slug = Slug::parse("third-thoughts").unwrap();
    service
        .update(
            published.id,
            published.version,
            moved,
            SaveIntent::Explicit,
            now,
        )
        .await
        .unwrap();
    assert_eq!(
        repository
            .resolve_redirect(&Slug::parse("second-thoughts").unwrap())
            .await
            .unwrap()
            .as_ref()
            .map(Slug::as_str),
        Some("third-thoughts")
    );
}

#[tokio::test]
async fn renaming_a_scheduled_piece_leaves_no_redirect_behind() {
    let (_temp, repository, service) = harness().await;
    let now = Utc::now();
    let later = now + chrono::Duration::hours(1);
    let created = service
        .create(
            post("tomorrow", Publication::Public { publish_at: later }),
            SaveIntent::Explicit,
            now,
        )
        .await
        .unwrap();
    let mut draft = created.to_draft();
    draft.slug = Slug::parse("tomorrow-revised").unwrap();
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
    assert_eq!(
        repository
            .resolve_redirect(&Slug::parse("tomorrow").unwrap())
            .await
            .unwrap(),
        None,
        "an address that was never visible is not remembered"
    );
    // The old address is free again for another piece.
    service
        .create(
            post("tomorrow", Publication::Draft),
            SaveIntent::Explicit,
            now,
        )
        .await
        .unwrap();
}
