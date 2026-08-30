use std::{net::SocketAddr, sync::Arc};

use axum::{
    body::Body,
    extract::ConnectInfo,
    http::{Request, StatusCode, header},
};
use chrono::Utc;
use simple_blog::{
    application::auth::AuthService,
    config::{Config, ConfigSources, Overrides},
    domain::auth::SetupPurpose,
    infrastructure::sqlite::SqliteRepository,
    web::{AppState, router},
};
use tower::ServiceExt;

async fn body_text(response: axum::response::Response) -> String {
    use http_body_util::BodyExt;
    String::from_utf8(
        response
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes()
            .to_vec(),
    )
    .unwrap()
}

#[tokio::test]
async fn auth_endpoints_are_limited_by_peer_and_untrusted_forwarding_is_ignored() {
    let temp = tempfile::tempdir().unwrap();
    let config = Config::resolve(ConfigSources {
        cli: Overrides {
            data_dir: Some(temp.path().to_path_buf()),
            public_url: Some("http://localhost:8080".into()),
            ..Overrides::default()
        },
        ..ConfigSources::default()
    })
    .unwrap();
    let repository = Arc::new(
        SqliteRepository::connect(&config.database_path())
            .await
            .unwrap(),
    );
    let token = AuthService::new(repository.clone())
        .issue_setup_token(SetupPurpose::Initial, Utc::now())
        .await
        .unwrap();
    let state = AppState::new(config, repository).unwrap();
    let peer: SocketAddr = "203.0.113.4:40123".parse().unwrap();
    let payload = serde_json::json!({ "token": token.expose() }).to_string();

    for attempt in 0..10 {
        let mut request = Request::builder()
            .method("POST")
            .uri("/admin/auth/setup/start")
            .header(header::HOST, "localhost:8080")
            .header(header::CONTENT_TYPE, "application/json")
            .header("x-forwarded-for", format!("198.51.100.{attempt}"))
            .body(Body::from(payload.clone()))
            .unwrap();
        request.extensions_mut().insert(ConnectInfo(peer));
        let response = router(state.clone()).oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK, "attempt {attempt}");
    }

    let mut request = Request::builder()
        .method("POST")
        .uri("/admin/auth/setup/start")
        .header(header::HOST, "localhost:8080")
        .header(header::CONTENT_TYPE, "application/json")
        .header("x-forwarded-for", "192.0.2.200")
        .body(Body::from(payload))
        .unwrap();
    request.extensions_mut().insert(ConnectInfo(peer));
    let limited = router(state).oneshot(request).await.unwrap();
    assert_eq!(limited.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(limited.headers()[header::RETRY_AFTER], "60");
    assert!(limited.headers().contains_key("x-request-id"));
}

#[tokio::test]
async fn forwarded_client_ip_is_used_only_for_an_explicitly_trusted_proxy() {
    let temp = tempfile::tempdir().unwrap();
    let config = Config::resolve(ConfigSources {
        cli: Overrides {
            data_dir: Some(temp.path().to_path_buf()),
            public_url: Some("http://localhost:8080".into()),
            trusted_proxies: Some(vec!["127.0.0.1".parse().unwrap()]),
            ..Overrides::default()
        },
        ..ConfigSources::default()
    })
    .unwrap();
    let repository = Arc::new(
        SqliteRepository::connect(&config.database_path())
            .await
            .unwrap(),
    );
    let token = AuthService::new(repository.clone())
        .issue_setup_token(SetupPurpose::Initial, Utc::now())
        .await
        .unwrap();
    let state = AppState::new(config, repository).unwrap();
    let proxy: SocketAddr = "127.0.0.1:40123".parse().unwrap();
    let payload = serde_json::json!({ "token": token.expose() }).to_string();

    for client in 1..=11 {
        let mut request = Request::builder()
            .method("POST")
            .uri("/admin/auth/setup/start")
            .header(header::HOST, "localhost:8080")
            .header(header::CONTENT_TYPE, "application/json")
            .header("x-forwarded-for", format!("198.51.100.{client}"))
            .body(Body::from(payload.clone()))
            .unwrap();
        request.extensions_mut().insert(ConnectInfo(proxy));
        let response = router(state.clone()).oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK, "client {client}");
    }
}

#[tokio::test]
async fn https_origin_sets_hsts_and_secure_strict_auth_cookies() {
    let temp = tempfile::tempdir().unwrap();
    let config = Config::resolve(ConfigSources {
        cli: Overrides {
            data_dir: Some(temp.path().to_path_buf()),
            public_url: Some("https://blog.example.com".into()),
            ..Overrides::default()
        },
        ..ConfigSources::default()
    })
    .unwrap();
    let repository = Arc::new(
        SqliteRepository::connect(&config.database_path())
            .await
            .unwrap(),
    );
    let auth = AuthService::new(repository.clone());
    let code = auth
        .replace_recovery_codes(Utc::now())
        .await
        .unwrap()
        .remove(0);
    let state = AppState::new(config, repository).unwrap();
    let response = router(state)
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/admin/auth/recovery")
                .header(header::HOST, "blog.example.com")
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(
                    serde_urlencoded::to_string([("code", code.expose())]).unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        response.headers()[header::STRICT_TRANSPORT_SECURITY],
        "max-age=31536000"
    );
    let cookies = response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .map(|value| value.to_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(cookies.len(), 2);
    assert!(cookies.iter().all(|cookie| cookie.contains("Secure")));
    assert!(
        cookies
            .iter()
            .all(|cookie| cookie.contains("SameSite=Strict"))
    );
    assert!(cookies.iter().any(|cookie| cookie.contains("HttpOnly")));
    assert_eq!(body_text(response).await, "");
}
