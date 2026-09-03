//! Host-neutral `.simple-blog` migration archive.
//!
//! The logical site model is independent of SQLite, D1, R2, or a particular
//! runtime. Derived public releases are deliberately excluded and rebuilt by
//! the destination adapter from canonical Markdown and media bytes.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs::File,
    io::{Read, Write},
    path::{Component, Path, PathBuf},
};

use chrono::{DateTime, Utc};
use serde::{
    Deserialize, Deserializer, Serialize,
    de::{Error as _, MapAccess, SeqAccess, Visitor},
};
use tar::{Builder, EntryType, Header};
use thiserror::Error;
use url::Url;
use uuid::Uuid;

use crate::domain::{
    content::{Content, ContentId, ContentRevision, Slug},
    media::{MediaAsset, MediaId},
    theme::{NavigationItem, SiteSettings, validate_navigation},
};

pub const PORTABLE_SITE_FORMAT_VERSION: u16 = 1;
const PORTABLE_ARCHIVE_FORMAT_VERSION: u16 = 1;
const MANIFEST_PATH: &str = "manifest.json";
const SITE_PATH: &str = "site.json";
const MAX_ENTRY_COUNT: usize = 100_000;
const MAX_METADATA_BYTES: u64 = 64 * 1024 * 1024;
const MAX_MEDIA_FILE_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const MAX_DECODED_BYTES: u64 = 8 * 1024 * 1024 * 1024;
const MAX_MARKDOWN_BYTES: usize = 2 * 1024 * 1024;
const MAX_SQLITE_INTEGER: u64 = 9_223_372_036_854_775_807;
const MAX_TAR_ZERO_PADDING: usize = 20 * 512;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PortableSiteV1 {
    pub format_version: u16,
    pub exported_at: DateTime<Utc>,
    pub canonical_origin: String,
    pub settings: SiteSettings,
    pub navigation: Vec<NavigationItem>,
    pub contents: Vec<PortableContent>,
    pub redirects: Vec<PortableRedirect>,
    pub media: Vec<MediaAsset>,
    pub engagement: BTreeMap<i64, PortableEngagement>,
    pub owner: Option<PortableOwner>,
    pub publication: PortablePublicationState,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PortableContent {
    pub current: Content,
    pub revisions: Vec<ContentRevision>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PortableRedirect {
    pub old_slug: Slug,
    pub content_id: ContentId,
    pub created_at: DateTime<Utc>,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PortableEngagement {
    pub likes: u64,
    pub views: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PortableOwner {
    pub user_handle: Uuid,
    pub created_at: DateTime<Utc>,
    pub passkeys: Vec<PortablePasskey>,
    pub recovery_codes: Vec<PortableRecoveryCode>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PortablePasskey {
    /// URL-safe unpadded base64, to keep the JSON schema language-neutral.
    pub credential_id: String,
    pub name: String,
    pub passkey_json: String,
    pub created_at: DateTime<Utc>,
    pub last_used_at: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PortableRecoveryCode {
    /// Lowercase hexadecimal SHA-256 hash; the bearer code is never exported.
    pub code_hash: String,
    pub consumed_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PortablePublicationState {
    pub public_revision: u64,
    pub next_publish_at: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PortablePackage {
    pub site: PortableSiteV1,
    /// Keyed by the exact portable filename stored in media metadata.
    pub media_files: BTreeMap<String, Vec<u8>>,
}

impl PortablePackage {
    pub fn validate(&self) -> Result<(), PortableArchiveError> {
        self.site.validate()?;
        let mut expected = BTreeMap::new();
        for asset in &self.site.media {
            let original = self
                .media_files
                .get(&asset.original_filename)
                .ok_or_else(|| {
                    PortableArchiveError::InvalidPackage(format!(
                        "missing media file: {}",
                        asset.original_filename
                    ))
                })?;
            validate_media_filename(&asset.original_filename)?;
            let expected_original = format!("{}.{}", asset.id, asset.extension);
            if asset.original_filename != expected_original {
                return Err(PortableArchiveError::InvalidPackage(format!(
                    "invalid original media filename: {}",
                    asset.original_filename
                )));
            }
            validate_byte_size(&asset.original_filename, asset.byte_size, original.len())?;
            let checksum = blake3::hash(original).to_hex().to_string();
            if checksum != asset.id.as_str() {
                return Err(PortableArchiveError::InvalidPackage(format!(
                    "media checksum mismatch: {}",
                    asset.original_filename
                )));
            }
            expected.insert(asset.original_filename.clone(), ());
            for variant in &asset.variants {
                validate_media_filename(&variant.filename)?;
                validate_byte_size(
                    &variant.filename,
                    variant.byte_size,
                    self.media_files
                        .get(&variant.filename)
                        .ok_or_else(|| {
                            PortableArchiveError::InvalidPackage(format!(
                                "missing media file: {}",
                                variant.filename
                            ))
                        })?
                        .len(),
                )?;
                expected.insert(variant.filename.clone(), ());
            }
        }
        for filename in self.media_files.keys() {
            if !expected.contains_key(filename) {
                return Err(PortableArchiveError::InvalidPackage(format!(
                    "unexpected media file: {filename}"
                )));
            }
        }
        Ok(())
    }
}

impl PortableSiteV1 {
    pub fn validate(&self) -> Result<(), PortableArchiveError> {
        if self.format_version != PORTABLE_SITE_FORMAT_VERSION {
            return Err(PortableArchiveError::UnsupportedSiteFormat(
                self.format_version,
            ));
        }
        validate_origin(&self.canonical_origin)?;
        let normalized_settings = self
            .settings
            .clone()
            .validated()
            .map_err(|error| PortableArchiveError::InvalidPackage(error.to_string()))?;
        if normalized_settings != self.settings {
            return invalid("site settings are not normalized");
        }
        validate_portable_navigation(&self.navigation)?;
        let (content_ids, slugs) = validate_contents(&self.contents)?;
        validate_redirects(&self.redirects, &content_ids, &slugs)?;
        validate_engagement(&self.engagement, &content_ids)?;
        let media_ids = validate_media_metadata(&self.media)?;
        validate_media_references(self, &media_ids)?;
        validate_publication_state(self)?;
        if let Some(owner) = &self.owner {
            validate_owner(owner)?;
        }
        Ok(())
    }
}

fn validate_portable_navigation(items: &[NavigationItem]) -> Result<(), PortableArchiveError> {
    let normalized = validate_navigation(items.to_vec())
        .map_err(|error| PortableArchiveError::InvalidPackage(error.to_string()))?;
    if normalized != items {
        return invalid("navigation is not normalized");
    }
    let mut ids = BTreeSet::new();
    for (position, item) in items.iter().enumerate() {
        let position = u16::try_from(position)
            .map_err(|error| PortableArchiveError::InvalidPackage(error.to_string()))?;
        if item.id <= 0 || item.position != position || !ids.insert(item.id) {
            return Err(PortableArchiveError::InvalidPackage(
                "navigation identities and positions are not canonical".into(),
            ));
        }
    }
    Ok(())
}

fn validate_contents(
    records: &[PortableContent],
) -> Result<(BTreeSet<i64>, BTreeSet<Slug>), PortableArchiveError> {
    let mut content_ids = BTreeSet::new();
    let mut slugs = BTreeSet::new();
    let mut revision_ids = BTreeSet::new();
    let mut tags = BTreeMap::new();
    for record in records {
        let content = &record.current;
        validate_content(content, content.id)?;
        if !content_ids.insert(content.id.as_i64()) || !slugs.insert(content.slug.clone()) {
            return invalid("duplicate content identity or slug");
        }
        validate_tags(content, &mut tags)?;
        for revision in &record.revisions {
            validate_content(&revision.snapshot, content.id)?;
            validate_tags(&revision.snapshot, &mut tags)?;
            if revision.content_id != content.id
                || revision.id <= 0
                || revision.snapshot.version > content.version
                || !revision_ids.insert(revision.id)
            {
                return invalid("invalid or duplicate content revision");
            }
        }
    }
    Ok((content_ids, slugs))
}

fn validate_content(content: &Content, expected_id: ContentId) -> Result<(), PortableArchiveError> {
    let clean_title = content.title.trim();
    let clean_summary = content.summary.trim();
    let valid_optional = |value: &Option<String>, maximum: usize| {
        value.as_ref().is_none_or(|value| {
            !value.trim().is_empty() && value.trim() == value && value.chars().count() <= maximum
        })
    };
    if content.id != expected_id
        || content.id.as_i64() <= 0
        || content.version <= 0
        || clean_title != content.title
        || clean_title.is_empty()
        || clean_title.chars().count() > 200
        || clean_summary != content.summary
        || clean_summary.chars().count() > 500
        || content.body_markdown.len() > MAX_MARKDOWN_BYTES
        || !valid_optional(&content.seo_title, 70)
        || !valid_optional(&content.seo_description, 200)
        || content.created_at > content.updated_at
        || content
            .deleted_at
            .is_some_and(|deleted_at| deleted_at < content.created_at)
    {
        return invalid("content violates the portable content contract");
    }
    Ok(())
}

fn validate_tags(
    content: &Content,
    known: &mut BTreeMap<Slug, String>,
) -> Result<(), PortableArchiveError> {
    if content.tags.len() > 20 {
        return invalid("content has too many tags");
    }
    let mut local = BTreeSet::new();
    for tag in &content.tags {
        if tag.name.trim() != tag.name
            || tag.name.is_empty()
            || tag.name.chars().count() > 50
            || !local.insert(tag.slug.clone())
        {
            return invalid("content has an invalid or duplicate tag");
        }
        if let Some(existing) = known.insert(tag.slug.clone(), tag.name.clone())
            && existing != tag.name
        {
            return invalid("one tag slug has conflicting names");
        }
    }
    Ok(())
}

fn validate_redirects(
    redirects: &[PortableRedirect],
    content_ids: &BTreeSet<i64>,
    slugs: &BTreeSet<Slug>,
) -> Result<(), PortableArchiveError> {
    let mut old_slugs = BTreeSet::new();
    for redirect in redirects {
        if !old_slugs.insert(redirect.old_slug.clone())
            || slugs.contains(&redirect.old_slug)
            || !content_ids.contains(&redirect.content_id.as_i64())
        {
            return invalid("invalid redirect graph");
        }
    }
    Ok(())
}

fn validate_engagement(
    engagement: &BTreeMap<i64, PortableEngagement>,
    content_ids: &BTreeSet<i64>,
) -> Result<(), PortableArchiveError> {
    if engagement.keys().copied().collect::<BTreeSet<_>>() != *content_ids {
        return invalid("engagement must account for every content identity");
    }
    if engagement
        .values()
        .any(|totals| totals.likes > MAX_SQLITE_INTEGER || totals.views > MAX_SQLITE_INTEGER)
    {
        return invalid("engagement counter exceeds the portable integer range");
    }
    Ok(())
}

fn validate_media_metadata(media: &[MediaAsset]) -> Result<BTreeSet<&str>, PortableArchiveError> {
    let mut media_ids = BTreeSet::new();
    let mut filenames = BTreeSet::new();
    for asset in media {
        if !media_ids.insert(asset.id.as_str())
            || asset.width == 0
            || asset.height == 0
            || asset.byte_size == 0
            || asset.byte_size > MAX_SQLITE_INTEGER
            || !filenames.insert(asset.original_filename.as_str())
        {
            return invalid("invalid or duplicate media metadata");
        }
        let mut widths = BTreeSet::new();
        for variant in &asset.variants {
            if variant.width == 0
                || variant.height == 0
                || variant.byte_size == 0
                || variant.byte_size > MAX_SQLITE_INTEGER
                || !widths.insert(variant.width)
                || !filenames.insert(&variant.filename)
            {
                return invalid("invalid or duplicate media variant");
            }
        }
    }
    Ok(media_ids)
}

fn validate_media_references(
    site: &PortableSiteV1,
    media_ids: &BTreeSet<&str>,
) -> Result<(), PortableArchiveError> {
    for media_id in site
        .contents
        .iter()
        .filter_map(|record| record.current.cover_media_id.as_deref())
        .chain(site.settings.logo_media_id.as_deref())
        .chain(site.settings.favicon_media_id.as_deref())
    {
        MediaId::parse(media_id)
            .map_err(|error| PortableArchiveError::InvalidPackage(error.to_string()))?;
        if !media_ids.contains(media_id) {
            return Err(PortableArchiveError::InvalidPackage(format!(
                "missing referenced media {media_id}"
            )));
        }
    }
    Ok(())
}

fn validate_publication_state(site: &PortableSiteV1) -> Result<(), PortableArchiveError> {
    if site.publication.public_revision > MAX_SQLITE_INTEGER {
        return invalid("public revision exceeds the portable integer range");
    }
    // Trashed entries never hold the clock: a scheduled piece in the trash
    // must not delay or trigger a publication boundary.
    let expected_next = site
        .contents
        .iter()
        .filter(|record| !record.current.is_trashed())
        .filter_map(|record| record.current.publication.publish_at())
        .filter(|publish_at| *publish_at > site.exported_at)
        .min();
    if expected_next != site.publication.next_publish_at {
        return invalid("publication clock does not match scheduled content");
    }
    Ok(())
}

fn invalid<T>(message: &str) -> Result<T, PortableArchiveError> {
    Err(PortableArchiveError::InvalidPackage(message.into()))
}

fn validate_owner(owner: &PortableOwner) -> Result<(), PortableArchiveError> {
    use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
    if owner.passkeys.is_empty() {
        return invalid("a portable owner must have at least one passkey");
    }
    let mut credentials = BTreeSet::new();
    for passkey in &owner.passkeys {
        let credential = URL_SAFE_NO_PAD
            .decode(&passkey.credential_id)
            .map_err(|error| {
                PortableArchiveError::InvalidPackage(format!("invalid passkey credential: {error}"))
            })?;
        if credential.is_empty() || !credentials.insert(credential) {
            return Err(PortableArchiveError::InvalidPackage(
                "empty or duplicate passkey credential".into(),
            ));
        }
        if passkey.name.trim() != passkey.name
            || passkey.name.is_empty()
            || passkey.name.chars().count() > 80
            || passkey.created_at < owner.created_at
            || passkey
                .last_used_at
                .is_some_and(|last_used_at| last_used_at < passkey.created_at)
        {
            return invalid("invalid portable passkey metadata");
        }
        serde_json::from_str::<StrictJsonValue>(&passkey.passkey_json).map_err(|error| {
            PortableArchiveError::InvalidPackage(format!("invalid passkey JSON: {error}"))
        })?;
    }
    let mut recovery = BTreeSet::new();
    for code in &owner.recovery_codes {
        if code.code_hash.len() != 64
            || !code
                .code_hash
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            || !recovery.insert(&code.code_hash)
        {
            return Err(PortableArchiveError::InvalidPackage(
                "invalid or duplicate recovery-code hash".into(),
            ));
        }
        if code.created_at < owner.created_at
            || code
                .consumed_at
                .is_some_and(|consumed_at| consumed_at < code.created_at)
        {
            return invalid("invalid portable recovery-code timestamps");
        }
    }
    Ok(())
}

fn validate_origin(origin: &str) -> Result<(), PortableArchiveError> {
    let parsed = Url::parse(origin)
        .map_err(|error| PortableArchiveError::InvalidPackage(error.to_string()))?;
    if !matches!(parsed.scheme(), "http" | "https")
        || parsed.host_str().is_none()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
        || parsed.path() != "/"
        || parsed.as_str().trim_end_matches('/') != origin
    {
        return Err(PortableArchiveError::InvalidPackage(
            "canonical origin is not normalized".into(),
        ));
    }
    Ok(())
}

fn validate_media_filename(filename: &str) -> Result<(), PortableArchiveError> {
    if filename.is_empty()
        || filename.len() > 200
        || filename.contains(['/', '\\', '\0'])
        || matches!(filename, "." | "..")
    {
        return Err(PortableArchiveError::InvalidPackage(format!(
            "unsafe media filename: {filename}"
        )));
    }
    Ok(())
}

fn validate_byte_size(
    filename: &str,
    expected: u64,
    actual: usize,
) -> Result<(), PortableArchiveError> {
    let actual = u64::try_from(actual)
        .map_err(|error| PortableArchiveError::InvalidPackage(error.to_string()))?;
    if expected != actual {
        return Err(PortableArchiveError::InvalidPackage(format!(
            "media byte size mismatch: {filename} (expected {expected}, actual {actual})"
        )));
    }
    Ok(())
}

pub struct PortableArchive;

impl PortableArchive {
    pub fn write(
        package: &PortablePackage,
        output: &Path,
    ) -> Result<PortableArchiveReport, PortableArchiveError> {
        package.validate()?;
        if output.exists() {
            return Err(PortableArchiveError::OutputExists(output.to_owned()));
        }
        let parent = output.parent().unwrap_or_else(|| Path::new("."));
        std::fs::create_dir_all(parent)?;
        let site = serde_json::to_vec(&package.site)
            .map_err(|error| PortableArchiveError::InvalidPackage(error.to_string()))?;
        let mut payloads = BTreeMap::from([(SITE_PATH.to_owned(), site)]);
        for (filename, bytes) in &package.media_files {
            payloads.insert(format!("media/{filename}"), bytes.clone());
        }
        let entries = payloads
            .iter()
            .map(|(path, bytes)| {
                (
                    path.clone(),
                    PortableArchiveEntry {
                        checksum: blake3::hash(bytes).to_hex().to_string(),
                        byte_size: u64::try_from(bytes.len()).unwrap_or(u64::MAX),
                    },
                )
            })
            .collect::<BTreeMap<_, _>>();
        let identity = PortableArchiveIdentity {
            archive_format_version: PORTABLE_ARCHIVE_FORMAT_VERSION,
            site_format_version: package.site.format_version,
            producer_version: env!("CARGO_PKG_VERSION").to_owned(),
            exported_at: package.site.exported_at,
            entries,
        };
        let identity_bytes = serde_json::to_vec(&identity)
            .map_err(|error| PortableArchiveError::InvalidPackage(error.to_string()))?;
        let archive_id = blake3::hash(&identity_bytes).to_hex().to_string();
        let manifest = PortableArchiveManifest {
            archive_id: archive_id.clone(),
            identity,
        };
        let manifest = serde_json::to_vec(&manifest)
            .map_err(|error| PortableArchiveError::InvalidPackage(error.to_string()))?;

        let partial = parent.join(format!(".simple-blog-archive-{}.partial", Uuid::new_v4()));
        let result = (|| {
            let file = File::create(&partial)?;
            let mut encoder = zstd::Encoder::new(file, 9)?;
            encoder.include_checksum(true)?;
            let mut archive = Builder::new(encoder);
            append_archive_bytes(
                &mut archive,
                MANIFEST_PATH,
                &manifest,
                package.site.exported_at,
            )?;
            for (path, bytes) in &payloads {
                append_archive_bytes(&mut archive, path, bytes, package.site.exported_at)?;
            }
            let encoder = archive.into_inner()?;
            encoder.finish()?.sync_all()?;
            install_archive(&partial, output, parent, crate::durable_fs::sync_directory)?;
            Ok::<_, PortableArchiveError>(())
        })();
        if result.is_err() {
            let _cleanup = std::fs::remove_file(&partial);
        }
        result?;
        tracing::info!(
            event = "portable.archive.written",
            archive_id,
            entry_count = payloads.len(),
            output = %output.display()
        );
        Ok(PortableArchiveReport {
            archive_id,
            entry_count: payloads.len(),
        })
    }

    pub fn read(path: &Path) -> Result<PortablePackage, PortableArchiveError> {
        let file = File::open(path)?;
        let decoder = zstd::Decoder::new(file)?.single_frame();
        let mut archive = tar::Archive::new(decoder);
        let mut files = BTreeMap::new();
        let mut decoded_bytes = 0_u64;
        for entry in archive.entries()? {
            if files.len() >= MAX_ENTRY_COUNT {
                return Err(PortableArchiveError::SafetyLimit(
                    "archive contains too many entries".into(),
                ));
            }
            let mut entry = entry?;
            if entry.header().entry_type() != EntryType::Regular {
                return Err(PortableArchiveError::UnsafeEntry(
                    "links, directories, and special files are forbidden".into(),
                ));
            }
            let entry_path = entry.path()?.into_owned();
            let name = validate_archive_path(&entry_path)?;
            let declared = entry.header().size()?;
            let maximum = if name == MANIFEST_PATH || name == SITE_PATH {
                MAX_METADATA_BYTES
            } else {
                MAX_MEDIA_FILE_BYTES
            };
            if declared > maximum {
                return Err(PortableArchiveError::SafetyLimit(format!(
                    "archive entry is too large: {name}"
                )));
            }
            decoded_bytes = decoded_bytes.saturating_add(declared);
            if decoded_bytes > MAX_DECODED_BYTES {
                return Err(PortableArchiveError::SafetyLimit(
                    "decoded archive is too large".into(),
                ));
            }
            let capacity = usize::try_from(declared.min(64 * 1024))
                .map_err(|error| PortableArchiveError::SafetyLimit(error.to_string()))?;
            let mut bytes = Vec::with_capacity(capacity);
            entry.read_to_end(&mut bytes)?;
            if files.insert(name.clone(), bytes).is_some() {
                return Err(PortableArchiveError::UnsafeEntry(format!(
                    "duplicate archive entry: {name}"
                )));
            }
        }
        let mut decoder = archive.into_inner();
        let mut trailing = [0_u8; 1024];
        let mut padding_bytes = 0_usize;
        loop {
            let read = decoder.read(&mut trailing)?;
            if read == 0 {
                break;
            }
            padding_bytes = padding_bytes.saturating_add(read);
            if padding_bytes > MAX_TAR_ZERO_PADDING
                || trailing[..read].iter().any(|byte| *byte != 0)
            {
                return Err(PortableArchiveError::InvalidArchive(
                    "trailing decoded data after tar archive".into(),
                ));
            }
        }
        let mut compressed = decoder.finish();
        if compressed.read(&mut trailing[..1])? != 0 {
            return Err(PortableArchiveError::InvalidArchive(
                "trailing bytes after zstd frame".into(),
            ));
        }
        let manifest_bytes = files.remove(MANIFEST_PATH).ok_or_else(|| {
            PortableArchiveError::InvalidArchive("manifest.json is missing".into())
        })?;
        let manifest = parse_archive_manifest(&manifest_bytes)?;
        manifest.verify(&files)?;
        let site_bytes = files
            .remove(SITE_PATH)
            .ok_or_else(|| PortableArchiveError::InvalidArchive("site.json is missing".into()))?;
        let site_value = strict_json_value(&site_bytes)?;
        let site: PortableSiteV1 = serde_json::from_value(site_value)
            .map_err(|error| PortableArchiveError::InvalidArchive(error.to_string()))?;
        manifest.verify_site_identity(&site)?;
        let media_files = files
            .into_iter()
            .map(|(path, bytes)| {
                path.strip_prefix("media/")
                    .map(|filename| (filename.to_owned(), bytes))
                    .ok_or(PortableArchiveError::UnsafeEntry(path))
            })
            .collect::<Result<BTreeMap<_, _>, _>>()?;
        let package = PortablePackage { site, media_files };
        package.validate()?;
        tracing::info!(
            event = "portable.archive.read",
            archive_id = manifest.archive_id,
            entry_count = manifest.identity.entries.len(),
            input = %path.display()
        );
        Ok(package)
    }
}

fn install_without_overwrite(
    source: &Path,
    destination: &Path,
) -> Result<(), PortableArchiveError> {
    match std::fs::hard_link(source, destination) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            Err(PortableArchiveError::OutputExists(destination.to_owned()))
        }
        Err(error) => Err(error.into()),
    }
}

fn install_archive(
    partial: &Path,
    output: &Path,
    parent: &Path,
    sync_parent: impl FnOnce(&Path) -> std::io::Result<()>,
) -> Result<(), PortableArchiveError> {
    install_without_overwrite(partial, output)?;
    let result = (|| {
        std::fs::remove_file(partial)?;
        sync_parent(parent)?;
        Ok::<_, PortableArchiveError>(())
    })();
    if result.is_err() {
        cleanup_failed_archive_path(partial, "portable.archive.partial_cleanup_failed");
        cleanup_failed_archive_path(output, "portable.archive.output_cleanup_failed");
    }
    result
}

fn cleanup_failed_archive_path(path: &Path, event: &'static str) {
    if let Err(error) = std::fs::remove_file(path)
        && error.kind() != std::io::ErrorKind::NotFound
    {
        tracing::warn!(event, path = %path.display(), error = %error);
    }
}

#[cfg(test)]
mod archive_install_tests {
    use super::*;

    #[test]
    fn a_post_install_directory_sync_failure_removes_every_visible_output() {
        let temp = tempfile::tempdir().unwrap();
        let partial = temp.path().join("archive.partial");
        let output = temp.path().join("archive.simple-blog");
        std::fs::write(&partial, b"complete archive").unwrap();

        let error = install_archive(&partial, &output, temp.path(), |_| {
            Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "injected directory sync failure",
            ))
        })
        .unwrap_err();

        assert!(matches!(error, PortableArchiveError::Io(_)));
        assert!(!partial.exists());
        assert!(!output.exists());
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PortableArchiveReport {
    pub archive_id: String,
    pub entry_count: usize,
}

#[derive(Debug, Deserialize, Serialize)]
struct PortableArchiveManifest {
    archive_id: String,
    #[serde(flatten)]
    identity: PortableArchiveIdentity,
}

impl PortableArchiveManifest {
    fn verify(&self, files: &BTreeMap<String, Vec<u8>>) -> Result<(), PortableArchiveError> {
        if self.identity.archive_format_version != PORTABLE_ARCHIVE_FORMAT_VERSION {
            return Err(PortableArchiveError::UnsupportedArchiveFormat(
                self.identity.archive_format_version,
            ));
        }
        if self.identity.site_format_version != PORTABLE_SITE_FORMAT_VERSION {
            return Err(PortableArchiveError::UnsupportedSiteFormat(
                self.identity.site_format_version,
            ));
        }
        if self.identity.producer_version.trim() != self.identity.producer_version
            || self.identity.producer_version.is_empty()
            || self.identity.producer_version.len() > 128
            || self
                .identity
                .producer_version
                .bytes()
                .any(|byte| byte.is_ascii_control())
        {
            return Err(PortableArchiveError::InvalidArchive(
                "invalid archive producer version".into(),
            ));
        }
        let identity = serde_json::to_vec(&self.identity)
            .map_err(|error| PortableArchiveError::InvalidArchive(error.to_string()))?;
        let actual_id = blake3::hash(&identity).to_hex().to_string();
        if actual_id != self.archive_id {
            return Err(PortableArchiveError::InvalidArchive(
                "archive identity checksum mismatch".into(),
            ));
        }
        let actual_names = files.keys().cloned().collect::<BTreeSet<_>>();
        let expected_names = self
            .identity
            .entries
            .keys()
            .cloned()
            .collect::<BTreeSet<_>>();
        if actual_names != expected_names {
            return Err(PortableArchiveError::InvalidArchive(
                "manifest does not match archive entries".into(),
            ));
        }
        for (name, expected) in &self.identity.entries {
            let bytes = &files[name];
            let actual_size = u64::try_from(bytes.len())
                .map_err(|error| PortableArchiveError::SafetyLimit(error.to_string()))?;
            let checksum = blake3::hash(bytes).to_hex().to_string();
            if actual_size != expected.byte_size || checksum != expected.checksum {
                return Err(PortableArchiveError::InvalidArchive(format!(
                    "checksum or size mismatch: {name}"
                )));
            }
        }
        Ok(())
    }

    fn verify_site_identity(&self, site: &PortableSiteV1) -> Result<(), PortableArchiveError> {
        if self.identity.site_format_version != site.format_version
            || self.identity.exported_at != site.exported_at
        {
            return Err(PortableArchiveError::InvalidArchive(
                "archive and site identity do not match".into(),
            ));
        }
        Ok(())
    }
}

fn parse_archive_manifest(bytes: &[u8]) -> Result<PortableArchiveManifest, PortableArchiveError> {
    const FIELDS: [&str; 6] = [
        "archive_id",
        "archive_format_version",
        "site_format_version",
        "producer_version",
        "exported_at",
        "entries",
    ];
    let value = strict_json_value(bytes)?;
    let Some(object) = value.as_object() else {
        return Err(PortableArchiveError::InvalidArchive(
            "archive manifest is not an object".into(),
        ));
    };
    if object.len() != FIELDS.len() || FIELDS.iter().any(|field| !object.contains_key(*field)) {
        return Err(PortableArchiveError::InvalidArchive(
            "archive manifest fields do not match format version 1".into(),
        ));
    }
    serde_json::from_value(value)
        .map_err(|error| PortableArchiveError::InvalidArchive(error.to_string()))
}

fn strict_json_value(bytes: &[u8]) -> Result<serde_json::Value, PortableArchiveError> {
    serde_json::from_slice::<StrictJsonValue>(bytes)
        .map(|value| value.0)
        .map_err(|error| PortableArchiveError::InvalidArchive(error.to_string()))
}

struct StrictJsonValue(serde_json::Value);

impl<'de> Deserialize<'de> for StrictJsonValue {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_any(StrictJsonVisitor)
    }
}

struct StrictJsonVisitor;

impl<'de> Visitor<'de> for StrictJsonVisitor {
    type Value = StrictJsonValue;

    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("JSON without duplicate object fields")
    }

    fn visit_bool<E>(self, value: bool) -> Result<Self::Value, E> {
        Ok(StrictJsonValue(value.into()))
    }

    fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E> {
        Ok(StrictJsonValue(value.into()))
    }

    fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E> {
        Ok(StrictJsonValue(value.into()))
    }

    fn visit_f64<E>(self, value: f64) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        serde_json::Number::from_f64(value)
            .map(serde_json::Value::Number)
            .map(StrictJsonValue)
            .ok_or_else(|| E::custom("non-finite JSON number"))
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E> {
        Ok(StrictJsonValue(value.into()))
    }

    fn visit_string<E>(self, value: String) -> Result<Self::Value, E> {
        Ok(StrictJsonValue(value.into()))
    }

    fn visit_none<E>(self) -> Result<Self::Value, E> {
        Ok(StrictJsonValue(serde_json::Value::Null))
    }

    fn visit_unit<E>(self) -> Result<Self::Value, E> {
        Ok(StrictJsonValue(serde_json::Value::Null))
    }

    fn visit_seq<A>(self, mut sequence: A) -> Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        let mut values = Vec::new();
        while let Some(value) = sequence.next_element::<StrictJsonValue>()? {
            values.push(value.0);
        }
        Ok(StrictJsonValue(serde_json::Value::Array(values)))
    }

    fn visit_map<A>(self, mut object: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut values = serde_json::Map::new();
        while let Some(key) = object.next_key::<String>()? {
            if values.contains_key(&key) {
                return Err(A::Error::custom(format!("duplicate JSON field: {key}")));
            }
            let value = object.next_value::<StrictJsonValue>()?;
            values.insert(key, value.0);
        }
        Ok(StrictJsonValue(serde_json::Value::Object(values)))
    }
}

#[derive(Debug, Deserialize, Serialize)]
struct PortableArchiveIdentity {
    archive_format_version: u16,
    site_format_version: u16,
    producer_version: String,
    exported_at: DateTime<Utc>,
    entries: BTreeMap<String, PortableArchiveEntry>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct PortableArchiveEntry {
    checksum: String,
    byte_size: u64,
}

fn append_archive_bytes<W: Write>(
    archive: &mut Builder<W>,
    path: &str,
    bytes: &[u8],
    timestamp: DateTime<Utc>,
) -> Result<(), PortableArchiveError> {
    let mut header = Header::new_gnu();
    header.set_entry_type(EntryType::Regular);
    header.set_mode(0o600);
    header.set_uid(0);
    header.set_gid(0);
    header.set_mtime(u64::try_from(timestamp.timestamp()).unwrap_or_default());
    header.set_size(
        u64::try_from(bytes.len())
            .map_err(|error| PortableArchiveError::SafetyLimit(error.to_string()))?,
    );
    header.set_cksum();
    archive.append_data(&mut header, path, bytes)?;
    Ok(())
}

fn validate_archive_path(path: &Path) -> Result<String, PortableArchiveError> {
    if path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(PortableArchiveError::UnsafeEntry(
            path.display().to_string(),
        ));
    }
    let name = path
        .to_str()
        .ok_or_else(|| PortableArchiveError::UnsafeEntry("non-UTF-8 entry".into()))?;
    let valid = matches!(name, MANIFEST_PATH | SITE_PATH)
        || name.strip_prefix("media/").is_some_and(|filename| {
            !filename.contains('/') && validate_media_filename(filename).is_ok()
        });
    if !valid {
        return Err(PortableArchiveError::UnsafeEntry(name.to_owned()));
    }
    Ok(name.to_owned())
}

#[derive(Debug, Error)]
pub enum PortableArchiveError {
    #[error("portable archive output already exists: {0}")]
    OutputExists(PathBuf),
    #[error("unsupported portable archive format: {0}")]
    UnsupportedArchiveFormat(u16),
    #[error("unsupported portable site format: {0}")]
    UnsupportedSiteFormat(u16),
    #[error("invalid portable package: {0}")]
    InvalidPackage(String),
    #[error("invalid portable archive: {0}")]
    InvalidArchive(String),
    #[error("unsafe portable archive entry: {0}")]
    UnsafeEntry(String),
    #[error("portable archive safety limit: {0}")]
    SafetyLimit(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}
