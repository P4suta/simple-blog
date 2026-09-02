use std::{collections::BTreeMap, path::Path, time::Duration};

use sqlx::{
    Connection, Row, SqlitePool,
    sqlite::{SqliteConnectOptions, SqliteConnection, SqlitePoolOptions},
};

use crate::{application::ports::RepositoryError, infrastructure::sqlite::MIGRATOR};

pub struct SqliteMaintenance;

impl SqliteMaintenance {
    /// Runs SQLite's integrity check without migrating or otherwise writing to
    /// the database. Restore validation must preserve the archived bytes and
    /// schema until the database has been installed successfully.
    pub async fn quick_check_read_only(path: &Path) -> Result<String, RepositoryError> {
        let options = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(false)
            .read_only(true)
            .busy_timeout(Duration::from_secs(5));
        let mut connection = SqliteConnection::connect_with(&options)
            .await
            .map_err(storage)?;
        let result = sqlx::query_scalar("PRAGMA quick_check")
            .fetch_one(&mut connection)
            .await
            .map_err(storage);
        let close_result = connection.close().await.map_err(storage);
        match result {
            Ok(check) => {
                close_result?;
                Ok(check)
            }
            Err(error) => Err(error),
        }
    }

    pub async fn safety_backup_required(path: &Path) -> Result<bool, RepositoryError> {
        if !path.is_file() {
            return Ok(false);
        }
        let pool = raw_pool(path).await?;
        let result = migration_state_differs(&pool).await;
        pool.close().await;
        result
    }

    pub async fn consistent_snapshot(
        source: &Path,
        destination: &Path,
    ) -> Result<(), RepositoryError> {
        if destination.exists() {
            return Err(RepositoryError::Storage(format!(
                "snapshot already exists: {}",
                destination.display()
            )));
        }
        let pool = raw_pool(source).await?;
        sqlx::query("PRAGMA wal_checkpoint(PASSIVE)")
            .execute(&pool)
            .await
            .map_err(storage)?;
        let destination = destination
            .to_str()
            .ok_or_else(|| RepositoryError::Storage("snapshot path is not UTF-8".into()))?;
        let result = sqlx::query("VACUUM INTO ?")
            .bind(destination)
            .execute(&pool)
            .await
            .map(|_| ())
            .map_err(storage);
        pool.close().await;
        result
    }
}

async fn raw_pool(path: &Path) -> Result<SqlitePool, RepositoryError> {
    let options = SqliteConnectOptions::new()
        .filename(path)
        .create_if_missing(false)
        .busy_timeout(Duration::from_secs(5));
    SqlitePoolOptions::new()
        .min_connections(1)
        .max_connections(1)
        .connect_with(options)
        .await
        .map_err(storage)
}

async fn migration_state_differs(pool: &SqlitePool) -> Result<bool, RepositoryError> {
    let table_exists: i64 = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = '_sqlx_migrations')",
    )
    .fetch_one(pool)
    .await
    .map_err(storage)?;
    if table_exists == 0 {
        return Ok(true);
    }
    let rows = sqlx::query("SELECT version, checksum, success FROM _sqlx_migrations")
        .fetch_all(pool)
        .await
        .map_err(storage)?;
    let mut applied = BTreeMap::new();
    for row in rows {
        let version: i64 = row.try_get("version").map_err(storage)?;
        let checksum: Vec<u8> = row.try_get("checksum").map_err(storage)?;
        let success: bool = row.try_get("success").map_err(storage)?;
        applied.insert(version, (checksum, success));
    }
    if applied.len() != MIGRATOR.iter().count() {
        return Ok(true);
    }
    Ok(MIGRATOR.iter().any(|migration| {
        applied
            .get(&migration.version)
            .is_none_or(|(checksum, success)| {
                !success || checksum.as_slice() != migration.checksum.as_ref()
            })
    }))
}

fn storage(error: impl std::fmt::Display) -> RepositoryError {
    RepositoryError::Storage(error.to_string())
}
