mod backup;
mod doctor;
mod export;
mod import;
mod migration;
mod portable;
mod restore;

use std::{
    collections::BTreeMap,
    fs::File,
    io::Read,
    path::{Path, PathBuf},
};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;

pub use backup::BackupService;
pub use doctor::{Doctor, DoctorCheck, DoctorReport};
pub use export::Exporter;
pub use import::{ImportReport, Importer};
pub use migration::{ManagedDatabase, MigrationCoordinator};
pub use portable::{PortableImportReport, PortableMigrationService};
pub use restore::RestoreService;

#[derive(Debug, Error)]
pub enum OperationError {
    #[error("file operation failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("database operation failed: {0}")]
    Database(String),
    #[error(
        "database migration failed: {message}; pre-migration backup remains at {}",
        .backup.display()
    )]
    Migration { message: String, backup: PathBuf },
    #[error("archive is invalid: {0}")]
    InvalidArchive(String),
    #[error("destination already contains simple-blog data; pass --force to replace it")]
    DestinationExists,
    #[error("export destination already exists: {0}")]
    ExportExists(String),
    #[error("stored data is invalid: {0}")]
    InvalidData(String),
    #[error(transparent)]
    Portable(#[from] crate::portable::PortableArchiveError),
    #[error(
        "portable archive belongs to {archive_origin}, not configured origin {configured_origin}"
    )]
    PortableOriginMismatch {
        archive_origin: String,
        configured_origin: String,
    },
    #[error("unsafe import destination: {0}")]
    UnsafeDestination(String),
    #[error("could not activate imported data: {0}")]
    ImportActivation(String),
}

#[derive(Debug, Deserialize, Serialize)]
struct BackupManifest {
    format_version: u32,
    application_version: String,
    created_at: DateTime<Utc>,
    entries: BTreeMap<String, String>,
}

fn checksum_file(path: &Path) -> Result<String, OperationError> {
    let mut file = File::open(path)?;
    let mut hasher = blake3::Hasher::new();
    let mut buffer = vec![0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hasher.finalize().to_hex().to_string())
}
