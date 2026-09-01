use std::sync::Arc;

use chrono::{Duration, TimeZone, Utc};
use simple_blog::{
    application::{
        content::{ContentService, SaveIntent},
        ports::PublicationState,
        publication::{PublicationDisposition, PublicationService, publication_delay},
        site_compiler::SiteCompiler,
    },
    domain::content::{ContentDraft, ContentKind, Publication, Slug},
    infrastructure::{markdown::ComrakMarkdownRenderer, sqlite::SqliteRepository},
    release::{FilesystemReleaseStore, ReleaseReader, ReleaseStore},
};

#[test]
fn scheduler_delay_never_fires_early_and_caps_idle_health_checks() {
    let now = Utc.with_ymd_and_hms(2026, 9, 2, 12, 0, 0).unwrap();
    let cap = std::time::Duration::from_secs(60);
    assert_eq!(
        publication_delay(
            PublicationState {
                revision: 1,
                next_publish_at: Some(now + Duration::seconds(30)),
            },
            now,
            cap,
        ),
        std::time::Duration::from_secs(30)
    );
    assert_eq!(
        publication_delay(
            PublicationState {
                revision: 1,
                next_publish_at: Some(now),
            },
            now,
            cap,
        ),
        std::time::Duration::ZERO
    );
    for next_publish_at in [None, Some(now + Duration::hours(1))] {
        assert_eq!(
            publication_delay(
                PublicationState {
                    revision: 1,
                    next_publish_at,
                },
                now,
                cap,
            ),
            cap
        );
    }
}

async fn harness() -> (
    tempfile::TempDir,
    Arc<SqliteRepository>,
    ContentService,
    Arc<FilesystemReleaseStore>,
    PublicationService<SqliteRepository, FilesystemReleaseStore>,
) {
    let temp = tempfile::tempdir().unwrap();
    let repository = Arc::new(
        SqliteRepository::connect(&temp.path().join("simple-blog.sqlite3"))
            .await
            .unwrap(),
    );
    let content = ContentService::new(
        repository.clone(),
        Arc::new(ComrakMarkdownRenderer::default()),
    );
    let store = Arc::new(FilesystemReleaseStore::new(temp.path().join("releases")));
    let publication = PublicationService::new(
        repository.clone(),
        store.clone(),
        SiteCompiler::embedded().unwrap(),
        "https://writing.example",
    )
    .unwrap();
    (temp, repository, content, store, publication)
}

fn draft(title: &str, publish_at: chrono::DateTime<Utc>) -> ContentDraft {
    ContentDraft {
        kind: ContentKind::Post,
        title: title.into(),
        slug: Slug::parse(title.to_ascii_lowercase()).unwrap(),
        summary: String::new(),
        body_markdown: format!("# {title}"),
        tags: Vec::new(),
        cover_media_id: None,
        seo_title: None,
        seo_description: None,
        publication: Publication::Public { publish_at },
    }
}

#[tokio::test]
async fn publication_service_builds_once_then_reports_a_verified_noop() {
    let (_temp, _repository, content, store, publication) = harness().await;
    let now = Utc.with_ymd_and_hms(2026, 9, 2, 12, 0, 0).unwrap();
    content
        .create(
            draft("Published", now - Duration::seconds(1)),
            SaveIntent::Explicit,
            now,
        )
        .await
        .unwrap();

    let built = publication.publish(now).await.unwrap();
    assert_eq!(built.disposition, PublicationDisposition::Published);
    assert_eq!(built.public_revision, 1);
    assert!(built.route_count > 8);
    assert!(built.staged_object_count > 8);
    assert_eq!(store.active().await.unwrap().unwrap().id, built.release_id);

    let unchanged = publication.publish(now).await.unwrap();
    assert_eq!(unchanged.disposition, PublicationDisposition::Unchanged);
    assert_eq!(unchanged.release_id, built.release_id);
    assert_eq!(unchanged.staged_object_count, 0);
    let manifest = store.manifest(&unchanged.release_id).await.unwrap();
    for object in manifest
        .routes
        .values()
        .filter_map(|route| route.object_id())
    {
        store.object(object).await.unwrap();
    }
}

#[tokio::test]
async fn scheduled_publication_advances_the_clock_and_builds_at_the_exact_boundary() {
    let (_temp, _repository, content, store, publication) = harness().await;
    let before = Utc.with_ymd_and_hms(2026, 9, 2, 12, 0, 0).unwrap();
    let due = before + Duration::minutes(5);
    content
        .create(draft("Scheduled", due), SaveIntent::Explicit, before)
        .await
        .unwrap();

    let before_release = publication.publish(before).await.unwrap();
    assert!(
        !body_for(&store, &before_release.release_id, "/")
            .await
            .contains("Scheduled")
    );
    let due_release = publication.publish(due).await.unwrap();
    assert_eq!(due_release.disposition, PublicationDisposition::Published);
    assert_eq!(
        due_release.public_revision,
        before_release.public_revision + 1
    );
    assert!(
        body_for(&store, &due_release.release_id, "/")
            .await
            .contains("Scheduled")
    );
}

#[tokio::test]
async fn a_public_edit_activates_a_new_complete_release() {
    let (_temp, _repository, content, store, publication) = harness().await;
    let now = Utc.with_ymd_and_hms(2026, 9, 2, 12, 0, 0).unwrap();
    let created = content
        .create(draft("Original", now), SaveIntent::Explicit, now)
        .await
        .unwrap();
    let first = publication.publish(now).await.unwrap();
    let mut edited = created.to_draft();
    edited.title = "Edited".into();
    content
        .update(
            created.id,
            created.version,
            edited,
            SaveIntent::Explicit,
            now + Duration::seconds(1),
        )
        .await
        .unwrap();

    let second = publication
        .publish(now + Duration::seconds(1))
        .await
        .unwrap();

    assert_ne!(second.release_id, first.release_id);
    assert!(second.staged_object_count < second.route_count);
    assert!(
        body_for(&store, &second.release_id, "/original/")
            .await
            .contains("Edited")
    );
}

async fn body_for(
    store: &FilesystemReleaseStore,
    release_id: &simple_blog::release::ReleaseId,
    path: &str,
) -> String {
    let manifest = store.manifest(release_id).await.unwrap();
    let object_id = manifest.routes[path].object_id().unwrap();
    String::from_utf8(store.object(object_id).await.unwrap()).unwrap()
}
