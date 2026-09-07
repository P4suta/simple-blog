//! The `.simple-blog` migration archive: its logical model, and the reader
//! and writer that carry it between conforming hosts.
//!
//! The logical site model is independent of SQLite, D1, R2, or a particular
//! runtime. Derived public releases are deliberately excluded and rebuilt by
//! the destination adapter from canonical Markdown and media bytes. The tar
//! and zstd framing in this module is the native implementation of that
//! model on a filesystem.

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
/// Starting size for an entry buffer. A hint only, and therefore invisible
/// to any behavioural test, so it is named and pinned rather than inlined.
const ENTRY_READ_CAPACITY: u64 = 64 * 1024;

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
    /// The kept states of the settings, oldest first. Omitted while empty so
    /// archives without a history stay byte-identical to older ones.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub settings_revisions: Vec<PortableSettingsRevision>,
}

/// One kept state of the settings and navigation. Navigation items carry no
/// durable identity here; the destination assigns its own on import.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PortableSettingsRevision {
    pub settings: SiteSettings,
    pub navigation: Vec<NavigationItem>,
    pub created_at: DateTime<Utc>,
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
        validate_settings_revisions(&self.settings_revisions)?;
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

/// A kept state must be one the destination could have saved itself:
/// normalized settings and a normalized navigation, in time order.
fn validate_settings_revisions(
    revisions: &[PortableSettingsRevision],
) -> Result<(), PortableArchiveError> {
    let mut previous: Option<DateTime<Utc>> = None;
    for revision in revisions {
        let normalized = revision
            .settings
            .clone()
            .validated()
            .map_err(|error| PortableArchiveError::InvalidPackage(error.to_string()))?;
        if normalized != revision.settings {
            return invalid("a settings revision is not normalized");
        }
        let navigation = validate_navigation(revision.navigation.clone())
            .map_err(|error| PortableArchiveError::InvalidPackage(error.to_string()))?;
        if navigation != revision.navigation {
            return invalid("a settings revision's navigation is not normalized");
        }
        if previous.is_some_and(|at| at > revision.created_at) {
            return invalid("settings revisions are not in time order");
        }
        previous = Some(revision.created_at);
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
    // A kept state of the settings may still name a logo or favicon the
    // current settings no longer do; restoring it must find the file.
    let remembered = site.settings_revisions.iter().flat_map(|revision| {
        revision
            .settings
            .logo_media_id
            .as_deref()
            .into_iter()
            .chain(revision.settings.favicon_media_id.as_deref())
    });
    for media_id in site
        .contents
        .iter()
        .filter_map(|record| record.current.cover_media_id.as_deref())
        .chain(site.settings.logo_media_id.as_deref())
        .chain(site.settings.favicon_media_id.as_deref())
        .chain(remembered)
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
            let capacity = usize::try_from(declared.min(ENTRY_READ_CAPACITY))
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

#[cfg(test)]
mod portable_contract_tests {
    use super::*;

    /// The limits are a compatibility promise: an archive one build accepts,
    /// another build of the same format version must accept too. Spelling the
    /// numbers out here means an edit to the arithmetic has to be deliberate.
    #[test]
    fn the_portable_limits_are_the_documented_numbers() {
        assert_eq!(MAX_ENTRY_COUNT, 100_000);
        assert_eq!(MAX_METADATA_BYTES, 67_108_864);
        assert_eq!(MAX_MEDIA_FILE_BYTES, 2_147_483_648);
        assert_eq!(MAX_DECODED_BYTES, 8_589_934_592);
        assert_eq!(MAX_MARKDOWN_BYTES, 2_097_152);
        assert_eq!(MAX_SQLITE_INTEGER, 9_223_372_036_854_775_807);
        assert_eq!(MAX_TAR_ZERO_PADDING, 10_240);
        assert_eq!(ENTRY_READ_CAPACITY, 65_536);
        assert_eq!(PORTABLE_SITE_FORMAT_VERSION, 1);
        assert_eq!(PORTABLE_ARCHIVE_FORMAT_VERSION, 1);
    }

    #[test]
    fn a_normalized_origin_is_accepted() {
        validate_origin("https://writing.example").unwrap();
        validate_origin("http://writing.example").unwrap();
        validate_origin("https://writing.example:8443").unwrap();
    }

    /// One case per clause, each tripping exactly that clause, so no clause can
    /// be dropped from the guard without a case turning green.
    #[test]
    fn every_way_an_origin_is_not_normalized_is_rejected() {
        // Each origin is already in the form Url::parse would print, so it
        // trips exactly one clause and no other.
        for origin in [
            "ftp://writing.example",
            "https://writer@writing.example",
            "https://:secret@writing.example",
            "https://writing.example/?draft=1",
            "https://writing.example/#top",
            "https://writing.example/blog",
            "https://writing.example/",
        ] {
            assert!(
                validate_origin(origin).is_err(),
                "accepted an origin that is not normalized: {origin}"
            );
        }
        assert!(validate_origin("not a url").is_err());
    }

    /// The limit is on the bytes a filesystem has to store, not on the
    /// characters a person sees, so 100 two-byte characters are the same
    /// length as 200 one-byte ones.
    #[test]
    fn a_plain_media_filename_is_accepted() {
        validate_media_filename("cover.png").unwrap();
        validate_media_filename(&"a".repeat(200)).unwrap();
        validate_media_filename(&"é".repeat(100)).unwrap();
        assert!(validate_media_filename(&"é".repeat(101)).is_err());
    }

    #[test]
    fn every_unsafe_media_filename_shape_is_rejected() {
        for filename in ["", "a/b.png", r"a\b.png", "a\0b.png", ".", ".."] {
            assert!(
                validate_media_filename(filename).is_err(),
                "accepted an unsafe media filename: {filename:?}"
            );
        }
        assert!(validate_media_filename(&"a".repeat(201)).is_err());
    }

    #[test]
    fn a_byte_size_matches_only_the_length_it_declares() {
        validate_byte_size("cover.png", 3, 3).unwrap();
        assert!(validate_byte_size("cover.png", 3, 4).is_err());
        assert!(validate_byte_size("cover.png", 4, 3).is_err());
    }

    #[test]
    fn engagement_accounts_for_every_content_identity_and_stays_in_range() {
        let ids = BTreeSet::from([7_i64]);
        let totals =
            |likes: u64, views: u64| BTreeMap::from([(7_i64, PortableEngagement { likes, views })]);

        validate_engagement(&totals(0, 0), &ids).unwrap();
        // The maximum is representable: the guard is `>`, not `>=`.
        validate_engagement(&totals(MAX_SQLITE_INTEGER, MAX_SQLITE_INTEGER), &ids).unwrap();

        assert!(validate_engagement(&BTreeMap::new(), &ids).is_err());
        assert!(validate_engagement(&totals(0, 0), &BTreeSet::from([7_i64, 8_i64])).is_err());
        assert!(validate_engagement(&totals(MAX_SQLITE_INTEGER + 1, 0), &ids).is_err());
        assert!(validate_engagement(&totals(0, MAX_SQLITE_INTEGER + 1), &ids).is_err());
    }
}

#[cfg(test)]
mod portable_validator_tests {
    use super::*;
    use chrono::TimeZone as _;

    use crate::domain::{
        content::{ContentKind, Publication, Tag},
        media::MediaVariant,
    };

    fn at() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 9, 2, 12, 0, 0).unwrap()
    }

    fn hex(fill: char, length: usize) -> String {
        std::iter::repeat_n(fill, length).collect()
    }

    fn content() -> Content {
        Content {
            id: ContentId::from_i64(7),
            kind: ContentKind::Post,
            title: "Portable".into(),
            slug: Slug::parse("portable").unwrap(),
            summary: "Leaves any host".into(),
            body_markdown: "# Canonical".into(),
            body_html: "<h1>Canonical</h1>".into(),
            tags: Vec::new(),
            cover_media_id: None,
            seo_title: None,
            seo_description: None,
            publication: Publication::Public { publish_at: at() },
            version: 3,
            created_at: at(),
            updated_at: at(),
            deleted_at: None,
        }
    }

    fn tag(name: &str, slug: &str) -> Tag {
        Tag {
            name: name.into(),
            slug: Slug::parse(slug).unwrap(),
        }
    }

    fn asset(id_fill: char, filename: &str) -> MediaAsset {
        MediaAsset {
            id: MediaId::parse(hex(id_fill, 64)).unwrap(),
            original_name: "Cover".into(),
            original_filename: filename.into(),
            mime_type: "image/png".into(),
            extension: "png".into(),
            width: 800,
            height: 600,
            byte_size: 1024,
            alt_text: String::new(),
            caption: String::new(),
            animated: false,
            variants: Vec::new(),
            created_at: at(),
        }
    }

    fn variant(width: u32, filename: &str) -> MediaVariant {
        MediaVariant {
            width,
            height: 600,
            byte_size: 512,
            filename: filename.into(),
        }
    }

    fn passkey(credential_id: &str, name: &str) -> PortablePasskey {
        PortablePasskey {
            credential_id: credential_id.into(),
            name: name.into(),
            passkey_json: "{}".into(),
            created_at: at(),
            last_used_at: None,
        }
    }

    fn recovery(code_hash: String) -> PortableRecoveryCode {
        PortableRecoveryCode {
            code_hash,
            consumed_at: None,
            created_at: at(),
        }
    }

    fn owner() -> PortableOwner {
        PortableOwner {
            user_handle: Uuid::nil(),
            created_at: at(),
            passkeys: vec![passkey("AQID", "Laptop")],
            recovery_codes: vec![recovery(hex('a', 64))],
        }
    }

    type Cases<T> = Vec<(&'static str, Box<dyn Fn(&mut T)>)>;

    /// Each case trips exactly one clause of a guard, so dropping any clause
    /// leaves one case accepting a package the contract forbids.
    fn each<T>(base: impl Fn() -> T, cases: Cases<T>, rejected: impl Fn(&T) -> bool) {
        for (label, mutate) in cases {
            let mut value = base();
            mutate(&mut value);
            assert!(
                rejected(&value),
                "accepted what the contract forbids: {label}"
            );
        }
    }

    #[test]
    fn a_conforming_piece_of_content_is_accepted() {
        validate_content(&content(), ContentId::from_i64(7)).unwrap();
    }

    #[test]
    fn the_content_contract_holds_at_its_boundaries() {
        let mut exact = content();
        exact.title = "t".repeat(200);
        exact.summary = "s".repeat(500);
        exact.body_markdown = "b".repeat(MAX_MARKDOWN_BYTES);
        exact.seo_title = Some("o".repeat(70));
        exact.seo_description = Some("d".repeat(200));
        exact.version = 1;
        validate_content(&exact, ContentId::from_i64(7)).unwrap();

        let mut minimal = content();
        minimal.id = ContentId::from_i64(1);
        validate_content(&minimal, ContentId::from_i64(1)).unwrap();
    }

    #[test]
    fn every_way_content_can_violate_the_contract_is_rejected() {
        let cases: Cases<Content> = vec![
            (
                "identity does not match",
                Box::new(|c: &mut Content| c.id = ContentId::from_i64(8)),
            ),
            ("version is zero", Box::new(|c: &mut Content| c.version = 0)),
            (
                "title is untrimmed",
                Box::new(|c: &mut Content| c.title = " Portable ".into()),
            ),
            (
                "title is empty",
                Box::new(|c: &mut Content| c.title = String::new()),
            ),
            (
                "title is too long",
                Box::new(|c: &mut Content| c.title = "t".repeat(201)),
            ),
            (
                "summary is untrimmed",
                Box::new(|c: &mut Content| c.summary = " Leaves ".into()),
            ),
            (
                "summary is too long",
                Box::new(|c: &mut Content| c.summary = "s".repeat(501)),
            ),
            (
                "body is too long",
                Box::new(|c: &mut Content| c.body_markdown = "b".repeat(MAX_MARKDOWN_BYTES + 1)),
            ),
            (
                "body is too long in bytes rather than in characters",
                Box::new(|c: &mut Content| {
                    c.body_markdown = "é".repeat(MAX_MARKDOWN_BYTES / 2 + 1);
                }),
            ),
            (
                "seo title is blank",
                Box::new(|c: &mut Content| c.seo_title = Some("   ".into())),
            ),
            (
                "seo title is untrimmed",
                Box::new(|c: &mut Content| c.seo_title = Some(" Title ".into())),
            ),
            (
                "seo title is too long",
                Box::new(|c: &mut Content| c.seo_title = Some("o".repeat(71))),
            ),
            (
                "seo description is blank",
                Box::new(|c: &mut Content| c.seo_description = Some("   ".into())),
            ),
            (
                "seo description is untrimmed",
                Box::new(|c: &mut Content| c.seo_description = Some(" Text ".into())),
            ),
            (
                "seo description is too long",
                Box::new(|c: &mut Content| c.seo_description = Some("d".repeat(201))),
            ),
            (
                "created after updated",
                Box::new(|c: &mut Content| {
                    c.created_at = c.updated_at + chrono::Duration::seconds(1);
                }),
            ),
            (
                "deleted before created",
                Box::new(|c: &mut Content| {
                    c.deleted_at = Some(c.created_at - chrono::Duration::seconds(1));
                }),
            ),
        ];
        each(content, cases, |c| {
            validate_content(c, ContentId::from_i64(7)).is_err()
        });
    }

    #[test]
    fn a_content_identity_at_or_below_zero_is_rejected() {
        for identity in [0_i64, -1] {
            let mut record = content();
            record.id = ContentId::from_i64(identity);
            assert!(
                validate_content(&record, ContentId::from_i64(identity)).is_err(),
                "accepted content identity {identity}"
            );
        }
    }

    #[test]
    fn tags_are_accepted_up_to_their_limits() {
        let mut twenty = content();
        twenty.tags = (0..20)
            .map(|index| tag("Tag", &format!("tag-{index}")))
            .collect();
        validate_tags(&twenty, &mut BTreeMap::new()).unwrap();

        let mut longest = content();
        longest.tags = vec![tag(&"n".repeat(50), "long")];
        validate_tags(&longest, &mut BTreeMap::new()).unwrap();
    }

    #[test]
    fn every_invalid_tag_shape_is_rejected() {
        let cases: Vec<(&str, Vec<Tag>)> = vec![
            (
                "too many tags",
                (0..21)
                    .map(|index| tag("Tag", &format!("tag-{index}")))
                    .collect(),
            ),
            ("untrimmed name", vec![tag(" Tag ", "topic")]),
            ("empty name", vec![tag("", "topic")]),
            ("name too long", vec![tag(&"n".repeat(51), "topic")]),
            (
                "duplicate slug",
                vec![tag("One", "topic"), tag("Two", "topic")],
            ),
        ];
        for (label, tags) in cases {
            let mut record = content();
            record.tags = tags;
            assert!(
                validate_tags(&record, &mut BTreeMap::new()).is_err(),
                "accepted an invalid tag set: {label}"
            );
        }
    }

    #[test]
    fn one_tag_slug_may_not_carry_two_names_across_content() {
        let mut known = BTreeMap::new();
        let mut first = content();
        first.tags = vec![tag("Rust", "rust")];
        validate_tags(&first, &mut known).unwrap();

        let mut repeated = content();
        repeated.tags = vec![tag("Rust", "rust")];
        validate_tags(&repeated, &mut known).unwrap();

        let mut conflicting = content();
        conflicting.tags = vec![tag("Rustlang", "rust")];
        assert!(validate_tags(&conflicting, &mut known).is_err());
    }

    #[test]
    fn conforming_media_metadata_returns_every_asset_identity() {
        let media = vec![asset('a', "one.png"), asset('b', "two.png")];
        let first = hex('a', 64);
        let second = hex('b', 64);
        assert_eq!(
            validate_media_metadata(&media).unwrap(),
            BTreeSet::from([first.as_str(), second.as_str()])
        );
        assert!(validate_media_metadata(&[]).unwrap().is_empty());
    }

    #[test]
    fn media_byte_sizes_are_accepted_up_to_the_portable_integer_range() {
        let mut largest = asset('a', "one.png");
        largest.byte_size = MAX_SQLITE_INTEGER;
        largest.variants = vec![MediaVariant {
            byte_size: MAX_SQLITE_INTEGER,
            ..variant(400, "one-400.png")
        }];
        validate_media_metadata(std::slice::from_ref(&largest)).unwrap();
    }

    #[test]
    fn every_invalid_media_shape_is_rejected() {
        let cases: Cases<Vec<MediaAsset>> = vec![
            (
                "duplicate media identity",
                Box::new(|m: &mut Vec<MediaAsset>| m.push(asset('a', "other.png"))),
            ),
            (
                "zero width",
                Box::new(|m: &mut Vec<MediaAsset>| m[0].width = 0),
            ),
            (
                "zero height",
                Box::new(|m: &mut Vec<MediaAsset>| m[0].height = 0),
            ),
            (
                "zero byte size",
                Box::new(|m: &mut Vec<MediaAsset>| m[0].byte_size = 0),
            ),
            (
                "byte size beyond the portable integer range",
                Box::new(|m: &mut Vec<MediaAsset>| m[0].byte_size = MAX_SQLITE_INTEGER + 1),
            ),
            (
                "duplicate original filename",
                Box::new(|m: &mut Vec<MediaAsset>| m.push(asset('b', "one.png"))),
            ),
            (
                "variant of zero width",
                Box::new(|m: &mut Vec<MediaAsset>| m[0].variants = vec![variant(0, "v.png")]),
            ),
            (
                "variant of zero height",
                Box::new(|m: &mut Vec<MediaAsset>| {
                    m[0].variants = vec![MediaVariant {
                        height: 0,
                        ..variant(400, "v.png")
                    }];
                }),
            ),
            (
                "variant of zero byte size",
                Box::new(|m: &mut Vec<MediaAsset>| {
                    m[0].variants = vec![MediaVariant {
                        byte_size: 0,
                        ..variant(400, "v.png")
                    }];
                }),
            ),
            (
                "variant byte size beyond the portable integer range",
                Box::new(|m: &mut Vec<MediaAsset>| {
                    m[0].variants = vec![MediaVariant {
                        byte_size: MAX_SQLITE_INTEGER + 1,
                        ..variant(400, "v.png")
                    }];
                }),
            ),
            (
                "two variants of the same width",
                Box::new(|m: &mut Vec<MediaAsset>| {
                    m[0].variants = vec![variant(400, "v.png"), variant(400, "w.png")];
                }),
            ),
            (
                "variant filename collides with the original",
                Box::new(|m: &mut Vec<MediaAsset>| {
                    m[0].variants = vec![variant(400, "one.png")];
                }),
            ),
        ];
        each(
            || vec![asset('a', "one.png")],
            cases,
            |m| validate_media_metadata(m).is_err(),
        );
    }

    #[test]
    fn a_conforming_owner_is_accepted() {
        validate_owner(&owner()).unwrap();

        let mut boundary = owner();
        boundary.passkeys = vec![PortablePasskey {
            last_used_at: Some(at()),
            ..passkey("AQID", &"n".repeat(80))
        }];
        boundary.recovery_codes = vec![PortableRecoveryCode {
            consumed_at: Some(at()),
            ..recovery("0123456789abcdef".repeat(4))
        }];
        validate_owner(&boundary).unwrap();
    }

    #[test]
    fn every_invalid_owner_credential_is_rejected() {
        let cases: Cases<PortableOwner> = vec![
            (
                "no passkey at all",
                Box::new(|o: &mut PortableOwner| o.passkeys.clear()),
            ),
            (
                "credential is not base64",
                Box::new(|o: &mut PortableOwner| {
                    o.passkeys[0].credential_id = "not base64!".into();
                }),
            ),
            (
                "credential decodes to nothing",
                Box::new(|o: &mut PortableOwner| o.passkeys[0].credential_id = String::new()),
            ),
            (
                "duplicate credential",
                Box::new(|o: &mut PortableOwner| o.passkeys.push(passkey("AQID", "Phone"))),
            ),
            (
                "passkey name is untrimmed",
                Box::new(|o: &mut PortableOwner| o.passkeys[0].name = " Laptop ".into()),
            ),
            (
                "passkey name is empty",
                Box::new(|o: &mut PortableOwner| o.passkeys[0].name = String::new()),
            ),
            (
                "passkey name is too long",
                Box::new(|o: &mut PortableOwner| o.passkeys[0].name = "n".repeat(81)),
            ),
            (
                "passkey predates the owner",
                Box::new(|o: &mut PortableOwner| {
                    o.passkeys[0].created_at = o.created_at - chrono::Duration::seconds(1);
                }),
            ),
            (
                "passkey used before it existed",
                Box::new(|o: &mut PortableOwner| {
                    o.passkeys[0].last_used_at =
                        Some(o.passkeys[0].created_at - chrono::Duration::seconds(1));
                }),
            ),
            (
                "passkey JSON is malformed",
                Box::new(|o: &mut PortableOwner| o.passkeys[0].passkey_json = "{".into()),
            ),
            (
                "recovery hash is short",
                Box::new(|o: &mut PortableOwner| o.recovery_codes[0].code_hash = hex('a', 63)),
            ),
            (
                "recovery hash is long",
                Box::new(|o: &mut PortableOwner| o.recovery_codes[0].code_hash = hex('a', 65)),
            ),
            (
                "recovery hash is not hexadecimal",
                Box::new(|o: &mut PortableOwner| o.recovery_codes[0].code_hash = hex('g', 64)),
            ),
            (
                "recovery hash is uppercase",
                Box::new(|o: &mut PortableOwner| o.recovery_codes[0].code_hash = hex('A', 64)),
            ),
            (
                "duplicate recovery hash",
                Box::new(|o: &mut PortableOwner| o.recovery_codes.push(recovery(hex('a', 64)))),
            ),
            (
                "recovery code predates the owner",
                Box::new(|o: &mut PortableOwner| {
                    o.recovery_codes[0].created_at = o.created_at - chrono::Duration::seconds(1);
                }),
            ),
            (
                "recovery code consumed before it existed",
                Box::new(|o: &mut PortableOwner| {
                    o.recovery_codes[0].consumed_at =
                        Some(o.recovery_codes[0].created_at - chrono::Duration::seconds(1));
                }),
            ),
        ];
        each(owner, cases, |o| validate_owner(o).is_err());
    }
}

#[cfg(test)]
mod portable_graph_tests {
    use super::*;
    use chrono::TimeZone as _;

    use crate::domain::content::{ContentKind, Publication, SaveIntent, Tag};

    type Cases = Vec<(&'static str, Box<dyn Fn(&mut Vec<PortableContent>)>)>;

    fn at() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 9, 2, 12, 0, 0).unwrap()
    }

    fn content(id: i64, slug: &str) -> Content {
        Content {
            id: ContentId::from_i64(id),
            kind: ContentKind::Post,
            title: "Portable".into(),
            slug: Slug::parse(slug).unwrap(),
            summary: "Leaves any host".into(),
            body_markdown: "# Canonical".into(),
            body_html: "<h1>Canonical</h1>".into(),
            tags: Vec::new(),
            cover_media_id: None,
            seo_title: None,
            seo_description: None,
            publication: Publication::Public { publish_at: at() },
            version: 3,
            created_at: at(),
            updated_at: at(),
            deleted_at: None,
        }
    }

    fn revision(id: i64, content_id: i64, version: i64) -> ContentRevision {
        let mut snapshot = content(content_id, "portable");
        snapshot.version = version;
        ContentRevision {
            id,
            content_id: ContentId::from_i64(content_id),
            intent: SaveIntent::Explicit,
            snapshot,
            created_at: at(),
        }
    }

    fn record() -> PortableContent {
        PortableContent {
            current: content(7, "portable"),
            revisions: vec![revision(1, 7, 2)],
        }
    }

    fn item(id: i64, position: u16, label: &str) -> NavigationItem {
        NavigationItem {
            id,
            label: label.into(),
            destination: "/about".into(),
            is_external: false,
            position,
        }
    }

    #[test]
    fn a_conforming_content_graph_yields_its_identities_and_slugs() {
        let (ids, slugs) = validate_contents(std::slice::from_ref(&record())).unwrap();
        assert_eq!(ids, BTreeSet::from([7]));
        assert_eq!(slugs, BTreeSet::from([Slug::parse("portable").unwrap()]));

        let (empty_ids, empty_slugs) = validate_contents(&[]).unwrap();
        assert!(empty_ids.is_empty());
        assert!(empty_slugs.is_empty());
    }

    #[test]
    fn every_broken_content_graph_is_rejected() {
        let cases: Cases = vec![
            (
                "two records with one identity",
                Box::new(|r: &mut Vec<PortableContent>| {
                    let mut duplicate = record();
                    duplicate.current.slug = Slug::parse("other").unwrap();
                    duplicate.revisions = vec![revision(2, 7, 2)];
                    r.push(duplicate);
                }),
            ),
            (
                "two records with one slug",
                Box::new(|r: &mut Vec<PortableContent>| {
                    let mut duplicate = record();
                    duplicate.current.id = ContentId::from_i64(8);
                    duplicate.revisions = vec![revision(2, 8, 2)];
                    r.push(duplicate);
                }),
            ),
            (
                "a revision belonging to other content",
                Box::new(|r: &mut Vec<PortableContent>| {
                    r[0].revisions[0].content_id = ContentId::from_i64(8);
                }),
            ),
            (
                "a revision identity at zero",
                Box::new(|r: &mut Vec<PortableContent>| r[0].revisions[0].id = 0),
            ),
            (
                "a revision newer than the piece it belongs to",
                Box::new(|r: &mut Vec<PortableContent>| {
                    r[0].revisions[0].snapshot.version = r[0].current.version + 1;
                }),
            ),
            (
                "two revisions with one identity",
                Box::new(|r: &mut Vec<PortableContent>| {
                    r[0].revisions.push(revision(1, 7, 1));
                }),
            ),
        ];
        for (label, mutate) in cases {
            let mut records = vec![record()];
            mutate(&mut records);
            assert!(
                validate_contents(&records).is_err(),
                "accepted a content graph that cannot be restored: {label}"
            );
        }
    }

    #[test]
    fn one_tag_slug_with_two_names_across_records_is_rejected() {
        let mut first = record();
        first.current.tags = vec![Tag {
            name: "Rust".into(),
            slug: Slug::parse("rust").unwrap(),
        }];
        first.revisions = Vec::new();
        let mut second = record();
        second.current.id = ContentId::from_i64(8);
        second.current.slug = Slug::parse("second").unwrap();
        second.current.tags = vec![Tag {
            name: "Rustlang".into(),
            slug: Slug::parse("rust").unwrap(),
        }];
        second.revisions = Vec::new();

        assert!(validate_contents(&[first, second]).is_err());
    }

    #[test]
    fn a_redirect_points_from_a_retired_slug_to_a_piece_that_is_here() {
        let ids = BTreeSet::from([7_i64]);
        let slugs = BTreeSet::from([Slug::parse("portable").unwrap()]);
        let redirect = |slug: &str, id: i64| PortableRedirect {
            old_slug: Slug::parse(slug).unwrap(),
            content_id: ContentId::from_i64(id),
            created_at: at(),
        };

        validate_redirects(&[redirect("older", 7)], &ids, &slugs).unwrap();
        validate_redirects(&[], &ids, &slugs).unwrap();

        for (label, redirects) in [
            (
                "the same retired slug twice",
                vec![redirect("older", 7), redirect("older", 7)],
            ),
            (
                "a retired slug that is still live",
                vec![redirect("portable", 7)],
            ),
            (
                "a destination that is not in the archive",
                vec![redirect("older", 8)],
            ),
        ] {
            assert!(
                validate_redirects(&redirects, &ids, &slugs).is_err(),
                "accepted a redirect graph that cannot be restored: {label}"
            );
        }
    }

    #[test]
    fn navigation_travels_only_in_its_canonical_form() {
        validate_portable_navigation(&[]).unwrap();
        validate_portable_navigation(&[item(1, 0, "About"), item(2, 1, "Contact")]).unwrap();

        for (label, items) in [
            ("a label that is not trimmed", vec![item(1, 0, " About ")]),
            ("an identity at zero", vec![item(0, 0, "About")]),
            (
                "a position that is not the item's own",
                vec![item(1, 3, "About")],
            ),
            (
                "two items with one identity",
                vec![item(1, 0, "About"), item(1, 1, "Contact")],
            ),
        ] {
            assert!(
                validate_portable_navigation(&items).is_err(),
                "accepted navigation that is not canonical: {label}"
            );
        }
    }
}

