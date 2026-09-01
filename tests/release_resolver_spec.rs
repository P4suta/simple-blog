use std::sync::Arc;

use chrono::{TimeZone, Utc};
use simple_blog::release::{
    FilesystemReleaseStore, ReleaseBuilder, ReleaseError, ReleasePublisher, ReleaseResolver,
    ReleaseStore, ResolvedRoute,
};

async fn published_site() -> (
    tempfile::TempDir,
    Arc<FilesystemReleaseStore>,
    simple_blog::release::ReleaseId,
) {
    let temp = tempfile::tempdir().unwrap();
    let store = Arc::new(FilesystemReleaseStore::new(temp.path().join("releases")));
    let release = ReleaseBuilder::clean(3, "https://writing.example")
        .unwrap()
        .asset_with_metadata(
            "/essay/",
            b"essay".to_vec(),
            "text/html; charset=utf-8",
            Some(42),
            200,
            Some(Utc.with_ymd_and_hms(2026, 9, 2, 1, 2, 3).unwrap()),
        )
        .unwrap()
        .redirect("/essay", "/essay/", 308)
        .unwrap()
        .asset_with_metadata(
            "/404/",
            b"missing".to_vec(),
            "text/html; charset=utf-8",
            None,
            404,
            None,
        )
        .unwrap()
        .finish()
        .unwrap();
    ReleasePublisher::new(store.clone())
        .publish(&release, None)
        .await
        .unwrap();
    let id = release.id;
    (temp, store, id)
}

#[tokio::test]
async fn resolver_returns_host_neutral_asset_metadata_and_bytes() {
    let (_temp, store, release_id) = published_site().await;

    let ResolvedRoute::Asset(asset) = ReleaseResolver::new(store)
        .resolve("/essay/")
        .await
        .unwrap()
    else {
        panic!("expected asset");
    };

    assert_eq!(asset.release_id, release_id);
    assert_eq!(asset.status, 200);
    assert_eq!(asset.content_type, "text/html; charset=utf-8");
    assert_eq!(asset.cache_control, "public, max-age=0, must-revalidate");
    assert_eq!(asset.content_id, Some(42));
    assert_eq!(
        asset.last_modified,
        Some(Utc.with_ymd_and_hms(2026, 9, 2, 1, 2, 3).unwrap())
    );
    assert_eq!(asset.body, b"essay");
    assert!(!asset.fallback);
}

#[tokio::test]
async fn resolver_returns_redirects_and_the_release_owned_not_found_page() {
    let (_temp, store, release_id) = published_site().await;
    let resolver = ReleaseResolver::new(store);

    let ResolvedRoute::Redirect(redirect) = resolver.resolve("/essay").await.unwrap() else {
        panic!("expected redirect");
    };
    assert_eq!(redirect.release_id, release_id);
    assert_eq!(redirect.status, 308);
    assert_eq!(redirect.location, "/essay/");

    let ResolvedRoute::Asset(missing) = resolver.resolve("/does-not-exist").await.unwrap() else {
        panic!("expected fallback asset");
    };
    assert_eq!(missing.status, 404);
    assert_eq!(missing.body, b"missing");
    assert!(missing.fallback);
}

#[tokio::test]
async fn resolver_surfaces_missing_active_state_and_object_corruption() {
    let temp = tempfile::tempdir().unwrap();
    let empty = Arc::new(FilesystemReleaseStore::new(temp.path().join("empty")));
    let error = ReleaseResolver::new(empty).resolve("/").await.unwrap_err();
    assert!(matches!(
        error,
        ReleaseError::NotFound {
            kind: "active release",
            ..
        }
    ));

    let (_temp, store, _release_id) = published_site().await;
    let active = store.active().await.unwrap().unwrap();
    let manifest = simple_blog::release::ReleaseReader::manifest(store.as_ref(), &active.id)
        .await
        .unwrap();
    let object_id = manifest.routes["/essay/"].object_id().unwrap();
    std::fs::write(store.root().join("objects").join(object_id), b"corrupt").unwrap();

    let error = ReleaseResolver::new(store)
        .resolve("/essay/")
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        ReleaseError::Integrity { kind: "object", .. }
    ));
    assert!(error.to_string().contains(object_id));
}
