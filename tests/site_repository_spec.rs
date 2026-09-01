use std::sync::Arc;

use chrono::Utc;
use simple_blog::{
    application::{ports::SiteRepository, site::SiteService},
    domain::theme::{Locale, NavigationItem, SiteSettings},
    infrastructure::sqlite::SqliteRepository,
};

fn settings(title: &str) -> SiteSettings {
    SiteSettings {
        site_title: title.into(),
        site_description: "A focused publication".into(),
        locale: Locale::En,
        logo_media_id: None,
        favicon_media_id: None,
        custom_css: String::new(),
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
