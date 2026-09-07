//! The read-only doctor: every check the software can make of its own
//! installation and every safety limit in force, each reported under a stable
//! name.

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
    application::{
        auth::{AUTHENTICATION_ATTEMPTS_PER_MINUTE, LIKES_PER_MINUTE},
        content::{MAX_MARKDOWN_BYTES, MAX_SUMMARY_CHARS, MAX_TITLE_CHARS},
        ports::MediaRepository,
    },
    config::Config,
    domain::{
        search::{MAX_QUERY_CHARS, MAX_TERMS},
        theme::{MAX_CUSTOM_CSS_BYTES, MAX_NAVIGATION_ITEMS},
    },
    infrastructure::{
        media::{MAX_PIXELS, MAX_WEBP_SIDE},
        sqlite::{AUTOSAVE_REVISIONS_KEPT, MIGRATOR, SETTINGS_REVISIONS_KEPT, SqliteRepository},
    },
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

/// Every limit the software enforces to keep itself safe, in one place, so
/// an operator can see them before anyone runs into one. None of them is a
/// quota: there is no cap on pieces, on total bytes, or on traffic.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct SafetyLimits {
    /// One upload, in bytes; `max_upload_bytes` in the configuration.
    pub upload_bytes: usize,
    pub markdown_bytes: usize,
    pub title_chars: usize,
    pub summary_chars: usize,
    pub image_pixels: u64,
    pub image_side_pixels: u32,
    pub stylesheet_bytes: usize,
    pub navigation_items: usize,
    pub search_query_chars: usize,
    pub search_terms: usize,
    pub authentication_attempts_per_minute: usize,
    pub likes_per_minute: usize,
    pub autosave_revisions_kept: i64,
    pub settings_revisions_kept: i64,
    /// Scheduled backups kept; `backup_retention` in the configuration, where
    /// zero switches the schedule off.
    pub backup_generations: usize,
}

impl SafetyLimits {
    #[must_use]
    pub const fn for_config(config: &Config) -> Self {
        Self {
            upload_bytes: config.max_upload_bytes,
            markdown_bytes: MAX_MARKDOWN_BYTES,
            title_chars: MAX_TITLE_CHARS,
            summary_chars: MAX_SUMMARY_CHARS,
            image_pixels: MAX_PIXELS,
            image_side_pixels: MAX_WEBP_SIDE,
            stylesheet_bytes: MAX_CUSTOM_CSS_BYTES,
            navigation_items: MAX_NAVIGATION_ITEMS,
            search_query_chars: MAX_QUERY_CHARS,
            search_terms: MAX_TERMS,
            authentication_attempts_per_minute: AUTHENTICATION_ATTEMPTS_PER_MINUTE,
            likes_per_minute: LIKES_PER_MINUTE,
            autosave_revisions_kept: AUTOSAVE_REVISIONS_KEPT,
            settings_revisions_kept: SETTINGS_REVISIONS_KEPT,
            backup_generations: config.backup_retention,
        }
    }
}

#[derive(Debug)]
pub struct DoctorReport {
    pub checks: Vec<DoctorCheck>,
    pub issues: Vec<String>,
    /// The limits in force, so they can be seen without tripping over them.
    pub limits: SafetyLimits,
}

