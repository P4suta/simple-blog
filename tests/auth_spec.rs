use std::sync::Arc;

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{Duration, Utc};
use simple_blog::{
    application::auth::{AuthRateLimiter, AuthService, PasskeyAccountService, RateLimitDecision},
    application::ports::{AuthError, EntropyError, EntropySource, PasskeyRepository},
    domain::auth::{SetupPurpose, StoredPasskey},
    infrastructure::{entropy::SystemEntropy, sqlite::SqliteRepository, webauthn::PasskeyCeremony},
};
use url::Url;
use uuid::Uuid;

fn system_entropy() -> Arc<SystemEntropy> {
    Arc::new(SystemEntropy)
}

#[derive(Debug)]
struct UnavailableEntropy;

impl EntropySource for UnavailableEntropy {
    fn fill(&self, _destination: &mut [u8]) -> Result<(), EntropyError> {
        Err(EntropyError)
    }
}

async fn auth_harness() -> (tempfile::TempDir, AuthService) {
    let temp = tempfile::tempdir().unwrap();
    let repository = Arc::new(
        SqliteRepository::connect(&temp.path().join("blog.sqlite3"))
            .await
            .unwrap(),
    );
    (temp, AuthService::new(repository, system_entropy()))
}

#[tokio::test]
async fn entropy_failure_is_explicit_and_never_persists_a_partial_capability() {
    let temp = tempfile::tempdir().unwrap();
    let repository = Arc::new(
        SqliteRepository::connect(&temp.path().join("blog.sqlite3"))
            .await
            .unwrap(),
    );
    let auth = AuthService::new(repository.clone(), Arc::new(UnavailableEntropy));

    let error = auth
        .issue_setup_token(SetupPurpose::Initial, Utc::now())
        .await
        .err();

    assert_eq!(error, Some(AuthError::EntropyUnavailable));
    let setup_token_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM setup_tokens")
        .fetch_one(repository.pool())
        .await
        .unwrap();
    assert_eq!(setup_token_count, 0);
}

#[tokio::test]
async fn initial_registration_commits_owner_passkey_session_and_recovery_codes_atomically() {
    let temp = tempfile::tempdir().unwrap();
    let repository = Arc::new(
        SqliteRepository::connect(&temp.path().join("blog.sqlite3"))
            .await
            .unwrap(),
    );
    let auth = AuthService::new(repository.clone(), system_entropy());
    let accounts = PasskeyAccountService::new(repository.clone(), system_entropy());
    let now = Utc::now();
    let setup = auth
        .issue_setup_token(SetupPurpose::Initial, now)
        .await
        .unwrap();
    let user_handle = Uuid::new_v4();

    let login = accounts
        .complete_setup_registration(
            setup.expose(),
            SetupPurpose::Initial,
            user_handle,
            StoredPasskey {
                credential_id: vec![1, 2, 3],
                name: "Laptop".into(),
                passkey_json: "{\"counter\":0}".into(),
            },
            now,
        )
        .await
        .unwrap()
        .expect("valid setup");

    assert_eq!(repository.owner_handle().await.unwrap(), Some(user_handle));
    assert_eq!(repository.list_passkeys().await.unwrap().len(), 1);
    assert_eq!(login.recovery_codes.len(), 10);
    assert!(
        auth.authenticate(login.session.session.expose(), now)
            .await
            .unwrap()
            .is_some()
    );

    let replay = accounts
        .complete_setup_registration(
            setup.expose(),
            SetupPurpose::Initial,
            user_handle,
            StoredPasskey {
                credential_id: vec![4, 5, 6],
                name: "Injected".into(),
                passkey_json: "{}".into(),
            },
            now,
        )
        .await
        .unwrap();
    assert!(replay.is_none());
    assert_eq!(repository.list_passkeys().await.unwrap().len(), 1);
}

