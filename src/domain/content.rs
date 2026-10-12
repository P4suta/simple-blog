//! Pieces of writing and their history: identities, addresses, tags,
//! publication, revisions, and the trash, as pure rules with no storage
//! attached.

use std::{fmt, str::FromStr};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;

const RESERVED_SLUGS: &[&str] = &[
    "admin",
    "archive",
    "feed.json",
    "feed.xml",
    "healthz",
    "media",
    "page",
    "robots.txt",
    "search",
    "sitemap.xml",
    "tag",
];

/// A globally unique, canonical URL segment.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Slug(String);

#[derive(Clone, Debug, Error, Eq, PartialEq)]
#[error(
    "slug must be 1-120 lowercase ASCII characters, using only letters, digits, and interior hyphens"
)]
pub struct InvalidSlug;

impl Slug {
    pub fn parse(value: impl AsRef<str>) -> Result<Self, InvalidSlug> {
        let value = value.as_ref();
        let valid_shape = !value.is_empty()
            && value.len() <= 120
            && value.is_ascii()
            && value
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
            && value
                .as_bytes()
                .first()
                .is_some_and(u8::is_ascii_alphanumeric)
            && value
                .as_bytes()
                .last()
                .is_some_and(u8::is_ascii_alphanumeric)
            && !value.contains("--");

        if valid_shape && !RESERVED_SLUGS.contains(&value) {
            Ok(Self(value.to_owned()))
        } else {
            Err(InvalidSlug)
        }
    }

    /// Default slug for new content: minute-resolution timestamp, e.g. `20260831-2145`.
    ///
    /// Always digit-leading, so it can never collide with `RESERVED_SLUGS`.
    #[must_use]
    pub fn timestamped(now: DateTime<Utc>) -> Self {
        Self(now.format("%Y%m%d-%H%M").to_string())
    }

    /// Second-resolution variant used to resolve same-minute collisions.
    #[must_use]
    pub fn timestamped_precise(now: DateTime<Utc>) -> Self {
        Self(now.format("%Y%m%d-%H%M%S").to_string())
    }

    /// The address a title suggests: transliterated and hyphenated when the
    /// script allows it, a reserved word given a suffix, and the clock for
    /// the rest. CJK titles take the clock deliberately: the transliterator
    /// would read 日本語 with Chinese readings and produce an address nobody
    /// recognises, and a date is at least honest.
    #[must_use]
    pub fn from_title(title: &str, now: DateTime<Utc>) -> Self {
        if title.chars().any(is_cjk) {
            return Self::timestamped(now);
        }
        let mut candidate = slug::slugify(title);
        candidate.truncate(120);
        let candidate = candidate.trim_matches('-');
        if candidate.is_empty() {
            return Self::timestamped(now);
        }
        Self::parse(candidate)
            .or_else(|_| Self::parse(format!("{candidate}-post")))
            .unwrap_or_else(|_| Self::timestamped(now))
    }

    /// `{slug}-{n}` for resolving a collision, trimmed to fit the limit.
    #[must_use]
    pub fn numbered(&self, n: u32) -> Self {
        let suffix = format!("-{n}");
        let room = 120_usize.saturating_sub(suffix.len());
        let mut base = self.0.clone();
        base.truncate(room);
        let base = base.trim_end_matches('-');
        Self::parse(format!("{base}{suffix}")).unwrap_or_else(|_| self.clone())
    }

    /// Whether this slug has the shape produced by [`Slug::timestamped`] or
    /// [`Slug::timestamped_precise`].
    #[must_use]
    pub fn is_timestamped(&self) -> bool {
        let bytes = self.0.as_bytes();
        (bytes.len() == 13 || bytes.len() == 15)
            && bytes[8] == b'-'
            && bytes
                .iter()
                .enumerate()
                .all(|(index, byte)| index == 8 || byte.is_ascii_digit())
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Han, kana and hangul: scripts a Latin transliteration misrepresents.
const fn is_cjk(character: char) -> bool {
    matches!(character,
        '\u{3040}'..='\u{30FF}'
        | '\u{3400}'..='\u{4DBF}'
        | '\u{4E00}'..='\u{9FFF}'
        | '\u{AC00}'..='\u{D7AF}'
        | '\u{F900}'..='\u{FAFF}'
        | '\u{FF66}'..='\u{FF9F}')
}

impl fmt::Display for Slug {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl FromStr for Slug {
    type Err = InvalidSlug;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value)
    }
}

impl From<Slug> for String {
    fn from(value: Slug) -> Self {
        value.0
    }
}

impl TryFrom<String> for Slug {
    type Error = InvalidSlug;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(value)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContentKind {
    Post,
    Page,
}

impl ContentKind {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Post => "post",
            Self::Page => "page",
        }
    }
}