impl DoctorReport {
    #[must_use]
    pub const fn new(limits: SafetyLimits) -> Self {
        Self {
            checks: Vec::new(),
            issues: Vec::new(),
            limits,
        }
    }

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

/// A report for one check on its own. The limits a real report carries come
/// from the configuration the command was given, and a check that says
/// nothing about them still has to have somewhere to put them.
#[cfg(test)]
fn report_for_a_check() -> DoctorReport {
    DoctorReport::new(SafetyLimits::for_config(&Config {
        data_dir: PathBuf::from("."),
        bind: "127.0.0.1:8080".parse().expect("a literal socket address"),
        public_url: "http://localhost:8080/".parse().expect("a literal URL"),
        trusted_proxies: Vec::new(),
        max_upload_bytes: 8 * 1024 * 1024,
        backup_retention: 14,
    }))
}

pub struct Doctor;

impl Doctor {
    /// Opens its own non-migrating connection and continues independent checks
    /// when SQLite is absent, damaged or inaccessible.
    pub async fn inspect_installation(config: &Config, probe_writes: bool) -> DoctorReport {
        let mut report = DoctorReport::new(SafetyLimits::for_config(config));
        check_limits(&mut report);
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
        let mut report = DoctorReport::new(SafetyLimits::for_config(config));
        check_limits(&mut report);
        check_database(config, repository, &mut report).await;
        check_directories(config, false, &mut report);
        check_releases(config, &mut report).await;
        Ok(report)
    }
}

/// Every safety limit, spelled out as a passing check: the promise is that
/// each one can be seen and explained, not only met head-on.
fn check_limits(report: &mut DoctorReport) {
    let limits = report.limits;
    report.ok(
        "limits.upload",
        format!(
            "{} byte(s) per upload (max_upload_bytes in config.toml)",
            limits.upload_bytes
        ),
    );
    report.ok(
        "limits.text",
        format!(
            "{} byte(s) of Markdown, {} title and {} summary characters per piece",
            limits.markdown_bytes, limits.title_chars, limits.summary_chars
        ),
    );
    report.ok(
        "limits.image",
        format!(
            "{} pixels and {} pixels per side per image",
            limits.image_pixels, limits.image_side_pixels
        ),
    );
    report.ok(
        "limits.theme",
        format!(
            "{} byte(s) of stylesheet, {} navigation items",
            limits.stylesheet_bytes, limits.navigation_items
        ),
    );
    report.ok(
        "limits.search",
        format!(
            "{} characters and {} terms per query",
            limits.search_query_chars, limits.search_terms
        ),
    );
    report.ok(
        "limits.rate",
        format!(
            "{} authentication attempts and {} likes per minute per client",
            limits.authentication_attempts_per_minute, limits.likes_per_minute
        ),
    );
    report.ok(
        "limits.history",
        format!(
            "{} autosave revisions per piece (explicit saves are never pruned), {} settings states",
            limits.autosave_revisions_kept, limits.settings_revisions_kept
        ),
    );
    report.ok(
        "limits.backups",
        if limits.backup_generations == 0 {
            "scheduled backups are off (backup_retention = 0 in config.toml)".to_owned()
        } else {
            format!(
                "{} scheduled backup(s) kept (backup_retention in config.toml)",
                limits.backup_generations
            )
        },
    );
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

#[cfg(test)]
mod inspection_tests {
    use super::*;

    /// The hint is what an operator reads when a check fails, so each family
    /// has to say something of its own rather than the general advice.
    #[test]
    fn every_check_family_carries_its_own_hint() {
        let sqlite = diagnostic_hint("sqlite.runtime_pragmas");
        let filesystem = diagnostic_hint("filesystem.writes");
        let media = diagnostic_hint("media.files");
        let release = diagnostic_hint("release.temporary_files");
        let other = diagnostic_hint("content.trash");

        assert!(sqlite.contains("WAL"));
        assert!(filesystem.contains("--probe-writes"));
        assert!(media.contains("verified backup"));
        assert!(release.contains("publication events"));
        assert!(other.contains("before changing data"));

        let hints = [sqlite, filesystem, media, release, other];
        for hint in hints {
            assert!(!hint.is_empty());
        }
        let distinct: std::collections::BTreeSet<&str> = hints.into_iter().collect();
        assert_eq!(distinct.len(), 5, "two check families share one hint");

        // The family is the part before the first dot, and an unknown family
        // falls back to the general advice.
        assert_eq!(diagnostic_hint("sqlite"), sqlite);
        assert_eq!(diagnostic_hint("sqlitex.thing"), other);
        assert_eq!(diagnostic_hint(""), other);
    }

    #[test]
    fn a_release_temporary_is_a_dotted_tmp_file_or_a_materializing_one() {
        assert!(is_release_temporary(".release.tmp"));
        assert!(is_release_temporary(".release.TMP"));
        assert!(is_release_temporary("objects.materializing-7"));
        assert!(is_release_temporary(".objects.materializing-7"));

        for filename in ["release.tmp", ".release.txt", ".tmp", "manifest.json"] {
            assert!(
                !is_release_temporary(filename),
                "treated a durable release file as an interrupted write: {filename}"
            );
        }
    }

    #[test]
    fn an_interrupted_release_write_is_found_and_a_clean_tree_reports_so() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("releases");
        std::fs::create_dir_all(root.join("objects")).unwrap();
        std::fs::write(root.join("objects/manifest.json"), b"{}").unwrap();

        let mut clean = report_for_a_check();
        check_release_temporaries(&root, &mut clean);
        assert!(clean.is_healthy());

        std::fs::write(root.join("objects/.half-written.tmp"), b"").unwrap();
        let mut interrupted = report_for_a_check();
        check_release_temporaries(&root, &mut interrupted);
        assert!(!interrupted.is_healthy());

        // The root itself is at depth zero and is never an interrupted write,
        // even when it is named like one.
        let dotted = temp.path().join(".root.tmp");
        std::fs::create_dir_all(&dotted).unwrap();
        let mut root_named_like_a_temporary = report_for_a_check();
        check_release_temporaries(&dotted, &mut root_named_like_a_temporary);
        assert!(root_named_like_a_temporary.is_healthy());

        // A tree that is not there at all is not an interrupted write either.
        let mut absent = report_for_a_check();
        check_release_temporaries(&temp.path().join("never-created"), &mut absent);
        assert!(absent.is_healthy());
    }

    #[test]
    fn release_entries_are_the_regular_files_and_anything_else_is_an_issue() {
        let temp = tempfile::tempdir().unwrap();
        let directory = temp.path().join("manifests");
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(directory.join("b.json"), b"{}").unwrap();
        std::fs::write(directory.join("a.json"), b"{}").unwrap();

        let mut issues = Vec::new();
        let paths = regular_release_entries(&directory, "manifest", &mut issues);
        assert_eq!(
            paths,
            vec![directory.join("a.json"), directory.join("b.json")]
        );
        assert!(issues.is_empty());

        std::fs::create_dir(directory.join("nested")).unwrap();
        let mut issues = Vec::new();
        let paths = regular_release_entries(&directory, "manifest", &mut issues);
        assert_eq!(paths.len(), 2);
        assert_eq!(issues.len(), 1, "a directory among manifests is an issue");

        // A directory that was never created is not an issue: an installation
        // that has never published has no manifests.
        let mut issues = Vec::new();
        let paths = regular_release_entries(&temp.path().join("absent"), "manifest", &mut issues);
        assert!(paths.is_empty());
        assert!(issues.is_empty());
    }

    #[test]
    fn decoded_dimensions_report_the_image_that_was_decoded() {
        let mut bytes = Vec::new();
        image::DynamicImage::new_rgb8(4, 2)
            .write_to(&mut Cursor::new(&mut bytes), image::ImageFormat::Png)
            .unwrap();
        assert_eq!(decoded_dimensions(&bytes).unwrap(), (4, 2));

        assert!(decoded_dimensions(b"not an image").is_err());
    }
}

#[cfg(test)]
mod media_inspection_tests {
    use super::*;

