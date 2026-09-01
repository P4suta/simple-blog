use std::{collections::BTreeMap, sync::Arc};

use async_trait::async_trait;
use base64::Engine as _;
use serde::Deserialize;
use simple_blog::release::{
    ActiveRelease, PreparedRelease, ReleaseError, ReleaseId, ReleaseManifest, ReleaseReader,
    ReleaseResolver, ReleaseStore, ResolvedRoute,
};

#[derive(Deserialize)]
struct Contract {
    format_version: u16,
    active_release: String,
    manifest: serde_json::Value,
    objects: BTreeMap<String, String>,
    cases: Vec<ContractCase>,
}

#[derive(Deserialize)]
struct ContractCase {
    path: String,
    kind: String,
    status: u16,
    object_id: Option<String>,
    location: Option<String>,
    fallback: Option<bool>,
}

struct FixtureStore {
    active: ReleaseId,
    manifest: ReleaseManifest,
    objects: BTreeMap<String, Vec<u8>>,
}

#[async_trait]
impl ReleaseStore for FixtureStore {
    async fn put_object(&self, _id: &str, _bytes: &[u8]) -> Result<(), ReleaseError> {
        Err(ReleaseError::Store("fixture is read-only".into()))
    }

    async fn put_manifest(&self, _release: &PreparedRelease) -> Result<(), ReleaseError> {
        Err(ReleaseError::Store("fixture is read-only".into()))
    }

    async fn active(&self) -> Result<Option<ActiveRelease>, ReleaseError> {
        Ok(Some(ActiveRelease::new(self.active.clone())))
    }

    async fn activate(
        &self,
        _expected: Option<&ReleaseId>,
        _replacement: &ReleaseId,
    ) -> Result<(), ReleaseError> {
        Err(ReleaseError::Store("fixture is read-only".into()))
    }
}

#[async_trait]
impl ReleaseReader for FixtureStore {
    async fn manifest(&self, _id: &ReleaseId) -> Result<ReleaseManifest, ReleaseError> {
        Ok(self.manifest.clone())
    }

    async fn object(&self, id: &str) -> Result<Vec<u8>, ReleaseError> {
        self.objects
            .get(id)
            .cloned()
            .ok_or_else(|| ReleaseError::NotFound {
                kind: "fixture object",
                id: id.into(),
            })
    }
}

#[tokio::test]
async fn rust_and_cloudflare_resolvers_share_the_same_versioned_contract() {
    let contract: Contract =
        serde_json::from_str(include_str!("../contracts/release-resolution-v1.json")).unwrap();
    assert_eq!(contract.format_version, 1);
    assert!(!contract.cases.is_empty(), "contract fixture has no cases");
    let manifest =
        ReleaseManifest::from_bytes(&serde_json::to_vec(&contract.manifest).unwrap()).unwrap();
    let objects = contract
        .objects
        .into_iter()
        .map(|(id, encoded)| {
            Ok((
                id,
                base64::engine::general_purpose::STANDARD.decode(encoded)?,
            ))
        })
        .collect::<Result<BTreeMap<_, _>, base64::DecodeError>>()
        .unwrap();
    let store = Arc::new(FixtureStore {
        active: ReleaseId::parse(contract.active_release).unwrap(),
        manifest,
        objects,
    });
    let resolver = ReleaseResolver::new(store);

    for scenario in contract.cases {
        let actual = resolver.resolve(&scenario.path).await.unwrap();
        match actual {
            ResolvedRoute::Asset(asset) => {
                assert_eq!(scenario.kind, "asset", "{}", scenario.path);
                assert_eq!(scenario.location, None, "{}", scenario.path);
                assert_eq!(asset.status, scenario.status, "{}", scenario.path);
                assert_eq!(
                    Some(asset.object_id),
                    scenario.object_id,
                    "{}",
                    scenario.path
                );
                assert_eq!(Some(asset.fallback), scenario.fallback, "{}", scenario.path);
            }
            ResolvedRoute::Redirect(redirect) => {
                assert_eq!(scenario.kind, "redirect", "{}", scenario.path);
                assert_eq!(scenario.object_id, None, "{}", scenario.path);
                assert_eq!(redirect.status, scenario.status, "{}", scenario.path);
                assert_eq!(
                    Some(redirect.location),
                    scenario.location,
                    "{}",
                    scenario.path
                );
            }
        }
    }
}
