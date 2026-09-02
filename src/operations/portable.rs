use std::{
    collections::BTreeMap,
    fs::OpenOptions,
    io::Write,
    path::{Path, PathBuf},
    sync::Arc,
};

use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::{
    application::{
        ports::{PortableRepository, PublicSnapshotRepository},
        publication::PublicationService,
        site_compiler::SiteCompiler,
    },
    config::Config,
    infrastructure::{markdown::ComrakMarkdownRenderer, sqlite::SqliteRepository},
    operations::{Doctor, OperationError},
    portable::{PortableArchive, PortableArchiveReport, PortablePackage},
    release::FilesystemReleaseStore,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PortableImportReport {
    pub release_id: String,
    pub content_count: usize,
    pub media_count: usize,
    /// Existing data is retained here after a forced replacement.
    pub replaced_data_dir: Option<PathBuf>,
}

pub struct PortableMigrationService;

impl PortableMigrationService {
    #[tracing::instrument(
        name = "operation.portable.export",
        skip_all,
        fields(output = %output.display(), exported_at = %exported_at),
        err
    )]
    pub async fn export(
        config: &Config,
        repository: &SqliteRepository,
        output: &Path,
        exported_at: DateTime<Utc>,
    ) -> Result<PortableArchiveReport, OperationError> {
        repository
            .advance_publication_clock(exported_at)
            .await
            .map_err(database)?;
        let origin = config.public_url.as_str().trim_end_matches('/');
        let site = repository
            .portable_site(origin, exported_at)
            .await
            .map_err(database)?;
        let media_files = read_media_files(config, &site.media)?;
        let package = PortablePackage { site, media_files };
        PortableArchive::write(&package, output).map_err(OperationError::from)
    }

    pub async fn import(
        archive: &Path,
        config: &Config,
        force: bool,
    ) -> Result<PortableImportReport, OperationError> {
        let package = PortableArchive::read(archive)?;
        Self::import_package(archive, package, config, force).await
    }

    /// Installs an archive that has already passed the bounded parser. This
    /// avoids decoding a potentially large migration twice when the caller
    /// needs its canonical origin to resolve destination configuration.
    #[tracing::instrument(
        name = "operation.portable.import",
        skip_all,
        fields(archive = %archive.display(), destination = %config.data_dir.display()),
        err
    )]
    pub async fn import_package(
        archive: &Path,
        package: PortablePackage,
        config: &Config,
        force: bool,
    ) -> Result<PortableImportReport, OperationError> {
        verify_origin(config, &package)?;
        validate_destination(&config.data_dir)?;
        if config.data_dir.exists() && !force {
            return Err(OperationError::DestinationExists);
        }
        if config.data_dir.exists() && !config.data_dir.is_dir() {
            return Err(OperationError::UnsafeDestination(format!(
                "{} is not a directory",
                config.data_dir.display()
            )));
        }
        if config
            .data_dir
            .symlink_metadata()
            .is_ok_and(|metadata| metadata.file_type().is_symlink())
        {
            return Err(OperationError::UnsafeDestination(format!(
                "{} is a symbolic link",
                config.data_dir.display()
            )));
        }

        let parent = usable_parent(&config.data_dir);
        std::fs::create_dir_all(parent)?;
        let staging = parent.join(format!(".simple-blog-import-{}.staging", Uuid::new_v4()));
        std::fs::create_dir(&staging)?;
        let mut staging_config = config.clone();
        staging_config.data_dir.clone_from(&staging);
        let prepared = prepare_staging(&staging_config, &package).await;
        let release_id = match prepared {
            Ok(release_id) => release_id,
            Err(error) => {
                cleanup_staging(&staging);
                return Err(error);
            }
        };
        let replaced_data_dir = match activate_staging(&staging, &config.data_dir) {
            Ok(backup) => backup,
            Err(error) => {
                cleanup_staging(&staging);
                return Err(error);
            }
        };
        tracing::info!(
            event = "portable.import.activated",
            release_id,
            content_count = package.site.contents.len(),
            media_count = package.site.media.len(),
            retained_previous = replaced_data_dir.is_some()
        );
        Ok(PortableImportReport {
            release_id,
            content_count: package.site.contents.len(),
            media_count: package.site.media.len(),
            replaced_data_dir,
        })
    }
}

fn read_media_files(
    config: &Config,
    media: &[crate::domain::media::MediaAsset],
) -> Result<BTreeMap<String, Vec<u8>>, OperationError> {
    let mut files = BTreeMap::new();
    for asset in media {
        read_media_file(config, &asset.original_filename, &mut files)?;
        for variant in &asset.variants {
            read_media_file(config, &variant.filename, &mut files)?;
        }
    }
    Ok(files)
}