    fn png(width: u32, height: u32) -> Vec<u8> {
        let mut bytes = Vec::new();
        image::DynamicImage::new_rgb8(width, height)
            .write_to(&mut Cursor::new(&mut bytes), image::ImageFormat::Png)
            .unwrap();
        bytes
    }

    fn expectation<'a>(
        filename: &'a str,
        bytes: &[u8],
        checksum: Option<&'a str>,
    ) -> MediaFileExpectation<'a> {
        MediaFileExpectation {
            filename,
            kind: "media file",
            byte_size: bytes.len() as u64,
            mime_type: "image/png",
            width: 4,
            height: 2,
            checksum,
        }
    }

    fn issues_for(path: &Path, expected: MediaFileExpectation<'_>) -> Vec<String> {
        let mut issues = Vec::new();
        inspect_media_file(path, expected, &mut issues);
        issues
    }

    #[test]
    fn a_media_file_that_matches_its_record_raises_nothing() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("cover.png");
        let bytes = png(4, 2);
        std::fs::write(&path, &bytes).unwrap();
        let checksum = checksum_file(&path).unwrap();

        assert!(issues_for(&path, expectation("cover.png", &bytes, Some(&checksum))).is_empty());
        assert!(issues_for(&path, expectation("cover.png", &bytes, None)).is_empty());
    }

    /// Each case breaks one thing the record claims, so no check can be
    /// dropped without a corrupt installation reading as healthy.
    #[test]
    fn every_way_a_media_file_can_disagree_with_its_record_is_reported() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("cover.png");
        let bytes = png(4, 2);
        std::fs::write(&path, &bytes).unwrap();
        let checksum = checksum_file(&path).unwrap();

        let mut wrong_size = expectation("cover.png", &bytes, Some(&checksum));
        wrong_size.byte_size += 1;
        assert!(
            issues_for(&path, wrong_size)
                .iter()
                .any(|issue| issue.contains("byte size mismatch"))
        );

        let elsewhere = "af".repeat(32);
        assert!(
            issues_for(&path, expectation("cover.png", &bytes, Some(&elsewhere)))
                .iter()
                .any(|issue| issue.contains("checksum mismatch"))
        );

        let mut wrong_type = expectation("cover.png", &bytes, None);
        wrong_type.mime_type = "image/jpeg";
        assert!(
            issues_for(&path, wrong_type)
                .iter()
                .any(|issue| issue.contains("media type mismatch"))
        );

        let mut wrong_dimensions = expectation("cover.png", &bytes, None);
        wrong_dimensions.width = 8;
        assert!(
            issues_for(&path, wrong_dimensions)
                .iter()
                .any(|issue| issue.contains("dimensions mismatch"))
        );

        // Something that is not an image at all cannot be decoded.
        let broken = temp.path().join("broken.png");
        std::fs::write(&broken, b"not an image").unwrap();
        let mut expectation_for_broken = expectation("broken.png", b"not an image", None);
        expectation_for_broken.byte_size = 12;
        assert!(
            issues_for(&broken, expectation_for_broken)
                .iter()
                .any(|issue| issue.contains("decode failed"))
        );
    }

    #[test]
    fn a_media_file_that_is_absent_or_not_a_file_is_reported_as_such() {
        let temp = tempfile::tempdir().unwrap();
        let bytes = png(4, 2);

        let absent = temp.path().join("absent.png");
        assert!(
            issues_for(&absent, expectation("absent.png", &bytes, None))
                .iter()
                .any(|issue| issue.contains("missing media file")),
            "an absent media file must be reported as missing"
        );

        let directory = temp.path().join("cover.png");
        std::fs::create_dir(&directory).unwrap();
        assert!(
            issues_for(&directory, expectation("cover.png", &bytes, None))
                .iter()
                .any(|issue| issue.contains("is not a regular file")),
            "a directory where a media file belongs must not read as missing"
        );

        // A name the operating system will not even accept cannot be looked
        // at, which is a different thing from having looked and found nothing.
        let unaskable = temp.path().join("cover\0.png");
        assert!(
            issues_for(&unaskable, expectation("cover\0.png", &bytes, None))
                .iter()
                .any(|issue| issue.contains("could not inspect")),
            "a media path that cannot be asked about must not read as missing"
        );
    }

    #[test]
    fn a_release_directory_that_cannot_be_enumerated_is_an_issue_not_an_absence() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("manifests");
        std::fs::write(&file, b"not a directory").unwrap();

        let mut issues = Vec::new();
        let paths = regular_release_entries(&file, "manifest", &mut issues);
        assert!(paths.is_empty());
        assert_eq!(
            issues.len(),
            1,
            "a manifests path that is a file must be reported, not read as no manifests"
        );
        assert!(issues[0].contains("could not enumerate"));
    }

    #[test]
    fn a_directory_check_says_whether_it_tested_writing() {
        let temp = tempfile::tempdir().unwrap();

        let mut read_only = report_for_a_check();
        check_directory("filesystem.data", temp.path(), false, &mut read_only);
        assert!(read_only.is_healthy());
        assert!(
            read_only.checks[0]
                .detail
                .contains("writability was not tested"),
            "a check that did not probe must say it did not: {}",
            read_only.checks[0].detail
        );

        let mut probed = report_for_a_check();
        check_directory("filesystem.data", temp.path(), true, &mut probed);
        assert!(probed.is_healthy());
        assert!(
            probed.checks[0].detail.contains("is writable"),
            "a check that probed must say what the probe found: {}",
            probed.checks[0].detail
        );
        assert_eq!(
            std::fs::read_dir(temp.path()).unwrap().count(),
            0,
            "a write probe leaves nothing behind"
        );

        let absent = temp.path().join("never-created");
        let mut unreadable = report_for_a_check();
        check_directory("filesystem.data", &absent, false, &mut unreadable);
        assert!(!unreadable.is_healthy());

        let mut unwritable = report_for_a_check();
        check_directory("filesystem.data", &absent, true, &mut unwritable);
        assert!(!unwritable.is_healthy());
    }
}

