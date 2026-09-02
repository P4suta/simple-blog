use std::sync::Arc;

use chrono::{TimeZone, Utc};
use simple_blog::{
    application::{
        content::{ContentService, SaveIntent},
        ports::ContentRepository,
    },
    config::{Config, ConfigSources, Overrides},
    domain::content::{ContentDraft, ContentKind, Publication, Slug},
    infrastructure::{
        markdown::ComrakMarkdownRenderer, media::LocalMediaService, sqlite::SqliteRepository,
    },
    operations::{OperationError, PortableMigrationService},
    portable::PortableArchive,
    release::{FilesystemReleaseStore, ReleaseStore},
};

fn config(data_dir: &std::path::Path, origin: &str) -> Config {
    Config::resolve(ConfigSources {
        cli: Overrides {
            data_dir: Some(data_dir.to_path_buf()),
            public_url: Some(origin.into()),
            ..Overrides::default()
        },
        ..ConfigSources::default()
    })
    .unwrap()
}

fn gif() -> Vec<u8> {
    base64::Engine::decode(
        &base64::engine::general_purpose::STANDARD,
        "R0lGODlhAQABAIAAAAAAAP///ywAAAAAAQABAAACAUwAOw==",
    )
    .unwrap()
}

async fn source(temp: &tempfile::TempDir) -> (Config, Arc<SqliteRepository>, String) {
    let config = config(&temp.path().join("source"), "https://writing.example");
    for directory in [
        config.media_dir(),
        config.backup_dir(),
        config.release_dir(),
    ] {
        std::fs::create_dir_all(directory).unwrap();
    }
    config.persist().unwrap();
    let repository = Arc::new(
        SqliteRepository::connect(&config.database_path())
            .await
            .unwrap(),
    );
    let content = ContentService::new(
        repository.clone(),
        Arc::new(ComrakMarkdownRenderer::default()),
    );
    let now = Utc.with_ymd_and_hms(2026, 9, 2, 12, 0, 0).unwrap();
    content
        .create(
            ContentDraft {
                kind: ContentKind::Post,
                title: "Host neutral".into(),
                slug: Slug::parse("host-neutral").unwrap(),
                summary: "The adapter is replaceable".into(),
                body_markdown: "# Canonical Markdown".into(),
                tags: vec!["Portability".into()],
                cover_media_id: None,
                seo_title: None,
                seo_description: None,
                publication: Publication::Public {
                    publish_at: now + chrono::Duration::minutes(15),
                },
            },
            SaveIntent::Explicit,
            now,
        )
        .await
        .unwrap();
    let media = LocalMediaService::new(config.media_dir(), repository.clone(), 1024 * 1024);
    let asset = media
        .store("pixel.gif", gif(), "pixel", "", now)
        .await
        .unwrap();
    (config, repository, asset.original_filename)
}

#[tokio::test]
async fn portable_export_and_fresh_import_verify_media_database_and_public_release() {
    let temp = tempfile::tempdir().unwrap();
    let (source_config, source_repository, media_filename) = source(&temp).await;
    let archive = temp.path().join("site.simple-blog");
    let now = Utc.with_ymd_and_hms(2026, 9, 2, 12, 30, 0).unwrap();

    let exported =
        PortableMigrationService::export(&source_config, source_repository.as_ref(), &archive, now)
            .await
            .unwrap();
    assert_eq!(exported.entry_count, 3);
    assert_eq!(
        PortableArchive::read(&archive).unwrap().site.exported_at,
        now
    );

    let destination = temp.path().join("destination");
    let destination_config = config(&destination, "https://writing.example");
    let imported = PortableMigrationService::import(&archive, &destination_config, false)
        .await
        .unwrap();

    assert_eq!(imported.content_count, 1);
    assert_eq!(imported.media_count, 1);
    assert!(imported.replaced_data_dir.is_none());
    assert!(destination.join("media").join(media_filename).is_file());
    assert!(destination.join("config.toml").is_file());
    let repository = SqliteRepository::connect(&destination_config.database_path())
        .await
        .unwrap();
    let content = repository
        .find_public_by_slug(&Slug::parse("host-neutral").unwrap(), now)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(content.body_markdown, "# Canonical Markdown");
    let active = FilesystemReleaseStore::new(destination_config.release_dir())
        .active()
        .await
        .unwrap()
        .unwrap();
    assert_eq!(active.id.as_str(), imported.release_id);
}

#[tokio::test]
async fn forced_import_retains_the_previous_installation_as_a_recoverable_directory() {
    let temp = tempfile::tempdir().unwrap();
    let (source_config, source_repository, _) = source(&temp).await;
    let archive = temp.path().join("site.simple-blog");
    PortableMigrationService::export(
        &source_config,
        source_repository.as_ref(),
        &archive,
        Utc.with_ymd_and_hms(2026, 9, 2, 12, 30, 0).unwrap(),
    )
    .await
    .unwrap();
    let destination = temp.path().join("destination");
    std::fs::create_dir(&destination).unwrap();
    std::fs::write(destination.join("sentinel"), "previous installation").unwrap();
    let destination_config = config(&destination, "https://writing.example");

    let refused = PortableMigrationService::import(&archive, &destination_config, false)
        .await
        .unwrap_err();
    assert!(matches!(refused, OperationError::DestinationExists));
    assert_eq!(
        std::fs::read_to_string(destination.join("sentinel")).unwrap(),
        "previous installation"
    );

    let report = PortableMigrationService::import(&archive, &destination_config, true)
        .await
        .unwrap();
    let retained = report.replaced_data_dir.unwrap();
    assert_eq!(
        std::fs::read_to_string(retained.join("sentinel")).unwrap(),
        "previous installation"
    );
    assert!(destination.join("simple-blog.sqlite3").is_file());
}

#[tokio::test]
async fn origin_mismatch_is_rejected_before_the_destination_is_touched() {
    let temp = tempfile::tempdir().unwrap();
    let (source_config, source_repository, _) = source(&temp).await;
    let archive = temp.path().join("site.simple-blog");
    PortableMigrationService::export(
        &source_config,
        source_repository.as_ref(),
        &archive,
        Utc.with_ymd_and_hms(2026, 9, 2, 12, 30, 0).unwrap(),
    )
    .await
    .unwrap();
    let destination = temp.path().join("destination");
    let destination_config = config(&destination, "https://somewhere-else.example");

    let error = PortableMigrationService::import(&archive, &destination_config, false)
        .await
        .unwrap_err();

    assert!(matches!(
        error,
        OperationError::PortableOriginMismatch { .. }
    ));
    assert!(!destination.exists());
}
