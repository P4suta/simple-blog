//! Safe filesystem materialization of an active immutable release.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::Arc,
};

use thiserror::Error;
use tokio::io::AsyncWriteExt;

use crate::release::{ReleaseError, ReleaseId, ReleaseReader, ReleaseRoute, ReleaseStore};

const MANIFEST_FILE: &str = ".simple-blog-release.json";
const REDIRECTS_FILE: &str = "_redirects";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MaterializeReport {
    pub release_id: ReleaseId,
    pub asset_count: usize,
    pub redirect_count: usize,
    pub total_bytes: u64,
}

pub struct ReleaseMaterializer<S: ReleaseStore + ReleaseReader + ?Sized> {
    store: Arc<S>,
}

impl<S: ReleaseStore + ReleaseReader + ?Sized> ReleaseMaterializer<S> {
    #[must_use]
    pub const fn new(store: Arc<S>) -> Self {
        Self { store }
    }

    #[tracing::instrument(name = "release.materialize", skip_all, fields(output = %output.display()))]
    pub async fn materialize(&self, output: &Path) -> Result<MaterializeReport, MaterializeError> {
        if path_exists(output).await? {
            return Err(MaterializeError::OutputExists(output.to_owned()));
        }
        let active = self
            .store
            .active()
            .await?
            .ok_or_else(|| ReleaseError::NotFound {
                kind: "active release",
                id: "active".into(),
            })?;
        let manifest = self.store.manifest(&active.id).await?;

        // Verify and load the complete graph before creating any output path.
        let mut assets = BTreeMap::new();
        let mut redirects = String::new();
        let mut redirect_count = 0_usize;
        let mut total_bytes = 0_u64;
        for (route_path, route) in &manifest.routes {
            match route {
                ReleaseRoute::Asset { object_id, .. } => {
                    let relative = output_path(route_path)?;
                    if matches!(relative.to_str(), Some(MANIFEST_FILE | REDIRECTS_FILE))
                        || assets.contains_key(&relative)
                    {
                        return Err(MaterializeError::OutputCollision(relative));
                    }
                    let bytes = self.store.object(object_id).await?;
                    total_bytes = total_bytes.saturating_add(
                        u64::try_from(bytes.len())
                            .map_err(|error| MaterializeError::InvalidOutput(error.to_string()))?,
                    );
                    assets.insert(relative, bytes);
                }
                ReleaseRoute::Redirect { status, location } => {
                    use std::fmt::Write as _;
                    writeln!(redirects, "{route_path} {location} {status}")
                        .map_err(|error| MaterializeError::InvalidOutput(error.to_string()))?;
                    redirect_count += 1;
                }
            }
        }
        let manifest_bytes = manifest.canonical_bytes()?;

        let parent = output.parent().ok_or_else(|| {
            MaterializeError::InvalidOutput(format!("output has no parent: {}", output.display()))
        })?;
        create_dir_all(parent).await?;
        let name = output
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| {
                MaterializeError::InvalidOutput(format!(
                    "output has no portable file name: {}",
                    output.display()
                ))
            })?;
        let staging = parent.join(format!(".{name}.materializing-{}", uuid::Uuid::new_v4()));
        create_dir(&staging).await?;
        let install = async {
            for (relative, bytes) in &assets {
                write_new_file(&staging.join(relative), bytes).await?;
            }
            write_new_file(&staging.join(REDIRECTS_FILE), redirects.as_bytes()).await?;
            write_new_file(&staging.join(MANIFEST_FILE), &manifest_bytes).await?;
            sync_directory(&staging).await?;
            if path_exists(output).await? {
                return Err(MaterializeError::OutputExists(output.to_owned()));
            }
            tokio::fs::rename(&staging, output)
                .await
                .map_err(|error| io("install materialized release", output, &error))?;
            sync_directory(parent).await
        }
        .await;
        if install.is_err()
            && let Err(error) = tokio::fs::remove_dir_all(&staging).await
            && error.kind() != std::io::ErrorKind::NotFound
        {
            tracing::warn!(
                event = "release.materialize.cleanup_failed",
                path = %staging.display(),
                error = %error
            );
        }
        install?;

