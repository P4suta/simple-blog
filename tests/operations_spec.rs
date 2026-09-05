use std::sync::Arc;

use chrono::Utc;
use simple_blog::{
    application::{
        content::{ContentService, SaveIntent},
        ports::{ContentRepository, MediaRepository},
    },
    config::{Config, ConfigSources, Overrides},
    domain::content::{ContentDraft, ContentKind, Publication, Slug},
    infrastructure::{
        markdown::ComrakMarkdownRenderer, media::LocalMediaService, sqlite::SqliteRepository,
    },
    operations::{
        BackupService, Doctor, Exporter, Importer, MigrationCoordinator, OperationError,
        RestoreService,
    },
    release::{FilesystemReleaseStore, ReleaseBuilder, ReleasePublisher},
};
use sqlx::{
    Connection, Executor,
    sqlite::{SqliteConnectOptions, SqliteConnection},
};

fn config(data_dir: &std::path::Path) -> Config {
    Config::resolve(ConfigSources {
        cli: Overrides {
            data_dir: Some(data_dir.to_path_buf()),
            public_url: Some("http://localhost:8080".into()),
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

#[test]
fn migration_errors_display_the_recoverable_backup_path_without_debug_escaping() {
    let backup = std::path::PathBuf::from(r"C:\simple blog\backup.tar.zst");
    let error = OperationError::Migration {
        message: "schema rejected".into(),
        backup: backup.clone(),
    };

    assert!(error.to_string().contains(&backup.display().to_string()));
}

async fn seeded(temp: &tempfile::TempDir) -> (Config, Arc<SqliteRepository>) {
    let config = config(temp.path());
    for directory in [
        config.media_dir(),
        config.backup_dir(),
        config.release_dir(),
    ] {
        std::fs::create_dir_all(directory).unwrap();
    }
    let repository = Arc::new(
        SqliteRepository::connect(&config.database_path())
            .await
            .unwrap(),
    );
    let content = ContentService::new(
        repository.clone(),
        Arc::new(ComrakMarkdownRenderer::default()),
    );
    content
        .create(
            ContentDraft {
                kind: ContentKind::Post,
                title: "Portable post".into(),
                slug: Slug::parse("portable-post").unwrap(),
                summary: "Can leave the product".into(),
                body_markdown: "# Canonical Markdown\n\nBody.".into(),
                tags: vec!["Rust".into()],
                cover_media_id: None,
                seo_title: Some("Portable SEO".into()),
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
    (config, repository)
}

#[tokio::test]
async fn backup_and_restore_round_trip_database_config_and_media() {
    let source = tempfile::tempdir().unwrap();
    let (source_config, repository) = seeded(&source).await;
    std::fs::write(
        source_config.data_dir.join("config.toml"),
        "public_url = \"http://localhost:8080\"\n",
    )
    .unwrap();
    let media = LocalMediaService::new(source_config.media_dir(), repository.clone(), 1024 * 1024);
    let asset = media
        .store("pixel.gif", gif(), "pixel", "", Utc::now())
        .await
        .unwrap();

    let archive = BackupService::create(&source_config, repository.as_ref(), None, Utc::now())
        .await
        .unwrap();
    assert!(archive.is_file());
    assert!(
        archive
            .extension()
            .is_some_and(|extension| extension == "zst")
    );
    repository.close().await;

    let destination = tempfile::tempdir().unwrap();
    RestoreService::restore(&archive, destination.path(), false)
        .await
        .unwrap();
    let restored = Arc::new(
        SqliteRepository::connect(&destination.path().join("simple-blog.sqlite3"))
            .await
            .unwrap(),
    );
    let post = restored
        .find_public_by_slug(&Slug::parse("portable-post").unwrap(), Utc::now())
        .await
        .unwrap();
    assert_eq!(post.unwrap().body_markdown, "# Canonical Markdown\n\nBody.");
    let restored_asset = restored.find_media(&asset.id).await.unwrap().unwrap();
    assert!(
        destination
            .path()
            .join("media")
            .join(restored_asset.original_filename)
            .is_file()
    );
    assert!(destination.path().join("config.toml").is_file());
}

#[tokio::test]
async fn export_writes_front_matter_markdown_and_plain_media_files() {
    let source = tempfile::tempdir().unwrap();
    let (config, repository) = seeded(&source).await;
    let output = source.path().join("portable-export");

    Exporter::export(&config, repository.as_ref(), &output, Utc::now())
        .await
        .unwrap();
    let markdown = std::fs::read_to_string(output.join("posts/portable-post.md")).unwrap();
    assert!(markdown.starts_with("---\n"));
    assert!(markdown.contains("title: \"Portable post\""));
    assert!(markdown.contains("status: public"));
    assert!(markdown.ends_with("# Canonical Markdown\n\nBody.\n"));
    assert!(output.join("media").is_dir());
}

#[tokio::test]
async fn export_and_import_carry_the_trash() {
    let source = tempfile::tempdir().unwrap();
    let (config, repository) = seeded(&source).await;
    let content = ContentService::new(
        repository.clone(),
        Arc::new(ComrakMarkdownRenderer::default()),
    );
    let discarded = content
        .create(
            ContentDraft {
                kind: ContentKind::Page,
                title: "Discarded".into(),
                slug: Slug::parse("discarded").unwrap(),
                summary: String::new(),
                body_markdown: "Not yet.".into(),
                tags: Vec::new(),
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
    content
        .move_to_trash(discarded.id, discarded.version, Utc::now())
        .await
        .unwrap();

    let output = source.path().join("with-trash");
    Exporter::export(&config, repository.as_ref(), &output, Utc::now())
        .await
        .unwrap();
    let exported = std::fs::read_to_string(output.join("trash/discarded.md"))
        .expect("the trash travels in its own folder");
    assert!(exported.contains("kind: page"));
    assert!(!output.join("pages/discarded.md").exists());

    let destination = tempfile::tempdir().unwrap();
    let target_config = config_for(destination.path());
    std::fs::create_dir_all(target_config.media_dir()).unwrap();
    let target = Arc::new(
        SqliteRepository::connect(&target_config.database_path())
            .await
            .unwrap(),
    );
    let report = Importer::import(&target_config, &target, &output, false, Utc::now())
        .await
        .unwrap();
    assert!(
        report.imported.contains(&"discarded".to_owned()),
        "{report:?}"
    );
    let back = target
        .list_all_content()
        .await
        .unwrap()
        .into_iter()
        .find(|piece| piece.slug.as_str() == "discarded")
        .unwrap();
    assert!(
        back.is_trashed(),
        "it comes back into the trash, not onto the site"
    );
    assert_eq!(back.kind, ContentKind::Page);
}

#[tokio::test]
async fn doctor_names_every_safety_limit_and_invents_no_quota() {
    let source = tempfile::tempdir().unwrap();
    let (config, repository) = seeded(&source).await;
    let report = Doctor::inspect(&config, repository.as_ref()).await.unwrap();

    assert_eq!(report.limits.upload_bytes, config.max_upload_bytes);
    assert_eq!(report.limits.backup_generations, config.backup_retention);
    assert_eq!(report.limits.markdown_bytes, 2 * 1024 * 1024);
    assert_eq!(report.limits.autosave_revisions_kept, 50);
    for name in [
        "limits.upload",
        "limits.text",
        "limits.image",
        "limits.theme",
        "limits.search",
        "limits.rate",
        "limits.history",
        "limits.backups",
    ] {
        let check = report
            .checks
            .iter()
            .find(|check| check.name == name)
            .unwrap_or_else(|| panic!("doctor is silent about {name}"));
        assert_eq!(check.status, "ok", "{name}");
    }
    let upload = report
        .checks
        .iter()
        .find(|check| check.name == "limits.upload")
        .unwrap();
    assert!(upload.detail.contains(&config.max_upload_bytes.to_string()));
    assert!(
        upload.detail.contains("max_upload_bytes"),
        "a configurable limit says where it is changed"
    );
    // Every limit is a safety limit on one request or one piece; nothing
    // caps how many pieces, how many bytes in total, or how many readers.
    let limits = serde_json::to_value(report.limits).unwrap();
    for key in limits.as_object().unwrap().keys() {
        assert!(
            !["quota", "traffic", "total", "pieces", "posts"]
                .iter()
                .any(|word| key.contains(word)),
            "{key} reads like a quota"
        );
    }
}

#[tokio::test]
async fn doctor_reports_missing_media_without_mutating_state() {
    let source = tempfile::tempdir().unwrap();
    let (config, repository) = seeded(&source).await;
    std::fs::create_dir_all(config.media_dir()).unwrap();
    std::fs::create_dir_all(config.backup_dir()).unwrap();
    let healthy = Doctor::inspect(&config, repository.as_ref()).await.unwrap();
    assert!(healthy.is_healthy(), "{:?}", healthy.issues);
    assert!(healthy.checks.iter().all(|check| check.status == "ok"));
    assert!(!config.data_dir.join(".simple-blog-doctor-probe").exists());

    sqlx::query("INSERT INTO media (id, original_name, mime_type, extension, width, height, byte_size, created_at) VALUES (?, 'missing.png', 'image/png', 'png', 1, 1, 1, ?)")
        .bind("a".repeat(64)).bind(Utc::now()).execute(repository.pool()).await.unwrap();
    let report = Doctor::inspect(&config, repository.as_ref()).await.unwrap();
    assert!(!report.is_healthy());
    assert!(
        report
            .issues
            .iter()
            .any(|issue| issue.contains("missing media file"))
    );
}

#[tokio::test]
async fn doctor_detects_migration_checksum_drift() {
    let source = tempfile::tempdir().unwrap();
    let (config, repository) = seeded(&source).await;
    std::fs::create_dir_all(config.media_dir()).unwrap();
    std::fs::create_dir_all(config.backup_dir()).unwrap();
    sqlx::query("UPDATE _sqlx_migrations SET checksum = X'00' WHERE version = 1")
        .execute(repository.pool())
        .await
        .unwrap();

    let report = Doctor::inspect(&config, repository.as_ref()).await.unwrap();

    assert!(!report.is_healthy());
    let check = report
        .checks
        .iter()
        .find(|check| check.name == "sqlite.migrations")
        .unwrap();
    assert_eq!(check.status, "error");
    assert!(check.detail.contains("checksum mismatch for migration 1"));
}

#[tokio::test]
async fn doctor_distinguishes_corrupt_orphaned_and_interrupted_media_files() {
    let source = tempfile::tempdir().unwrap();
    let (config, repository) = seeded(&source).await;
    std::fs::create_dir_all(config.backup_dir()).unwrap();
    let media = LocalMediaService::new(config.media_dir(), repository.clone(), 1024 * 1024);
    let asset = media
        .store("pixel.gif", gif(), "pixel", "", Utc::now())
        .await
        .unwrap();
    std::fs::write(
        config.media_dir().join(&asset.variants[0].filename),
        b"corrupt",
    )
    .unwrap();
    std::fs::write(config.media_dir().join("orphan.webp"), b"orphan").unwrap();
    std::fs::write(
        config.media_dir().join(".upload-interrupted.tmp"),
        b"partial",
    )
    .unwrap();

    let report = Doctor::inspect(&config, repository.as_ref()).await.unwrap();

    assert!(!report.is_healthy());
    let records = report
        .checks
        .iter()
        .find(|check| check.name == "media.records")
        .unwrap();
    assert_eq!(records.status, "error");
    assert!(records.detail.contains("variant byte size mismatch"));
    let orphans = report
        .checks
        .iter()
        .find(|check| check.name == "media.orphans")
        .unwrap();
    assert_eq!(orphans.status, "error");
    assert!(orphans.detail.contains("orphan media file: orphan.webp"));
    assert!(
        orphans
            .detail
            .contains("interrupted upload: .upload-interrupted.tmp")
    );
}

#[tokio::test]
async fn doctor_verifies_active_release_history_orphans_and_interrupted_writes() {
    let source = tempfile::tempdir().unwrap();
    let (config, repository) = seeded(&source).await;
    for directory in [
        config.media_dir(),
        config.backup_dir(),
        config.release_dir(),
    ] {
        std::fs::create_dir_all(directory).unwrap();
    }
    let store = Arc::new(FilesystemReleaseStore::new(config.release_dir()));
    let release = ReleaseBuilder::clean(1, config.public_url.as_str())
        .unwrap()
        .asset("/", b"healthy".to_vec(), "text/html; charset=utf-8", None)
        .unwrap()
        .finish()
        .unwrap();
    let object_id = release.manifest.routes["/"].object_id().unwrap().to_owned();
    ReleasePublisher::new(store)
        .publish(&release, None)
        .await
        .unwrap();

    let healthy = Doctor::inspect(&config, repository.as_ref()).await.unwrap();
    for name in [
        "filesystem.releases",
        "release.active",
        "release.history",
        "release.temporary_files",
    ] {
        let check = healthy
            .checks
            .iter()
            .find(|check| check.name == name)
            .unwrap();
        assert_eq!(check.status, "ok", "{}: {}", check.name, check.detail);
    }

    std::fs::write(
        config.release_dir().join("objects").join(&object_id),
        b"corrupt",
    )
    .unwrap();
    std::fs::write(
        config.release_dir().join("objects").join("f".repeat(64)),
        b"orphan",
    )
    .unwrap();
    std::fs::write(
        config.release_dir().join("manifests/.interrupted.tmp"),
        b"partial",
    )
    .unwrap();

    let broken = Doctor::inspect(&config, repository.as_ref()).await.unwrap();
    assert!(!broken.is_healthy());
    let details = broken
        .checks
        .iter()
        .filter(|check| check.name.starts_with("release."))
        .map(|check| check.detail.as_str())
        .collect::<Vec<_>>()
        .join("; ");
    assert!(details.contains(&object_id));
    assert!(details.contains("unreferenced release object"));
    assert!(details.contains("interrupted release write"));
}

#[tokio::test]
async fn pending_migrations_create_a_schema_independent_safety_backup_first() {
    let source = tempfile::tempdir().unwrap();
    let config = config(source.path());
    std::fs::create_dir_all(config.media_dir()).unwrap();
    std::fs::create_dir_all(config.backup_dir()).unwrap();
    std::fs::write(
        config.data_dir.join("config.toml"),
        "public_url = \"http://localhost:8080\"\n",
    )
    .unwrap();
    std::fs::write(config.media_dir().join("legacy.bin"), b"legacy-media").unwrap();
    let options = SqliteConnectOptions::new()
        .filename(config.database_path())
        .create_if_missing(true);
    let mut legacy = SqliteConnection::connect_with(&options).await.unwrap();
    legacy
        .execute("CREATE TABLE legacy_marker (value TEXT NOT NULL)")
        .await
        .unwrap();
    legacy
        .execute("INSERT INTO legacy_marker VALUES ('before-migration')")
        .await
        .unwrap();
    legacy.close().await.unwrap();

    let opened = MigrationCoordinator::open(&config, Utc::now())
        .await
        .unwrap();
    let backup = opened.safety_backup.clone().expect("pre-migration backup");
    assert!(backup.is_file());
    let migrated: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM _sqlx_migrations")
        .fetch_one(opened.repository.pool())
        .await
        .unwrap();
    assert!(migrated >= 2);
    opened.repository.close().await;

    let current = MigrationCoordinator::open(&config, Utc::now())
        .await
        .unwrap();
    assert!(current.safety_backup.is_none());
    current.repository.close().await;

    let restored = tempfile::tempdir().unwrap();
    RestoreService::restore(&backup, restored.path(), false)
        .await
        .unwrap();
    assert_eq!(
        std::fs::read(restored.path().join("media/legacy.bin")).unwrap(),
        b"legacy-media"
    );
    let restored_database = restored.path().join("simple-blog.sqlite3");
    let restored_options = SqliteConnectOptions::new()
        .filename(&restored_database)
        .create_if_missing(false);
    let mut restored_legacy = SqliteConnection::connect_with(&restored_options)
        .await
        .unwrap();
    let migration_table_exists: i64 = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = '_sqlx_migrations')",
    )
    .fetch_one(&mut restored_legacy)
    .await
    .unwrap();
    assert_eq!(
        migration_table_exists, 0,
        "restore validation must not migrate or otherwise rewrite the archived database"
    );
    restored_legacy.close().await.unwrap();
    let repository = SqliteRepository::connect(&restored.path().join("simple-blog.sqlite3"))
        .await
        .unwrap();
    let marker: String = sqlx::query_scalar("SELECT value FROM legacy_marker")
        .fetch_one(repository.pool())
        .await
        .unwrap();
    assert_eq!(marker, "before-migration");
}

#[tokio::test]
async fn failed_migration_leaves_a_restorable_safety_backup_and_no_snapshot() {
    let source = tempfile::tempdir().unwrap();
    let config = config(source.path());
    std::fs::create_dir_all(config.media_dir()).unwrap();
    std::fs::create_dir_all(config.backup_dir()).unwrap();
    std::fs::write(config.media_dir().join("legacy.bin"), b"legacy-media").unwrap();
    let options = SqliteConnectOptions::new()
        .filename(config.database_path())
        .create_if_missing(true);
    let mut legacy = SqliteConnection::connect_with(&options).await.unwrap();
    legacy
        .execute("CREATE TABLE legacy_marker (value TEXT NOT NULL)")
        .await
        .unwrap();
    legacy
        .execute("INSERT INTO legacy_marker VALUES ('before-failure')")
        .await
        .unwrap();
    legacy
        .execute("CREATE TABLE media (incompatible TEXT)")
        .await
        .unwrap();
    legacy.close().await.unwrap();

    let error = MigrationCoordinator::open(&config, Utc::now())
        .await
        .unwrap_err();
    let entries: Vec<_> = std::fs::read_dir(config.backup_dir())
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    let backup = entries
        .iter()
        .find(|path| {
            path.file_name()
                .is_some_and(|name| name.to_string_lossy().ends_with(".tar.zst"))
        })
        .expect("safety backup");
    let OperationError::Migration {
        backup: reported_backup,
        ..
    } = &error
    else {
        panic!("unexpected migration error: {error}");
    };
    assert_eq!(reported_backup, backup);
    assert!(error.to_string().contains(&backup.display().to_string()));
    assert!(entries.iter().all(|path| {
        !path
            .file_name()
            .is_some_and(|name| name.to_string_lossy().starts_with(".pre-migration-"))
    }));

    let extracted = tempfile::tempdir().unwrap();
    let decoder = zstd::Decoder::new(std::fs::File::open(backup).unwrap()).unwrap();
    tar::Archive::new(decoder).unpack(extracted.path()).unwrap();
    assert_eq!(
        std::fs::read(extracted.path().join("media/legacy.bin")).unwrap(),
        b"legacy-media"
    );
    let options = SqliteConnectOptions::new()
        .filename(extracted.path().join("database.sqlite3"))
        .create_if_missing(false);
    let mut snapshot = SqliteConnection::connect_with(&options).await.unwrap();
    let marker: String = sqlx::query_scalar("SELECT value FROM legacy_marker")
        .fetch_one(&mut snapshot)
        .await
        .unwrap();
    assert_eq!(marker, "before-failure");
    snapshot.close().await.unwrap();
}

#[tokio::test]
async fn export_then_import_round_trips_every_field() {
    let source = tempfile::tempdir().unwrap();
    let (config, repository) = seeded(&source).await;
    let media = LocalMediaService::new(config.media_dir(), repository.clone(), 1024 * 1024);
    let asset = media
        .store("pixel.gif", gif(), "pixel", "", Utc::now())
        .await
        .unwrap();
    let content = ContentService::new(
        repository.clone(),
        Arc::new(ComrakMarkdownRenderer::default()),
    );
    content
        .create(
            ContentDraft {
                kind: ContentKind::Page,
                title: "About: the colon survives".into(),
                slug: Slug::parse("about").unwrap(),
                summary: "Who writes here".into(),
                body_markdown: format!("![pixel](/media/{})", asset.original_filename),
                tags: vec!["Meta".into()],
                cover_media_id: Some(asset.id.to_string()),
                seo_title: None,
                seo_description: Some("An about page".into()),
                publication: Publication::Draft,
            },
            SaveIntent::Explicit,
            Utc::now(),
        )
        .await
        .unwrap();
    let output = source.path().join("portable-export");
    Exporter::export(&config, repository.as_ref(), &output, Utc::now())
        .await
        .unwrap();

    let destination = tempfile::tempdir().unwrap();
    let target_config = config_for(destination.path());
    std::fs::create_dir_all(target_config.media_dir()).unwrap();
    let target = Arc::new(
        SqliteRepository::connect(&target_config.database_path())
            .await
            .unwrap(),
    );
    let report = Importer::import(&target_config, &target, &output, false, Utc::now())
        .await
        .unwrap();
    assert_eq!(
        report.imported,
        vec!["portable-post".to_owned(), "about".to_owned()],
        "posts are read before pages"
    );
    assert!(report.skipped.is_empty(), "{:?}", report.skipped);
    assert_eq!(report.media, 1);

    let mut pieces = target.list_all_content().await.unwrap();
    pieces.sort_by(|a, b| a.slug.as_str().cmp(b.slug.as_str()));
    let about = &pieces[0];
    assert_eq!(about.title, "About: the colon survives");
    assert_eq!(about.kind, ContentKind::Page);
    assert_eq!(about.summary, "Who writes here");
    assert_eq!(about.tags[0].name, "Meta");
    assert_eq!(about.cover_media_id.as_deref(), Some(asset.id.as_str()));
    assert_eq!(about.seo_description.as_deref(), Some("An about page"));
    assert_eq!(about.publication, Publication::Draft);
    assert!(
        target.find_media(&asset.id).await.unwrap().is_some(),
        "the same bytes get the same identity, so references keep working"
    );
    let post = &pieces[1];
    assert_eq!(post.title, "Portable post");
    assert_eq!(post.seo_title.as_deref(), Some("Portable SEO"));
    assert!(matches!(post.publication, Publication::Public { .. }));
    assert_eq!(post.body_markdown, "# Canonical Markdown\n\nBody.\n");

    // A second import skips what exists unless told to replace it.
    let again = Importer::import(&target_config, &target, &output, false, Utc::now())
        .await
        .unwrap();
    assert!(again.imported.is_empty());
    assert_eq!(again.skipped.len(), 2);
    assert!(again.skipped[0].1.contains("--force"));
    std::fs::write(
        output.join("posts/portable-post.md"),
        "---\ntitle: \"Portable post, revised\"\nslug: portable-post\nkind: post\nstatus: public\n---\nNew body.\n",
    )
    .unwrap();
    let forced = Importer::import(&target_config, &target, &output, true, Utc::now())
        .await
        .unwrap();
    assert!(forced.imported.contains(&"portable-post".to_owned()));
    let revised = target
        .find_public_by_slug(&Slug::parse("portable-post").unwrap(), Utc::now())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(revised.title, "Portable post, revised");
    assert_eq!(revised.body_markdown, "New body.\n");
}

#[tokio::test]
async fn plain_markdown_files_become_drafts_titled_from_the_first_heading() {
    let source = tempfile::tempdir().unwrap();
    let folder = source.path().join("notes");
    std::fs::create_dir_all(folder.join("posts")).unwrap();
    std::fs::write(
        folder.join("posts/2026-09-03-morning.md"),
        "# A morning note\n\nCoffee first.\n",
    )
    .unwrap();
    std::fs::write(
        folder.join("posts/Second Thoughts.md"),
        "No heading here.\n",
    )
    .unwrap();
    std::fs::write(folder.join("posts/README.txt"), "not markdown").unwrap();
    let destination = tempfile::tempdir().unwrap();
    let config = config_for(destination.path());
    let repository = Arc::new(
        SqliteRepository::connect(&config.database_path())
            .await
            .unwrap(),
    );

    let report = Importer::import(&config, &repository, &folder, false, Utc::now())
        .await
        .unwrap();
    assert_eq!(
        report.imported,
        vec![
            "2026-09-03-morning".to_owned(),
            "second-thoughts".to_owned()
        ]
    );
    let mut pieces = repository.list_all_content().await.unwrap();
    pieces.sort_by(|a, b| a.slug.as_str().cmp(b.slug.as_str()));
    assert_eq!(pieces[0].title, "A morning note");
    assert_eq!(pieces[0].publication, Publication::Draft);
    assert_eq!(pieces[1].title, "Second Thoughts");
    assert_eq!(pieces[1].kind, ContentKind::Post);

    let missing = Importer::import(
        &config,
        &repository,
        &folder.join("nowhere"),
        false,
        Utc::now(),
    )
    .await;
    assert!(matches!(missing, Err(OperationError::InvalidData(_))));
}

fn config_for(data_dir: &std::path::Path) -> Config {
    config(data_dir)
}

#[tokio::test]
async fn backup_rotation_keeps_the_newest_n() {
    let temp = tempfile::tempdir().unwrap();
    let (config, repository) = seeded(&temp).await;
    let base = chrono::DateTime::parse_from_rfc3339("2026-09-03T01:00:00Z")
        .unwrap()
        .with_timezone(&Utc);
    for hour in 0..3 {
        BackupService::create(
            &config,
            repository.as_ref(),
            None,
            base + chrono::Duration::hours(hour),
        )
        .await
        .unwrap();
    }
    std::fs::write(config.backup_dir().join("notes.txt"), "left alone").unwrap();

    let removed = BackupService::prune(&config, 2).unwrap();
    assert_eq!(removed.len(), 1);
    assert!(removed[0].ends_with("simple-blog-20260903-010000.tar.zst"));
    let mut remaining = std::fs::read_dir(config.backup_dir())
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect::<Vec<_>>();
    remaining.sort();
    assert_eq!(
        remaining,
        vec![
            "notes.txt",
            "simple-blog-20260903-020000.tar.zst",
            "simple-blog-20260903-030000.tar.zst",
        ]
    );
    assert!(BackupService::prune(&config, 5).unwrap().is_empty());
}