#[cfg(test)]
mod publication_clock_tests {
    use super::*;
    use chrono::TimeZone as _;

    use crate::domain::{
        content::{ContentKind, Publication},
        theme::Locale,
    };

    fn at() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 9, 2, 12, 0, 0).unwrap()
    }

    fn later(days: i64) -> DateTime<Utc> {
        at() + chrono::Duration::days(days)
    }

    fn scheduled(id: i64, slug: &str, publish_at: DateTime<Utc>) -> PortableContent {
        PortableContent {
            current: Content {
                id: ContentId::from_i64(id),
                kind: ContentKind::Post,
                title: "Portable".into(),
                slug: Slug::parse(slug).unwrap(),
                summary: String::new(),
                body_markdown: "# Canonical".into(),
                body_html: "<h1>Canonical</h1>".into(),
                tags: Vec::new(),
                cover_media_id: None,
                seo_title: None,
                seo_description: None,
                publication: Publication::Public { publish_at },
                version: 1,
                created_at: at(),
                updated_at: at(),
                deleted_at: None,
            },
            revisions: Vec::new(),
        }
    }

    fn site(contents: Vec<PortableContent>, next: Option<DateTime<Utc>>) -> PortableSiteV1 {
        PortableSiteV1 {
            format_version: PORTABLE_SITE_FORMAT_VERSION,
            exported_at: at(),
            canonical_origin: "https://writing.example".into(),
            settings: SiteSettings {
                site_title: "Portable site".into(),
                site_description: String::new(),
                locale: Locale::En,
                logo_media_id: None,
                favicon_media_id: None,
                custom_css: String::new(),
                timezone: "UTC".into(),
                author_name: String::new(),
                custom_css_backup: None,
            },
            navigation: Vec::new(),
            contents,
            redirects: Vec::new(),
            media: Vec::new(),
            engagement: BTreeMap::new(),
            owner: None,
            publication: PortablePublicationState {
                public_revision: 1,
                next_publish_at: next,
            },
            settings_revisions: Vec::new(),
        }
    }

    #[test]
    fn a_public_revision_is_accepted_right_up_to_the_portable_integer_range() {
        let mut largest = site(Vec::new(), None);
        largest.publication.public_revision = MAX_SQLITE_INTEGER;
        validate_publication_state(&largest).unwrap();

        let mut beyond = site(Vec::new(), None);
        beyond.publication.public_revision = MAX_SQLITE_INTEGER + 1;
        assert!(validate_publication_state(&beyond).is_err());
    }

    #[test]
    fn the_clock_is_the_earliest_piece_still_ahead_of_the_export() {
        let contents = vec![
            scheduled(7, "later", later(9)),
            scheduled(8, "sooner", later(2)),
            scheduled(9, "already-out", later(-3)),
        ];
        validate_publication_state(&site(contents.clone(), Some(later(2)))).unwrap();

        // Naming any other moment, or none at all, is a clock that disagrees
        // with the content the archive carries.
        assert!(validate_publication_state(&site(contents.clone(), Some(later(9)))).is_err());
        assert!(validate_publication_state(&site(contents, None)).is_err());
    }

    #[test]
    fn nothing_scheduled_means_no_clock_at_all() {
        validate_publication_state(&site(Vec::new(), None)).unwrap();
        validate_publication_state(&site(vec![scheduled(7, "already-out", later(-1))], None))
            .unwrap();
        assert!(validate_publication_state(&site(Vec::new(), Some(later(1)))).is_err());
    }

    #[test]
    fn a_scheduled_piece_in_the_trash_does_not_hold_the_clock() {
        let mut trashed = scheduled(7, "trashed", later(2));
        trashed.current.deleted_at = Some(at());
        let live = scheduled(8, "live", later(5));

        validate_publication_state(&site(vec![trashed.clone(), live], Some(later(5)))).unwrap();
        validate_publication_state(&site(vec![trashed], None)).unwrap();
    }

    #[test]
    fn a_draft_never_sets_the_clock() {
        let mut draft = scheduled(7, "draft", later(2));
        draft.current.publication = Publication::Draft;
        validate_publication_state(&site(vec![draft], None)).unwrap();
    }
}

