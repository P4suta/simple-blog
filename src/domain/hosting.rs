//! Pure custom-domain registration lifecycle shared by host adapters.

use std::{fmt, net::IpAddr};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct DomainName(String);

impl DomainName {
    pub fn parse(value: impl AsRef<str>) -> Result<Self, InvalidDomainName> {
        let value = value.as_ref();
        if !value.is_ascii()
            || value.is_empty()
            || value.len() > 253
            || value.trim() != value
            || value.ends_with('.')
            || value.parse::<IpAddr>().is_ok()
        {
            return Err(InvalidDomainName);
        }
        let normalized = value.to_ascii_lowercase();
        let labels = normalized.split('.').collect::<Vec<_>>();
        let valid_label = |label: &&str| {
            !label.is_empty()
                && label.len() <= 63
                && label
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
                && label
                    .as_bytes()
                    .first()
                    .is_some_and(u8::is_ascii_alphanumeric)
                && label
                    .as_bytes()
                    .last()
                    .is_some_and(u8::is_ascii_alphanumeric)
        };
        let suffix = labels.last().copied().unwrap_or_default();
        if labels.len() < 2
            || !labels.iter().all(valid_label)
            || matches!(suffix, "example" | "invalid" | "localhost" | "test")
            || suffix.bytes().all(|byte| byte.is_ascii_digit())
        {
            return Err(InvalidDomainName);
        }
        Ok(Self(normalized))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    #[must_use]
    pub fn canonical_origin(&self) -> String {
        format!("https://{}", self.0)
    }
}

impl fmt::Display for DomainName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl From<DomainName> for String {
    fn from(value: DomainName) -> Self {
        value.0
    }
}

impl TryFrom<String> for DomainName {
    type Error = InvalidDomainName;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(value)
    }
}

#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
#[error("domain must be a normalized registrable ASCII hostname")]
pub struct InvalidDomainName;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProvisioningStatus {
    Pending,
    Active,
    Failed,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DomainObservation {
    pub checked_at: DateTime<Utc>,
    pub hostname: ProvisioningStatus,
    pub certificate: ProvisioningStatus,
    pub dns_routed: bool,
    pub provider_error_code: Option<String>,
}

impl DomainObservation {
    fn is_ready(&self) -> bool {
        self.hostname == ProvisioningStatus::Active
            && self.certificate == ProvisioningStatus::Active
            && self.dns_routed
    }

    fn has_failed(&self) -> bool {
        self.hostname == ProvisioningStatus::Failed
            || self.certificate == ProvisioningStatus::Failed
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DomainRegistrationState {
    PendingOwnership,
    PendingCertificate,
    PendingDns,
    ReadyForOwner,
    Active,
    ActionRequired,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DomainRegistration {
    pub id: Uuid,
    pub domain: DomainName,
    pub provider_hostname_id: String,
    pub state: DomainRegistrationState,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub last_observation: Option<DomainObservation>,
    pub owner_registered_at: Option<DateTime<Utc>>,
    pub provider_error_code: Option<String>,
}

impl DomainRegistration {
    #[must_use]
    pub const fn new(
        id: Uuid,
        domain: DomainName,
        provider_hostname_id: String,
        now: DateTime<Utc>,
    ) -> Self {
        Self {
            id,
            domain,
            provider_hostname_id,
            state: DomainRegistrationState::PendingOwnership,
            created_at: now,
            updated_at: now,
            last_observation: None,
            owner_registered_at: None,
            provider_error_code: None,
        }
    }

    pub fn reconcile(
        &mut self,
        observation: DomainObservation,
    ) -> Result<DomainTransition, DomainRegistrationError> {
        if let Some(previous) = &self.last_observation {
            if observation.checked_at < previous.checked_at {
                return Err(DomainRegistrationError::StaleObservation {
                    previous: previous.checked_at,
                    received: observation.checked_at,
                });
            }
            if observation.checked_at == previous.checked_at {
                if observation == *previous {
                    return Ok(DomainTransition::unchanged(self.state));
                }
                return Err(DomainRegistrationError::ConflictingObservation(
                    observation.checked_at,
                ));
            }
        }
        let from = self.state;
        let to = self.state_for(&observation);
        self.provider_error_code = observation
            .has_failed()
            .then(|| observation.provider_error_code.clone())
            .flatten();
        self.updated_at = observation.checked_at;
        self.last_observation = Some(observation);
        self.state = to;
        Ok(DomainTransition {
            from,
            to,
            changed: from != to,
        })
    }

    pub fn complete_owner_setup(
        &mut self,
        now: DateTime<Utc>,
    ) -> Result<DomainTransition, DomainRegistrationError> {
        if self.owner_registered_at.is_some() && self.state == DomainRegistrationState::Active {
            return Ok(DomainTransition::unchanged(self.state));
        }
        if self.state != DomainRegistrationState::ReadyForOwner {
            return Err(DomainRegistrationError::NotReady);
        }
        let from = self.state;
        self.state = DomainRegistrationState::Active;
        self.owner_registered_at = Some(now);
        self.updated_at = now;
        Ok(DomainTransition {
            from,
            to: self.state,
            changed: true,
        })
    }

    fn state_for(&self, observation: &DomainObservation) -> DomainRegistrationState {
        if observation.has_failed()
            || (self.owner_registered_at.is_some() && !observation.is_ready())
        {
            DomainRegistrationState::ActionRequired
        } else if observation.hostname != ProvisioningStatus::Active {
            DomainRegistrationState::PendingOwnership
        } else if observation.certificate != ProvisioningStatus::Active {
            DomainRegistrationState::PendingCertificate
        } else if !observation.dns_routed {
            DomainRegistrationState::PendingDns
        } else if self.owner_registered_at.is_some() {
            DomainRegistrationState::Active
        } else {
            DomainRegistrationState::ReadyForOwner
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DomainTransition {
    pub from: DomainRegistrationState,
    pub to: DomainRegistrationState,
    pub changed: bool,
}

impl DomainTransition {
    const fn unchanged(state: DomainRegistrationState) -> Self {
        Self {
            from: state,
            to: state,
            changed: false,
        }
    }
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum DomainRegistrationError {
    #[error("provider observation at {received} predates the accepted observation at {previous}")]
    StaleObservation {
        previous: DateTime<Utc>,
        received: DateTime<Utc>,
    },
    #[error("provider returned conflicting observations at {0}")]
    ConflictingObservation(DateTime<Utc>),
    #[error("owner setup is unavailable until domain, certificate, and DNS are ready")]
    NotReady,
}
