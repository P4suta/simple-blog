use std::sync::Arc;

use chrono::{Duration, TimeZone, Utc};
use simple_blog::{
    application::{
        content::{ContentService, SaveIntent},
        ports::{PublicSnapshotRepository, SiteRepository},
    },
    domain::{
        content::{ContentDraft, ContentKind, Publication, Slug},
        theme::SiteSettings,
    },
    infrastructure::{markdown::ComrakMarkdownRenderer, sqlite::SqliteRepository},
};

async fn harness() -> (tempfile::TempDir, Arc<SqliteRepository>, ContentService) {
    let temp = tempfile::tempdir().unwrap();
    let repository = Arc::new(
        SqliteRepository::connect(&temp.path().join("simple-blog.sqlite3"))
            .await
            .unwrap(),
    );
    let service = ContentService::new(
        repository.clone(),
        Arc::new(ComrakMarkdownRenderer::default()),
    );
    (temp, repository, service)
}

fn draft(title: &str, publication: Publication) -> ContentDraft {
    ContentDraft {
        kind: ContentKind::Post,
        title: title.into(),
        slug: Slug::parse(title.to_ascii_lowercase().replace(' ', "-")).unwrap(),
        summary: String::new(),
        body_markdown: format!("# {title}"),
        tags: vec!["Rust".into()],
        cover_media_id: None,
        seo_title: None,
        seo_description: None,
        publication,
    }
}

#[tokio::test]
async fn publication_revision_tracks_only_changes_that_can_affect_public_output() {
    let (_temp, repository, service) = harness().await;
    let now = Utc.with_ymd_and_hms(2026, 9, 2, 12, 0, 0).unwrap();
    assert_eq!(repository.publication_state().await.unwrap().revision, 0);

    let private = service
        .create(
            draft("Private", Publication::Draft),
            SaveIntent::Explicit,
            now,
        )
        .await
        .unwrap();
    assert_eq!(repository.publication_state().await.unwrap().revision, 0);

    let due = now + Duration::hours(1);
    let mut scheduled = private.to_draft();
    scheduled.publication = Publication::Public { publish_at: due };
    let scheduled = service
        .update(
            private.id,
            private.version,
            scheduled,
            SaveIntent::Explicit,
            now,
        )
        .await
        .unwrap();
    let state = repository.publication_state().await.unwrap();
    assert_eq!(state.revision, 0);
    assert_eq!(state.next_publish_at, Some(due));

    assert!(
        !repository
            .advance_publication_clock(due - Duration::nanoseconds(1))
            .await
            .unwrap()
    );
    assert!(repository.advance_publication_clock(due).await.unwrap());
    assert_eq!(repository.publication_state().await.unwrap().revision, 1);

    let mut edited = scheduled.to_draft();
    edited.body_markdown = "# Visible edit".into();
    let edited = service
        .update(
            scheduled.id,
            scheduled.version,
            edited,
            SaveIntent::Autosave,
            due,
        )
        .await
        .unwrap();
    assert_eq!(repository.publication_state().await.unwrap().revision, 2);

    let mut unpublished = edited.to_draft();
    unpublished.publication = Publication::Draft;
    service
        .update(
            edited.id,
            edited.version,
            unpublished,
            SaveIntent::Explicit,
            due,
        )
        .await
        .unwrap();
    let state = repository.publication_state().await.unwrap();
    assert_eq!(state.revision, 3);
    assert_eq!(state.next_publish_at, None);
}

#[tokio::test]
async fn a_failed_content_transaction_cannot_advance_publication_revision() {
    let (_temp, repository, service) = harness().await;
    let now = Utc.with_ymd_and_hms(2026, 9, 2, 12, 0, 0).unwrap();
    let post = service
        .create(
            draft("Public", Publication::Public { publish_at: now }),
            SaveIntent::Explicit,
            now,
        )
        .await
        .unwrap();
    assert_eq!(repository.publication_state().await.unwrap().revision, 1);

    let error = service
        .update(
            post.id,
            post.version - 1,
            post.to_draft(),
            SaveIntent::Explicit,
            now,
        )
        .await
        .unwrap_err();
    assert!(error.to_string().contains("changed"));
    assert_eq!(repository.publication_state().await.unwrap().revision, 1);
}

