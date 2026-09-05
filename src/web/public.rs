use std::{sync::Arc, time::SystemTime};

use axum::{
    Json,
    body::Body,
    extract::{OriginalUri, Path, State},
    http::{HeaderMap, HeaderValue, Method, StatusCode, header},
    response::{IntoResponse, Response},
};
use chrono::{DateTime, Utc};
use serde::Deserialize;

use tracing::Instrument as _;

use crate::{
    application::ports::{EngagementRepository, RepositoryError},
    domain::{content::ContentId, media::mime_for_media_filename},
    release::{ReleaseId, ReleaseResolver, ResolvedAsset, ResolvedRoute},
    web::{AppState, WebError},
};

const RELEASE_HEADER: &str = "x-simple-blog-release";

/// Native host adapter for the host-neutral immutable release contract.
/// Every public route, including the generated 404 page, passes through here.
pub async fn release_site(
    State(state): State<AppState>,
    OriginalUri(uri): OriginalUri,
    method: Method,
    headers: HeaderMap,
) -> Result<Response, WebError> {
    if !matches!(method, Method::GET | Method::HEAD) {
        let mut response = StatusCode::METHOD_NOT_ALLOWED.into_response();
        response
            .headers_mut()
            .insert(header::ALLOW, HeaderValue::from_static("GET, HEAD"));
        return Ok(response);
    }

    match ReleaseResolver::new(state.release_store.clone())
        .resolve(uri.path())
        .await?
    {
        ResolvedRoute::Asset(asset) => {
            if method == Method::GET
                && let Some(content_id) = asset.content_id
                && !is_probably_bot(&headers)
            {
                record_view_in_background(
                    state.engagement.clone(),
                    content_id,
                    asset.release_id.clone(),
                );
            }
            asset_response(asset, &method, &headers)
        }
        ResolvedRoute::Redirect(redirect) => {
            let status = StatusCode::from_u16(redirect.status).map_err(WebError::header)?;
            let mut response = status.into_response();
            response.headers_mut().insert(
                header::LOCATION,
                HeaderValue::from_str(&redirect.location).map_err(WebError::header)?,
            );
            response.headers_mut().insert(
                header::CACHE_CONTROL,
                HeaderValue::from_static("public, max-age=0, must-revalidate"),
            );
            response.headers_mut().insert(
                RELEASE_HEADER,
                HeaderValue::from_str(redirect.release_id.as_str()).map_err(WebError::header)?,
            );
            Ok(response)
        }
    }
}

fn asset_response(
    mut asset: ResolvedAsset,
    method: &Method,
    request_headers: &HeaderMap,
) -> Result<Response, WebError> {
    let etag = format!("\"blake3-{}\"", asset.object_id);
    if asset.status == 200 && request_is_fresh(request_headers, &etag, asset.last_modified) {
        let mut response = StatusCode::NOT_MODIFIED.into_response();
        apply_release_headers(&mut response, &asset, &etag, false)?;
        return Ok(response);
    }

    let status = StatusCode::from_u16(asset.status).map_err(WebError::header)?;
    let mut response = if method == Method::HEAD {
        Body::empty().into_response()
    } else {
        std::mem::take(&mut asset.body).into_response()
    };
    *response.status_mut() = status;
    apply_release_headers(&mut response, &asset, &etag, true)?;
    Ok(response)
}

fn request_is_fresh(headers: &HeaderMap, etag: &str, last_modified: Option<DateTime<Utc>>) -> bool {
    if let Some(value) = headers
        .get(header::IF_NONE_MATCH)
        .and_then(|value| value.to_str().ok())
    {
        return value.split(',').any(|candidate| {
            let candidate = candidate.trim();
            candidate == "*" || candidate.strip_prefix("W/").unwrap_or(candidate) == etag
        });
    }
    let Some(last_modified) = last_modified else {
        return false;
    };
    headers
        .get(header::IF_MODIFIED_SINCE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| httpdate::parse_http_date(value).ok())
        .is_some_and(|since| last_modified.timestamp() <= DateTime::<Utc>::from(since).timestamp())
}

