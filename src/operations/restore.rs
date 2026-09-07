use std::{
    collections::BTreeSet,
    fs::File,
    path::{Component, Path, PathBuf},
};

use uuid::Uuid;
use walkdir::WalkDir;

use crate::{
    infrastructure::sqlite_maintenance::SqliteMaintenance,
    operations::{BackupManifest, OperationError, checksum_file},
};

pub struct RestoreService;

impl RestoreService {
    #[tracing::instrument(
        name = "backup.restore",
        skip_all,
        fields(
            archive = %archive.display(),
            destination = %data_dir.display(),
            force
        )
    )]
    pub async fn restore(
        archive: &Path,
        data_dir: &Path,
        force: bool,
    ) -> Result<(), OperationError> {
        crate::observability::operation("restore", Self::restore_inner(archive, data_dir, force))
            .await
    }

    async fn restore_inner(
        archive: &Path,
        data_dir: &Path,
        force: bool,
    ) -> Result<(), OperationError> {
        if installation_exists(data_dir) && !force {
            return Err(OperationError::DestinationExists);
        }
        let parent = data_dir.parent().unwrap_or_else(|| Path::new("."));
        std::fs::create_dir_all(parent)?;
        let staging = parent.join(format!(".simple-blog-restore-{}.staging", Uuid::new_v4()));
        std::fs::create_dir(&staging)?;
        let mut guard = DirectoryGuard(Some(staging.clone()));
        extract(archive, &staging)?;
        tracing::debug!(event = "backup.restore.extracted");
        verify(&staging)?;
        tracing::debug!(event = "backup.restore.manifest_verified");

        let database = staging.join("database.sqlite3");
        let check = SqliteMaintenance::quick_check_read_only(&database)
            .await
            .map_err(|error| OperationError::InvalidArchive(error.to_string()))?;
        if check != "ok" {
            return Err(OperationError::InvalidArchive(format!(
                "SQLite quick_check failed: {check}"
            )));
        }
        tracing::debug!(event = "backup.restore.database_verified");

        std::fs::rename(&database, staging.join("simple-blog.sqlite3"))?;
        std::fs::remove_file(staging.join("manifest.json"))?;
        // Keep the existing public release, backups and unrelated installation
        // files available until the restored database is published successfully.
        if data_dir.is_dir() {
            for entry in std::fs::read_dir(data_dir)? {
                let entry = entry?;
                if matches!(
                    entry.file_name().to_str(),
                    Some(
                        "simple-blog.sqlite3"
                            | "simple-blog.sqlite3-wal"
                            | "simple-blog.sqlite3-shm"
                            | "simple-blog.sqlite3-journal"
                            | "config.toml"
                            | "media"
                    )
                ) {
                    continue;
                }
                super::activation::copy_tree(&entry.path(), &staging.join(entry.file_name()))?;
            }
        }
        for name in ["media", "backups", "releases"] {
            std::fs::create_dir_all(staging.join(name))?;
        }
        let activated = super::activation::activate(&staging, data_dir, force);
        // Only a confirmed absence of an intent leaves this disposable staging
        // directory ours to clean. Ambiguous state must retain recovery inputs.
        if activated.is_ok() || !matches!(super::activation::pending(data_dir), Ok(false)) {
            guard.0 = None;
        }
        activated?;
        drop(guard);
        tracing::info!(event = "backup.restore.completed");
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

struct DirectoryGuard(Option<PathBuf>);

impl Drop for DirectoryGuard {
    fn drop(&mut self) {
        if let Some(path) = &self.0 {
            let _ = std::fs::remove_dir_all(path);
        }
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

#[cfg(test)]
mod archive_entry_tests {
    use super::*;

    #[test]
    fn only_the_entries_a_backup_carries_are_accepted() {
        for entry in [
            "database.sqlite3",
            "config.toml",
            "manifest.json",
            "media",
            "media/cover.png",
            "media/nested/cover.png",
        ] {
            validate_archive_path(Path::new(entry)).unwrap_or_else(|error| {
                panic!("rejected an entry a backup carries: {entry} ({error})")
            });
        }
    }

    #[test]
    fn every_unexpected_or_unsafe_archive_entry_is_refused() {
        for entry in [
            "/etc/passwd",
            "../escape",
            "./database.sqlite3",
            "unexpected.txt",
            "mediax/cover.png",
            "database.sqlite3.bak",
        ] {
            assert!(
                validate_archive_path(Path::new(entry)).is_err(),
                "accepted an archive entry that is not part of a backup: {entry}"
            );
        }
    }

    #[test]
    fn an_installation_is_present_when_any_of_its_own_files_is() {
        let temp = tempfile::tempdir().unwrap();
        assert!(!installation_exists(temp.path()));

        for name in ["simple-blog.sqlite3", "config.toml"] {
            let empty = tempfile::tempdir().unwrap();
            std::fs::write(empty.path().join(name), b"").unwrap();
            assert!(
                installation_exists(empty.path()),
                "did not see an installation holding {name}"
            );
        }

        let with_media = tempfile::tempdir().unwrap();
        std::fs::create_dir(with_media.path().join("media")).unwrap();
        assert!(installation_exists(with_media.path()));

        // Something else entirely is not an installation.
        let unrelated = tempfile::tempdir().unwrap();
        std::fs::write(unrelated.path().join("notes.txt"), b"").unwrap();
        assert!(!installation_exists(unrelated.path()));
    }
}

#[cfg(test)]
mod archive_extraction_tests {
    use super::*;

    /// Writes a backup archive from entries given as name, kind and bytes, so
    /// an archive `BackupService` would never produce can still be handed to
    /// the extractor.
    fn archive(directory: &Path, entries: &[(&str, tar::EntryType, &[u8])]) -> PathBuf {
        let path = directory.join(format!("{}.tar.zst", Uuid::new_v4()));
        let mut encoder = zstd::Encoder::new(File::create(&path).unwrap(), 1).unwrap();
        {
            let mut builder = tar::Builder::new(&mut encoder);
            for (name, kind, bytes) in entries {
                let mut header = tar::Header::new_gnu();
                header.set_path(name).unwrap();
                header.set_size(u64::try_from(bytes.len()).unwrap());
                header.set_mode(0o644);
                header.set_mtime(0);
                header.set_entry_type(*kind);
                if *kind == tar::EntryType::Symlink {
                    header.set_link_name("database.sqlite3").unwrap();
                }
                header.set_cksum();
                builder.append(&header, *bytes).unwrap();
            }
            builder.finish().unwrap();
        }
        encoder.finish().unwrap().sync_all().unwrap();
        path
    }

    fn staging(temp: &tempfile::TempDir) -> PathBuf {
        let staging = temp.path().join(Uuid::new_v4().to_string());
        std::fs::create_dir(&staging).unwrap();
        staging
    }

    /// A backup is made of the files and the directories of an installation.
    /// Everything else an archive can describe is refused before it lands.
    #[test]
    fn a_backup_carries_files_and_directories_and_nothing_else() {
        let temp = tempfile::tempdir().unwrap();
        let ordinary = archive(
            temp.path(),
            &[
                ("media", tar::EntryType::Directory, b""),
                ("database.sqlite3", tar::EntryType::Regular, b"pages"),
            ],
        );
        let restored = staging(&temp);
        extract(&ordinary, &restored).unwrap();
        assert!(
            restored.join("media").is_dir(),
            "a directory entry must be restored as the directory it is"
        );
        assert_eq!(
            std::fs::read(restored.join("database.sqlite3")).unwrap(),
            b"pages"
        );

        let linked = archive(
            temp.path(),
            &[("config.toml", tar::EntryType::Symlink, b"")],
        );
        let error = extract(&linked, &staging(&temp)).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("links and special files are not allowed"),
            "a link must be refused as a link, not as {error}"
        );
    }

    /// A name whose bytes are not text on this platform.
    #[cfg(unix)]
    fn unreadable_name() -> std::ffi::OsString {
        use std::os::unix::ffi::OsStringExt as _;

        std::ffi::OsString::from_vec(vec![0xff])
    }

    #[cfg(windows)]
    fn unreadable_name() -> std::ffi::OsString {
        use std::os::windows::ffi::OsStringExt as _;

        std::ffi::OsString::from_wide(&[0xd800])
    }

    /// The guard exists to refuse a path that leaves the backup, and it says
    /// so whatever the bytes of that path turn out to be. Its own answer must
    /// not be shadowed by the encoding check that follows it.
    #[test]
    fn an_entry_that_leaves_the_backup_is_refused_as_a_traversal_whatever_its_bytes() {
        let escaping = Path::new(&unreadable_name()).join("..");
        let error = validate_archive_path(&escaping).unwrap_err();
        assert!(
            error.to_string().contains("unsafe entry path"),
            "a traversal entry must be named as the unsafe path it is, not as {error}"
        );
    }

    /// One component of an entry name is one plain segment. A separator or a
    /// NUL inside a component is a name that means one thing to the archive
    /// and another to the filesystem.
    #[test]
    fn a_component_carrying_a_separator_or_a_nul_is_not_a_name() {
        assert_eq!(
            archive_entry_name(Path::new("media/cover.png")).unwrap(),
            "media/cover.png"
        );
        let error = archive_entry_name(Path::new("a\0b")).unwrap_err();
        assert!(
            error.to_string().contains("unsafe entry path"),
            "a component carrying a NUL must be refused, not read as {error}"
        );
    }

    /// A file whose bytes changed after the manifest was written is the case
    /// verification exists for; it is not enough that the file is there.
    #[test]
    fn a_staged_file_whose_bytes_changed_is_a_checksum_mismatch() {
        let temp = tempfile::tempdir().unwrap();
        let staging = temp.path();
        let database = staging.join("database.sqlite3");
        std::fs::write(&database, b"pages").unwrap();
        let manifest = BackupManifest {
            format_version: 1,
            application_version: "0.1.0".into(),
            created_at: chrono::Utc::now(),
            entries: std::collections::BTreeMap::from([(
                "database.sqlite3".to_owned(),
                checksum_file(&database).unwrap(),
            )]),
        };
        std::fs::write(
            staging.join("manifest.json"),
            serde_json::to_vec(&manifest).unwrap(),
        )
        .unwrap();
        verify(staging).unwrap();

        std::fs::write(&database, b"other pages").unwrap();
        let error = verify(staging).unwrap_err();
        assert!(
            error.to_string().contains("checksum mismatch"),
            "a file whose bytes changed must be a checksum mismatch, not {error}"
        );

        std::fs::remove_file(&database).unwrap();
        let error = verify(staging).unwrap_err();
        assert!(
            error.to_string().contains("checksum mismatch"),
            "a file the manifest names and the archive lacks must be a checksum mismatch, not {error}"
        );
    }
}

#[cfg(test)]
mod staging_retention_tests {
    use super::*;

    use crate::{
        config::{Config, ConfigSources, Overrides},
        infrastructure::sqlite::SqliteRepository,
        operations::backup::BackupService,
    };

    fn config(data_dir: &Path) -> Config {
        Config::resolve(ConfigSources {
            cli: Overrides {
                data_dir: Some(data_dir.to_path_buf()),
                public_url: Some("http://localhost:8080".into()),
                ..Overrides::default()
            },
            ..ConfigSources::default()
        })
        .unwrap()
    }

    async fn archive(directory: &Path) -> PathBuf {
        let config = config(directory);
        std::fs::create_dir_all(config.backup_dir()).unwrap();
        let repository = SqliteRepository::connect(&config.database_path())
            .await
            .unwrap();
        let archive = BackupService::create(&config, &repository, None, chrono::Utc::now())
            .await
            .unwrap();
        repository.close().await;
        archive
    }

    fn staging_directories(parent: &Path) -> Vec<PathBuf> {
        std::fs::read_dir(parent)
            .unwrap()
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.ends_with(".staging"))
            })
            .collect()
    }

    /// A restore that cannot finish because the installation is mid-activation
    /// must leave every input an operator would recover from. The extracted
    /// staging directory is one of them, and it is only disposable once the
    /// absence of an activation record is confirmed.
    #[tokio::test]
    async fn a_restore_onto_an_interrupted_activation_keeps_what_recovery_needs() {
        let source = tempfile::tempdir().unwrap();
        let archive = archive(source.path()).await;

        let temp = tempfile::tempdir().unwrap();
        let destination = temp.path().join("site");
        std::fs::write(
            super::super::activation::sibling_path(&destination, "activation.json").unwrap(),
            b"{}",
        )
        .unwrap();

        let error = RestoreService::restore(&archive, &destination, false)
            .await
            .unwrap_err();
        assert!(
            error.to_string().contains("recover pending activation"),
            "a mid-activation installation must be named as the reason: {error}"
        );
        assert_eq!(
            staging_directories(temp.path()).len(),
            1,
            "the extracted copy has to survive a refusal it cannot explain away"
        );
    }

    /// With no activation record in the way, the same refusal owns its
    /// staging directory and cleans it up.
    #[tokio::test]
    async fn a_restore_refused_before_any_activation_leaves_nothing_behind() {
        let source = tempfile::tempdir().unwrap();
        let archive = archive(source.path()).await;

        let temp = tempfile::tempdir().unwrap();
        let destination = temp.path().join("site");
        // A file where the installation belongs cannot be replaced by one.
        std::fs::write(&destination, b"not an installation").unwrap();

        RestoreService::restore(&archive, &destination, false)
            .await
            .unwrap_err();
        assert!(
            staging_directories(temp.path()).is_empty(),
            "a refusal with no activation record leaves no disposable copy"
        );
    }
}