#[cfg(test)]
mod orphan_media_tests {
    use super::*;

    fn detail_of(report: &DoctorReport, name: &str) -> String {
        report
            .checks
            .iter()
            .find(|check| check.name == name)
            .unwrap_or_else(|| panic!("{name} was not checked"))
            .detail
            .clone()
    }

    /// A media directory holding only what the database references is what a
    /// healthy installation looks like.
    #[test]
    fn a_media_directory_of_referenced_files_alone_raises_nothing() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join("cover.png"), b"x").unwrap();

        let mut report = report_for_a_check();
        let expected = BTreeSet::from(["cover.png".to_owned()]);
        check_orphan_media(temp.path(), &expected, &mut report);
        assert!(report.is_healthy());
        assert_eq!(
            detail_of(&report, "media.orphans"),
            "no unreferenced media files"
        );
    }

    /// An upload interrupted part way has a name of its own, so the operator
    /// is told it was an interruption and not a file nobody accounts for.
    #[test]
    fn an_interrupted_upload_is_named_as_one_and_anything_else_as_an_orphan() {
        let temp = tempfile::tempdir().unwrap();
        for name in [
            "cover.png",
            "stranger.png",
            ".upload-abcd.tmp",
            ".upload-abcd.png",
        ] {
            std::fs::write(temp.path().join(name), b"x").unwrap();
        }

        let mut report = report_for_a_check();
        let expected = BTreeSet::from(["cover.png".to_owned()]);
        check_orphan_media(temp.path(), &expected, &mut report);
        assert!(!report.is_healthy());

        let detail = detail_of(&report, "media.orphans");
        assert!(
            detail.contains("interrupted upload: .upload-abcd.tmp"),
            "a half-written upload must be named as one: {detail}"
        );
        assert!(
            detail.contains("orphan media file: .upload-abcd.png"),
            "an upload name that is not a temporary file is still an orphan: {detail}"
        );
        assert!(
            detail.contains("orphan media file: stranger.png"),
            "a file the database does not reference is an orphan: {detail}"
        );
        assert!(
            !detail.contains("cover.png"),
            "a referenced file must not be reported at all: {detail}"
        );
    }

    /// A media directory that is not there at all is one failure, not one per
    /// file the database expected.
    #[test]
    fn a_media_directory_that_cannot_be_read_is_reported_once() {
        let temp = tempfile::tempdir().unwrap();
        let mut report = report_for_a_check();
        check_orphan_media(&temp.path().join("absent"), &BTreeSet::new(), &mut report);
        assert_eq!(report.issues.len(), 1);
        assert!(!detail_of(&report, "media.orphans").is_empty());
    }
}