#[cfg(test)]
mod archive_entry_tests {
    use super::*;
    use chrono::TimeZone as _;

    use crate::domain::{
        content::{ContentKind, Publication},
        theme::Locale,
    };

    fn at() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 9, 2, 12, 0, 0).unwrap()
    }

    fn hex(fill: char) -> String {
        std::iter::repeat_n(fill, 64).collect()
    }

    /// Renders the visitor's own description of what it accepts.
    struct Expectation(StrictJsonVisitor);

    impl std::fmt::Display for Expectation {
        fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            self.0.expecting(formatter)
        }
    }

    fn site() -> PortableSiteV1 {
        PortableSiteV1 {
            format_version: PORTABLE_SITE_FORMAT_VERSION,
            exported_at: at(),
            canonical_origin: "https://writing.example".into(),
            settings: SiteSettings {
                site_title: "Portable site".into(),
                site_description: String::new(),
                locale: Locale::En,
                logo_media_id: None,
                favicon_media_id: None,
                custom_css: String::new(),
                timezone: "UTC".into(),
                author_name: String::new(),
                custom_css_backup: None,
            },
            navigation: Vec::new(),
            contents: Vec::new(),
            redirects: Vec::new(),
            media: Vec::new(),
            engagement: BTreeMap::new(),
            owner: None,
            publication: PortablePublicationState {
                public_revision: 1,
                next_publish_at: None,
            },
            settings_revisions: Vec::new(),
        }
    }

    fn with_cover(media_id: &str) -> PortableContent {
        PortableContent {
            current: Content {
                id: ContentId::from_i64(7),
                kind: ContentKind::Post,
                title: "Portable".into(),
                slug: Slug::parse("portable").unwrap(),
                summary: String::new(),
                body_markdown: "# Canonical".into(),
                body_html: "<h1>Canonical</h1>".into(),
                tags: Vec::new(),
                cover_media_id: Some(media_id.to_owned()),
                seo_title: None,
                seo_description: None,
                publication: Publication::Public { publish_at: at() },
                version: 1,
                created_at: at(),
                updated_at: at(),
                deleted_at: None,
            },
            revisions: Vec::new(),
        }
    }

    #[test]
    fn an_archive_carries_two_documents_and_a_flat_media_directory() {
        assert_eq!(
            validate_archive_path(Path::new(MANIFEST_PATH)).unwrap(),
            MANIFEST_PATH
        );
        assert_eq!(
            validate_archive_path(Path::new(SITE_PATH)).unwrap(),
            SITE_PATH
        );
        assert_eq!(
            validate_archive_path(Path::new("media/cover.png")).unwrap(),
            "media/cover.png"
        );
    }

    #[test]
    fn every_archive_entry_outside_that_shape_is_unsafe() {
        for entry in [
            "/etc/passwd",
            "../escape",
            "./manifest.json",
            "other.json",
            "media",
            "media/",
            "media/nested/cover.png",
            "media/../escape.png",
            "media/.",
            "media/..",
        ] {
            assert!(
                validate_archive_path(Path::new(entry)).is_err(),
                "accepted an archive entry outside the portable shape: {entry}"
            );
        }
        assert!(validate_archive_path(Path::new(&format!("media/{}", "a".repeat(201)))).is_err());
    }

    #[test]
    fn every_referenced_media_identity_has_to_be_in_the_archive() {
        let present = hex('a');
        let absent = hex('b');
        let ids = BTreeSet::from([present.as_str()]);

        let mut referencing = site();
        referencing.contents = vec![with_cover(&present)];
        validate_media_references(&referencing, &ids).unwrap();
        validate_media_references(&site(), &ids).unwrap();

        let mut missing_cover = site();
        missing_cover.contents = vec![with_cover(&absent)];
        assert!(validate_media_references(&missing_cover, &ids).is_err());

        let mut missing_logo = site();
        missing_logo.settings.logo_media_id = Some(absent.clone());
        assert!(validate_media_references(&missing_logo, &ids).is_err());

        let mut missing_favicon = site();
        missing_favicon.settings.favicon_media_id = Some(absent);
        assert!(validate_media_references(&missing_favicon, &ids).is_err());

        let mut malformed = site();
        malformed.settings.logo_media_id = Some("not-a-media-identity".into());
        assert!(validate_media_references(&malformed, &ids).is_err());
    }

    #[test]
    fn an_archive_is_installed_only_where_nothing_stands() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("archive.partial");
        std::fs::write(&source, b"complete archive").unwrap();

        let destination = temp.path().join("site.simple-blog");
        install_without_overwrite(&source, &destination).unwrap();
        assert_eq!(std::fs::read(&destination).unwrap(), b"complete archive");

        let error = install_without_overwrite(&source, &destination).unwrap_err();
        assert!(
            matches!(&error, PortableArchiveError::OutputExists(path) if path == &destination),
            "a destination that already exists must be reported as such, not as {error:?}"
        );
        assert_eq!(std::fs::read(&destination).unwrap(), b"complete archive");

        // Anything else stays the I/O failure it is.
        let into_nowhere = temp.path().join("absent").join("site.simple-blog");
        assert!(matches!(
            install_without_overwrite(&source, &into_nowhere).unwrap_err(),
            PortableArchiveError::Io(_)
        ));
    }

    #[test]
    fn strict_json_names_what_it_expected() {
        // A well-formed document parses.
        strict_json_value(br#"{"a":1,"b":[true,null,"x"]}"#).unwrap();

        // A duplicate field is the thing this visitor exists to refuse.
        let error = strict_json_value(br#"{"a":1,"a":2}"#).unwrap_err();
        assert!(matches!(error, PortableArchiveError::InvalidArchive(_)));
        assert!(error.to_string().contains("duplicate"));

        // JSON carries no value this visitor leaves unhandled, so serde never
        // formats its expectation while parsing. It is still the sentence a
        // reader of any future invalid-type error would get, so it is pinned
        // by rendering it directly.
        assert_eq!(
            Expectation(StrictJsonVisitor).to_string(),
            "JSON without duplicate object fields"
        );
    }
}

#[cfg(test)]
mod archive_reader_tests {
    use super::*;
    use chrono::TimeZone as _;

    use crate::domain::theme::Locale;

    fn at() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 9, 2, 12, 0, 0).unwrap()
    }

    fn site() -> PortableSiteV1 {
        PortableSiteV1 {
            format_version: PORTABLE_SITE_FORMAT_VERSION,
            exported_at: at(),
            canonical_origin: "https://writing.example".into(),
            settings: SiteSettings {
                site_title: "Portable site".into(),
                site_description: String::new(),
                locale: Locale::En,
                logo_media_id: None,
                favicon_media_id: None,
                custom_css: String::new(),
                timezone: "UTC".into(),
                author_name: String::new(),
                custom_css_backup: None,
            },
            navigation: Vec::new(),
            contents: Vec::new(),
            redirects: Vec::new(),
            media: Vec::new(),
            engagement: BTreeMap::new(),
            owner: None,
            publication: PortablePublicationState {
                public_revision: 1,
                next_publish_at: None,
            },
            settings_revisions: Vec::new(),
        }
    }

    /// Writes a tar.zst stream from entries given as name, declared size and
    /// bytes. A declared size that disagrees with the bytes, or data after the
    /// tar terminator, is what a hostile archive looks like and what this
    /// crate would never itself produce.
    fn archive_bytes(entries: &[(&str, u64, Vec<u8>)], trailing: &[u8]) -> Vec<u8> {
        let mut encoder = zstd::Encoder::new(Vec::new(), 1).unwrap();
        {
            let mut builder = Builder::new(&mut encoder);
            for (name, declared, bytes) in entries {
                let mut header = Header::new_gnu();
                header.set_path(name).unwrap();
                header.set_size(*declared);
                header.set_mode(0o644);
                header.set_mtime(0);
                header.set_entry_type(EntryType::Regular);
                header.set_cksum();
                builder.append(&header, bytes.as_slice()).unwrap();
            }
            builder.finish().unwrap();
        }
        encoder.write_all(trailing).unwrap();
        encoder.finish().unwrap()
    }

    /// The two documents of a well-formed archive, carrying an identity that
    /// matches them.
    fn documents() -> Vec<(&'static str, u64, Vec<u8>)> {
        let site_bytes = serde_json::to_vec(&site()).unwrap();
        let entries = BTreeMap::from([(
            SITE_PATH.to_owned(),
            PortableArchiveEntry {
                checksum: blake3::hash(&site_bytes).to_hex().to_string(),
                byte_size: u64::try_from(site_bytes.len()).unwrap(),
            },
        )]);
        let identity = PortableArchiveIdentity {
            archive_format_version: PORTABLE_ARCHIVE_FORMAT_VERSION,
            site_format_version: PORTABLE_SITE_FORMAT_VERSION,
            producer_version: env!("CARGO_PKG_VERSION").to_owned(),
            exported_at: at(),
            entries,
        };
        let archive_id = blake3::hash(&serde_json::to_vec(&identity).unwrap())
            .to_hex()
            .to_string();
        let manifest_bytes = serde_json::to_vec(&PortableArchiveManifest {
            archive_id,
            identity,
        })
        .unwrap();
        vec![
            (
                MANIFEST_PATH,
                u64::try_from(manifest_bytes.len()).unwrap(),
                manifest_bytes,
            ),
            (
                SITE_PATH,
                u64::try_from(site_bytes.len()).unwrap(),
                site_bytes,
            ),
        ]
    }

    fn read_archive(bytes: &[u8]) -> Result<PortablePackage, PortableArchiveError> {
        let temp = tempfile::tempdir()?;
        let path = temp.path().join("site.simple-blog");
        std::fs::write(&path, bytes)?;
        PortableArchive::read(&path)
    }

    #[test]
    fn a_hand_written_archive_of_the_documented_shape_reads_back() {
        let package = read_archive(&archive_bytes(&documents(), b"")).unwrap();
        assert_eq!(package.site.canonical_origin, "https://writing.example");
        assert!(package.media_files.is_empty());
    }

    /// Which size limit applies is decided by the entry's own name: the two
    /// documents are metadata, capped far below a media file. An entry over
    /// its limit is refused from the header alone, before a byte is decoded.
    #[test]
    fn a_document_over_the_metadata_limit_is_refused_before_it_is_decoded() {
        for name in [MANIFEST_PATH, SITE_PATH] {
            let oversized = vec![(name, MAX_METADATA_BYTES + 1, b"{}".to_vec())];
            let error = read_archive(&archive_bytes(&oversized, b"")).unwrap_err();
            assert!(
                matches!(&error, PortableArchiveError::SafetyLimit(message)
                    if message == &format!("archive entry is too large: {name}")),
                "{name} beyond the metadata limit must be refused as too large, not as {error:?}"
            );
        }

        // Exactly at the limit is within it. Such an archive is refused for
        // the contents it turns out not to have, not for its declared size.
        let at_the_limit = vec![(MANIFEST_PATH, MAX_METADATA_BYTES, b"{}".to_vec())];
        let error = read_archive(&archive_bytes(&at_the_limit, b"")).unwrap_err();
        assert!(
            matches!(&error, PortableArchiveError::Io(_)),
            "an entry the size of the limit is within it, not over it: {error:?}"
        );
    }

    /// A tar archive is customarily padded out to a twenty-block boundary.
    /// That much zero padding is part of the format; one byte more is not, and
    /// a byte that is not zero is not padding at all.
    #[test]
    fn trailing_data_is_padding_only_while_it_is_zero_and_within_the_block_factor() {
        // tar ends an archive with two zero blocks, of which the reader
        // consumes one. What is left of it is padding like any other.
        const TERMINATOR_REMAINDER: usize = 512;

        let at_the_boundary = vec![0_u8; MAX_TAR_ZERO_PADDING - TERMINATOR_REMAINDER];
        read_archive(&archive_bytes(&documents(), &at_the_boundary)).unwrap();

        let one_byte_too_far = vec![0_u8; MAX_TAR_ZERO_PADDING - TERMINATOR_REMAINDER + 1];
        let error = read_archive(&archive_bytes(&documents(), &one_byte_too_far)).unwrap_err();
        assert!(
            matches!(&error, PortableArchiveError::InvalidArchive(message)
                if message == "trailing decoded data after tar archive"),
            "padding beyond the block factor must be refused, not read as {error:?}"
        );

        let error = read_archive(&archive_bytes(&documents(), b"appended")).unwrap_err();
        assert!(
            matches!(&error, PortableArchiveError::InvalidArchive(message)
                if message == "trailing decoded data after tar archive"),
            "data smuggled after the tar terminator must be refused, not read as {error:?}"
        );
    }

    fn manifest(producer_version: &str) -> PortableArchiveManifest {
        let identity = PortableArchiveIdentity {
            archive_format_version: PORTABLE_ARCHIVE_FORMAT_VERSION,
            site_format_version: PORTABLE_SITE_FORMAT_VERSION,
            producer_version: producer_version.to_owned(),
            exported_at: at(),
            entries: BTreeMap::new(),
        };
        let archive_id = blake3::hash(&serde_json::to_vec(&identity).unwrap())
            .to_hex()
            .to_string();
        PortableArchiveManifest {
            archive_id,
            identity,
        }
    }

    /// The producer version is written into every archive and read back by
    /// another host, so it is one short printable line and nothing else. Each
    /// case breaks exactly one clause of that sentence.
    #[test]
    fn a_producer_version_is_one_short_printable_line() {
        let empty = BTreeMap::new();
        manifest("0.1.0").verify(&empty).unwrap();
        manifest(&"v".repeat(128)).verify(&empty).unwrap();

        for (label, version) in [
            ("surrounded by whitespace", " 0.1.0 ".to_owned()),
            ("empty", String::new()),
            ("one character too long", "v".repeat(129)),
            ("carrying a control character", "0.1\u{7}.0".to_owned()),
        ] {
            let error = manifest(&version).verify(&empty).unwrap_err();
            assert!(
                matches!(&error, PortableArchiveError::InvalidArchive(message)
                    if message == "invalid archive producer version"),
                "a producer version {label} must be refused as invalid, not as {error:?}"
            );
        }
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

    /// The guard exists to refuse a path that leaves the archive, and it says
    /// so whatever the bytes of that path turn out to be. Its own answer must
    /// not be shadowed by the encoding check that follows it.
    #[test]
    fn an_entry_that_leaves_the_archive_is_refused_as_unsafe_whatever_its_bytes() {
        let escaping = Path::new("..").join(unreadable_name());
        let error = validate_archive_path(&escaping).unwrap_err();
        assert!(
            matches!(&error, PortableArchiveError::UnsafeEntry(entry)
                if entry != "non-UTF-8 entry"),
            "a traversal entry must be named as the unsafe path it is, not as {error:?}"
        );
    }
}