impl fmt::Display for ContentKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for ContentKind {
    type Err = &'static str;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "post" => Ok(Self::Post),
            "page" => Ok(Self::Page),
            _ => Err("unknown content kind"),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Publication {
    Draft,
    Public { publish_at: DateTime<Utc> },
}

impl Publication {
    #[must_use]
    pub fn is_visible_at(&self, now: DateTime<Utc>) -> bool {
        matches!(self, Self::Public { publish_at } if *publish_at <= now)
    }

    #[must_use]
    pub const fn publish_at(&self) -> Option<DateTime<Utc>> {
        match self {
            Self::Draft => None,
            Self::Public { publish_at } => Some(*publish_at),
        }
    }

    /// Public, but not yet visible: the entry is waiting for its boundary.
    #[must_use]
    pub fn is_scheduled_at(&self, now: DateTime<Utc>) -> bool {
        matches!(self, Self::Public { publish_at } if *publish_at > now)
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct ContentId(i64);

impl ContentId {
    #[must_use]
    pub const fn from_i64(value: i64) -> Self {
        Self(value)
    }

    #[must_use]
    pub const fn as_i64(self) -> i64 {
        self.0
    }
}

impl fmt::Display for ContentId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SaveIntent {
    Autosave,
    Explicit,
}

impl SaveIntent {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Autosave => "autosave",
            Self::Explicit => "explicit",
        }
    }
}

impl FromStr for SaveIntent {
    type Err = &'static str;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "autosave" => Ok(Self::Autosave),
            "explicit" => Ok(Self::Explicit),
            _ => Err("unknown save intent"),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Tag {
    pub name: String,
    pub slug: Slug,
}

/// Editable fields shared by posts and pages.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContentDraft {
    pub kind: ContentKind,
    pub title: String,
    pub slug: Slug,
    pub summary: String,
    pub body_markdown: String,
    pub tags: Vec<String>,
    pub cover_media_id: Option<String>,
    pub seo_title: Option<String>,
    pub seo_description: Option<String>,
    pub publication: Publication,
}

/// Stored content. Markdown remains canonical; HTML is a safe derived value.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Content {
    pub id: ContentId,
    pub kind: ContentKind,
    pub title: String,
    pub slug: Slug,
    pub summary: String,
    pub body_markdown: String,
    pub body_html: String,
    pub tags: Vec<Tag>,
    pub cover_media_id: Option<String>,
    pub seo_title: Option<String>,
    pub seo_description: Option<String>,
    pub publication: Publication,
    pub version: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    /// Set while the piece sits in the trash (ADR 0014). Omitted from the
    /// serialized form when empty, so revision snapshots and portable
    /// archives without trashed content stay byte-identical to older ones.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deleted_at: Option<DateTime<Utc>>,
}

impl Content {
    #[must_use]
    pub const fn is_trashed(&self) -> bool {
        self.deleted_at.is_some()
    }

    #[must_use]
    pub fn to_draft(&self) -> ContentDraft {
        ContentDraft {
            kind: self.kind,
            title: self.title.clone(),
            slug: self.slug.clone(),
            summary: self.summary.clone(),
            body_markdown: self.body_markdown.clone(),
            tags: self.tags.iter().map(|tag| tag.name.clone()).collect(),
            cover_media_id: self.cover_media_id.clone(),
            seo_title: self.seo_title.clone(),
            seo_description: self.seo_description.clone(),
            publication: self.publication.clone(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContentRevision {
    pub id: i64,
    pub content_id: ContentId,
    pub intent: SaveIntent,
    pub snapshot: Content,
    pub created_at: DateTime<Utc>,
}