#[cfg(test)]
mod database_check_tests {
    use super::*;

    async fn repository(temp: &tempfile::TempDir) -> SqliteRepository {
        SqliteRepository::connect(&temp.path().join("simple-blog.sqlite3"))
            .await
            .unwrap()
    }

    fn detail_of(report: &DoctorReport, name: &str) -> String {
        report
            .checks
            .iter()
            .find(|check| check.name == name)
            .unwrap_or_else(|| panic!("{name} was not checked"))
            .detail
            .clone()
    }

    /// The runtime settings are not a preference. Each of them is what keeps a
    /// concurrent writer from losing data, so a database running without one
    /// is reported even though every query still answers.
    #[tokio::test]
    async fn a_database_not_running_on_the_settings_it_needs_is_reported() {
        let temp = tempfile::tempdir().unwrap();
        let repository = repository(&temp).await;

        let mut healthy = report_for_a_check();
        check_runtime_pragmas(&repository, &mut healthy).await;
        assert!(healthy.is_healthy());
        let reported = detail_of(&healthy, "sqlite.runtime_pragmas");
        assert!(
            reported.contains("journal_mode=wal") && reported.contains("busy_timeout=5000ms"),
            "an application database runs in write-ahead logging and waits out a writer: {reported}"
        );

        // Every one of the pool's connections has to be reached, because the
        // check reads whichever one is free when it asks.
        let mut held = Vec::new();
        for _ in 0..5 {
            let mut connection = repository.pool().acquire().await.unwrap();
            sqlx::query("PRAGMA busy_timeout = 100")
                .execute(&mut *connection)
                .await
                .unwrap();
            held.push(connection);
        }
        drop(held);

        let mut downgraded = report_for_a_check();
        check_runtime_pragmas(&repository, &mut downgraded).await;
        assert!(
            !downgraded.is_healthy(),
            "a database that no longer waits out a writer must be reported"
        );
        assert!(
            detail_of(&downgraded, "sqlite.runtime_pragmas").contains("busy_timeout=100ms"),
            "the report must name the setting it found"
        );
        repository.close().await;
    }

