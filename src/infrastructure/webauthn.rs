use chrono::{DateTime, Duration as ChronoDuration, Utc};
use dashmap::DashMap;
use thiserror::Error;
use url::Url;
use uuid::Uuid;
use webauthn_rs::{Webauthn, WebauthnBuilder, prelude::*};

const CHALLENGE_TTL_MINUTES: i64 = 5;

pub struct PasskeyCeremony {
    webauthn: Webauthn,
    challenges: DashMap<Uuid, Challenge>,
}

pub struct RegistrationStart {
    pub flow_id: Uuid,
    pub public: CreationChallengeResponse,
}

pub struct AuthenticationStart {
    pub flow_id: Uuid,
    pub public: RequestChallengeResponse,
}

pub struct RegisteredPasskey {
    pub user_handle: Uuid,
    pub credential_id: Vec<u8>,
    pub passkey_json: String,
}

pub struct AuthenticatedPasskey {
    pub credential_id: Vec<u8>,
    pub passkey_json: String,
}

enum Challenge {
    Registration {
        user_handle: Uuid,
        state: PasskeyRegistration,
        expires_at: DateTime<Utc>,
    },
    Authentication {
        state: PasskeyAuthentication,
        passkeys: Vec<Passkey>,
        expires_at: DateTime<Utc>,
    },
}

#[derive(Debug, Error)]
pub enum PasskeyError {
    #[error("WebAuthn requires HTTPS outside localhost")]
    InsecureOrigin,
    #[error("public URL must include a domain")]
    MissingDomain,
    #[error("invalid WebAuthn configuration or response: {0}")]
    Webauthn(String),
    #[error("challenge is missing, expired, or has already been consumed")]
    InvalidChallenge,
    #[error("no passkey is registered")]
    NoPasskeys,
    #[error("could not encode or decode a passkey: {0}")]
    Serialization(String),
}

impl PasskeyCeremony {
    pub fn new(public_url: &Url, relying_party_name: &str) -> Result<Self, PasskeyError> {
        let host = public_url.host_str().ok_or(PasskeyError::MissingDomain)?;
        let local = host == "localhost" || host.ends_with(".localhost");
        if public_url.scheme() != "https" && !local {
            return Err(PasskeyError::InsecureOrigin);
        }
        let webauthn = WebauthnBuilder::new(host, public_url)
            .map_err(passkey_error)?
            .rp_name(relying_party_name)
            .build()
            .map_err(passkey_error)?;
        Ok(Self {
            webauthn,
            challenges: DashMap::new(),
        })
    }

    pub fn start_registration(
        &self,
        user_handle: Uuid,
        excluded_credentials: &[Vec<u8>],
        now: DateTime<Utc>,
    ) -> Result<RegistrationStart, PasskeyError> {
        self.remove_expired(now);
        let excluded = (!excluded_credentials.is_empty()).then(|| {
            excluded_credentials
                .iter()
                .map(|bytes| CredentialID::from(bytes.as_slice()))
                .collect()
        });
        let (public, state) = self
            .webauthn
            .start_passkey_registration(user_handle, "owner", "Owner", excluded)
            .map_err(passkey_error)?;
        let flow_id = Uuid::new_v4();
        self.challenges.insert(
            flow_id,
            Challenge::Registration {
                user_handle,
                state,
                expires_at: now + ChronoDuration::minutes(CHALLENGE_TTL_MINUTES),
            },
        );
        Ok(RegistrationStart { flow_id, public })
    }

    pub fn finish_registration(
        &self,
        flow_id: Uuid,
        credential: &RegisterPublicKeyCredential,
        now: DateTime<Utc>,
    ) -> Result<RegisteredPasskey, PasskeyError> {
        let Some((_, challenge)) = self.challenges.remove(&flow_id) else {
            return Err(PasskeyError::InvalidChallenge);
        };
        let Challenge::Registration {
            user_handle,
            state,
            expires_at,
        } = challenge
        else {
            return Err(PasskeyError::InvalidChallenge);
        };
        if expires_at < now {
            return Err(PasskeyError::InvalidChallenge);
        }
        let passkey = self
            .webauthn
            .finish_passkey_registration(credential, &state)
            .map_err(passkey_error)?;
        let credential_id = passkey.cred_id().as_ref().to_vec();
        let passkey_json = serde_json::to_string(&passkey)
            .map_err(|error| PasskeyError::Serialization(error.to_string()))?;
        Ok(RegisteredPasskey {
            user_handle,
            credential_id,
            passkey_json,
        })
    }

    pub fn start_authentication(
        &self,
        passkey_json: &[String],
        now: DateTime<Utc>,
    ) -> Result<AuthenticationStart, PasskeyError> {
        self.remove_expired(now);
        let passkeys: Vec<Passkey> = passkey_json
            .iter()
            .map(|json| {
                serde_json::from_str(json)
                    .map_err(|error| PasskeyError::Serialization(error.to_string()))
            })
            .collect::<Result<_, _>>()?;
        if passkeys.is_empty() {
            return Err(PasskeyError::NoPasskeys);
        }
        let (public, state) = self
            .webauthn
            .start_passkey_authentication(&passkeys)
            .map_err(passkey_error)?;
        let flow_id = Uuid::new_v4();
        self.challenges.insert(
            flow_id,
            Challenge::Authentication {
                state,
                passkeys,
                expires_at: now + ChronoDuration::minutes(CHALLENGE_TTL_MINUTES),
            },
        );
        Ok(AuthenticationStart { flow_id, public })
    }

    pub fn finish_authentication(
        &self,
        flow_id: Uuid,
        credential: &PublicKeyCredential,
        now: DateTime<Utc>,
    ) -> Result<AuthenticatedPasskey, PasskeyError> {
        let Some((_, challenge)) = self.challenges.remove(&flow_id) else {
            return Err(PasskeyError::InvalidChallenge);
        };
        let Challenge::Authentication {
            state,
            mut passkeys,
            expires_at,
        } = challenge
        else {
            return Err(PasskeyError::InvalidChallenge);
        };
        if expires_at < now {
            return Err(PasskeyError::InvalidChallenge);
        }
        let result = self
            .webauthn
            .finish_passkey_authentication(credential, &state)
            .map_err(passkey_error)?;
        let passkey = passkeys
            .iter_mut()
            .find(|passkey| passkey.cred_id() == result.cred_id())
            .ok_or(PasskeyError::InvalidChallenge)?;
        passkey
            .update_credential(&result)
            .ok_or(PasskeyError::InvalidChallenge)?;
        let credential_id = passkey.cred_id().as_ref().to_vec();
        let passkey_json = serde_json::to_string(passkey)
            .map_err(|error| PasskeyError::Serialization(error.to_string()))?;
        Ok(AuthenticatedPasskey {
            credential_id,
            passkey_json,
        })
    }

    #[must_use]
    pub fn discard_challenge(&self, flow_id: Uuid) -> bool {
        self.challenges.remove(&flow_id).is_some()
    }

    #[must_use]
    pub fn active_challenge_count(&self) -> usize {
        self.challenges.len()
    }

    fn remove_expired(&self, now: DateTime<Utc>) {
        self.challenges.retain(|_, challenge| match challenge {
            Challenge::Registration { expires_at, .. }
            | Challenge::Authentication { expires_at, .. } => *expires_at >= now,
        });
    }
}

fn passkey_error(error: impl std::fmt::Display) -> PasskeyError {
    PasskeyError::Webauthn(error.to_string())
}
