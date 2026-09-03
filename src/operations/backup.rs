use std::{
    collections::BTreeMap,
    fs::File,
    path::{Path, PathBuf},
    time::Duration,
};

use chrono::{DateTime, Utc};
use sqlx::Executor;
use tar::{Builder, Header};
use uuid::Uuid;

use crate::{
    application::ports::MediaRepository,
    config::Config,
    infrastructure::sqlite::SqliteRepository,
    operations::{BackupManifest, OperationError, checksum_file},
};

/// When `serve` writes archives on its own: a first one shortly after boot
/// (so a site that is restarted daily still gets one) and then at a fixed
/// interval.
#[derive(Clone, Copy, Debug)]
pub struct BackupCadence {
    pub initial: Duration,
    pub every: Duration,
}

impl BackupCadence {
    pub const DEFAULT: Self = Self {
        initial: Duration::from_secs(10 * 60),
        every: Duration::from_secs(24 * 60 * 60),
    };
}

pub struct BackupService;

impl BackupService {
    /// Deletes the oldest `simple-blog-*.tar.zst` archives so that at most
    /// `keep` remain, answering what was removed. Names embed their creation
    /// time, so name order is age order; anything else in the folder (a
    /// safety backup from a migration, a hand-named archive) is left alone.
    pub fn prune(config: &Config, keep: usize) -> Result<Vec<PathBuf>, OperationError> {
        let directory = config.backup_dir();
        if !directory.is_dir() {
            return Ok(Vec::new());
        }
        let mut archives = std::fs::read_dir(&directory)?
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.is_file() && is_scheduled_archive(path))
            .collect::<Vec<_>>();
        archives.sort();
        archives.reverse();
        let mut removed = Vec::new();
        for path in archives.into_iter().skip(keep) {
            std::fs::remove_file(&path)?;
            removed.push(path);
        }
        Ok(removed)
    }

    pub async fn create(
        config: &Config,
        repository: &SqliteRepository,
        output: Option<PathBuf>,
        now: DateTime<Utc>,
    ) -> Result<PathBuf, OperationError> {
        std::fs::create_dir_all(config.backup_dir())?;
        let output = output.unwrap_or_else(|| {
            config.backup_dir().join(format!(
                "simple-blog-{}.tar.zst",
                now.format("%Y%m%d-%H%M%S")
            ))
        });
        if output.exists() {
            return Err(OperationError::DestinationExists);
        }
        let snapshot = config
            .backup_dir()
            .join(format!(".snapshot-{}.sqlite3", Uuid::new_v4()));
        let snapshot_string = snapshot
            .to_str()
            .ok_or_else(|| OperationError::InvalidData("database path is not UTF-8".into()))?;
        repository
            .pool()
            .execute("PRAGMA wal_checkpoint(PASSIVE)")
            .await
            .map_err(database)?;
        sqlx::query("VACUUM INTO ?")
            .bind(snapshot_string)
            .execute(repository.pool())
            .await
            .map_err(database)?;

        let result = Self::archive(config, repository, &snapshot, &output, now).await;
        let _ = std::fs::remove_file(&snapshot);
        result.map(|()| output)
    }

    async fn archive(
        config: &Config,
        repository: &SqliteRepository,
        snapshot: &Path,
        output: &Path,
        now: DateTime<Utc>,
    ) -> Result<(), OperationError> {
        let mut files = vec![("database.sqlite3".to_owned(), snapshot.to_owned())];
        let config_path = config.data_dir.join("config.toml");
        if config_path.is_file() {
            files.push(("config.toml".to_owned(), config_path));
        }
        for asset in repository
            .list_media()
            .await
            .map_err(|error| OperationError::Database(error.to_string()))?
        {
            files.push((
                format!("media/{}", asset.original_filename),
                config.media_dir().join(asset.original_filename),
            ));
            for variant in asset.variants {
                files.push((
                    format!("media/{}", variant.filename),
                    config.media_dir().join(variant.filename),
                ));
            }
        }
        archive_files(&files, output, now)
    }
}

pub(super) fn archive_files(
    files: &[(String, PathBuf)],
    output: &Path,
    now: DateTime<Utc>,
) -> Result<(), OperationError> {
    for (_, path) in files {
        if !path.is_file() {
            return Err(OperationError::InvalidData(format!(
                "referenced file is missing: {}",
                path.display()
            )));
        }
    }
    let entries: BTreeMap<_, _> = files
        .iter()
        .map(|(name, path)| Ok((name.clone(), checksum_file(path)?)))
        .collect::<Result<_, OperationError>>()?;
    let manifest = BackupManifest {
        format_version: 1,
        application_version: env!("CARGO_PKG_VERSION").into(),
        created_at: now,
        entries,
    };
    let manifest = serde_json::to_vec_pretty(&manifest)
        .map_err(|error| OperationError::InvalidData(error.to_string()))?;
    let partial = output.with_extension(format!("partial-{}", Uuid::new_v4()));
    let result = (|| {
        let file = File::create(&partial)?;
        let encoder = zstd::Encoder::new(file, 9)?;
        let mut archive = Builder::new(encoder);
        for (name, path) in files {
            archive.append_path_with_name(path, name)?;
        }
        append_bytes(&mut archive, "manifest.json", &manifest)?;
        let encoder = archive.into_inner()?;
        encoder.finish()?.sync_all()?;
        std::fs::rename(&partial, output)?;
        Ok::<_, OperationError>(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&partial);
    }
    result
}

fn append_bytes<W: std::io::Write>(
    archive: &mut Builder<W>,
    name: &str,
    bytes: &[u8],
) -> Result<(), OperationError> {
    let mut header = Header::new_gnu();
    header.set_size(
        u64::try_from(bytes.len())
            .map_err(|error| OperationError::InvalidData(error.to_string()))?,
    );
    header.set_mode(0o600);
    header.set_cksum();
    archive.append_data(&mut header, name, bytes)?;
    Ok(())
}

fn is_scheduled_archive(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.starts_with("simple-blog-") && name.ends_with(".tar.zst"))
}

fn database(error: impl std::fmt::Display) -> OperationError {
    OperationError::Database(error.to_string())
}
