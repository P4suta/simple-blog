use std::{
    collections::{BTreeMap, HashSet},
    sync::{Arc, Mutex},
};

use async_trait::async_trait;
use chrono::{TimeZone, Utc};
use simple_blog::release::{
    ActiveRelease, FilesystemReleaseStore, PreparedRelease, ReleaseBuilder, ReleaseError,
    ReleaseId, ReleasePublisher, ReleaseReader, ReleaseStore,
};

#[test]
fn release_identity_is_deterministic_and_independent_of_insertion_order() {
    let first = ReleaseBuilder::clean(7, "https://writing.example")
        .unwrap()
        .asset(
            "/essay/",
            b"<h1>Essay</h1>".to_vec(),
            "text/html; charset=utf-8",
            Some(42),
        )
        .unwrap()
        .asset(
            "/",
            b"<h1>Home</h1>".to_vec(),
            "text/html; charset=utf-8",
            None,
        )
        .unwrap()
        .finish()
        .unwrap();
    let second = ReleaseBuilder::clean(7, "https://writing.example/")
        .unwrap()
        .asset(
            "/",
            b"<h1>Home</h1>".to_vec(),
            "text/html; charset=utf-8",
            None,
        )
        .unwrap()
        .asset(
            "/essay/",
            b"<h1>Essay</h1>".to_vec(),
            "text/html; charset=utf-8",
            Some(42),
        )
        .unwrap()
        .finish()
        .unwrap();

    assert_eq!(first.id, second.id);
    assert_eq!(first.manifest_bytes, second.manifest_bytes);
    assert_eq!(first.manifest, second.manifest);
    assert_eq!(first.objects, second.objects);
}

