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
    let name = archive_entry_name(path)?;
    let allowed = matches!(
        name.as_str(),
        "database.sqlite3" | "config.toml" | "manifest.json" | "media"
    ) || name.starts_with("media/");
    if !allowed {
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
    let expected: BTreeSet<_> = manifest
        .entries
        .keys()
        .map(|name| {
            validate_manifest_entry_name(name)?;
            Ok(name.clone())
        })
        .collect::<Result<_, OperationError>>()?;
    for (name, checksum) in &manifest.entries {
        let path = staging.join(name);
        if !path.is_file() || checksum_file(&path)? != *checksum {
            return Err(OperationError::InvalidArchive(format!(
                "checksum mismatch: {name}"
            )));
        }
    }
    let mut actual = BTreeSet::new();
    for entry in WalkDir::new(staging) {
        let entry = entry.map_err(|error| OperationError::InvalidArchive(error.to_string()))?;
        if !entry.file_type().is_file() {
            continue;
        }
        let relative = entry
            .path()
            .strip_prefix(staging)
            .map_err(|error| OperationError::InvalidArchive(error.to_string()))?;
        let name = archive_entry_name(relative)?;
        if name != "manifest.json" {
            actual.insert(name);
        }
    }
    if actual != expected || !staging.join("database.sqlite3").is_file() {
        let missing = expected.difference(&actual).cloned().collect::<Vec<_>>();
        let unexpected = actual.difference(&expected).cloned().collect::<Vec<_>>();
        return Err(OperationError::InvalidArchive(format!(
            "manifest does not match archive entries (missing: {}; unexpected: {})",
            display_names(&missing),
            display_names(&unexpected)
        )));
    }
    Ok(())
}

fn archive_entry_name(path: &Path) -> Result<String, OperationError> {
    let mut parts = Vec::new();
    for component in path.components() {
        let Component::Normal(value) = component else {
            return Err(OperationError::InvalidArchive(format!(
                "unsafe entry path: {}",
                path.display()
            )));
        };
        let value = value
            .to_str()
            .ok_or_else(|| OperationError::InvalidArchive("entry path is not UTF-8".into()))?;
        if value.is_empty() || value.contains(['/', '\\', '\0']) {
            return Err(OperationError::InvalidArchive(format!(
                "unsafe entry path: {}",
                path.display()
            )));
        }
        parts.push(value);
    }
    if parts.is_empty() {
        return Err(OperationError::InvalidArchive("entry path is empty".into()));
    }
    Ok(parts.join("/"))
}

fn validate_manifest_entry_name(name: &str) -> Result<(), OperationError> {
    let safe_segments = !name.is_empty()
        && !name.contains(['\\', '\0'])
        && name
            .split('/')
            .all(|segment| !segment.is_empty() && !matches!(segment, "." | ".."));
    let allowed = matches!(name, "database.sqlite3" | "config.toml")
        || name
            .strip_prefix("media/")
            .is_some_and(|relative| !relative.is_empty());
    if !safe_segments || !allowed {
        return Err(OperationError::InvalidArchive(format!(
            "invalid manifest entry: {name}"
        )));
    }
    Ok(())
}

fn display_names(names: &[String]) -> String {
    if names.is_empty() {
        "none".into()
    } else {
        names.join(", ")
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    #[test]
    fn archive_entry_names_use_forward_slashes_on_every_platform() {
        let relative = Path::new("media").join("nested").join("asset.bin");
        assert_eq!(
            archive_entry_name(&relative).unwrap(),
            "media/nested/asset.bin"
        );
    }

    #[test]
    fn manifest_entry_names_are_canonical_and_confined_to_backup_data() {
        for valid in [
            "database.sqlite3",
            "config.toml",
            "media/asset.bin",
            "media/nested/asset.bin",
        ] {
            validate_manifest_entry_name(valid).unwrap();
        }
        for invalid in [
            "manifest.json",
            "../database.sqlite3",
            "/database.sqlite3",
            "media\\asset.bin",
            "media//asset.bin",
            "media/../asset.bin",
            "unexpected.bin",
        ] {
            assert!(
                validate_manifest_entry_name(invalid).is_err(),
                "accepted unsafe manifest entry: {invalid}"
            );
        }
    }

    #[test]
    fn manifest_mismatch_reports_the_unexpected_portable_name() {
        let staging = tempfile::tempdir().unwrap();
        let database = staging.path().join("database.sqlite3");
        std::fs::write(&database, b"database").unwrap();
        std::fs::create_dir(staging.path().join("media")).unwrap();
        std::fs::write(staging.path().join("media/unexpected.bin"), b"unexpected").unwrap();
        let mut entries = std::collections::BTreeMap::new();
        entries.insert("database.sqlite3".into(), checksum_file(&database).unwrap());
        let manifest = BackupManifest {
            format_version: 1,
            application_version: "test".into(),
            created_at: Utc::now(),
            entries,
        };
        std::fs::write(
            staging.path().join("manifest.json"),
            serde_json::to_vec(&manifest).unwrap(),
        )
        .unwrap();

        let error = verify(staging.path()).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("unexpected: media/unexpected.bin")
        );
    }
}
