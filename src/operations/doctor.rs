use std::{
    collections::{BTreeMap, BTreeSet},
    fs::OpenOptions,
    io::{Cursor, Write},
    path::{Path, PathBuf},
};

use image::{DynamicImage, ImageDecoder, ImageReader};
use serde::Serialize;
use uuid::Uuid;

use crate::{
    application::ports::MediaRepository,
    config::Config,
    infrastructure::sqlite::{MIGRATOR, SqliteRepository},
    operations::{OperationError, checksum_file},
    release::{FilesystemReleaseStore, ReleaseId, ReleaseReader, ReleaseStore},
};

#[derive(Debug, Serialize)]
pub struct DoctorCheck {
    pub name: &'static str,
    pub status: &'static str,
    pub detail: String,
    pub code: String,
    pub hint: &'static str,
}

#[derive(Debug, Default)]
pub struct DoctorReport {
    pub checks: Vec<DoctorCheck>,
    pub issues: Vec<String>,
}

impl DoctorReport {
    #[must_use]
    pub const fn is_healthy(&self) -> bool {
        self.issues.is_empty()
    }

    fn ok(&mut self, name: &'static str, detail: impl Into<String>) {
        self.checks.push(DoctorCheck {
            name,
            status: "ok",
            detail: detail.into(),
            code: format!("{name}.ok"),
            hint: "No action required for this check's inspection scope.",
        });
    }

    fn fail(&mut self, name: &'static str, detail: impl Into<String>) {
        let detail = detail.into();
        self.issues.push(format!("{name}: {detail}"));
        self.checks.push(DoctorCheck {
            name,
            status: "error",
            detail,
            code: format!("{name}.failed"),
            hint: diagnostic_hint(name),
        });
    }
}

pub struct Doctor;

impl Doctor {
    /// Opens its own non-migrating connection and continues independent checks
    /// when SQLite is absent, damaged or inaccessible.
    pub async fn inspect_installation(config: &Config, probe_writes: bool) -> DoctorReport {
        let mut report = DoctorReport::default();
        match super::activation::pending(&config.data_dir) {
            Ok(false) => report.ok("installation.activation", "No interrupted activation record."),
            Ok(true) => report.fail("installation.activation", "A replacement was interrupted. Preserve sibling staging and previous directories. Stop the server, then run a normal command to recover; doctor does not recover."),
            Err(_) => report.fail("installation.activation", "Activation state could not be inspected."),
        }
        match SqliteRepository::connect_diagnostics(&config.database_path()).await {
            Ok(repository) => {
                report.ok("sqlite.connection", "Read-only connection; no migration or repair was attempted.");
                check_database(config, &repository, &mut report).await;
                repository.close().await;
            }
            Err(_) => report.fail("sqlite.connection", "Read-only database inspection is unavailable; no migration or repair was attempted."),
        }
        check_directories(config, probe_writes, &mut report);
        check_releases(config, &mut report).await;
        report
    }

    #[tracing::instrument(name = "operation.doctor", skip_all)]
    pub async fn inspect(
        config: &Config,
        repository: &SqliteRepository,
    ) -> Result<DoctorReport, OperationError> {
        let mut report = DoctorReport::default();
        check_database(config, repository, &mut report).await;
        check_directories(config, false, &mut report);
        check_releases(config, &mut report).await;
        Ok(report)
    }
}

async fn check_database(config: &Config, repository: &SqliteRepository, report: &mut DoctorReport) {
    check_quick_check(repository, report).await;
    check_foreign_keys(repository, report).await;
    check_runtime_pragmas(repository, report).await;
    check_migrations(repository, report).await;
    check_media(config, repository, report).await;
    check_content_trash(repository, report).await;
}

fn check_directories(config: &Config, probe_writes: bool, report: &mut DoctorReport) {
    for (name, path) in [
        ("filesystem.data", config.data_dir.clone()),
        ("filesystem.media", config.media_dir()),
        ("filesystem.backups", config.backup_dir()),
        ("filesystem.releases", config.release_dir()),
    ] {
        check_directory(name, &path, probe_writes, report);
    }
}

fn diagnostic_hint(name: &str) -> &'static str {
    match name.split('.').next().unwrap_or("") {
        "sqlite" => {
            "Preserve the database and its WAL together. Check access and locks; restore a verified backup or use normal startup for pending migrations."
        }
        "filesystem" => {
            "Check directory access and free space. Use --probe-writes to explicitly test creation, synchronization and removal."
        }
        "media" => {
            "Preserve the affected files and compare them with a verified backup before recovery."
        }
        "release" => {
            "Keep the last verified release available; inspect publication events before rebuilding."
        }
        _ => "Preserve the installation and investigate this check before changing data.",
    }
}

