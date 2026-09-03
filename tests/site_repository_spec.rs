use std::{borrow::Cow, path::Path, sync::Arc};

use chrono::{Duration, SecondsFormat, Utc};
use simple_blog::{
    application::{
        ports::{ContentRepository, SiteRepository},
        site::SiteService,
    },
    domain::{
        content::ContentId,
        theme::{Locale, NavigationItem, SiteSettings},
    },
    infrastructure::sqlite::SqliteRepository,
};
use sqlx::{
    SqlitePool,
    migrate::Migrator,
    sqlite::{SqliteConnectOptions, SqlitePoolOptions},
};

async fn migration_fixture(path: &Path, target: i64) -> SqlitePool {
    let pool = SqlitePoolOptions::new()
        .connect_with(
            SqliteConnectOptions::new()
                .filename(path)
                .create_if_missing(true),
        )
        .await
        .unwrap();
    let migration_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("migrations");
    let mut migrator = Migrator::new(migration_path).await.unwrap();
    migrator.migrations = Cow::Owned(
        migrator
            .iter()
            .filter(|migration| migration.version <= target)
            .cloned()
            .collect(),
    );
    migrator.run(&pool).await.unwrap();
    pool
}

async fn finish_migrations(pool: &SqlitePool) {
    let migration_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("migrations");
    Migrator::new(migration_path)
        .await
        .unwrap()
        .run(pool)
        .await
        .unwrap();
}

fn settings(title: &str) -> SiteSettings {
    SiteSettings {
        site_title: title.into(),
        site_description: "A focused publication".into(),
        locale: Locale::En,
        logo_media_id: None,
        favicon_media_id: None,
        custom_css: String::new(),
        timezone: "UTC".into(),
        author_name: String::new(),
        custom_css_backup: None,
    }
}

#[tokio::test]
async fn migration_seeds_default_theme_as_ordinary_custom_css() {
    let temp = tempfile::tempdir().unwrap();
    let repository = SqliteRepository::connect(&temp.path().join("blog.sqlite3"))
        .await
        .unwrap();
    let stored = repository.site_settings().await.unwrap();
    assert_eq!(
        stored.custom_css,
        include_str!("../static/default-theme.css")
    );
}

#[tokio::test]
async fn search_route_migration_preserves_legacy_content_tags_revisions_and_navigation() {
    let temp = tempfile::tempdir().unwrap();
    let database = temp.path().join("legacy-search.sqlite3");
    let pool = migration_fixture(&database, 6).await;
    let at = "2026-09-02T00:00:00+00:00";
    sqlx::query(
        "INSERT INTO contents (
           id, kind, title, slug, summary, body_markdown, body_html, status, publish_at,
           version, created_at, updated_at
         ) VALUES (7, 'post', 'Legacy search', 'search', '', '# Legacy', '<h1>Legacy</h1>',
                   'draft', NULL, 1, ?, ?)",
    )
    .bind(at)
    .bind(at)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO tags (id, name, slug) VALUES (9, 'Search', 'search')")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO content_tags (content_id, tag_id, position) VALUES (7, 9, 0)")
        .execute(&pool)
        .await
        .unwrap();
    let snapshot = serde_json::json!({
        "id": 7,
        "kind": "post",
        "title": "Legacy search",
        "slug": "search",
        "summary": "",
        "body_markdown": "# Legacy",
        "body_html": "<h1>Legacy</h1>",
        "tags": [{ "name": "Search", "slug": "search" }],
        "cover_media_id": null,
        "seo_title": null,
        "seo_description": null,
        "publication": { "state": "draft" },
        "version": 1,
        "created_at": at,
        "updated_at": at
    });
    sqlx::query(
        "INSERT INTO revisions (content_id, intent, snapshot_json, created_at)
         VALUES (7, 'explicit', ?, ?)",
    )
    .bind(snapshot.to_string())
    .bind(at)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO navigation (label, destination, is_external, position)
         VALUES ('Legacy search', '/search/', 0, 0)",
    )
    .execute(&pool)
    .await
    .unwrap();

    finish_migrations(&pool).await;
    pool.close().await;

    let repository = SqliteRepository::connect(&database).await.unwrap();
    let contents = repository.list_all_content().await.unwrap();
    assert_eq!(contents.len(), 1);
    assert_eq!(contents[0].slug.as_str(), "search-content-7");
    assert_eq!(contents[0].tags[0].slug.as_str(), "search-tag-9");
    let revisions = repository
        .list_revisions(ContentId::from_i64(7))
        .await
        .unwrap();
    assert_eq!(revisions[0].snapshot.slug.as_str(), "search-content-7");
    assert_eq!(revisions[0].snapshot.tags[0].slug.as_str(), "search-tag-9");
    assert_eq!(
        repository.navigation().await.unwrap()[0].destination,
        "/search-content-7/"
    );
}