#[tokio::test]
async fn passkeys_are_removable_except_for_the_last_owner_credential() {
    let temp = tempfile::tempdir().unwrap();
    let repository = Arc::new(
        SqliteRepository::connect(&temp.path().join("blog.sqlite3"))
            .await
            .unwrap(),
    );
    let auth = AuthService::new(repository.clone(), system_entropy());
    let accounts = PasskeyAccountService::new(repository.clone(), system_entropy());
    let now = Utc::now();
    let setup = auth
        .issue_setup_token(SetupPurpose::Initial, now)
        .await
        .unwrap();
    accounts
        .complete_setup_registration(
            setup.expose(),
            SetupPurpose::Initial,
            Uuid::new_v4(),
            StoredPasskey {
                credential_id: vec![1],
                name: "First".into(),
                passkey_json: "{}".into(),
            },
            now,
        )
        .await
        .unwrap()
        .unwrap();
    accounts
        .add_passkey(
            &StoredPasskey {
                credential_id: vec![2],
                name: "Second".into(),
                passkey_json: "{}".into(),
            },
            now,
        )
        .await
        .unwrap();

    assert!(accounts.remove_passkey(&[1]).await.unwrap());
    assert_eq!(repository.list_passkeys().await.unwrap()[0].name, "Second");
    assert!(!accounts.remove_passkey(&[2]).await.unwrap());
    assert_eq!(repository.list_passkeys().await.unwrap().len(), 1);
}

#[tokio::test]
async fn owner_recovery_replaces_credentials_and_invalidates_old_sessions() {
    let temp = tempfile::tempdir().unwrap();
    let repository = Arc::new(
        SqliteRepository::connect(&temp.path().join("blog.sqlite3"))
            .await
            .unwrap(),
    );
    let auth = AuthService::new(repository.clone(), system_entropy());
    let accounts = PasskeyAccountService::new(repository.clone(), system_entropy());
    let now = Utc::now();
    let user_handle = Uuid::new_v4();
    let setup = auth
        .issue_setup_token(SetupPurpose::Initial, now)
        .await
        .unwrap();
    let original = accounts
        .complete_setup_registration(
            setup.expose(),
            SetupPurpose::Initial,
            user_handle,
            StoredPasskey {
                credential_id: vec![1],
                name: "Lost key".into(),
                passkey_json: "{}".into(),
            },
            now,
        )
        .await
        .unwrap()
        .unwrap();
    let recovery = auth
        .issue_setup_token(SetupPurpose::Recovery, now)
        .await
        .unwrap();
    let recovered = accounts
        .complete_setup_registration(
            recovery.expose(),
            SetupPurpose::Recovery,
            user_handle,
            StoredPasskey {
                credential_id: vec![2],
                name: "Replacement key".into(),
                passkey_json: "{}".into(),
            },
            now,
        )
        .await
        .unwrap()
        .unwrap();

    let passkeys = repository.list_passkeys().await.unwrap();
    assert_eq!(passkeys.len(), 1);
    assert_eq!(passkeys[0].name, "Replacement key");
    assert!(
        auth.authenticate(original.session.session.expose(), now)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        auth.authenticate(recovered.session.session.expose(), now)
            .await
            .unwrap()
            .is_some()
    );
}