async fn check_content_trash(repository: &SqliteRepository, report: &mut DoctorReport) {
    let count: Result<i64, _> =
        sqlx::query_scalar("SELECT COUNT(*) FROM contents WHERE deleted_at IS NOT NULL")
            .fetch_one(repository.pool())
            .await;
    match count {
        Ok(count) => report.ok("content.trash", format!("{count} piece(s) in the trash")),
        Err(error) => report.fail("content.trash", error.to_string()),
    }
}

async fn check_releases(config: &Config, report: &mut DoctorReport) {
    let root = config.release_dir();
    let store = FilesystemReleaseStore::new(root.clone());
    match store.active().await {
        Ok(None) => report.ok(
            "release.active",
            "no active release; build or serve will create one",
        ),
        Ok(Some(active)) => match store.verify_active().await {
            Ok(verification) => report.ok(
                "release.active",
                format!(
                    "{}: {} object(s), {} byte(s) verified",
                    verification.release_id, verification.object_count, verification.total_bytes
                ),
            ),
            Err(error) => report.fail("release.active", format!("{}: {error}", active.id)),
        },
        Err(error) => report.fail("release.active", error.to_string()),
    }
    check_release_history(&root, &store, report).await;
    check_release_temporaries(&root, report);
}

async fn check_release_history(
    root: &Path,
    store: &FilesystemReleaseStore,
    report: &mut DoctorReport,
) {
    let mut issues = Vec::new();
    let mut referenced_objects = BTreeSet::new();
    let mut verified_objects = BTreeSet::new();
    let mut manifest_count = 0_usize;
    let mut total_bytes = 0_u64;
    let manifests = root.join("manifests");
    for path in regular_release_entries(&manifests, "manifest", &mut issues) {
        let Some(filename) = path.file_name().and_then(|name| name.to_str()) else {
            issues.push(format!("non-UTF-8 release manifest: {}", path.display()));
            continue;
        };
        if is_release_temporary(filename) {
            continue;
        }
        let Some(stem) = filename.strip_suffix(".json") else {
            issues.push(format!("unexpected release manifest file: {filename}"));
            continue;
        };
        let id = match ReleaseId::parse(stem.to_owned()) {
            Ok(id) => id,
            Err(error) => {
                issues.push(format!("invalid release manifest name {filename}: {error}"));
                continue;
            }
        };
        match store.manifest(&id).await {
            Ok(manifest) => {
                manifest_count += 1;
                for object_id in manifest
                    .routes
                    .values()
                    .filter_map(|route| route.object_id())
                {
                    referenced_objects.insert(object_id.to_owned());
                    if verified_objects.insert(object_id.to_owned()) {
                        match store.object(object_id).await {
                            Ok(bytes) => {
                                total_bytes = total_bytes
                                    .saturating_add(u64::try_from(bytes.len()).unwrap_or(u64::MAX));
                            }
                            Err(error) => issues.push(error.to_string()),
                        }
                    }
                }
            }
            Err(error) => issues.push(error.to_string()),
        }
    }

    let objects = root.join("objects");
    for path in regular_release_entries(&objects, "object", &mut issues) {
        let Some(filename) = path.file_name().and_then(|name| name.to_str()) else {
            issues.push(format!("non-UTF-8 release object: {}", path.display()));
            continue;
        };
        if is_release_temporary(filename) {
            continue;
        }
        match ReleaseId::parse(filename.to_owned()) {
            Ok(_) if referenced_objects.contains(filename) => {}
            Ok(_) => issues.push(format!("unreferenced release object: {filename}")),
            Err(error) => issues.push(format!("invalid release object name {filename}: {error}")),
        }
    }

    issues.sort();
    issues.dedup();
    if issues.is_empty() {
        report.ok(
            "release.history",
            format!(
                "{manifest_count} manifest(s), {} object(s), {total_bytes} byte(s) verified",
                verified_objects.len()
            ),
        );
    } else {
        report.fail("release.history", issues.join("; "));
    }
}

fn regular_release_entries(directory: &Path, kind: &str, issues: &mut Vec<String>) -> Vec<PathBuf> {
    let entries = match std::fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
        Err(error) => {
            issues.push(format!(
                "could not enumerate release {kind}s at {}: {error}",
                directory.display()
            ));
            return Vec::new();
        }
    };
    let mut paths = Vec::new();
    for entry in entries {
        match entry {
            Ok(entry) => match entry.file_type() {
                Ok(file_type) if file_type.is_file() => paths.push(entry.path()),
                Ok(_) => issues.push(format!(
                    "release {kind} is not a regular file: {}",
                    entry.path().display()
                )),
                Err(error) => issues.push(format!(
                    "could not inspect release {kind} {}: {error}",
                    entry.path().display()
                )),
            },
            Err(error) => issues.push(format!("could not enumerate release {kind}: {error}")),
        }
    }
    paths.sort();
    paths
}