        let report = MaterializeReport {
            release_id: active.id,
            asset_count: assets.len(),
            redirect_count,
            total_bytes,
        };
        tracing::info!(
            event = "release.materialize.completed",
            release_id = %report.release_id,
            asset_count = report.asset_count,
            redirect_count = report.redirect_count,
            total_bytes = report.total_bytes
        );
        Ok(report)
    }
}

fn output_path(route: &str) -> Result<PathBuf, MaterializeError> {
    if route == "/" {
        return Ok(PathBuf::from("index.html"));
    }
    if route == "/404/" {
        return Ok(PathBuf::from("404.html"));
    }
    let relative = route
        .strip_prefix('/')
        .ok_or_else(|| MaterializeError::InvalidRoute(route.to_owned()))?;
    if relative.is_empty() || relative.contains(['\\', '\0']) {
        return Err(MaterializeError::InvalidRoute(route.to_owned()));
    }
    Ok(relative.strip_suffix('/').map_or_else(
        || PathBuf::from(relative),
        |directory| PathBuf::from(directory).join("index.html"),
    ))
}

async fn path_exists(path: &Path) -> Result<bool, MaterializeError> {
    tokio::fs::try_exists(path)
        .await
        .map_err(|error| io("inspect materialization target", path, &error))
}

async fn create_dir(path: &Path) -> Result<(), MaterializeError> {
    tokio::fs::create_dir(path)
        .await
        .map_err(|error| io("create materialization staging directory", path, &error))
}

async fn create_dir_all(path: &Path) -> Result<(), MaterializeError> {
    tokio::fs::create_dir_all(path)
        .await
        .map_err(|error| io("create materialization directory", path, &error))
}

async fn write_new_file(path: &Path, bytes: &[u8]) -> Result<(), MaterializeError> {
    let parent = path.parent().ok_or_else(|| {
        MaterializeError::InvalidOutput(format!("file has no parent: {}", path.display()))
    })?;
    create_dir_all(parent).await?;
    let mut file = tokio::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(path)
        .await
        .map_err(|error| io("create materialized file", path, &error))?;
    file.write_all(bytes)
        .await
        .map_err(|error| io("write materialized file", path, &error))?;
    file.sync_all()
        .await
        .map_err(|error| io("sync materialized file", path, &error))
}

async fn sync_directory(path: &Path) -> Result<(), MaterializeError> {
    let owned = path.to_owned();
    let display = path.display().to_string();
    tokio::task::spawn_blocking(move || crate::durable_fs::sync_directory(&owned))
        .await
        .map_err(|error| MaterializeError::Io {
            operation: "join directory sync",
            path: display.clone(),
            message: error.to_string(),
        })?
        .map_err(|error| MaterializeError::Io {
            operation: "sync materialization directory",
            path: display,
            message: error.to_string(),
        })
}

fn io(operation: &'static str, path: &Path, error: &std::io::Error) -> MaterializeError {
    MaterializeError::Io {
        operation,
        path: path.display().to_string(),
        message: error.to_string(),
    }
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum MaterializeError {
    #[error(transparent)]
    Release(#[from] ReleaseError),
    #[error("materialization target already exists: {0}")]
    OutputExists(PathBuf),
    #[error("materialized routes collide at output path: {0}")]
    OutputCollision(PathBuf),
    #[error("invalid route for materialization: {0}")]
    InvalidRoute(String),
    #[error("invalid materialization output: {0}")]
    InvalidOutput(String),
    #[error("{operation} failed for {path}: {message}")]
    Io {
        operation: &'static str,
        path: String,
        message: String,
    },
}
