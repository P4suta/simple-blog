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
