use std::{
    collections::{HashMap, VecDeque},
    sync::{Arc, Mutex},
};

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{DateTime, Duration, Utc};
use rand::{RngCore, rngs::OsRng};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use uuid::Uuid;

use crate::{
    application::ports::{AuthError, AuthRepository, PasskeyRepository, SetupRegistration},
    domain::auth::{
        SecretHash, SecretToken, SessionIdentity, SessionRecord, SessionSecrets, SetupPurpose,
        StoredPasskey,
    },
};

const SETUP_TOKEN_TTL_MINUTES: i64 = 15;
const SESSION_TTL_DAYS: i64 = 7;
const RECOVERY_CODE_COUNT: usize = 10;

#[derive(Clone)]
pub struct AuthRateLimiter {
    maximum: usize,
    window: Duration,
    attempts: Arc<Mutex<HashMap<String, VecDeque<DateTime<Utc>>>>>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RateLimitDecision {
    Allowed,
    Limited { retry_after: u64 },
}

impl AuthRateLimiter {
    #[must_use]
    pub fn new(maximum: usize, window: Duration) -> Self {
        assert!(maximum > 0, "rate limit maximum must be positive");
        assert!(
            window > Duration::zero(),
            "rate limit window must be positive"
        );
        Self {
            maximum,
            window,
            attempts: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    #[must_use]
    pub fn authentication_default() -> Self {
        Self::new(10, Duration::minutes(1))
    }

    #[must_use]
    pub fn check(&self, key: &str, now: DateTime<Utc>) -> RateLimitDecision {
        let cutoff = now - self.window;
        let mut state = self.attempts.lock().unwrap_or_else(|poisoned| {
            tracing::error!(event = "auth.rate_limiter.poison_recovered");
            poisoned.into_inner()
        });
        state.retain(|_, attempts| {
            attempts.retain(|attempt| *attempt > cutoff);
            !attempts.is_empty()
        });
        let attempts = state.entry(key.to_owned()).or_default();
        let decision = if attempts.len() >= self.maximum {
            let retry_after = attempts
                .front()
                .map_or(1, |oldest| {
                    let milliseconds = (*oldest + self.window - now).num_milliseconds();
                    milliseconds.saturating_add(999) / 1_000
                })
                .max(1);
            RateLimitDecision::Limited {
                retry_after: u64::try_from(retry_after).unwrap_or(1),
            }
        } else {
            attempts.push_back(now);
            RateLimitDecision::Allowed
        };
        drop(state);
        decision
    }
}

#[derive(Clone)]
pub struct AuthService {
    repository: Arc<dyn AuthRepository>,
}

pub struct CompletedSetup {
    pub session: SessionSecrets,
    pub recovery_codes: Vec<SecretToken>,
}

#[derive(Clone)]
pub struct PasskeyAccountService {
    repository: Arc<dyn PasskeyRepository>,
}

pub struct RegistrationContext {
    pub purpose: SetupPurpose,
    pub user_handle: Uuid,
    pub excluded_credentials: Vec<Vec<u8>>,
}

pub struct PasskeyRegistrationContext {
    pub user_handle: Uuid,
    pub excluded_credentials: Vec<Vec<u8>>,
}

impl PasskeyAccountService {
    pub fn new(repository: Arc<dyn PasskeyRepository>) -> Self {
        Self { repository }
    }

    pub async fn complete_setup_registration(
        &self,
        setup_token: &str,
        purpose: SetupPurpose,
        user_handle: Uuid,
        passkey: StoredPasskey,
        now: DateTime<Utc>,
    ) -> Result<Option<CompletedSetup>, AuthError> {
        let (session, session_record) = new_session(now);
        let recovery_codes: Vec<_> = (0..RECOVERY_CODE_COUNT).map(|_| random_token(20)).collect();
        let recovery_hashes: Vec<_> = recovery_codes
            .iter()
            .map(|code| hash_secret(code.expose()))
            .collect();
        let committed = self
            .repository
            .complete_setup_registration(SetupRegistration {
                setup_token_hash: hash_secret(setup_token),
                purpose,
                user_handle,
                passkey: &passkey,
                session: &session_record,
                recovery_code_hashes: &recovery_hashes,
                now,
            })
            .await?;
        Ok(committed.then_some(CompletedSetup {
            session,
            recovery_codes,
        }))
    }

    pub async fn setup_context(
        &self,
        setup_token: &str,
        now: DateTime<Utc>,
    ) -> Result<Option<RegistrationContext>, AuthError> {
        let Some(purpose) = self
            .repository
            .setup_token_purpose(hash_secret(setup_token), now)
            .await?
        else {
            return Ok(None);
        };
        let owner = self.repository.owner_handle().await?;
        let passkeys = self.repository.list_passkeys().await?;
        let user_handle = match (purpose, owner) {
            (SetupPurpose::Initial, None) => Uuid::new_v4(),
            (SetupPurpose::Recovery, Some(owner)) => owner,
            _ => return Ok(None),
        };
        Ok(Some(RegistrationContext {
            purpose,
            user_handle,
            excluded_credentials: passkeys
                .into_iter()
                .map(|passkey| passkey.credential_id)
                .collect(),
        }))
    }

    pub async fn passkeys(&self) -> Result<Vec<StoredPasskey>, AuthError> {
        self.repository.list_passkeys().await
    }

    pub async fn owner_registration_context(
        &self,
    ) -> Result<Option<PasskeyRegistrationContext>, AuthError> {
        let Some(user_handle) = self.repository.owner_handle().await? else {
            return Ok(None);
        };
        let excluded_credentials = self
            .repository
            .list_passkeys()
            .await?
            .into_iter()
            .map(|passkey| passkey.credential_id)
            .collect();
        Ok(Some(PasskeyRegistrationContext {
            user_handle,
            excluded_credentials,
        }))
    }

    pub async fn complete_authentication(
        &self,
        credential_id: &[u8],
        passkey_json: &str,
        now: DateTime<Utc>,
    ) -> Result<Option<SessionSecrets>, AuthError> {
        let (session, record) = new_session(now);
        let committed = self
            .repository
            .complete_authentication(credential_id, passkey_json, &record, now)
            .await?;
        Ok(committed.then_some(session))
    }

    pub async fn add_passkey(
        &self,
        passkey: &StoredPasskey,
        now: DateTime<Utc>,
    ) -> Result<(), AuthError> {
        self.repository.add_passkey(passkey, now).await
    }

    pub async fn remove_passkey(&self, credential_id: &[u8]) -> Result<bool, AuthError> {
        self.repository.remove_passkey(credential_id).await
    }
}

impl AuthService {
    pub fn new(repository: Arc<dyn AuthRepository>) -> Self {
        Self { repository }
    }

    pub async fn issue_setup_token(
        &self,
        purpose: SetupPurpose,
        now: DateTime<Utc>,
    ) -> Result<SecretToken, AuthError> {
        let token = random_token(32);
        self.repository
            .store_setup_token(
                hash_secret(token.expose()),
                purpose,
                now + Duration::minutes(SETUP_TOKEN_TTL_MINUTES),
            )
            .await?;
        Ok(token)
    }

    pub async fn consume_setup_token(
        &self,
        token: &str,
        purpose: SetupPurpose,
        now: DateTime<Utc>,
    ) -> Result<bool, AuthError> {
        self.repository
            .consume_setup_token(hash_secret(token), purpose, now)
            .await
    }

    pub async fn create_session(&self, now: DateTime<Utc>) -> Result<SessionSecrets, AuthError> {
        let (secrets, record) = new_session(now);
        self.repository.store_session(&record).await?;
        Ok(secrets)
    }

    pub async fn authenticate(
        &self,
        session_token: &str,
        now: DateTime<Utc>,
    ) -> Result<Option<SessionIdentity>, AuthError> {
        self.repository
            .find_session(hash_secret(session_token), now)
            .await
            .map(|record| record.map(SessionIdentity::from))
    }

    #[must_use]
    pub fn verify_csrf(&self, identity: &SessionIdentity, presented: &str) -> bool {
        hash_secret(presented)
            .as_bytes()
            .ct_eq(identity.csrf_hash.as_bytes())
            .into()
    }

    pub async fn rotate_session(
        &self,
        current_token: &str,
        now: DateTime<Utc>,
    ) -> Result<Option<SessionSecrets>, AuthError> {
        let (secrets, replacement) = new_session(now);
        let rotated = self
            .repository
            .rotate_session(hash_secret(current_token), &replacement, now)
            .await?;
        Ok(rotated.then_some(secrets))
    }

    pub async fn replace_recovery_codes(
        &self,
        now: DateTime<Utc>,
    ) -> Result<Vec<SecretToken>, AuthError> {
        let codes: Vec<_> = (0..RECOVERY_CODE_COUNT).map(|_| random_token(20)).collect();
        let hashes: Vec<_> = codes
            .iter()
            .map(|code| hash_secret(code.expose()))
            .collect();
        self.repository.replace_recovery_codes(&hashes, now).await?;
        Ok(codes)
    }

    pub async fn consume_recovery_code(
        &self,
        code: &str,
        now: DateTime<Utc>,
    ) -> Result<bool, AuthError> {
        self.repository
            .consume_recovery_code(hash_secret(code), now)
            .await
    }

    #[tracing::instrument(name = "auth.recover_session", skip_all)]
    pub async fn recover_session(
        &self,
        code: &str,
        now: DateTime<Utc>,
    ) -> Result<Option<SessionSecrets>, AuthError> {
        let (secrets, session) = new_session(now);
        let exchanged = self
            .repository
            .exchange_recovery_code(hash_secret(code), &session, now)
            .await?;
        Ok(exchanged.then_some(secrets))
    }
}

fn new_session(now: DateTime<Utc>) -> (SessionSecrets, SessionRecord) {
    let session = random_token(32);
    let csrf = random_token(32);
    let record = SessionRecord {
        token_hash: hash_secret(session.expose()),
        csrf_hash: hash_secret(csrf.expose()),
        created_at: now,
        expires_at: now + Duration::days(SESSION_TTL_DAYS),
        last_seen_at: now,
        reauthenticated_at: now,
    };
    (SessionSecrets { session, csrf }, record)
}

fn random_token(byte_count: usize) -> SecretToken {
    let mut bytes = vec![0_u8; byte_count];
    OsRng.fill_bytes(&mut bytes);
    SecretToken::new(URL_SAFE_NO_PAD.encode(bytes))
}

#[must_use]
pub fn hash_secret(value: &str) -> SecretHash {
    SecretHash::new(Sha256::digest(value.as_bytes()).into())
}