#[tokio::test]
async fn legacy_schema_rejects_multiple_case_insensitive_search_slugs() {
    let temp = tempfile::tempdir().unwrap();
    let database = temp.path().join("legacy-search-uniqueness.sqlite3");
    let pool = migration_fixture(&database, 6).await;
    let at = "2026-09-02T00:00:00+00:00";

    sqlx::query(
        "INSERT INTO contents (
           id, kind, title, slug, summary, body_markdown, body_html, status, publish_at,
           version, created_at, updated_at
         ) VALUES (1, 'post', 'Search', 'search', '', '', '', 'draft', NULL, 1, ?, ?)",
    )
    .bind(at)
    .bind(at)
    .execute(&pool)
    .await
    .unwrap();
    assert!(
        sqlx::query(
            "INSERT INTO contents (
               id, kind, title, slug, summary, body_markdown, body_html, status, publish_at,
               version, created_at, updated_at
             ) VALUES (2, 'post', 'Search variant', 'Search', '', '', '', 'draft', NULL, 1, ?, ?)",
        )
        .bind(at)
        .bind(at)
        .execute(&pool)
        .await
        .is_err()
    );

    sqlx::query("INSERT INTO tags (id, name, slug) VALUES (1, 'Search', 'search')")
        .execute(&pool)
        .await
        .unwrap();
    assert!(
        sqlx::query("INSERT INTO tags (id, name, slug) VALUES (2, 'Search variant', 'Search')")
            .execute(&pool)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn publication_state_migration_compares_sqlx_rfc3339_timestamps() {
    let temp = tempfile::tempdir().unwrap();
    let database = temp.path().join("publication-clock.sqlite3");
    let pool = migration_fixture(&database, 9).await;
    let now = Utc::now();
    let past = now - Duration::hours(1);
    let future = now + Duration::hours(1);
    for (id, slug, publish_at) in [(1, "past", past), (2, "future", future)] {
        sqlx::query(
            "INSERT INTO contents (
               id, kind, title, slug, summary, body_markdown, body_html, status, publish_at,
               version, created_at, updated_at
             ) VALUES (?, 'post', ?, ?, '', '', '', 'public', ?, 1, ?, ?)",
        )
        .bind(id)
        .bind(slug)
        .bind(slug)
        .bind(publish_at)
        .bind(now)
        .bind(now)
        .execute(&pool)
        .await
        .unwrap();
    }

    finish_migrations(&pool).await;
    let next: String =
        sqlx::query_scalar("SELECT next_publish_at FROM publication_state WHERE singleton = 1")
            .fetch_one(&pool)
            .await
            .unwrap();

    assert_eq!(next, future.to_rfc3339_opts(SecondsFormat::AutoSi, false));
}

#[tokio::test]
async fn site_configuration_is_validated_and_replaced_atomically() {
    let temp = tempfile::tempdir().unwrap();
    let repository = Arc::new(
        SqliteRepository::connect(&temp.path().join("blog.sqlite3"))
            .await
            .unwrap(),
    );
    let service = SiteService::new(repository.clone());
    service
        .update(
            settings("Field Notes"),
            vec![
                NavigationItem {
                    id: 0,
                    label: "Home".into(),
                    destination: "/".into(),
                    is_external: false,
                    position: 20,
                },
                NavigationItem {
                    id: 0,
                    label: "Elsewhere".into(),
                    destination: "https://example.com/writing".into(),
                    is_external: true,
                    position: 10,
                },
            ],
            Utc::now(),
        )
        .await
        .unwrap();

    let stored = repository.site_settings().await.unwrap();
    let navigation = repository.navigation().await.unwrap();
    assert_eq!(stored, settings("Field Notes"));
    assert_eq!(navigation.len(), 2);
    assert_eq!(navigation[0].position, 0);
    assert_eq!(navigation[1].position, 1);

    let mut invalid = settings("Must not be stored");
    invalid.custom_css = "</style>".into();
    assert!(
        service
            .update(invalid, Vec::new(), Utc::now())
            .await
            .is_err()
    );
    assert_eq!(
        repository.site_settings().await.unwrap().site_title,
        "Field Notes"
    );
    assert_eq!(repository.navigation().await.unwrap().len(), 2);
}
#[tokio::test]
async fn page_route_migration_preserves_legacy_content_tags_revisions_and_navigation() {
    let temp = tempfile::tempdir().unwrap();
    let database = temp.path().join("legacy-page.sqlite3");
    let pool = migration_fixture(&database, 12).await;
    let at = "2026-09-02T00:00:00+00:00";
    sqlx::query(
        "INSERT INTO contents (
           id, kind, title, slug, summary, body_markdown, body_html, status, publish_at,
           version, created_at, updated_at
         ) VALUES (7, 'post', 'Legacy page', 'page', '', '# Legacy', '<h1>Legacy</h1>',
                   'draft', NULL, 1, ?, ?)",
    )
    .bind(at)
    .bind(at)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO tags (id, name, slug) VALUES (9, 'Page', 'page')")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO content_tags (content_id, tag_id, position) VALUES (7, 9, 0)")
        .execute(&pool)
        .await
        .unwrap();
    let snapshot = serde_json::json!({
        "id": 7,
        "kind": "post",
        "title": "Legacy page",
        "slug": "page",
        "summary": "",
        "body_markdown": "# Legacy",
        "body_html": "<h1>Legacy</h1>",
        "tags": [{ "name": "Page", "slug": "page" }],
        "cover_media_id": null,
        "seo_title": null,
        "seo_description": null,
        "publication": { "state": "draft" },
        "version": 1,
        "created_at": at,
        "updated_at": at
    });
    sqlx::query(
        "INSERT INTO revisions (content_id, intent, snapshot_json, created_at)
         VALUES (7, 'explicit', ?, ?)",
    )
    .bind(snapshot.to_string())
    .bind(at)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO navigation (label, destination, is_external, position)
         VALUES ('Legacy page', '/page/', 0, 0)",
    )
    .execute(&pool)
    .await
    .unwrap();

    finish_migrations(&pool).await;
    pool.close().await;

    let repository = SqliteRepository::connect(&database).await.unwrap();
    let contents = repository.list_all_content().await.unwrap();
    assert_eq!(contents.len(), 1);
    assert_eq!(contents[0].slug.as_str(), "page-content-7");
    assert_eq!(contents[0].tags[0].slug.as_str(), "page-tag-9");
    let revisions = repository
        .list_revisions(ContentId::from_i64(7))
        .await
        .unwrap();
    assert_eq!(revisions[0].snapshot.slug.as_str(), "page-content-7");
    assert_eq!(revisions[0].snapshot.tags[0].slug.as_str(), "page-tag-9");
    assert_eq!(
        repository.navigation().await.unwrap()[0].destination,
        "/page-content-7/"
    );
}