    /// A piece left in the trash is not a fault, but an operator looking at a
    /// database that seems short of content has to be able to see it.
    #[tokio::test]
    async fn what_is_waiting_in_the_trash_is_counted() {
        let temp = tempfile::tempdir().unwrap();
        let repository = repository(&temp).await;

        let mut report = report_for_a_check();
        check_content_trash(&repository, &mut report).await;
        assert!(report.is_healthy());
        assert_eq!(
            detail_of(&report, "content.trash"),
            "0 piece(s) in the trash"
        );
        repository.close().await;
    }

    /// A row pointing at a parent that is not there survives every query that
    /// does not join it. Only this check finds it.
    #[tokio::test]
    async fn a_row_whose_parent_is_gone_is_reported_as_a_violation() {
        let temp = tempfile::tempdir().unwrap();
        let repository = repository(&temp).await;

        let mut healthy = report_for_a_check();
        check_foreign_keys(&repository, &mut healthy).await;
        assert!(healthy.is_healthy());
        assert_eq!(detail_of(&healthy, "sqlite.foreign_keys"), "no violations");

        // Enforcement is per connection, which is exactly how a row like this
        // gets written in the first place.
        let mut connection = repository.pool().acquire().await.unwrap();
        sqlx::query("PRAGMA foreign_keys = OFF")
            .execute(&mut *connection)
            .await
            .unwrap();
        sqlx::query("INSERT INTO content_tags (content_id, tag_id, position) VALUES (?, ?, 0)")
            .bind(9_999_i64)
            .bind(9_999_i64)
            .execute(&mut *connection)
            .await
            .unwrap();
        drop(connection);

        let mut violated = report_for_a_check();
        check_foreign_keys(&repository, &mut violated).await;
        assert!(
            !violated.is_healthy(),
            "a row whose parent is gone must be reported"
        );
        assert!(
            detail_of(&violated, "sqlite.foreign_keys").contains("violation"),
            "the report must say how many violations it found"
        );
        repository.close().await;
    }
}

