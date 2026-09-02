//! Deterministic, host-neutral public release contracts.
//!
//! A release is immutable. Adapters first persist every referenced object and
//! the manifest, then replace one active pointer. A failed build or store write
//! therefore cannot expose a partially generated site.

use std::{
    collections::{BTreeMap, HashSet},
    fmt,
    path::{Path, PathBuf},
    sync::Arc,
};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::{io::AsyncWriteExt, sync::Mutex};
use url::Url;

pub const RELEASE_FORMAT_VERSION: u16 = 1;

#[derive(Clone, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct ReleaseId(String);

impl ReleaseId {
    pub fn parse(value: impl Into<String>) -> Result<Self, ReleaseError> {
        let value = value.into();
        if value.len() != 64
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(ReleaseError::InvalidIdentity);
        }
        Ok(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ReleaseId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ReleaseManifest {
    pub format_version: u16,
    pub compiler_version: String,
    pub public_revision: u64,
    pub canonical_origin: String,
    pub routes: BTreeMap<String, ReleaseRoute>,
}

impl ReleaseManifest {
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, ReleaseError> {
        let manifest: Self = serde_json::from_slice(bytes)
            .map_err(|error| ReleaseError::InvalidManifest(error.to_string()))?;
        manifest.validate()?;
        Ok(manifest)
    }

    pub fn canonical_bytes(&self) -> Result<Vec<u8>, ReleaseError> {
        self.validate()?;
        serde_json::to_vec(self).map_err(|error| ReleaseError::InvalidManifest(error.to_string()))
    }

    pub fn id(&self) -> Result<ReleaseId, ReleaseError> {
        let bytes = self.canonical_bytes()?;
        ReleaseId::parse(blake3::hash(&bytes).to_hex().to_string())
    }

    fn validate(&self) -> Result<(), ReleaseError> {
        if self.format_version != RELEASE_FORMAT_VERSION {
            return Err(ReleaseError::UnsupportedFormat(self.format_version));
        }
        canonical_origin(&self.canonical_origin)?;
        if self.compiler_version.trim().is_empty() {
            return Err(ReleaseError::InvalidManifest(
                "compiler version is empty".into(),
            ));
        }
        for (path, route) in &self.routes {
            validate_route(path)?;
            route.validate()?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ReleaseRoute {
    Asset {
        object_id: String,
        content_type: String,
        cache_control: String,
        status: u16,
        #[serde(skip_serializing_if = "Option::is_none")]
        last_modified: Option<DateTime<Utc>>,
        #[serde(skip_serializing_if = "Option::is_none")]
        content_id: Option<i64>,
    },
    Redirect {
        status: u16,
        location: String,
    },
}

impl ReleaseRoute {
    fn validate(&self) -> Result<(), ReleaseError> {
        match self {
            Self::Asset {
                object_id,
                content_type,
                cache_control,
                status,
                ..
            } => {
                if !matches!(status, 200 | 404 | 410) {
                    return Err(ReleaseError::InvalidAssetStatus(*status));
                }
                ReleaseId::parse(object_id.clone())?;
                validate_header_value("content type", content_type)?;
                validate_header_value("cache control", cache_control)
            }
            Self::Redirect { status, location } => {
                if !matches!(status, 301 | 302 | 307 | 308) {
                    return Err(ReleaseError::InvalidRedirectStatus(*status));
                }
                validate_route(location)
            }
        }
    }

    #[must_use]
    pub fn object_id(&self) -> Option<&str> {
        match self {
            Self::Asset { object_id, .. } => Some(object_id),
            Self::Redirect { .. } => None,
        }
    }

    #[must_use]
    pub const fn status(&self) -> Option<u16> {
        match self {
            Self::Asset { status, .. } | Self::Redirect { status, .. } => Some(*status),
        }
    }

    #[must_use]
    pub const fn last_modified(&self) -> Option<DateTime<Utc>> {
        match self {
            Self::Asset { last_modified, .. } => *last_modified,
            Self::Redirect { .. } => None,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PreparedRelease {
    pub id: ReleaseId,
    pub manifest: ReleaseManifest,
    pub manifest_bytes: Vec<u8>,
    /// Only newly rendered objects. Existing content-addressed objects are
    /// deliberately omitted from incremental builds.
    pub objects: BTreeMap<String, Vec<u8>>,
}

pub struct ReleaseBuilder {
    manifest: ReleaseManifest,
    previous_objects: HashSet<String>,
    objects: BTreeMap<String, Vec<u8>>,
}

impl ReleaseBuilder {
    pub fn clean(public_revision: u64, origin: &str) -> Result<Self, ReleaseError> {
        Ok(Self {
            manifest: ReleaseManifest {
                format_version: RELEASE_FORMAT_VERSION,
                compiler_version: env!("CARGO_PKG_VERSION").to_owned(),
                public_revision,
                canonical_origin: canonical_origin(origin)?,
                routes: BTreeMap::new(),
            },
            previous_objects: HashSet::new(),
            objects: BTreeMap::new(),
        })
    }

    /// Starts a complete route snapshot while reusing object identities from
    /// the previous manifest. Callers must add every route that belongs in the
    /// replacement release; omitted routes are deliberately pruned.
    pub fn incremental(
        public_revision: u64,
        origin: &str,
        previous: &ReleaseManifest,
    ) -> Result<Self, ReleaseError> {
        previous.validate()?;
        let origin = canonical_origin(origin)?;
        if origin != previous.canonical_origin {
            return Err(ReleaseError::OriginChanged {
                previous: previous.canonical_origin.clone(),
                replacement: origin,
            });
        }
        let previous_objects = previous
            .routes
            .values()
            .filter_map(ReleaseRoute::object_id)
            .map(str::to_owned)
            .collect();
        Ok(Self {
            manifest: ReleaseManifest {
                format_version: RELEASE_FORMAT_VERSION,
                compiler_version: env!("CARGO_PKG_VERSION").to_owned(),
                public_revision,
                canonical_origin: previous.canonical_origin.clone(),
                routes: BTreeMap::new(),
            },
            previous_objects,
            objects: BTreeMap::new(),
        })
    }

    pub fn asset(
        self,
        path: &str,
        bytes: Vec<u8>,
        content_type: &str,
        content_id: Option<i64>,
    ) -> Result<Self, ReleaseError> {
        self.asset_with_metadata(path, bytes, content_type, content_id, 200, None)
    }

    pub fn asset_with_metadata(
        mut self,
        path: &str,
        bytes: Vec<u8>,
        content_type: &str,
        content_id: Option<i64>,
        status: u16,
        last_modified: Option<DateTime<Utc>>,
    ) -> Result<Self, ReleaseError> {
        validate_route(path)?;
        validate_header_value("content type", content_type)?;
        if !matches!(status, 200 | 404 | 410) {
            return Err(ReleaseError::InvalidAssetStatus(status));
        }
        let object_id = blake3::hash(&bytes).to_hex().to_string();
        if !self.previous_objects.contains(&object_id) {
            self.objects.insert(object_id.clone(), bytes);
        }
        self.manifest.routes.insert(
            path.to_owned(),
            ReleaseRoute::Asset {
                object_id,
                content_type: content_type.to_owned(),
                cache_control: cache_policy(path).to_owned(),
                status,
                last_modified,
                content_id,
            },
        );
        Ok(self)
    }

    pub fn redirect(
        mut self,
        path: &str,
        location: &str,
        status: u16,
    ) -> Result<Self, ReleaseError> {
        validate_route(path)?;
        validate_route(location)?;
        if !matches!(status, 301 | 302 | 307 | 308) {
            return Err(ReleaseError::InvalidRedirectStatus(status));
        }
        self.manifest.routes.insert(
            path.to_owned(),
            ReleaseRoute::Redirect {
                status,
                location: location.to_owned(),
            },
        );
        Ok(self)
    }

    pub fn remove(mut self, path: &str) -> Result<Self, ReleaseError> {
        validate_route(path)?;
        self.manifest.routes.remove(path);
        Ok(self)
    }

    #[must_use]
    pub fn canonical_origin(&self) -> &str {
        &self.manifest.canonical_origin
    }

    pub fn finish(self) -> Result<PreparedRelease, ReleaseError> {
        let manifest_bytes = self.manifest.canonical_bytes()?;
        let id = ReleaseId::parse(blake3::hash(&manifest_bytes).to_hex().to_string())?;
        Ok(PreparedRelease {
            id,
            manifest: self.manifest,
            manifest_bytes,
            objects: self.objects,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ActiveRelease {
    pub id: ReleaseId,
}

impl ActiveRelease {
    #[must_use]
    pub const fn new(id: ReleaseId) -> Self {
        Self { id }
    }
}

#[async_trait]
pub trait ReleaseStore: Send + Sync {
    async fn put_object(&self, id: &str, bytes: &[u8]) -> Result<(), ReleaseError>;
    async fn put_manifest(&self, release: &PreparedRelease) -> Result<(), ReleaseError>;
    async fn active(&self) -> Result<Option<ActiveRelease>, ReleaseError>;
    async fn activate(
        &self,
        expected: Option<&ReleaseId>,
        replacement: &ReleaseId,
    ) -> Result<(), ReleaseError>;
}

#[async_trait]
pub trait ReleaseReader: Send + Sync {
    async fn manifest(&self, id: &ReleaseId) -> Result<ReleaseManifest, ReleaseError>;
    async fn object(&self, id: &str) -> Result<Vec<u8>, ReleaseError>;
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolvedAsset {
    pub release_id: ReleaseId,
    pub object_id: String,
    pub status: u16,
    pub content_type: String,
    pub cache_control: String,
    pub last_modified: Option<DateTime<Utc>>,
    pub content_id: Option<i64>,
    pub body: Vec<u8>,
    pub fallback: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolvedRedirect {
    pub release_id: ReleaseId,
    pub status: u16,
    pub location: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ResolvedRoute {
    Asset(ResolvedAsset),
    Redirect(ResolvedRedirect),
}

/// Resolves one request from an atomically activated release.
///
/// Filesystem or host-specific response types stay outside this boundary.
/// Native and edge adapters must preserve every returned field.
pub struct ReleaseResolver<S: ReleaseStore + ReleaseReader + ?Sized> {
    store: Arc<S>,
}

impl<S: ReleaseStore + ReleaseReader + ?Sized> ReleaseResolver<S> {
    #[must_use]
    pub const fn new(store: Arc<S>) -> Self {
        Self { store }
    }

    #[tracing::instrument(name = "release.resolve", skip_all, fields(path = path))]
    pub async fn resolve(&self, path: &str) -> Result<ResolvedRoute, ReleaseError> {
        validate_route(path)?;
        let active = self
            .store
            .active()
            .await?
            .ok_or_else(|| ReleaseError::NotFound {
                kind: "active release",
                id: "active".into(),
            })?;
        let manifest = self.store.manifest(&active.id).await?;
        let (route, fallback) = match manifest.routes.get(path) {
            Some(route) => (route, false),
            None => (
                manifest
                    .routes
                    .get("/404/")
                    .ok_or_else(|| ReleaseError::NotFound {
                        kind: "release route",
                        id: format!("{path} (and /404/ fallback)"),
                    })?,
                true,
            ),
        };
        match route {
            ReleaseRoute::Asset {
                object_id,
                content_type,
                cache_control,
                status,
                last_modified,
                content_id,
            } => {
                let body = self.store.object(object_id).await?;
                tracing::debug!(
                    event = "release.resolve.asset",
                    release_id = %active.id,
                    object_id,
                    status,
                    fallback
                );
                Ok(ResolvedRoute::Asset(ResolvedAsset {
                    release_id: active.id,
                    object_id: object_id.clone(),
                    status: *status,
                    content_type: content_type.clone(),
                    cache_control: cache_control.clone(),
                    last_modified: *last_modified,
                    content_id: *content_id,
                    body,
                    fallback,
                }))
            }
            ReleaseRoute::Redirect { status, location } => {
                tracing::debug!(
                    event = "release.resolve.redirect",
                    release_id = %active.id,
                    status,
                    location
                );
                Ok(ResolvedRoute::Redirect(ResolvedRedirect {
                    release_id: active.id,
                    status: *status,
                    location: location.clone(),
                }))
            }
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ActiveReleaseVerification {
    pub release_id: ReleaseId,
    pub object_count: usize,
    pub total_bytes: u64,
}

pub struct FilesystemReleaseStore {
    root: PathBuf,
    activation: Mutex<()>,
}

impl FilesystemReleaseStore {
    #[must_use]
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            activation: Mutex::new(()),
        }
    }

    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    pub async fn verify_active(&self) -> Result<ActiveReleaseVerification, ReleaseError> {
        let active = self.active().await?.ok_or_else(|| ReleaseError::NotFound {
            kind: "active release",
            id: self.active_path().display().to_string(),
        })?;
        let manifest = self.manifest(&active.id).await?;
        let mut objects = HashSet::new();
        let mut total_bytes = 0_u64;
        for object_id in manifest.routes.values().filter_map(ReleaseRoute::object_id) {
            if objects.insert(object_id) {
                let bytes = self.object(object_id).await?;
                total_bytes = total_bytes.saturating_add(
                    u64::try_from(bytes.len())
                        .map_err(|error| ReleaseError::Store(error.to_string()))?,
                );
            }
        }
        Ok(ActiveReleaseVerification {
            release_id: active.id,
            object_count: objects.len(),
            total_bytes,
        })
    }

    fn objects_dir(&self) -> PathBuf {
        self.root.join("objects")
    }

    fn manifests_dir(&self) -> PathBuf {
        self.root.join("manifests")
    }

    fn active_path(&self) -> PathBuf {
        self.root.join("active")
    }

    async fn ensure_layout(&self) -> Result<(), ReleaseError> {
        create_dir_all(&self.objects_dir()).await?;
        create_dir_all(&self.manifests_dir()).await
    }

    async fn read_active_id(&self) -> Result<Option<ReleaseId>, ReleaseError> {
        let path = self.active_path();
        let bytes = match tokio::fs::read(&path).await {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(io("read active release", &path, &error)),
        };
        let value = std::str::from_utf8(&bytes)
            .map_err(|error| ReleaseError::Integrity {
                kind: "active pointer",
                id: path.display().to_string(),
                detail: error.to_string(),
            })?
            .trim();
        ReleaseId::parse(value.to_owned())
            .map(Some)
            .map_err(|_| ReleaseError::Integrity {
                kind: "active pointer",
                id: path.display().to_string(),
                detail: "identity is malformed".into(),
            })
    }
}

#[async_trait]
impl ReleaseStore for FilesystemReleaseStore {
    async fn put_object(&self, id: &str, bytes: &[u8]) -> Result<(), ReleaseError> {
        let id = ReleaseId::parse(id.to_owned())?;
        verify_checksum("object", id.as_str(), bytes)?;
        self.ensure_layout().await?;
        write_content_addressed(&self.objects_dir().join(id.as_str()), bytes).await
    }

    async fn put_manifest(&self, release: &PreparedRelease) -> Result<(), ReleaseError> {
        release.manifest.validate()?;
        let actual = ReleaseId::parse(blake3::hash(&release.manifest_bytes).to_hex().to_string())?;
        if actual != release.id || release.manifest.canonical_bytes()? != release.manifest_bytes {
            return Err(ReleaseError::Integrity {
                kind: "manifest",
                id: release.id.to_string(),
                detail: "checksum or canonical encoding does not match release identity".into(),
            });
        }
        self.ensure_layout().await?;
        write_content_addressed(
            &self.manifests_dir().join(format!("{}.json", release.id)),
            &release.manifest_bytes,
        )
        .await
    }

    async fn active(&self) -> Result<Option<ActiveRelease>, ReleaseError> {
        self.read_active_id()
            .await
            .map(|active| active.map(ActiveRelease::new))
    }

    async fn activate(
        &self,
        expected: Option<&ReleaseId>,
        replacement: &ReleaseId,
    ) -> Result<(), ReleaseError> {
        let _guard = self.activation.lock().await;
        let actual = self.read_active_id().await?;
        if actual.as_ref() != expected {
            return Err(ReleaseError::Conflict {
                expected: expected.cloned(),
                actual,
            });
        }

        // Activation is the trust boundary: verify the complete graph again
        // immediately before making it visible.
        let manifest = self.manifest(replacement).await?;
        for object_id in manifest.routes.values().filter_map(ReleaseRoute::object_id) {
            self.object(object_id).await?;
        }
        self.ensure_layout().await?;
        atomic_replace(&self.active_path(), format!("{replacement}\n").as_bytes()).await
    }
}

#[async_trait]
impl ReleaseReader for FilesystemReleaseStore {
    async fn manifest(&self, id: &ReleaseId) -> Result<ReleaseManifest, ReleaseError> {
        let path = self.manifests_dir().join(format!("{id}.json"));
        let bytes = read_required("manifest", id.as_str(), &path).await?;
        verify_checksum("manifest", id.as_str(), &bytes)?;
        let manifest = ReleaseManifest::from_bytes(&bytes)?;
        if manifest.id()? != *id {
            return Err(ReleaseError::Integrity {
                kind: "manifest",
                id: id.to_string(),
                detail: "canonical manifest checksum mismatch".into(),
            });
        }
        Ok(manifest)
    }

    async fn object(&self, id: &str) -> Result<Vec<u8>, ReleaseError> {
        let id = ReleaseId::parse(id.to_owned())?;
        let path = self.objects_dir().join(id.as_str());
        let bytes = read_required("object", id.as_str(), &path).await?;
        verify_checksum("object", id.as_str(), &bytes)?;
        Ok(bytes)
    }
}

pub struct ReleasePublisher<S: ReleaseStore + ?Sized> {
    store: Arc<S>,
}

impl<S: ReleaseStore + ?Sized> ReleasePublisher<S> {
    #[must_use]
    pub const fn new(store: Arc<S>) -> Self {
        Self { store }
    }

    #[tracing::instrument(
        name = "release.publish",
        skip_all,
        fields(release_id = %release.id, public_revision = release.manifest.public_revision)
    )]
    pub async fn publish(
        &self,
        release: &PreparedRelease,
        expected: Option<&ReleaseId>,
    ) -> Result<(), ReleaseError> {
        tracing::info!(
            event = "release.publish.started",
            object_count = release.objects.len()
        );
        for (id, bytes) in &release.objects {
            if let Err(error) = self.store.put_object(id, bytes).await {
                tracing::error!(
                    event = "release.publish.failed",
                    error_code = "release_object_store_failed",
                    phase = "object",
                    error = %error
                );
                return Err(error);
            }
        }
        if let Err(error) = self.store.put_manifest(release).await {
            tracing::error!(
                event = "release.publish.failed",
                error_code = "release_manifest_store_failed",
                phase = "manifest",
                error = %error
            );
            return Err(error);
        }
        if let Err(error) = self.store.activate(expected, &release.id).await {
            tracing::error!(
                event = "release.publish.failed",
                error_code = "release_activation_failed",
                phase = "activation",
                error = %error
            );
            return Err(error);
        }
        tracing::info!(event = "release.publish.completed");
        Ok(())
    }
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum ReleaseError {
    #[error("release identity must be a 64-character lowercase BLAKE3 digest")]
    InvalidIdentity,
    #[error("invalid canonical origin: {0}")]
    InvalidOrigin(String),
    #[error("incremental build cannot change canonical origin from {previous} to {replacement}")]
    OriginChanged {
        previous: String,
        replacement: String,
    },
    #[error("invalid release route: {0}")]
    InvalidRoute(String),
    #[error("invalid redirect status: {0}")]
    InvalidRedirectStatus(u16),
    #[error("invalid asset status: {0}")]
    InvalidAssetStatus(u16),
    #[error("invalid {name}: {reason}")]
    InvalidHeader { name: &'static str, reason: String },
    #[error("unsupported release format version: {0}")]
    UnsupportedFormat(u16),
    #[error("invalid release manifest: {0}")]
    InvalidManifest(String),
    #[error("release store failure: {0}")]
    Store(String),
    #[error("{operation} failed for {path}: {message}")]
    Io {
        operation: &'static str,
        path: String,
        message: String,
    },
    #[error("{kind} {id} failed integrity verification: {detail}")]
    Integrity {
        kind: &'static str,
        id: String,
        detail: String,
    },
    #[error("{kind} does not exist: {id}")]
    NotFound { kind: &'static str, id: String },
    #[error("active release changed (expected {expected:?}, actual {actual:?})")]
    Conflict {
        expected: Option<ReleaseId>,
        actual: Option<ReleaseId>,
    },
}

fn canonical_origin(origin: &str) -> Result<String, ReleaseError> {
    let parsed =
        Url::parse(origin).map_err(|error| ReleaseError::InvalidOrigin(error.to_string()))?;
    if !matches!(parsed.scheme(), "http" | "https")
        || parsed.host_str().is_none()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
        || parsed.path() != "/"
    {
        return Err(ReleaseError::InvalidOrigin(origin.to_owned()));
    }
    Ok(parsed.as_str().trim_end_matches('/').to_owned())
}

fn validate_route(path: &str) -> Result<(), ReleaseError> {
    let invalid_segment = path.split('/').any(|segment| matches!(segment, "." | ".."));
    if !path.starts_with('/')
        || path.starts_with("//")
        || path.contains(['?', '#', '\\', '\0', '\r', '\n'])
        || invalid_segment
    {
        return Err(ReleaseError::InvalidRoute(path.to_owned()));
    }
    Ok(())
}

fn validate_header_value(name: &'static str, value: &str) -> Result<(), ReleaseError> {
    if value.is_empty() || value.len() > 256 || value.contains(['\r', '\n', '\0']) {
        return Err(ReleaseError::InvalidHeader {
            name,
            reason: "value is empty, too long, or contains a control character".into(),
        });
    }
    Ok(())
}

fn cache_policy(path: &str) -> &'static str {
    if path.starts_with("/assets/") || path.starts_with("/media/") {
        "public, max-age=31536000, immutable"
    } else {
        "public, max-age=0, must-revalidate"
    }
}

async fn create_dir_all(path: &Path) -> Result<(), ReleaseError> {
    tokio::fs::create_dir_all(path)
        .await
        .map_err(|error| io("create release directory", path, &error))
}

async fn read_required(kind: &'static str, id: &str, path: &Path) -> Result<Vec<u8>, ReleaseError> {
    match tokio::fs::read(path).await {
        Ok(bytes) => Ok(bytes),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Err(ReleaseError::NotFound {
            kind,
            id: id.to_owned(),
        }),
        Err(error) => Err(io("read release file", path, &error)),
    }
}

async fn write_content_addressed(path: &Path, bytes: &[u8]) -> Result<(), ReleaseError> {
    match tokio::fs::read(path).await {
        Ok(existing) if existing == bytes => return Ok(()),
        Ok(_) => {
            return Err(ReleaseError::Integrity {
                kind: "existing release file",
                id: path.display().to_string(),
                detail: "content-addressed path contains different bytes".into(),
            });
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(io("read release file", path, &error)),
    }
    atomic_create(path, bytes).await
}

async fn atomic_create(path: &Path, bytes: &[u8]) -> Result<(), ReleaseError> {
    let parent = path.parent().ok_or_else(|| {
        ReleaseError::Store(format!("release path has no parent: {}", path.display()))
    })?;
    create_dir_all(parent).await?;
    let temporary = parent.join(format!(
        ".{}.{}.tmp",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("release"),
        uuid::Uuid::new_v4()
    ));
    let result = async {
        let mut file = tokio::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary)
            .await
            .map_err(|error| io("create temporary release file", &temporary, &error))?;
        file.write_all(bytes)
            .await
            .map_err(|error| io("write temporary release file", &temporary, &error))?;
        file.sync_all()
            .await
            .map_err(|error| io("sync temporary release file", &temporary, &error))?;
        drop(file);
        match tokio::fs::rename(&temporary, path).await {
            Ok(()) => sync_directory(parent).await,
            Err(error) => Err(io("install release file", path, &error)),
        }
    }
    .await;
    if result.is_err()
        && let Err(error) = tokio::fs::remove_file(&temporary).await
        && error.kind() != std::io::ErrorKind::NotFound
    {
        tracing::warn!(
            event = "release.temporary_cleanup_failed",
            path = %temporary.display(),
            error = %error
        );
    }
    result
}

async fn atomic_replace(path: &Path, bytes: &[u8]) -> Result<(), ReleaseError> {
    let parent = path.parent().ok_or_else(|| {
        ReleaseError::Store(format!("release path has no parent: {}", path.display()))
    })?;
    create_dir_all(parent).await?;
    let temporary = parent.join(format!(".active.{}.tmp", uuid::Uuid::new_v4()));
    let result = async {
        let mut file = tokio::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary)
            .await
            .map_err(|error| io("create active pointer", &temporary, &error))?;
        file.write_all(bytes)
            .await
            .map_err(|error| io("write active pointer", &temporary, &error))?;
        file.sync_all()
            .await
            .map_err(|error| io("sync active pointer", &temporary, &error))?;
        drop(file);
        tokio::fs::rename(&temporary, path)
            .await
            .map_err(|error| io("replace active pointer", path, &error))?;
        sync_directory(parent).await
    }
    .await;
    if result.is_err()
        && let Err(error) = tokio::fs::remove_file(&temporary).await
        && error.kind() != std::io::ErrorKind::NotFound
    {
        tracing::warn!(
            event = "release.active_cleanup_failed",
            path = %temporary.display(),
            error = %error
        );
    }
    result
}

async fn sync_directory(path: &Path) -> Result<(), ReleaseError> {
    let path = path.to_owned();
    let display = path.display().to_string();
    tokio::task::spawn_blocking(move || crate::durable_fs::sync_directory(&path))
        .await
        .map_err(|error| ReleaseError::Io {
            operation: "join directory sync",
            path: display.clone(),
            message: error.to_string(),
        })?
        .map_err(|error| ReleaseError::Io {
            operation: "sync release directory",
            path: display,
            message: error.to_string(),
        })
}

fn verify_checksum(kind: &'static str, id: &str, bytes: &[u8]) -> Result<(), ReleaseError> {
    let actual = blake3::hash(bytes).to_hex().to_string();
    if actual != id {
        return Err(ReleaseError::Integrity {
            kind,
            id: id.to_owned(),
            detail: format!("checksum mismatch (actual {actual})"),
        });
    }
    Ok(())
}

fn io(operation: &'static str, path: &Path, error: &std::io::Error) -> ReleaseError {
    ReleaseError::Io {
        operation,
        path: path.display().to_string(),
        message: error.to_string(),
    }
}
