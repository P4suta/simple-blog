use std::fmt;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct MediaId(String);

#[derive(Clone, Debug, Error, Eq, PartialEq)]
#[error("media id must be a 64-character lowercase hexadecimal digest")]
pub struct InvalidMediaId;

impl MediaId {
    pub fn parse(value: impl AsRef<str>) -> Result<Self, InvalidMediaId> {
        let value = value.as_ref();
        if value.len() == 64
            && value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            Ok(Self(value.to_owned()))
        } else {
            Err(InvalidMediaId)
        }
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// The media identity a public `/media/…` path names, when it names one
/// (a complete lowercase digest right after the prefix).
#[must_use]
pub fn media_id_from_path(path: &str) -> Option<MediaId> {
    let rest = path.strip_prefix("/media/")?;
    MediaId::parse(rest.get(..64)?).ok()
}

/// The media type a public `/media/<filename>` is served as.
///
/// Decided from the name alone so a reader's request never asks the
/// database. Only the two shapes the media store writes are recognised:
/// `<digest>.<extension>` for an original and `<digest>-<width>w.webp` for a
/// responsive variant.
#[must_use]
pub fn mime_for_media_filename(filename: &str) -> Option<&'static str> {
    let (id, rest) = filename.split_at_checked(64)?;
    MediaId::parse(id).ok()?;
    if let Some(extension) = rest.strip_prefix('.') {
        return match extension {
            "webp" => Some("image/webp"),
            "png" => Some("image/png"),
            "jpeg" | "jpg" => Some("image/jpeg"),
            "gif" => Some("image/gif"),
            _ => None,
        };
    }
    let width = rest.strip_prefix('-')?.strip_suffix("w.webp")?;
    (!width.is_empty() && width.bytes().all(|byte| byte.is_ascii_digit())).then_some("image/webp")
}

impl fmt::Display for MediaId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl From<MediaId> for String {
    fn from(value: MediaId) -> Self {
        value.0
    }
}

impl TryFrom<String> for MediaId {
    type Error = InvalidMediaId;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(value)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MediaVariant {
    pub width: u32,
    pub height: u32,
    pub byte_size: u64,
    pub filename: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MediaAsset {
    pub id: MediaId,
    pub original_name: String,
    pub original_filename: String,
    pub mime_type: String,
    pub extension: String,
    pub width: u32,
    pub height: u32,
    pub byte_size: u64,
    pub alt_text: String,
    pub caption: String,
    pub animated: bool,
    pub variants: Vec<MediaVariant>,
    pub created_at: DateTime<Utc>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn media_id_from_path_accepts_only_media_urls_with_a_full_lowercase_id() {
        let id = "b".repeat(64);
        assert_eq!(
            media_id_from_path(&format!("/media/{id}.webp"))
                .unwrap()
                .as_str(),
            id
        );
        assert!(media_id_from_path(&format!("/media/{id}-480w.webp")).is_some());
        assert!(media_id_from_path(&format!("/images/{id}.webp")).is_none());
        assert!(media_id_from_path(&format!("/media/{}", "B".repeat(64))).is_none());
        assert!(media_id_from_path(&format!("/media/{}", "c".repeat(63))).is_none());
        assert!(media_id_from_path("/media/é").is_none());
    }

    #[test]
    fn media_filenames_carry_their_own_media_type() {
        let id = "a".repeat(64);
        assert_eq!(
            mime_for_media_filename(&format!("{id}.webp")),
            Some("image/webp")
        );
        assert_eq!(
            mime_for_media_filename(&format!("{id}.png")),
            Some("image/png")
        );
        assert_eq!(
            mime_for_media_filename(&format!("{id}.jpg")),
            Some("image/jpeg")
        );
        assert_eq!(
            mime_for_media_filename(&format!("{id}-480w.webp")),
            Some("image/webp")
        );
        for rejected in [
            format!("{id}.svg"),
            format!("{id}-w.webp"),
            format!("{id}-480w.png"),
            format!("{id}-480x.webp"),
            format!("{}.webp", "a".repeat(63)),
            format!("{}.webp", "A".repeat(64)),
            format!("{}é.webp", "a".repeat(63)),
            ".upload-abc.tmp".into(),
            id,
        ] {
            assert_eq!(mime_for_media_filename(&rejected), None, "{rejected}");
        }
    }
}