#[cfg(test)]
mod release_history_tests {
    use super::*;

    use crate::release::ReleaseBuilder;

    fn detail_of(report: &DoctorReport, name: &str) -> String {
        report
            .checks
            .iter()
            .find(|check| check.name == name)
            .unwrap_or_else(|| panic!("{name} was not checked"))
            .detail
            .clone()
    }

    /// The history check reads every release ever published, not only the
    /// active one, and says how much of it it actually read.
    #[tokio::test]
    async fn a_release_history_says_how_much_of_it_was_verified() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("releases");
        let store = FilesystemReleaseStore::new(root.clone());

        let mut empty = report_for_a_check();
        check_release_history(&root, &store, &mut empty).await;
        assert!(empty.is_healthy());
        assert_eq!(
            detail_of(&empty, "release.history"),
            "0 manifest(s), 0 object(s), 0 byte(s) verified"
        );

        for (revision, body) in [(1_u64, b"first page ".as_slice()), (2, b"second page")] {
            let release = ReleaseBuilder::clean(revision, "https://writing.example")
                .unwrap()
                .asset("/", body.to_vec(), "text/html; charset=utf-8", None)
                .unwrap()
                .finish()
                .unwrap();
            for (id, bytes) in &release.objects {
                store.put_object(id, bytes).await.unwrap();
            }
            store.put_manifest(&release).await.unwrap();
        }

        let mut published = report_for_a_check();
        check_release_history(&root, &store, &mut published).await;
        assert!(published.is_healthy());
        assert_eq!(
            detail_of(&published, "release.history"),
            "2 manifest(s), 2 object(s), 22 byte(s) verified",
            "the history has to account for every release it read"
        );
    }
}

#[cfg(test)]
mod integrity_tests {
    use super::*;

    fn detail_of(report: &DoctorReport, name: &str) -> String {
        report
            .checks
            .iter()
            .find(|check| check.name == name)
            .unwrap_or_else(|| panic!("{name} was not checked"))
            .detail
            .clone()
    }

    /// A database can open, migrate and answer every query while part of it is
    /// already damaged. This is the check that looks, so what it finds has to
    /// reach the operator instead of being read as health.
    #[tokio::test]
    async fn a_database_that_opens_but_is_damaged_is_reported_with_what_sqlite_found() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("simple-blog.sqlite3");
        let repository = SqliteRepository::connect(&path).await.unwrap();
        sqlx::query("PRAGMA wal_checkpoint(TRUNCATE)")
            .execute(repository.pool())
            .await
            .unwrap();

        let mut healthy = report_for_a_check();
        check_quick_check(&repository, &mut healthy).await;
        assert!(healthy.is_healthy());
        assert_eq!(detail_of(&healthy, "sqlite.quick_check"), "ok");
        repository.close().await;

        // The head of a page is where a b-tree keeps the pointers the check
        // follows, so overwriting it is damage the file survives being opened.
        let mut bytes = std::fs::read(&path).unwrap();
        let page_size = match u16::from_be_bytes([bytes[16], bytes[17]]) {
            1 => 65_536,
            value => usize::from(value),
        };
        let last_page = bytes.len() - page_size;
        for byte in &mut bytes[last_page..last_page + 200] {
            *byte = 0xff;
        }
        std::fs::write(&path, &bytes).unwrap();

        let damaged = SqliteRepository::connect(&path).await.unwrap();
        let mut report = report_for_a_check();
        check_quick_check(&damaged, &mut report).await;
        assert!(
            !report.is_healthy(),
            "a damaged database must not be reported as healthy"
        );
        let detail = detail_of(&report, "sqlite.quick_check");
        assert!(
            detail.contains("in database main"),
            "the operator has to be told what SQLite found: {detail}"
        );
        damaged.close().await;
    }
}