#[cfg(test)]
mod partial_cleanup_tests {
    use super::*;

    /// A partial archive left behind by a failed write is named to the
    /// operator, and one that is simply gone is not worth a word.
    #[test]
    fn a_partial_archive_that_cannot_be_removed_is_named_and_an_absent_one_is_not() {
        let temp = tempfile::tempdir().unwrap();
        let (traces, _guard) = crate::observability::capture::traces();

        cleanup_failed_archive_path(
            &temp.path().join("absent"),
            "portable.archive.partial_cleanup_failed",
        );
        assert_eq!(
            traces.text(),
            "",
            "a partial archive that is already gone is the cleanup having succeeded"
        );

        // A directory standing where the partial archive belongs cannot be
        // removed as a file, and the operator has to hear about it.
        let occupied = temp.path().join("occupied");
        std::fs::create_dir(&occupied).unwrap();
        cleanup_failed_archive_path(&occupied, "portable.archive.partial_cleanup_failed");

        let reported = traces.text();
        assert!(
            reported.contains("portable.archive.partial_cleanup_failed")
                && reported.contains("occupied"),
            "a partial archive that outlived its write must be named: {reported:?}"
        );
    }
}

#[cfg(test)]
mod manifest_entry_tests {
    use super::*;
    use chrono::TimeZone as _;

