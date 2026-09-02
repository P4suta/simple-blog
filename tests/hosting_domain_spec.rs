use chrono::{Duration, TimeZone, Utc};
use simple_blog::domain::hosting::{
    DomainName, DomainObservation, DomainRegistration, DomainRegistrationError,
    DomainRegistrationState, ProvisioningStatus,
};
use uuid::Uuid;

#[derive(serde::Deserialize)]
struct Contract {
    format_version: u16,
    cases: Vec<ContractCase>,
}

#[derive(serde::Deserialize)]
struct ContractCase {
    name: String,
    owner_registered: bool,
    hostname: ProvisioningStatus,
    certificate: ProvisioningStatus,
    dns_routed: bool,
    expected: DomainRegistrationState,
}

fn at(minute: u32) -> chrono::DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 9, 2, 12, minute, 0).unwrap()
}

const fn observation(
    checked_at: chrono::DateTime<Utc>,
    hostname: ProvisioningStatus,
    certificate: ProvisioningStatus,
    dns_routed: bool,
) -> DomainObservation {
    DomainObservation {
        checked_at,
        hostname,
        certificate,
        dns_routed,
        provider_error_code: None,
    }
}

#[test]
fn domain_identity_is_normalized_once_and_rejects_ambiguous_or_reserved_hosts() {
    let domain = DomainName::parse("BLOG.Writing.Example.Co.Jp").unwrap();
    assert_eq!(domain.as_str(), "blog.writing.example.co.jp");
    assert_eq!(
        domain.canonical_origin(),
        "https://blog.writing.example.co.jp"
    );
    assert_eq!(domain.to_string(), "blog.writing.example.co.jp");
    assert_eq!(String::from(domain.clone()), "blog.writing.example.co.jp");
    assert_eq!(
        DomainName::try_from("BLOG.WRITING.COM".to_owned())
            .unwrap()
            .as_str(),
        "blog.writing.com"
    );
    let serialized = serde_json::to_string(&domain).unwrap();
    assert_eq!(
        serde_json::from_str::<DomainName>(&serialized).unwrap(),
        domain
    );

    for invalid in [
        "localhost",
        "127.0.0.1",
        "https://blog.example.com",
        "blog.example.com:443",
        "*.example.com",
        "-blog.example.com",
        "blog..example.com",
        "blog.example",
        "blog.example.com.",
        "ブログ.example.com",
    ] {
        assert!(DomainName::parse(invalid).is_err(), "accepted {invalid}");
    }
}

#[test]
fn observations_at_the_same_instant_must_be_byte_for_byte_equivalent() {
    let mut registration = DomainRegistration::new(
        Uuid::from_u128(10),
        DomainName::parse("conflict.writer.com").unwrap(),
        "cf-hostname-10".into(),
        at(0),
    );
    registration
        .reconcile(observation(
            at(1),
            ProvisioningStatus::Active,
            ProvisioningStatus::Pending,
            false,
        ))
        .unwrap();

    let error = registration
        .reconcile(observation(
            at(1),
            ProvisioningStatus::Active,
            ProvisioningStatus::Active,
            false,
        ))
        .unwrap_err();
    assert!(matches!(
        error,
        DomainRegistrationError::ConflictingObservation(instant) if instant == at(1)
    ));
    assert_eq!(
        registration.state,
        DomainRegistrationState::PendingCertificate
    );
}

#[test]
fn registration_requires_provider_ownership_certificate_and_dns_before_owner_setup() {
    let mut registration = DomainRegistration::new(
        Uuid::from_u128(1),
        DomainName::parse("blog.writer.com").unwrap(),
        "cf-hostname-7".into(),
        at(0),
    );
    assert_eq!(
        registration.state,
        DomainRegistrationState::PendingOwnership
    );

    registration
        .reconcile(observation(
            at(1),
            ProvisioningStatus::Active,
            ProvisioningStatus::Pending,
            false,
        ))
        .unwrap();
    assert_eq!(
        registration.state,
        DomainRegistrationState::PendingCertificate
    );
    registration
        .reconcile(observation(
            at(2),
            ProvisioningStatus::Active,
            ProvisioningStatus::Active,
            false,
        ))
        .unwrap();
    assert_eq!(registration.state, DomainRegistrationState::PendingDns);
    registration
        .reconcile(observation(
            at(3),
            ProvisioningStatus::Active,
            ProvisioningStatus::Active,
            true,
        ))
        .unwrap();
    assert_eq!(registration.state, DomainRegistrationState::ReadyForOwner);

    registration.complete_owner_setup(at(4)).unwrap();
    assert_eq!(registration.state, DomainRegistrationState::Active);
    assert_eq!(registration.owner_registered_at, Some(at(4)));
}

