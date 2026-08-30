use std::net::{IpAddr, SocketAddr};

use crate::{application::auth::RateLimitDecision, config::Config, web::AppState};
use axum::{
    extract::{ConnectInfo, Request, State},
    http::{HeaderValue, Method, StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};

pub(super) async fn auth_rate_limit(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Response {
    if request.method() != Method::POST || !request.uri().path().starts_with("/admin/auth/") {
        return next.run(request).await;
    }
    let key =
        client_ip(&state.config, &request).map_or_else(|| "unknown".into(), |ip| ip.to_string());
    match state.auth_rate_limiter.check(&key, state.clock.now()) {
        RateLimitDecision::Allowed => next.run(request).await,
        RateLimitDecision::Limited { retry_after } => {
            tracing::warn!(retry_after, "authentication rate limit exceeded");
            let mut response = (
                StatusCode::TOO_MANY_REQUESTS,
                "Too many authentication requests",
            )
                .into_response();
            if let Ok(value) = HeaderValue::from_str(&retry_after.to_string()) {
                response.headers_mut().insert(header::RETRY_AFTER, value);
            }
            response
        }
    }
}

fn client_ip(config: &Config, request: &Request) -> Option<IpAddr> {
    let peer = request
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|ConnectInfo(address)| *address)?;
    if config.trusted_proxies.contains(&peer.ip()) {
        forwarded_ip(request).or_else(|| Some(peer.ip()))
    } else {
        Some(peer.ip())
    }
}

fn forwarded_ip(request: &Request) -> Option<IpAddr> {
    request
        .headers()
        .get("x-forwarded-for")?
        .to_str()
        .ok()?
        .split(',')
        .next()?
        .trim()
        .parse()
        .ok()
}
