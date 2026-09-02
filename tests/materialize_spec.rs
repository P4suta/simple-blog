use std::sync::Arc;

use simple_blog::{
    materialize::{MaterializeError, ReleaseMaterializer},
    release::{
        FilesystemReleaseStore, ReleaseBuilder, ReleasePublisher, ReleaseReader, ReleaseStore,
    },
};

async fn published() -> (
    tempfile::TempDir,
    Arc<FilesystemReleaseStore>,
    simple_blog::release::ReleaseId,
) {
    let temp = tempfile::tempdir().unwrap();
    let store = Arc::new(FilesystemReleaseStore::new(temp.path().join("releases")));
    let release = ReleaseBuilder::clean(2, "https://writing.example")
        .unwrap()
        .asset("/", b"home".to_vec(), "text/html; charset=utf-8", None)
        .unwrap()
        .asset(
            "/essay/",
            b"essay".to_vec(),
            "text/html; charset=utf-8",
            Some(7),
        )
        .unwrap()
        .asset(
            "/assets/app.js",
            b"app".to_vec(),
            "text/javascript; charset=utf-8",
            None,
        )
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
        .redirect("/old/", "/essay/", 301)
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
async fn materializer_verifies_and_installs_a_complete_inspectable_tree() {
    let (temp, store, release_id) = published().await;
    let output = temp.path().join("site");

    let report = ReleaseMaterializer::new(store)
        .materialize(&output)
        .await
        .unwrap();

    assert_eq!(report.release_id, release_id);
    assert_eq!(report.asset_count, 4);
    assert_eq!(report.redirect_count, 1);
    assert_eq!(report.total_bytes, 19);
    assert_eq!(std::fs::read(output.join("index.html")).unwrap(), b"home");
    assert_eq!(
        std::fs::read(output.join("essay/index.html")).unwrap(),
        b"essay"
    );
    assert_eq!(std::fs::read(output.join("assets/app.js")).unwrap(), b"app");
    assert_eq!(std::fs::read(output.join("404.html")).unwrap(), b"missing");
    assert_eq!(
        std::fs::read_to_string(output.join("_redirects")).unwrap(),
        "/old/ /essay/ 301\n"
    );
    let manifest = std::fs::read(output.join(".simple-blog-release.json")).unwrap();
    assert_eq!(
        simple_blog::release::ReleaseManifest::from_bytes(&manifest)
            .unwrap()
            .id()
            .unwrap(),
        release_id
    );
}

#[tokio::test]
async fn materializer_never_overwrites_an_existing_target() {
    let (temp, store, _release_id) = published().await;
    let output = temp.path().join("site");
    std::fs::create_dir(&output).unwrap();
    std::fs::write(output.join("owned.txt"), b"keep").unwrap();

    let error = ReleaseMaterializer::new(store)
        .materialize(&output)
        .await
        .unwrap_err();

    assert_eq!(error, MaterializeError::OutputExists(output));
    assert_eq!(
        std::fs::read(error_output(&error).join("owned.txt")).unwrap(),
        b"keep"
    );
}

#[tokio::test]
async fn corruption_aborts_without_exposing_a_partial_output() {
    let (temp, store, _release_id) = published().await;
    let active = store.active().await.unwrap().unwrap();
    let manifest = store.manifest(&active.id).await.unwrap();
    let object_id = manifest.routes["/essay/"].object_id().unwrap();
    std::fs::write(store.root().join("objects").join(object_id), b"bad").unwrap();
    let output = temp.path().join("site");

    let error = ReleaseMaterializer::new(store)
        .materialize(&output)
        .await
        .unwrap_err();

    assert!(error.to_string().contains(object_id));
    assert!(!output.exists());
    let leftovers = std::fs::read_dir(temp.path())
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| name.contains("materializing"))
        .collect::<Vec<_>>();
    assert!(
        leftovers.is_empty(),
        "temporary outputs remain: {leftovers:?}"
    );
}

fn error_output(error: &MaterializeError) -> &std::path::Path {
    let MaterializeError::OutputExists(path) = error else {
        panic!("unexpected error")
    };
    path
}