#[test]
fn registration_reconciliation_is_monotonic_idempotent_and_retains_an_active_account() {
    let mut registration = DomainRegistration::new(
        Uuid::from_u128(2),
        DomainName::parse("notes.writer.com").unwrap(),
        "cf-hostname-8".into(),
        at(0),
    );
    let ready = observation(
        at(1),
        ProvisioningStatus::Active,
        ProvisioningStatus::Active,
        true,
    );
    let transition = registration.reconcile(ready.clone()).unwrap();
    assert!(transition.changed);
    assert!(!registration.reconcile(ready).unwrap().changed);
    registration.complete_owner_setup(at(2)).unwrap();

    let stale = observation(
        at(1) - Duration::seconds(1),
        ProvisioningStatus::Pending,
        ProvisioningStatus::Pending,
        false,
    );
    assert!(matches!(
        registration.reconcile(stale).unwrap_err(),
        DomainRegistrationError::StaleObservation { .. }
    ));
    assert_eq!(registration.state, DomainRegistrationState::Active);

    registration
        .reconcile(DomainObservation {
            checked_at: at(3),
            hostname: ProvisioningStatus::Failed,
            certificate: ProvisioningStatus::Active,
            dns_routed: false,
            provider_error_code: Some("hostname_validation_failed".into()),
        })
        .unwrap();
    assert_eq!(registration.state, DomainRegistrationState::ActionRequired);
    assert_eq!(
        registration.provider_error_code.as_deref(),
        Some("hostname_validation_failed")
    );

    registration
        .reconcile(observation(
            at(4),
            ProvisioningStatus::Active,
            ProvisioningStatus::Active,
            true,
        ))
        .unwrap();
    assert_eq!(registration.state, DomainRegistrationState::Active);
}

#[test]
fn owner_setup_is_impossible_before_domain_readiness_and_safe_to_retry_after_commit() {
    let mut registration = DomainRegistration::new(
        Uuid::from_u128(3),
        DomainName::parse("journal.writer.com").unwrap(),
        "cf-hostname-9".into(),
        at(0),
    );
    assert!(matches!(
        registration.complete_owner_setup(at(1)).unwrap_err(),
        DomainRegistrationError::NotReady
    ));
    registration
        .reconcile(observation(
            at(2),
            ProvisioningStatus::Active,
            ProvisioningStatus::Active,
            true,
        ))
        .unwrap();
    assert!(registration.complete_owner_setup(at(3)).unwrap().changed);
    assert!(!registration.complete_owner_setup(at(3)).unwrap().changed);
}

#[test]
fn rust_core_satisfies_the_versioned_cross_adapter_domain_contract() {
    let contract: Contract =
        serde_json::from_str(include_str!("../contracts/domain-registration-v1.json")).unwrap();
    assert_eq!(contract.format_version, 1);
    for case in contract.cases {
        let mut registration = DomainRegistration::new(
            Uuid::from_u128(9),
            DomainName::parse("contract.writer.com").unwrap(),
            "cf-contract".into(),
            at(0),
        );
        if case.owner_registered {
            registration
                .reconcile(observation(
                    at(1),
                    ProvisioningStatus::Active,
                    ProvisioningStatus::Active,
                    true,
                ))
                .unwrap();
            registration.complete_owner_setup(at(2)).unwrap();
        }
        registration
            .reconcile(DomainObservation {
                checked_at: at(3),
                hostname: case.hostname,
                certificate: case.certificate,
                dns_routed: case.dns_routed,
                provider_error_code: (case.hostname == ProvisioningStatus::Failed
                    || case.certificate == ProvisioningStatus::Failed)
                    .then(|| "provider_failed".into()),
            })
            .unwrap();
        assert_eq!(registration.state, case.expected, "{}", case.name);
    }
}