#[tokio::test]
async fn theme_refresh_updates_only_the_untouched_previous_default() {
    let temp = tempfile::tempdir().unwrap();

    let untouched = temp.path().join("untouched.sqlite3");
    let pool = migration_fixture(&untouched, 15).await;
    finish_migrations(&pool).await;
    pool.close().await;
    let repository = SqliteRepository::connect(&untouched).await.unwrap();
    assert_eq!(
        repository.site_settings().await.unwrap().custom_css,
        include_str!("../static/default-theme.css"),
        "a stylesheet still equal to the previous default receives the new one"
    );

    let customized = temp.path().join("customized.sqlite3");
    let pool = migration_fixture(&customized, 15).await;
    sqlx::query(
        "UPDATE site_settings SET custom_css = 'body { color: teal; }' WHERE singleton = 1",
    )
    .execute(&pool)
    .await
    .unwrap();
    finish_migrations(&pool).await;
    pool.close().await;
    let repository = SqliteRepository::connect(&customized).await.unwrap();
    assert_eq!(
        repository.site_settings().await.unwrap().custom_css,
        "body { color: teal; }",
        "an edited stylesheet is never clobbered"
    );
}

#[tokio::test]
async fn locale_settings_migration_adds_columns_with_defaults_and_round_trips() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("locale.sqlite3");
    let pool = migration_fixture(&path, 13).await;
    finish_migrations(&pool).await;
    pool.close().await;
    let repository = SqliteRepository::connect(&path).await.unwrap();

    let stored = repository.site_settings().await.unwrap();
    assert_eq!(stored.timezone, "UTC");
    assert_eq!(stored.author_name, "");
    assert_eq!(stored.custom_css_backup, None);

    let mut updated = settings("Field Notes");
    updated.timezone = "Asia/Tokyo".into();
    updated.author_name = "Ryo".into();
    updated.custom_css_backup = Some("body {}".into());
    repository
        .save_configuration(&updated, &[], Utc::now())
        .await
        .unwrap();
    let stored = repository.site_settings().await.unwrap();
    assert_eq!(stored.timezone, "Asia/Tokyo");
    assert_eq!(stored.author_name, "Ryo");
    assert_eq!(stored.custom_css_backup.as_deref(), Some("body {}"));
}

#[tokio::test]
async fn setup_adopts_the_browser_zone_only_while_the_site_is_still_utc() {
    let temp = tempfile::tempdir().unwrap();
    let repository = Arc::new(
        SqliteRepository::connect(&temp.path().join("adopt.sqlite3"))
            .await
            .unwrap(),
    );
    let site = SiteService::new(repository.clone());
    let now = Utc::now();

    assert!(!site.adopt_timezone_once("Nowhere/Land", now).await.unwrap());
    assert!(!site.adopt_timezone_once("Etc/UTC", now).await.unwrap());
    assert!(site.adopt_timezone_once("Asia/Tokyo", now).await.unwrap());
    assert_eq!(
        repository.site_settings().await.unwrap().timezone,
        "Asia/Tokyo"
    );
    assert!(!site.adopt_timezone_once("Europe/Paris", now).await.unwrap());
    assert_eq!(
        repository.site_settings().await.unwrap().timezone,
        "Asia/Tokyo"
    );
}