#[test]
fn incremental_release_is_identical_to_a_clean_build_and_only_stages_new_objects() {
    let base = ReleaseBuilder::clean(1, "https://writing.example")
        .unwrap()
        .asset("/", b"home v1".to_vec(), "text/html; charset=utf-8", None)
        .unwrap()
        .asset(
            "/essay/",
            b"essay v1".to_vec(),
            "text/html; charset=utf-8",
            Some(7),
        )
        .unwrap()
        .finish()
        .unwrap();

    let incremental = ReleaseBuilder::incremental(2, "https://writing.example", &base.manifest)
        .unwrap()
        .asset(
            "/essay/",
            b"essay v2".to_vec(),
            "text/html; charset=utf-8",
            Some(7),
        )
        .unwrap()
        .redirect("/old/", "/essay/", 301)
        .unwrap()
        .finish()
        .unwrap();
    let clean = ReleaseBuilder::clean(2, "https://writing.example")
        .unwrap()
        .asset("/", b"home v1".to_vec(), "text/html; charset=utf-8", None)
        .unwrap()
        .asset(
            "/essay/",
            b"essay v2".to_vec(),
            "text/html; charset=utf-8",
            Some(7),
        )
        .unwrap()
        .redirect("/old/", "/essay/", 301)
        .unwrap()
        .finish()
        .unwrap();

    assert_eq!(incremental.id, clean.id);
    assert_eq!(incremental.manifest, clean.manifest);
    assert_eq!(incremental.objects.len(), 1);
    assert_eq!(clean.objects.len(), 2);
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum FailAt {
    Never,
    Object,
    Manifest,
    Activate,
}

#[derive(Default)]
struct RecordingState {
    objects: HashSet<String>,
    manifests: HashSet<String>,
    active: Option<ReleaseId>,
    events: Vec<&'static str>,
}

struct RecordingStore {
    fail_at: FailAt,
    state: Mutex<RecordingState>,
}

impl RecordingStore {
    fn new(fail_at: FailAt, active: Option<ReleaseId>) -> Self {
        Self {
            fail_at,
            state: Mutex::new(RecordingState {
                active,
                ..RecordingState::default()
            }),
        }
    }
}

#[async_trait]
impl ReleaseStore for RecordingStore {
    async fn put_object(&self, id: &str, _bytes: &[u8]) -> Result<(), ReleaseError> {
        let mut state = self.state.lock().unwrap();
        state.events.push("object");
        if self.fail_at == FailAt::Object {
            return Err(ReleaseError::Store("injected object failure".into()));
        }
        state.objects.insert(id.to_owned());
        drop(state);
        Ok(())
    }

    async fn put_manifest(&self, release: &PreparedRelease) -> Result<(), ReleaseError> {
        let mut state = self.state.lock().unwrap();
        state.events.push("manifest");
        if self.fail_at == FailAt::Manifest {
            return Err(ReleaseError::Store("injected manifest failure".into()));
        }
        state.manifests.insert(release.id.to_string());
        drop(state);
        Ok(())
    }

    async fn active(&self) -> Result<Option<ActiveRelease>, ReleaseError> {
        Ok(self
            .state
            .lock()
            .unwrap()
            .active
            .clone()
            .map(ActiveRelease::new))
    }

    async fn activate(
        &self,
        expected: Option<&ReleaseId>,
        replacement: &ReleaseId,
    ) -> Result<(), ReleaseError> {
        let mut state = self.state.lock().unwrap();
        state.events.push("activate");
        if self.fail_at == FailAt::Activate {
            return Err(ReleaseError::Store("injected activation failure".into()));
        }
        if state.active.as_ref() != expected {
            return Err(ReleaseError::Conflict {
                expected: expected.cloned(),
                actual: state.active.clone(),
            });
        }
        state.active = Some(replacement.clone());
        drop(state);
        Ok(())
    }
}

fn one_page(revision: u64, body: &[u8]) -> PreparedRelease {
    ReleaseBuilder::clean(revision, "https://writing.example")
        .unwrap()
        .asset("/", body.to_vec(), "text/html; charset=utf-8", None)
        .unwrap()
        .finish()
        .unwrap()
}

#[tokio::test]
async fn a_failure_before_activation_never_replaces_the_visible_release() {
    let old = one_page(1, b"old");
    let new = one_page(2, b"new");

    for fail_at in [FailAt::Object, FailAt::Manifest, FailAt::Activate] {
        let store = Arc::new(RecordingStore::new(fail_at, Some(old.id.clone())));
        let publisher = ReleasePublisher::new(store.clone());

        let error = publisher
            .publish(&new, Some(&old.id))
            .await
            .expect_err("fault must be visible");

        assert!(error.to_string().contains("injected"));
        assert_eq!(store.state.lock().unwrap().active, Some(old.id.clone()));
    }
}

#[tokio::test]
async fn publication_stages_objects_then_manifest_then_atomically_activates() {
    let release = one_page(1, b"published");
    let store = Arc::new(RecordingStore::new(FailAt::Never, None));

    ReleasePublisher::new(store.clone())
        .publish(&release, None)
        .await
        .unwrap();

    let (events, active) = {
        let state = store.state.lock().unwrap();
        (state.events.clone(), state.active.clone())
    };
    assert_eq!(events, ["object", "manifest", "activate"]);
    assert_eq!(active, Some(release.id));
}

#[test]
fn route_and_redirect_validation_rejects_ambiguous_or_unsafe_output() {
    for path in ["relative", "//authority/", "/has?query", "/has#fragment"] {
        assert!(
            ReleaseBuilder::clean(1, "https://writing.example")
                .unwrap()
                .asset(path, Vec::new(), "text/plain", None)
                .is_err(),
            "{path}"
        );
    }
    assert!(
        ReleaseBuilder::clean(1, "https://writing.example")
            .unwrap()
            .redirect("/old/", "https://attacker.example/", 301)
            .is_err()
    );
}

#[test]
fn manifest_json_is_a_stable_public_contract() {
    let release = one_page(9, b"hello");
    let value: serde_json::Value = serde_json::from_slice(&release.manifest_bytes).unwrap();

    assert_eq!(value["format_version"], 1);
    assert_eq!(value["public_revision"], 9);
    assert_eq!(value["canonical_origin"], "https://writing.example");
    let routes = value["routes"].as_object().unwrap();
    assert_eq!(routes.len(), 1);
    assert_eq!(routes["/"]["kind"], "asset");
    assert_eq!(routes["/"]["status"], 200);
    assert!(routes["/"]["last_modified"].is_null());

    let serialized_keys: BTreeMap<String, serde_json::Value> =
        serde_json::from_slice(&release.manifest_bytes).unwrap();
    assert!(serialized_keys.contains_key("compiler_version"));
}

#[test]
fn asset_response_metadata_is_portable_and_validated() {
    let modified = Utc.with_ymd_and_hms(2026, 9, 2, 12, 34, 56).unwrap();
    let release = ReleaseBuilder::clean(1, "https://writing.example")
        .unwrap()
        .asset_with_metadata(
            "/missing/",
            b"not found".to_vec(),
            "text/html; charset=utf-8",
            None,
            404,
            Some(modified),
        )
        .unwrap()
        .finish()
        .unwrap();
    let route = &release.manifest.routes["/missing/"];

    assert_eq!(route.status(), Some(404));
    assert_eq!(route.last_modified(), Some(modified));
    let json: serde_json::Value = serde_json::from_slice(&release.manifest_bytes).unwrap();
    assert_eq!(json["routes"]["/missing/"]["status"], 404);
    assert_eq!(
        json["routes"]["/missing/"]["last_modified"],
        "2026-09-02T12:34:56Z"
    );

    let error = ReleaseBuilder::clean(1, "https://writing.example")
        .unwrap()
        .asset_with_metadata("/", Vec::new(), "text/plain", None, 500, None)
        .err()
        .unwrap();
    assert_eq!(error, ReleaseError::InvalidAssetStatus(500));
}

#[tokio::test]
async fn filesystem_store_round_trips_and_verifies_every_active_byte() {
    let temp = tempfile::tempdir().unwrap();
    let store = Arc::new(FilesystemReleaseStore::new(temp.path().join("releases")));
    let release = one_page(1, b"durable page");

    ReleasePublisher::new(store.clone())
        .publish(&release, None)
        .await
        .unwrap();

    let active = store.active().await.unwrap().unwrap();
    assert_eq!(active.id, release.id);
    let manifest = store.manifest(&active.id).await.unwrap();
    assert_eq!(manifest, release.manifest);
    let object_id = manifest.routes["/"].object_id().unwrap();
    assert_eq!(store.object(object_id).await.unwrap(), b"durable page");

    let report = store.verify_active().await.unwrap();
    assert_eq!(report.release_id, release.id);
    assert_eq!(report.object_count, 1);
    assert_eq!(report.total_bytes, 12);
}

#[tokio::test]
async fn filesystem_store_reports_corruption_with_object_identity_and_keeps_pointer() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("releases");
    let store = Arc::new(FilesystemReleaseStore::new(root.clone()));
    let release = one_page(1, b"expected");
    ReleasePublisher::new(store.clone())
        .publish(&release, None)
        .await
        .unwrap();
    let object_id = release.manifest.routes["/"].object_id().unwrap();
    std::fs::write(root.join("objects").join(object_id), b"corrupt").unwrap();

    let error = store.verify_active().await.unwrap_err();

    assert!(error.to_string().contains(object_id));
    assert!(error.to_string().contains("checksum"));
    assert_eq!(store.active().await.unwrap().unwrap().id, release.id);
}

#[tokio::test]
async fn filesystem_activation_is_compare_and_swap_protected() {
    let temp = tempfile::tempdir().unwrap();
    let store = Arc::new(FilesystemReleaseStore::new(temp.path().join("releases")));
    let first = one_page(1, b"first");
    let second = one_page(2, b"second");
    ReleasePublisher::new(store.clone())
        .publish(&first, None)
        .await
        .unwrap();

    let stale = ReleaseId::parse("0".repeat(64)).unwrap();
    let error = ReleasePublisher::new(store.clone())
        .publish(&second, Some(&stale))
        .await
        .unwrap_err();

    assert_eq!(
        error,
        ReleaseError::Conflict {
            expected: Some(stale),
            actual: Some(first.id.clone()),
        }
    );
    assert_eq!(store.active().await.unwrap().unwrap().id, first.id);
}