fn check_release_temporaries(root: &Path, report: &mut DoctorReport) {
    let mut issues = Vec::new();
    if root.is_dir() {
        for entry in walkdir::WalkDir::new(root).follow_links(false) {
            match entry {
                Ok(entry) if entry.depth() > 0 => {
                    let filename = entry.file_name().to_string_lossy();
                    if is_release_temporary(&filename) {
                        issues.push(format!(
                            "interrupted release write: {}",
                            entry.path().display()
                        ));
                    }
                }
                Ok(_) => {}
                Err(error) => issues.push(format!("could not inspect release tree: {error}")),
            }
        }
    }
    issues.sort();
    if issues.is_empty() {
        report.ok("release.temporary_files", "no interrupted release writes");
    } else {
        report.fail("release.temporary_files", issues.join("; "));
    }
}

fn is_release_temporary(filename: &str) -> bool {
    (filename.starts_with('.')
        && Path::new(filename)
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("tmp")))
        || filename.contains(".materializing-")
}

async fn check_quick_check(repository: &SqliteRepository, report: &mut DoctorReport) {
    match sqlx::query_scalar::<_, String>("PRAGMA quick_check")
        .fetch_one(repository.pool())
        .await
    {
        Ok(result) if result == "ok" => report.ok("sqlite.quick_check", "ok"),
        Ok(result) => report.fail("sqlite.quick_check", result),
        Err(error) => report.fail("sqlite.quick_check", error.to_string()),
    }
}

async fn check_foreign_keys(repository: &SqliteRepository, report: &mut DoctorReport) {
    match sqlx::query("PRAGMA foreign_key_check")
        .fetch_all(repository.pool())
        .await
    {
        Ok(rows) if rows.is_empty() => report.ok("sqlite.foreign_keys", "no violations"),
        Ok(rows) => report.fail(
            "sqlite.foreign_keys",
            format!("{} violation(s)", rows.len()),
        ),
        Err(error) => report.fail("sqlite.foreign_keys", error.to_string()),
    }
}

async fn check_runtime_pragmas(repository: &SqliteRepository, report: &mut DoctorReport) {
    match repository.pragmas().await {
        Ok(pragmas)
            if pragmas.foreign_keys
                && (repository.is_diagnostic_snapshot()
                    || pragmas.journal_mode.eq_ignore_ascii_case("wal"))
                && pragmas.busy_timeout_ms >= 5_000 =>
        {
            report.ok(
                "sqlite.runtime_pragmas",
                format!(
                    "foreign_keys=on, journal_mode={}, busy_timeout={}ms; connection_scope={}",
                    pragmas.journal_mode,
                    pragmas.busy_timeout_ms,
                    if repository.is_diagnostic_snapshot() {
                        "diagnostic snapshot; live runtime settings are not observed"
                    } else {
                        "application"
                    }
                ),
            );
        }
        Ok(pragmas) => report.fail(
            "sqlite.runtime_pragmas",
            format!(
                "foreign_keys={}, journal_mode={}, busy_timeout={}ms",
                pragmas.foreign_keys, pragmas.journal_mode, pragmas.busy_timeout_ms
            ),
        ),
        Err(error) => report.fail("sqlite.runtime_pragmas", error.to_string()),
    }
}

async fn check_migrations(repository: &SqliteRepository, report: &mut DoctorReport) {
    let rows = sqlx::query_as::<_, (i64, String, bool, Vec<u8>)>(
        "SELECT version, description, success, checksum
         FROM _sqlx_migrations ORDER BY version",
    )
    .fetch_all(repository.pool())
    .await;
    let Ok(rows) = rows else {
        report.fail("sqlite.migrations", rows.unwrap_err().to_string());
        return;
    };
    let mut applied: BTreeMap<_, _> = rows
        .into_iter()
        .map(|(version, description, success, checksum)| {
            (version, (description, success, checksum))
        })
        .collect();
    let mut issues = Vec::new();
    for migration in MIGRATOR.iter() {
        let Some((description, success, checksum)) = applied.remove(&migration.version) else {
            issues.push(format!("missing migration {}", migration.version));
            continue;
        };
        if !success {
            issues.push(format!("migration {} is marked failed", migration.version));
        }
        if description != migration.description {
            issues.push(format!(
                "description mismatch for migration {}",
                migration.version
            ));
        }
        if checksum != migration.checksum.as_ref() {
            issues.push(format!(
                "checksum mismatch for migration {}",
                migration.version
            ));
        }
    }
    for version in applied.keys() {
        issues.push(format!("unknown migration {version}"));
    }
    if issues.is_empty() {
        report.ok(
            "sqlite.migrations",
            format!("{} embedded migration(s) verified", MIGRATOR.iter().count()),
        );
    } else {
        report.fail("sqlite.migrations", issues.join("; "));
    }
}