fn apply_release_headers(
    response: &mut Response,
    asset: &ResolvedAsset,
    etag: &str,
    include_content_type: bool,
) -> Result<(), WebError> {
    let headers = response.headers_mut();
    if include_content_type {
        headers.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_str(&asset.content_type).map_err(WebError::header)?,
        );
    }
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_str(&asset.cache_control).map_err(WebError::header)?,
    );
    headers.insert(
        header::ETAG,
        HeaderValue::from_str(etag).map_err(WebError::header)?,
    );
    headers.insert(
        RELEASE_HEADER,
        HeaderValue::from_str(asset.release_id.as_str()).map_err(WebError::header)?,
    );
    if let Some(modified) = asset.last_modified {
        let modified: SystemTime = modified.into();
        headers.insert(
            header::LAST_MODIFIED,
            HeaderValue::from_str(&httpdate::fmt_http_date(modified)).map_err(WebError::header)?,
        );
    }
    Ok(())
}

/// The page never waits for its own counter. The write runs on a task of its
/// own instead of in front of the response, so a slow or failing database
/// costs the reader nothing; a failure is a warning that keeps the request
/// ID. The official host does the same with `waitUntil`.
fn record_view_in_background(
    engagement: Arc<dyn EngagementRepository>,
    content_id: i64,
    release_id: ReleaseId,
) {
    tokio::spawn(
        async move {
            if let Err(error) = engagement
                .record_view(ContentId::from_i64(content_id))
                .await
            {
                tracing::warn!(
                    event = "views.record_failed",
                    content_id,
                    release_id = %release_id,
                    error = %error
                );
            }
        }
        .in_current_span(),
    );
}

/// A deliberately light heuristic: well-behaved crawlers identify themselves,
/// and those are the ones that would otherwise dominate the view counter.
fn is_probably_bot(headers: &HeaderMap) -> bool {
    headers
        .get(header::USER_AGENT)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|agent| {
            let agent = agent.to_ascii_lowercase();
            ["bot", "crawler", "spider", "preview", "curl", "wget"]
                .iter()
                .any(|marker| agent.contains(marker))
        })
}

pub async fn media_file(
    State(state): State<AppState>,
    Path(filename): Path<String>,
    headers: HeaderMap,
) -> Result<Response, WebError> {
    // The name alone decides the media type: the store only ever writes
    // `<digest>.<ext>` and `<digest>-<width>w.webp`, so no database is asked
    // and a temporary or foreign file under the media directory is never
    // served. A well-formed name for a file that does not exist is a 404.
    let Some(mime) = mime_for_media_filename(&filename) else {
        return Ok(StatusCode::NOT_FOUND.into_response());
    };
    let etag = format!("\"media-{filename}\"");
    if headers
        .get(header::IF_NONE_MATCH)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value == etag)
    {
        let mut response = StatusCode::NOT_MODIFIED.into_response();
        response.headers_mut().insert(
            header::ETAG,
            HeaderValue::from_str(&etag).map_err(WebError::header)?,
        );
        return Ok(response);
    }
    let path = state.config.media_dir().join(&filename);
    let Ok(bytes) = tokio::fs::read(path).await else {
        return Ok(StatusCode::NOT_FOUND.into_response());
    };
    let mut response = bytes.into_response();
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static(mime));
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("public, max-age=31536000, immutable"),
    );
    response.headers_mut().insert(
        header::ETAG,
        HeaderValue::from_str(&etag).map_err(WebError::header)?,
    );
    Ok(response)
}

#[derive(Deserialize)]
pub struct LikeOp {
    op: String,
}

/// The JSON body doubles as the CSRF guard: a cross-origin form cannot send
/// `application/json`, and there is no reader authentication to ride on anyway.
/// Totals are owner-facing only, so success is a bare 204.
pub async fn like_toggle(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(body): Json<LikeOp>,
) -> Result<Response, WebError> {
    let now = state.clock.now();
    let id = ContentId::from_i64(id);
    let result = match body.op.as_str() {
        "like" => state.likes.add_like(id, now).await,
        "unlike" => state.likes.remove_like(id, now).await,
        _ => {
            return Ok((
                StatusCode::UNPROCESSABLE_ENTITY,
                "op must be like or unlike",
            )
                .into_response());
        }
    };
    match result {
        Ok(_) => Ok(StatusCode::NO_CONTENT.into_response()),
        Err(RepositoryError::NotFound) => Ok(StatusCode::NOT_FOUND.into_response()),
        Err(error) => Err(WebError::Repository(error)),
    }
}

pub async fn health() -> Response {
    (
        [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
        "ok\n",
    )
        .into_response()
}