fn read_media_file(
    config: &Config,
    filename: &str,
    files: &mut BTreeMap<String, Vec<u8>>,
) -> Result<(), OperationError> {
    let path = config.media_dir().join(filename);
    let metadata = path
        .symlink_metadata()
        .map_err(|error| OperationError::InvalidData(format!("{}: {error}", path.display())))?;
    if !metadata.file_type().is_file() {
        return Err(OperationError::InvalidData(format!(
            "media path is not a regular file: {}",
            path.display()
        )));
    }
    let bytes = std::fs::read(&path)?;
    if files.insert(filename.to_owned(), bytes).is_some() {
        return Err(OperationError::InvalidData(format!(
            "duplicate media filename: {filename}"
        )));
    }
    Ok(())
}

fn verify_origin(config: &Config, package: &PortablePackage) -> Result<(), OperationError> {
    let configured = config.public_url.as_str().trim_end_matches('/');
    if configured == package.site.canonical_origin {
        return Ok(());
    }
    Err(OperationError::PortableOriginMismatch {
        archive_origin: package.site.canonical_origin.clone(),
        configured_origin: configured.to_owned(),
    })
}

fn validate_destination(destination: &Path) -> Result<(), OperationError> {
    if destination.as_os_str().is_empty()
        || destination.file_name().is_none()
        || destination == Path::new(".")
        || destination == Path::new("..")
    {
        return Err(OperationError::UnsafeDestination(
            destination.display().to_string(),
        ));
    }
    Ok(())
}

fn usable_parent(path: &Path) -> &Path {
    path.parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
}

async fn prepare_staging(
    config: &Config,
    package: &PortablePackage,
) -> Result<String, OperationError> {
    for directory in [
        config.media_dir(),
        config.backup_dir(),
        config.release_dir(),
    ] {
        std::fs::create_dir(&directory)?;
    }
    write_media_files(&config.media_dir(), &package.media_files)?;
    config
        .persist()
        .map_err(|error| OperationError::InvalidData(error.to_string()))?;
    let repository = Arc::new(
        SqliteRepository::connect(&config.database_path())
            .await
            .map_err(database)?,
    );
    let result = prepare_database_and_release(config, repository.clone(), package).await;
    repository.close().await;
    result
}

async fn prepare_database_and_release(
    config: &Config,
    repository: Arc<SqliteRepository>,
    package: &PortablePackage,
) -> Result<String, OperationError> {
    repository
        .replace_portable_site(&package.site, &ComrakMarkdownRenderer::default())
        .await
        .map_err(database)?;
    let releases = Arc::new(FilesystemReleaseStore::new(config.release_dir()));
    let publisher = PublicationService::new(
        repository.clone(),
        releases.clone(),
        SiteCompiler::embedded().map_err(|error| OperationError::InvalidData(error.to_string()))?,
        config.public_url.as_str(),
    )
    .map_err(|error| OperationError::InvalidData(error.to_string()))?;
    let outcome = publisher
        .publish(package.site.exported_at)
        .await
        .map_err(|error| OperationError::InvalidData(error.to_string()))?;
    releases
        .verify_active()
        .await
        .map_err(|error| OperationError::InvalidData(error.to_string()))?;
    let report = Doctor::inspect(config, repository.as_ref()).await?;
    if !report.is_healthy() {
        return Err(OperationError::InvalidData(format!(
            "staged import failed doctor checks: {}",
            report.issues.join("; ")
        )));
    }
    Ok(outcome.release_id.as_str().to_owned())
}

fn write_media_files(
    directory: &Path,
    files: &BTreeMap<String, Vec<u8>>,
) -> Result<(), OperationError> {
    for (filename, bytes) in files {
        let path = directory.join(filename);
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)?;
        file.write_all(bytes)?;
        file.sync_all()?;
    }
    sync_directory(directory)?;
    Ok(())
}

fn activate_staging(staging: &Path, destination: &Path) -> Result<Option<PathBuf>, OperationError> {
    let parent = usable_parent(destination);
    let previous = if destination.exists() {
        let filename = destination
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("simple-blog-data");
        let backup = parent.join(format!(
            ".{filename}.before-portable-import-{}",
            Uuid::new_v4()
        ));
        std::fs::rename(destination, &backup)?;
        Some(backup)
    } else {
        None
    };
    if let Err(error) = std::fs::rename(staging, destination) {
        if let Some(backup) = &previous
            && let Err(rollback) = std::fs::rename(backup, destination)
        {
            return Err(OperationError::ImportActivation(format!(
                "activation failed ({error}); rollback failed ({rollback}); previous data remains at {}",
                backup.display()
            )));
        }
        return Err(OperationError::ImportActivation(error.to_string()));
    }
    sync_directory(parent)?;
    Ok(previous)
}

fn cleanup_staging(staging: &Path) {
    if staging
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.starts_with(".simple-blog-import-") && name.ends_with(".staging"))
    {
        let _cleanup = std::fs::remove_dir_all(staging);
    }
}

fn sync_directory(path: &Path) -> Result<(), OperationError> {
    crate::durable_fs::sync_directory(path)?;
    Ok(())
}

fn database(error: impl std::fmt::Display) -> OperationError {
    OperationError::Database(error.to_string())
}