    fn at() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 9, 2, 12, 0, 0).unwrap()
    }

    fn manifest(checksum: String, byte_size: u64) -> PortableArchiveManifest {
        let identity = PortableArchiveIdentity {
            archive_format_version: PORTABLE_ARCHIVE_FORMAT_VERSION,
            site_format_version: PORTABLE_SITE_FORMAT_VERSION,
            producer_version: env!("CARGO_PKG_VERSION").to_owned(),
            exported_at: at(),
            entries: BTreeMap::from([(
                SITE_PATH.to_owned(),
                PortableArchiveEntry {
                    checksum,
                    byte_size,
                },
            )]),
        };
        let archive_id = blake3::hash(&serde_json::to_vec(&identity).unwrap())
            .to_hex()
            .to_string();
        PortableArchiveManifest {
            archive_id,
            identity,
        }
    }

    /// The record carries both the length and the checksum of every entry,
    /// and an entry has to answer to each of them on its own.
    #[test]
    fn an_entry_answers_to_its_recorded_length_and_to_its_recorded_checksum() {
        let files = BTreeMap::from([(SITE_PATH.to_owned(), b"site".to_vec())]);
        let checksum = blake3::hash(b"site").to_hex().to_string();
        manifest(checksum.clone(), 4).verify(&files).unwrap();

        // The same number of bytes, and not the same bytes.
        let tampered = BTreeMap::from([(SITE_PATH.to_owned(), b"SITE".to_vec())]);
        let error = manifest(checksum.clone(), 4).verify(&tampered).unwrap_err();
        assert!(
            matches!(&error, PortableArchiveError::InvalidArchive(message)
                if message == &format!("checksum or size mismatch: {SITE_PATH}")),
            "bytes that changed without changing length must be refused: {error:?}"
        );

        // The recorded bytes, and not the number of them the record claims.
        let error = manifest(checksum, 5).verify(&files).unwrap_err();
        assert!(
            matches!(&error, PortableArchiveError::InvalidArchive(message)
                if message == &format!("checksum or size mismatch: {SITE_PATH}")),
            "an entry of another length must be refused: {error:?}"
        );
    }
}
