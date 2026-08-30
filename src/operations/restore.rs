use std::{
    collections::BTreeSet,
    fs::File,
    path::{Component, Path, PathBuf},
};

use uuid::Uuid;
use walkdir::WalkDir;

use crate::{
    infrastructure::sqlite::SqliteRepository,
    operations::{BackupManifest, OperationError, checksum_file},
};

pub struct RestoreService;

impl RestoreService {
    pub async fn restore(
        archive: &Path,
        data_dir: &Path,
        force: bool,
    ) -> Result<(), OperationError> {
        if installation_exists(data_dir) && !force {
            return Err(OperationError::DestinationExists);
        }
        let parent = data_dir.parent().unwrap_or_else(|| Path::new("."));
        std::fs::create_dir_all(parent)?;
        let staging = parent.join(format!(".simple-blog-restore-{}", Uuid::new_v4()));
        std::fs::create_dir(&staging)?;
        let guard = DirectoryGuard(staging.clone());
        extract(archive, &staging)?;
        verify(&staging)?;

        let database = staging.join("database.sqlite3");
        let repository = SqliteRepository::connect(&database)
            .await
            .map_err(|error| OperationError::InvalidArchive(error.to_string()))?;
        let check: String = sqlx::query_scalar("PRAGMA quick_check")
            .fetch_one(repository.pool())
            .await
            .map_err(|error| OperationError::InvalidArchive(error.to_string()))?;
        repository.close().await;
        if check != "ok" {
            return Err(OperationError::InvalidArchive(format!(
                "SQLite quick_check failed: {check}"
            )));
        }

        std::fs::create_dir_all(data_dir)?;
        let rollback = parent.join(format!(".simple-blog-rollback-{}", Uuid::new_v4()));
        std::fs::create_dir(&rollback)?;
        let rollback_guard = DirectoryGuard(rollback.clone());
        for name in [
            "simple-blog.sqlite3",
            "simple-blog.sqlite3-wal",
            "simple-blog.sqlite3-shm",
            "config.toml",
            "media",
        ] {
            let source = data_dir.join(name);
            if source.exists() {
                std::fs::rename(&source, rollback.join(name))?;
            }
        }
        let install = (|| {
            std::fs::rename(
                staging.join("database.sqlite3"),
                data_dir.join("simple-blog.sqlite3"),
            )?;
            for name in ["config.toml", "media"] {
                let source = staging.join(name);
                if source.exists() {
                    std::fs::rename(source, data_dir.join(name))?;
                }
            }
            Ok::<_, std::io::Error>(())
        })();
        if let Err(error) = install {
            for name in ["simple-blog.sqlite3", "config.toml", "media"] {
                let installed = data_dir.join(name);
                if installed.exists() {
                    if installed.is_dir() {
                        let _ = std::fs::remove_dir_all(&installed);
                    } else {
                        let _ = std::fs::remove_file(&installed);
                    }
                }
                let previous = rollback.join(name);
                if previous.exists() {
                    let _ = std::fs::rename(previous, installed);
                }
            }
            return Err(OperationError::Io(error));
        }
        drop(rollback_guard);
        drop(guard);
        Ok(())
    }
}

fn extract(archive_path: &Path, staging: &Path) -> Result<(), OperationError> {
    let file = File::open(archive_path)?;
    let decoder = zstd::Decoder::new(file)?;
    let mut archive = tar::Archive::new(decoder);
    for entry in archive.entries()? {
        let mut entry = entry?;
        let path = entry.path()?.into_owned();
        validate_archive_path(&path)?;
        if !entry.header().entry_type().is_file() && !entry.header().entry_type().is_dir() {
            return Err(OperationError::InvalidArchive(
                "links and special files are not allowed".into(),
            ));
        }
        if !entry.unpack_in(staging)? {
            return Err(OperationError::InvalidArchive(
                "entry escaped the restore directory".into(),
            ));
        }
    }
    Ok(())
}

fn validate_archive_path(path: &Path) -> Result<(), OperationError> {
    if path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(OperationError::InvalidArchive(format!(
            "unsafe entry path: {}",
            path.display()
        )));
    }
    let mut components = path.components();
    let first = components.next().and_then(|component| match component {
        Component::Normal(name) => name.to_str(),
        _ => None,
    });
    if !matches!(
        first,
        Some("database.sqlite3" | "config.toml" | "manifest.json" | "media")
    ) {
        return Err(OperationError::InvalidArchive(format!(
            "unexpected entry: {}",
            path.display()
        )));
    }
    Ok(())
}

fn verify(staging: &Path) -> Result<(), OperationError> {
    let manifest_path = staging.join("manifest.json");
    let manifest: BackupManifest = serde_json::from_slice(&std::fs::read(&manifest_path)?)
        .map_err(|error| OperationError::InvalidArchive(error.to_string()))?;
    if manifest.format_version != 1 {
        return Err(OperationError::InvalidArchive(format!(
            "unsupported format version {}",
            manifest.format_version
        )));
    }
    let expected: BTreeSet<_> = manifest.entries.keys().cloned().collect();
    for (name, checksum) in &manifest.entries {
        let path = staging.join(name);
        if !path.is_file() || checksum_file(&path)? != *checksum {
            return Err(OperationError::InvalidArchive(format!(
                "checksum mismatch: {name}"
            )));
        }
    }
    let actual: BTreeSet<_> = WalkDir::new(staging)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_file())
        .filter_map(|entry| {
            entry
                .path()
                .strip_prefix(staging)
                .ok()
                .and_then(Path::to_str)
                .map(str::to_owned)
        })
        .filter(|name| name != "manifest.json")
        .collect();
    if actual != expected || !staging.join("database.sqlite3").is_file() {
        return Err(OperationError::InvalidArchive(
            "manifest does not match archive entries".into(),
        ));
    }
    Ok(())
}

fn installation_exists(data_dir: &Path) -> bool {
    ["simple-blog.sqlite3", "config.toml", "media"]
        .into_iter()
        .any(|name| data_dir.join(name).exists())
}

struct DirectoryGuard(PathBuf);

impl Drop for DirectoryGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