fn check_directory(name: &'static str, path: &Path, probe_writes: bool, report: &mut DoctorReport) {
    if !probe_writes {
        match std::fs::read_dir(path) {
            Ok(_) => report.ok(
                name,
                format!("{} is readable; writability was not tested", path.display()),
            ),
            Err(error) => report.fail(name, format!("{}: {error}", path.display())),
        }
        return;
    }
    match write_probe(path) {
        Ok(()) => report.ok(name, format!("{} is writable", path.display())),
        Err(error) => report.fail(name, format!("{}: {error}", path.display())),
    }
}

fn write_probe(directory: &Path) -> std::io::Result<()> {
    write_probe_with(
        directory,
        |file| {
            file.write_all(b"simple-blog doctor\n")
                .and_then(|()| file.sync_all())
        },
        |path| std::fs::remove_file(path),
    )
}

fn write_probe_with(
    directory: &Path,
    write_and_sync: impl FnOnce(&mut std::fs::File) -> std::io::Result<()>,
    cleanup: impl FnOnce(&Path) -> std::io::Result<()>,
) -> std::io::Result<()> {
    if !directory.is_dir() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "directory does not exist",
        ));
    }
    let path = directory.join(format!(".simple-blog-doctor-probe-{}", Uuid::new_v4()));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)?;
    let guard = ProbeGuard(path);
    let result = write_and_sync(&mut file);
    drop(file);
    let cleanup = cleanup(&guard.0);
    match (result, cleanup) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(error), Ok(())) | (Ok(()), Err(error)) => Err(error),
        (Err(error), Err(cleanup)) => Err(std::io::Error::other(format!(
            "probe failed: {error}; cleanup failed: {cleanup}"
        ))),
    }
}

async fn check_media(config: &Config, repository: &SqliteRepository, report: &mut DoctorReport) {
    let assets = match repository.list_media().await {
        Ok(assets) => assets,
        Err(error) => {
            report.fail("media.records", error.to_string());
            return;
        }
    };
    let mut expected_files = BTreeSet::new();
    let mut issues = Vec::new();
    for asset in &assets {
        let expected_original = format!("{}.{}", asset.id, asset.extension);
        if asset.original_filename == expected_original {
            expected_files.insert(asset.original_filename.clone());
            inspect_media_file(
                &config.media_dir().join(&asset.original_filename),
                MediaFileExpectation {
                    filename: &asset.original_filename,
                    kind: "original",
                    byte_size: asset.byte_size,
                    mime_type: &asset.mime_type,
                    width: asset.width,
                    height: asset.height,
                    checksum: Some(asset.id.as_str()),
                },
                &mut issues,
            );
        } else {
            issues.push(format!(
                "invalid original filename for {}: {}",
                asset.id, asset.original_filename
            ));
        }
        for variant in &asset.variants {
            let expected_variant = format!("{}-{}w.webp", asset.id, variant.width);
            if variant.filename == expected_variant {
                expected_files.insert(variant.filename.clone());
                inspect_media_file(
                    &config.media_dir().join(&variant.filename),
                    MediaFileExpectation {
                        filename: &variant.filename,
                        kind: "variant",
                        byte_size: variant.byte_size,
                        mime_type: "image/webp",
                        width: variant.width,
                        height: variant.height,
                        checksum: None,
                    },
                    &mut issues,
                );
            } else {
                issues.push(format!(
                    "invalid variant filename for {}: {}",
                    asset.id, variant.filename
                ));
            }
        }
    }
    if issues.is_empty() {
        report.ok(
            "media.records",
            format!("{} asset(s) and referenced files verified", assets.len()),
        );
    } else {
        report.fail("media.records", issues.join("; "));
    }

    check_orphan_media(&config.media_dir(), &expected_files, report);
}

#[derive(Clone, Copy)]
struct MediaFileExpectation<'a> {
    filename: &'a str,
    kind: &'static str,
    byte_size: u64,
    mime_type: &'a str,
    width: u32,
    height: u32,
    checksum: Option<&'a str>,
}

