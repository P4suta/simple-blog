use std::path::{Component, Path, PathBuf};

use chrono::{DateTime, Utc};
use tracing::instrument;
use uuid::Uuid;
use walkdir::WalkDir;

use crate::{
    config::Config,
    infrastructure::{sqlite::SqliteRepository, sqlite_maintenance::SqliteMaintenance},
    operations::{OperationError, backup::archive_files},
};

#[derive(Debug)]
pub struct ManagedDatabase {
    pub repository: SqliteRepository,
    pub safety_backup: Option<PathBuf>,
}

pub struct MigrationCoordinator;

impl MigrationCoordinator {
    #[instrument(name = "database.open", skip_all)]
    pub async fn open(
        config: &Config,
        now: DateTime<Utc>,
    ) -> Result<ManagedDatabase, OperationError> {
        let data_dir = config.data_dir.clone();
        let backup_dir = config.backup_dir();
        tokio::task::spawn_blocking(move || {
            std::fs::create_dir_all(&data_dir)?;
            std::fs::create_dir_all(backup_dir)
        })
        .await
        .map_err(|error| OperationError::InvalidData(error.to_string()))??;

        let safety_backup = if SqliteMaintenance::safety_backup_required(&config.database_path())
            .await
            .map_err(database)?
        {
            tracing::info!(event = "database.migration.backup_required");
            Some(create_safety_backup(config, now).await?)
        } else {
            None
        };

        let repository = match SqliteRepository::connect(&config.database_path()).await {
            Ok(repository) => repository,
            Err(error) => {
                tracing::error!(
                    event = "database.migration.failed",
                    safety_backup = safety_backup.as_ref().map(|path| path.display().to_string()),
                    error = %error,
                );
                return match safety_backup {
                    Some(backup) => Err(OperationError::Migration {
                        message: error.to_string(),
                        backup,
                    }),
                    None => Err(database(error)),
                };
            }
        };
        tracing::info!(
            event = "database.opened",
            migration_safety_backup = safety_backup.is_some()
        );
        Ok(ManagedDatabase {
            repository,
            safety_backup,
        })
    }
}

async fn create_safety_backup(
    config: &Config,
    now: DateTime<Utc>,
) -> Result<PathBuf, OperationError> {
    let identifier = Uuid::new_v4();
    let output = config.backup_dir().join(format!(
        "simple-blog-pre-migration-{}-{identifier}.tar.zst",
        now.format("%Y%m%d-%H%M%S")
    ));
    let snapshot = config
        .backup_dir()
        .join(format!(".pre-migration-{identifier}.sqlite3"));
    SqliteMaintenance::consistent_snapshot(&config.database_path(), &snapshot)
        .await
        .map_err(database)?;

    // Walking and archiving the media tree is blocking work; keep it off
    // the async runtime so the scheduler stays responsive during startup.
    let result = {
        let config = config.clone();
        let snapshot = snapshot.clone();
        let output = output.clone();
        tokio::task::spawn_blocking(move || archive_safety_files(&config, &snapshot, &output, now))
            .await
            .map_err(|error| OperationError::InvalidData(error.to_string()))?
    };
    let cleanup = std::fs::remove_file(&snapshot);
    if let Err(error) = result {
        if let Err(cleanup_error) = cleanup {
            tracing::warn!(
                event = "database.migration.snapshot_cleanup_failed",
                error = %cleanup_error
            );
        }
        return Err(error);
    }
    cleanup?;
    tracing::info!(
        event = "database.migration.backup_created",
        backup = %output.display()
    );
    Ok(output)
}

fn archive_safety_files(
    config: &Config,
    snapshot: &Path,
    output: &Path,
    now: DateTime<Utc>,
) -> Result<(), OperationError> {
    let mut files = vec![("database.sqlite3".to_owned(), snapshot.to_owned())];
    let config_path = config.data_dir.join("config.toml");
    if config_path.is_file() {
        files.push(("config.toml".to_owned(), config_path));
    }
    if config.media_dir().is_dir() {
        for entry in WalkDir::new(config.media_dir()).follow_links(false) {
            let entry = entry.map_err(|error| OperationError::InvalidData(error.to_string()))?;
            if entry.file_type().is_symlink() || !entry.file_type().is_file() {
                continue;
            }
            let relative = entry
                .path()
                .strip_prefix(config.media_dir())
                .map_err(|error| OperationError::InvalidData(error.to_string()))?;
            files.push((archive_media_name(relative)?, entry.into_path()));
        }
    }
    files.sort_by(|left, right| left.0.cmp(&right.0));
    archive_files(&files, output, now)
}

fn archive_media_name(relative: &Path) -> Result<String, OperationError> {
    let mut parts = vec!["media".to_owned()];
    for component in relative.components() {
        let Component::Normal(value) = component else {
            return Err(OperationError::InvalidData(
                "media path is not archive-safe".into(),
            ));
        };
        parts.push(
            value
                .to_str()
                .ok_or_else(|| OperationError::InvalidData("media path is not UTF-8".into()))?
                .to_owned(),
        );
    }
    if parts.len() == 1 {
        return Err(OperationError::InvalidData(
            "media path has no filename".into(),
        ));
    }
    Ok(parts.join("/"))
}

fn database(error: impl std::fmt::Display) -> OperationError {
    OperationError::Database(error.to_string())
}