#[tokio::test]
async fn public_snapshot_is_complete_ordered_and_carries_the_same_revision() {
    let (_temp, repository, service) = harness().await;
    let now = Utc.with_ymd_and_hms(2026, 9, 2, 12, 0, 0).unwrap();
    let first = service
        .create(
            draft(
                "First",
                Publication::Public {
                    publish_at: now - Duration::hours(2),
                },
            ),
            SaveIntent::Explicit,
            now,
        )
        .await
        .unwrap();
    let mut renamed = first.to_draft();
    renamed.slug = Slug::parse("renamed").unwrap();
    service
        .update(first.id, first.version, renamed, SaveIntent::Explicit, now)
        .await
        .unwrap();
    service
        .create(
            draft(
                "Second",
                Publication::Public {
                    publish_at: now - Duration::hours(1),
                },
            ),
            SaveIntent::Explicit,
            now,
        )
        .await
        .unwrap();
    service
        .create(
            draft("Hidden", Publication::Draft),
            SaveIntent::Explicit,
            now,
        )
        .await
        .unwrap();

    let snapshot = repository.public_snapshot(now).await.unwrap();
    let state = repository.publication_state().await.unwrap();

    assert_eq!(snapshot.public_revision, state.revision);
    assert_eq!(snapshot.effective_at, now);
    assert_eq!(
        snapshot
            .contents
            .iter()
            .map(|content| content.title.as_str())
            .collect::<Vec<_>>(),
        ["Second", "First"]
    );
    assert_eq!(snapshot.redirects.len(), 1);
    assert_eq!(snapshot.redirects[0].from.as_str(), "first");
    assert_eq!(snapshot.redirects[0].to.as_str(), "renamed");
    assert!(
        snapshot
            .contents
            .iter()
            .all(|content| !content.tags.is_empty())
    );
}

#[tokio::test]
async fn site_configuration_advances_the_public_revision_in_its_transaction() {
    let (_temp, repository, _service) = harness().await;
    let now = Utc.with_ymd_and_hms(2026, 9, 2, 12, 0, 0).unwrap();
    let mut settings: SiteSettings = repository.site_settings().await.unwrap();
    settings.site_title = "Changed title".into();

    repository
        .save_configuration(&settings, &[], now)
        .await
        .unwrap();

    assert_eq!(repository.publication_state().await.unwrap().revision, 1);
    assert_eq!(
        repository
            .public_snapshot(now)
            .await
            .unwrap()
            .settings
            .site_title,
        "Changed title"
    );
}
#[tokio::test]
#[expect(
    clippy::too_many_lines,
    reason = "visible, scheduled, and renamed pieces in one clock scenario"
)]
async fn trash_and_restore_move_the_publication_clock_only_when_visibility_changes() {
    use simple_blog::application::ports::ContentRepository;

    let (_temp, repository, service) = harness().await;
    let now = Utc.with_ymd_and_hms(2026, 9, 3, 12, 0, 0).unwrap();

    let visible = service
        .create(
            draft("Visible", Publication::Public { publish_at: now }),
            SaveIntent::Explicit,
            now,
        )
        .await
        .unwrap();
    assert_eq!(repository.publication_state().await.unwrap().revision, 1);

    let trashed = repository
        .move_to_trash(visible.id, visible.version, now)
        .await
        .unwrap();
    assert_eq!(repository.publication_state().await.unwrap().revision, 2);
    assert!(
        repository
            .public_snapshot(now)
            .await
            .unwrap()
            .contents
            .is_empty()
    );

    repository
        .restore_from_trash(visible.id, now)
        .await
        .unwrap();
    assert_eq!(repository.publication_state().await.unwrap().revision, 3);
    assert_eq!(
        repository
            .public_snapshot(now)
            .await
            .unwrap()
            .contents
            .len(),
        1
    );
    assert_eq!(trashed.publication, Publication::Public { publish_at: now });

    let due = now + Duration::hours(1);
    let scheduled = service
        .create(
            draft("Scheduled", Publication::Public { publish_at: due }),
            SaveIntent::Explicit,
            now,
        )
        .await
        .unwrap();
    let state = repository.publication_state().await.unwrap();
    assert_eq!(state.revision, 3);
    assert_eq!(state.next_publish_at, Some(due));

    repository
        .move_to_trash(scheduled.id, scheduled.version, now)
        .await
        .unwrap();
    let state = repository.publication_state().await.unwrap();
    assert_eq!(
        state.revision, 3,
        "an invisible piece leaving changes nothing public"
    );
    assert_eq!(
        state.next_publish_at, None,
        "a trashed entry must not hold the clock"
    );

    repository
        .restore_from_trash(scheduled.id, now)
        .await
        .unwrap();
    let state = repository.publication_state().await.unwrap();
    assert_eq!(state.revision, 3);
    assert_eq!(state.next_publish_at, Some(due));

    // A renamed, then trashed piece withdraws its historical redirect too.
    let mut renamed = visible.to_draft();
    renamed.slug = Slug::parse("visible-renamed").unwrap();
    let current = repository.find_by_id(visible.id).await.unwrap().unwrap();
    let renamed = service
        .update(
            visible.id,
            current.version,
            renamed,
            SaveIntent::Explicit,
            now,
        )
        .await
        .unwrap();
    assert_eq!(
        repository
            .public_snapshot(now)
            .await
            .unwrap()
            .redirects
            .len(),
        1
    );
    repository
        .move_to_trash(visible.id, renamed.version, now)
        .await
        .unwrap();
    assert!(
        repository
            .public_snapshot(now)
            .await
            .unwrap()
            .redirects
            .is_empty()
    );
}