fn inspect_media_file(path: &Path, expected: MediaFileExpectation<'_>, issues: &mut Vec<String>) {
    let MediaFileExpectation {
        filename,
        kind,
        byte_size,
        mime_type,
        width,
        height,
        checksum,
    } = expected;
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_file() => metadata,
        Ok(_) => {
            issues.push(format!("{kind} is not a regular file: {filename}"));
            return;
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            issues.push(format!("missing media file: {filename}"));
            return;
        }
        Err(error) => {
            issues.push(format!("could not inspect {kind} {filename}: {error}"));
            return;
        }
    };
    if metadata.len() != byte_size {
        issues.push(format!(
            "{kind} byte size mismatch: {filename} (stored {byte_size}, actual {})",
            metadata.len()
        ));
    }
    if let Some(expected_checksum) = checksum {
        match checksum_file(path) {
            Ok(checksum) if checksum == expected_checksum => {}
            Ok(_) => issues.push(format!("original checksum mismatch: {filename}")),
            Err(error) => issues.push(format!("could not checksum {filename}: {error}")),
        }
    }
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) => {
            issues.push(format!("could not read {kind} {filename}: {error}"));
            return;
        }
    };
    let actual_mime = infer::get(&bytes).map(|media_type| media_type.mime_type());
    if actual_mime != Some(mime_type) {
        issues.push(format!(
            "{kind} media type mismatch: {filename} (stored {mime_type}, actual {})",
            actual_mime.unwrap_or("unknown")
        ));
    }
    match decoded_dimensions(&bytes) {
        Ok(actual) if actual == (width, height) => {}
        Ok((actual_width, actual_height)) => issues.push(format!(
            "{kind} dimensions mismatch: {filename} (stored {width}x{height}, actual {actual_width}x{actual_height})"
        )),
        Err(error) => issues.push(format!("{kind} decode failed: {filename}: {error}")),
    }
}

fn decoded_dimensions(bytes: &[u8]) -> Result<(u32, u32), image::ImageError> {
    let reader = ImageReader::new(Cursor::new(bytes)).with_guessed_format()?;
    let mut decoder = reader.into_decoder()?;
    let orientation = decoder.orientation()?;
    let mut image = DynamicImage::from_decoder(decoder)?;
    image.apply_orientation(orientation);
    Ok((image.width(), image.height()))
}

fn check_orphan_media(
    directory: &Path,
    expected_files: &BTreeSet<String>,
    report: &mut DoctorReport,
) {
    let entries = match std::fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) => {
            report.fail("media.orphans", error.to_string());
            return;
        }
    };
    let mut issues = Vec::new();
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                issues.push(format!("could not enumerate media: {error}"));
                continue;
            }
        };
        let filename = entry.file_name().to_string_lossy().into_owned();
        if expected_files.contains(&filename) {
            continue;
        }
        if filename.starts_with(".upload-")
            && Path::new(&filename)
                .extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("tmp"))
        {
            issues.push(format!("interrupted upload: {filename}"));
        } else {
            issues.push(format!("orphan media file: {filename}"));
        }
    }
    issues.sort();
    if issues.is_empty() {
        report.ok("media.orphans", "no unreferenced media files");
    } else {
        report.fail("media.orphans", issues.join("; "));
    }
}

struct ProbeGuard(PathBuf);

impl Drop for ProbeGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

#[cfg(test)]
mod probe_tests {
    use super::*;
    #[test]
    fn sync_and_cleanup_failures_are_reported_and_cleanup_is_attempted() {
        let temporary = tempfile::tempdir().unwrap();
        let mut writes = 0;
        let mut removals = 0;
        let result = write_probe_with(
            temporary.path(),
            |_file| {
                writes += 1;
                Err(std::io::Error::other("synthetic sync failure"))
            },
            |_path| {
                removals += 1;
                Err(std::io::Error::other("synthetic cleanup failure"))
            },
        );
        let message = result.unwrap_err().to_string();
        assert_eq!((writes, removals), (1, 1));
        assert!(message.contains("sync failure") && message.contains("cleanup failure"));
        assert_eq!(
            std::fs::read_dir(temporary.path()).unwrap().count(),
            0,
            "guard retries cleanup without hiding the failed operation"
        );
    }
    #[test]
    fn success_and_creation_failure_leave_no_probe_files() {
        let temporary = tempfile::tempdir().unwrap();
        write_probe(temporary.path()).unwrap();
        assert_eq!(std::fs::read_dir(temporary.path()).unwrap().count(), 0);
        assert!(write_probe(&temporary.path().join("absent")).is_err());
    }
}
