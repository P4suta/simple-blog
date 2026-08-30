use std::fmt;

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// A raw bearer capability. It deliberately has no `Debug` implementation.
#[derive(Clone, Eq, PartialEq)]
pub struct SecretToken(String);

impl SecretToken {
    #[must_use]
    pub const fn new(value: String) -> Self {
        Self(value)
    }

    #[must_use]
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for SecretToken {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("[REDACTED]")
    }
}

#[derive(Clone, Copy, Eq, Hash, PartialEq)]
pub struct SecretHash([u8; 32]);

impl SecretHash {
    #[must_use]
    pub const fn new(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl fmt::Debug for SecretHash {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SecretHash([REDACTED])")
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SetupPurpose {
    Initial,
    Recovery,
}

impl SetupPurpose {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Initial => "setup",
            Self::Recovery => "recover",
        }
    }

    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "setup" => Some(Self::Initial),
            "recover" => Some(Self::Recovery),
            _ => None,
        }
    }
}

#[derive(Clone)]
pub struct SessionSecrets {
    pub session: SecretToken,
    pub csrf: SecretToken,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SessionRecord {
    pub token_hash: SecretHash,
    pub csrf_hash: SecretHash,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub last_seen_at: DateTime<Utc>,
    pub reauthenticated_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SessionIdentity {
    pub token_hash: SecretHash,
    pub csrf_hash: SecretHash,
    pub expires_at: DateTime<Utc>,
    pub reauthenticated_at: DateTime<Utc>,
}

impl From<SessionRecord> for SessionIdentity {
    fn from(record: SessionRecord) -> Self {
        Self {
            token_hash: record.token_hash,
            csrf_hash: record.csrf_hash,
            expires_at: record.expires_at,
            reauthenticated_at: record.reauthenticated_at,
        }
    }
}

impl SessionIdentity {
    #[must_use]
    pub fn was_reauthenticated_within(&self, now: DateTime<Utc>, maximum_age: Duration) -> bool {
        now >= self.reauthenticated_at && now - self.reauthenticated_at <= maximum_age
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct StoredPasskey {
    pub credential_id: Vec<u8>,
    pub name: String,
    pub passkey_json: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Owner {
    pub user_handle: Uuid,
}