#[tokio::test]
async fn setup_capability_is_256_bit_expiring_and_one_time() {
    let (_temp, auth) = auth_harness().await;
    let now = Utc::now();
    let token = auth
        .issue_setup_token(SetupPurpose::Initial, now)
        .await
        .unwrap();
    assert_eq!(URL_SAFE_NO_PAD.decode(token.expose()).unwrap().len(), 32);

    assert!(
        auth.consume_setup_token(token.expose(), SetupPurpose::Initial, now)
            .await
            .unwrap()
    );
    assert!(
        !auth
            .consume_setup_token(token.expose(), SetupPurpose::Initial, now)
            .await
            .unwrap(),
        "a consumed setup token must not be replayable"
    );

    let expired = auth
        .issue_setup_token(SetupPurpose::Recovery, now)
        .await
        .unwrap();
    assert!(
        !auth
            .consume_setup_token(
                expired.expose(),
                SetupPurpose::Recovery,
                now + Duration::minutes(16),
            )
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn sessions_are_opaque_rotatable_and_bound_to_csrf() {
    let (_temp, auth) = auth_harness().await;
    let now = Utc::now();
    let first = auth.create_session(now).await.unwrap();

    let identity = auth
        .authenticate(first.session.expose(), now + Duration::minutes(1))
        .await
        .unwrap()
        .expect("valid session");
    assert!(auth.verify_csrf(&identity, first.csrf.expose()));
    assert!(!auth.verify_csrf(&identity, "attacker-controlled"));

    let rotated = auth
        .rotate_session(first.session.expose(), now + Duration::minutes(2))
        .await
        .unwrap()
        .expect("rotation");
    assert!(
        auth.authenticate(first.session.expose(), now + Duration::minutes(3))
            .await
            .unwrap()
            .is_none(),
        "rotation must invalidate the old token"
    );
    assert!(
        auth.authenticate(rotated.session.expose(), now + Duration::minutes(3))
            .await
            .unwrap()
            .is_some()
    );
}

#[tokio::test]
async fn expired_sessions_are_rejected() {
    let (_temp, auth) = auth_harness().await;
    let now = Utc::now();
    let tokens = auth.create_session(now).await.unwrap();

    assert!(
        auth.authenticate(tokens.session.expose(), now + Duration::days(8))
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn recovery_codes_are_shown_once_and_each_can_only_be_consumed_once() {
    let (_temp, auth) = auth_harness().await;
    let now = Utc::now();
    let codes = auth.replace_recovery_codes(now).await.unwrap();
    assert_eq!(codes.len(), 10);
    assert_eq!(
        codes
            .iter()
            .map(simple_blog::domain::auth::SecretToken::expose)
            .collect::<std::collections::HashSet<_>>()
            .len(),
        10
    );

    assert!(
        auth.consume_recovery_code(codes[0].expose(), now)
            .await
            .unwrap()
    );
    assert!(
        !auth
            .consume_recovery_code(codes[0].expose(), now)
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn recovery_code_exchange_consumes_the_code_and_creates_a_session_atomically() {
    let (_temp, auth) = auth_harness().await;
    let now = Utc::now();
    let codes = auth.replace_recovery_codes(now).await.unwrap();
    let session = auth
        .recover_session(codes[0].expose(), now)
        .await
        .unwrap()
        .expect("unused recovery code");
    assert!(
        auth.authenticate(session.session.expose(), now)
            .await
            .unwrap()
            .is_some()
    );
    assert!(
        auth.recover_session(codes[0].expose(), now)
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn sensitive_actions_can_require_recent_reauthentication() {
    let (_temp, auth) = auth_harness().await;
    let now = Utc::now();
    let session = auth.create_session(now).await.unwrap();
    let identity = auth
        .authenticate(session.session.expose(), now)
        .await
        .unwrap()
        .unwrap();
    assert!(identity.was_reauthenticated_within(now + Duration::minutes(4), Duration::minutes(5)));
    assert!(!identity.was_reauthenticated_within(now + Duration::minutes(6), Duration::minutes(5)));
}

#[test]
fn webauthn_requires_https_except_for_local_development() {
    assert!(
        PasskeyCeremony::new(
            &Url::parse("https://blog.example.com").unwrap(),
            "Simple Blog"
        )
        .is_ok()
    );
    assert!(
        PasskeyCeremony::new(&Url::parse("http://localhost:8080").unwrap(), "Simple Blog").is_ok()
    );
    assert!(
        PasskeyCeremony::new(
            &Url::parse("http://blog.example.com").unwrap(),
            "Simple Blog"
        )
        .is_err()
    );
}

#[test]
fn registration_challenge_state_is_server_side_and_single_use() {
    let ceremony = PasskeyCeremony::new(
        &Url::parse("https://blog.example.com").unwrap(),
        "Simple Blog",
    )
    .unwrap();
    let start = ceremony
        .start_registration(Uuid::new_v4(), &[], Utc::now())
        .unwrap();
    let public_json = serde_json::to_value(&start.public).unwrap();
    assert!(public_json.get("publicKey").is_some());
    assert_eq!(ceremony.active_challenge_count(), 1);
    assert!(ceremony.discard_challenge(start.flow_id));
    assert!(!ceremony.discard_challenge(start.flow_id));
}

#[test]
fn authentication_rate_limit_is_bounded_and_time_injectable() {
    let limiter = AuthRateLimiter::new(2, Duration::minutes(1));
    let now = Utc::now();
    assert_eq!(
        limiter.check("203.0.113.4", now),
        RateLimitDecision::Allowed
    );
    assert_eq!(
        limiter.check("203.0.113.4", now),
        RateLimitDecision::Allowed
    );
    assert_eq!(
        limiter.check("203.0.113.4", now),
        RateLimitDecision::Limited { retry_after: 60 }
    );
    assert_eq!(
        limiter.check("203.0.113.4", now + Duration::seconds(61)),
        RateLimitDecision::Allowed
    );
    assert_eq!(
        limiter.check("198.51.100.8", now),
        RateLimitDecision::Allowed
    );
}
