//! Preview links: a writer hands a reader one unpublished piece for a while.
//!
//! The link is a 256-bit bearer capability. Only its hash is stored, it
//! lives seven days, every link of a piece can be revoked at once, and the
//! rows vanish with the piece. Like sessions, links are ephemeral state and
//! never travel in a `.simple-blog` archive.

use std::sync::Arc;

use chrono::{DateTime, Duration, Utc};

use crate::{
    application::{
        auth::{hash_secret, random_token},
        ports::{AuthError, EntropySource, PreviewLinkRepository},
    },
    domain::{
        auth::{PreviewLinkRecord, SecretToken},
        content::ContentId,
    },
};

pub const PREVIEW_LINK_TTL_DAYS: i64 = 7;
const TOKEN_BYTES: usize = 32;
/// 32 random bytes as unpadded URL-safe base64.
const TOKEN_LENGTH: usize = 43;

pub struct IssuedPreviewLink {
    pub token: SecretToken,
    pub expires_at: DateTime<Utc>,
}

#[derive(Clone)]
pub struct PreviewLinkService {
    repository: Arc<dyn PreviewLinkRepository>,
    entropy: Arc<dyn EntropySource>,
}

impl PreviewLinkService {
    pub fn new(
        repository: Arc<dyn PreviewLinkRepository>,
        entropy: Arc<dyn EntropySource>,
    ) -> Self {
        Self {
            repository,
            entropy,
        }
    }

    #[tracing::instrument(name = "preview_link.issue", skip(self), fields(content_id = %content_id))]
    pub async fn issue(
        &self,
        content_id: ContentId,
        now: DateTime<Utc>,
    ) -> Result<IssuedPreviewLink, AuthError> {
        let token = random_token(self.entropy.as_ref(), TOKEN_BYTES)?;
        let expires_at = now + Duration::days(PREVIEW_LINK_TTL_DAYS);
        self.repository
            .store_preview_link(&PreviewLinkRecord {
                token_hash: hash_secret(token.expose()),
                content_id,
                created_at: now,
                expires_at,
            })
            .await?;
        Ok(IssuedPreviewLink { token, expires_at })
    }

    /// The piece a presented token opens. Malformed tokens are answered
    /// without touching storage.
    pub async fn resolve(
        &self,
        token: &str,
        now: DateTime<Utc>,
    ) -> Result<Option<ContentId>, AuthError> {
        if !well_formed(token) {
            return Ok(None);
        }
        self.repository
            .find_preview_link(hash_secret(token), now)
            .await
    }

    #[tracing::instrument(name = "preview_link.revoke", skip(self), fields(content_id = %content_id))]
    pub async fn revoke(&self, content_id: ContentId) -> Result<u64, AuthError> {
        self.repository.revoke_preview_links(content_id).await
    }
}

fn well_formed(token: &str) -> bool {
    token.len() == TOKEN_LENGTH
        && token
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_unpadded_url_safe_tokens_of_the_right_length_are_looked_up() {
        assert!(well_formed(&"a".repeat(43)));
        assert!(well_formed(&format!("{}-_", "b".repeat(41))));
        assert!(!well_formed(&"a".repeat(42)));
        assert!(!well_formed(&"a".repeat(44)));
        assert!(!well_formed(&format!("{}+", "a".repeat(42))));
        assert!(!well_formed(&format!("{}/", "a".repeat(42))));
        assert!(!well_formed(""));
    }
}
