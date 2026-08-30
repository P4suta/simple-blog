use std::time::SystemTime;

use axum::{
    extract::{Path, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use chrono::SecondsFormat;
use serde::Serialize;

use crate::{
    domain::{
        content::{Content, ContentKind, Publication, Slug, Tag},
        media::MediaId,
        theme::{PageMeta, SiteSettings},
    },
    web::{AppState, MetaTitle, WebError},
};

#[derive(Serialize)]
struct CardsPage {
    posts: Vec<ContentCard>,
}

#[derive(Serialize)]
struct ArchivePage {
    heading: String,
    posts: Vec<ContentCard>,
}

#[derive(Serialize)]
struct ContentCard {
    title: String,
    slug: String,
    summary: String,
    publish_at: String,
    date: String,
    tags: Vec<Tag>,
}

#[derive(Serialize)]
struct ContentPage {
    kind: &'static str,
    title: String,
    summary: String,
    body_html: String,
    publish_at: Option<String>,
    date: Option<String>,
    tags: Vec<Tag>,
    cover: Option<CoverView>,
}

#[derive(Serialize)]
struct CoverView {
    original_url: String,
    srcset: String,
    alt_text: String,
    width: u32,
    height: u32,
}

#[derive(Serialize)]
struct FeedContext {
    site_title: String,
    site_url: String,
    feed_url: String,
    updated: String,
    entries: Vec<FeedEntry>,
}

#[derive(Serialize)]
struct FeedEntry {
    title: String,
    url: String,
    published: String,
    updated: String,
    summary: String,
}

#[derive(Serialize)]
struct SitemapContext {
    site_url: String,
    entries: Vec<SitemapEntry>,
}

#[derive(Serialize)]
struct SitemapEntry {
    url: String,
    updated: String,
}

pub async fn home(State(state): State<AppState>) -> Result<Response, WebError> {
    let now = state.clock.now();
    let posts = state.content.list_public_posts(now, 25, 0).await?;
    let context = state
        .theme_context(
            "/",
            MetaTitle::Site,
            None,
            "website",
            CardsPage {
                posts: posts.into_iter().map(ContentCard::from).collect(),
            },
        )
        .await?;
    state.render_html("public/home.html", context)
}

pub async fn canonical_content(Path(slug): Path<String>) -> Response {
    canonical_redirect(&format!("/{slug}/"))
}

pub async fn content(
    State(state): State<AppState>,
    Path(raw_slug): Path<String>,
    headers: HeaderMap,
) -> Result<Response, WebError> {
    let Ok(slug) = Slug::parse(&raw_slug) else {
        return not_found(State(state)).await;
    };
    let now = state.clock.now();
    let Some(content) = state.content.find_public_by_slug(&slug, now).await? else {
        if let Some(target) = state.content.resolve_redirect(&slug).await? {
            return Ok(redirect(
                StatusCode::MOVED_PERMANENTLY,
                &format!("/{target}/"),
            ));
        }
        return not_found(State(state)).await;
    };

    let etag = format!("\"content-{}-{}\"", content.id, content.version);
    if headers
        .get(header::IF_NONE_MATCH)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.split(',').any(|candidate| candidate.trim() == etag))
    {
        let mut response = StatusCode::NOT_MODIFIED.into_response();
        response.headers_mut().insert(
            header::ETAG,
            HeaderValue::from_str(&etag).map_err(WebError::header)?,
        );
        return Ok(response);
    }

    let path = format!("/{}/", content.slug);
    let title = content.seo_title.clone().map_or_else(
        || MetaTitle::Page(content.title.clone()),
        MetaTitle::Override,
    );
    let description = content
        .seo_description
        .clone()
        .unwrap_or_else(|| content.summary.clone());
    let cover = if let Some(id) = &content.cover_media_id {
        match MediaId::parse(id) {
            Ok(id) => state
                .media_repository
                .find_media(&id)
                .await
                .map_err(WebError::media_repository)?,
            Err(_) => None,
        }
    } else {
        None
    };
    let cover = cover.map(|asset| CoverView {
        original_url: format!("/media/{}", asset.original_filename),
        srcset: asset
            .variants
            .iter()
            .map(|variant| format!("/media/{} {}w", variant.filename, variant.width))
            .collect::<Vec<_>>()
            .join(", "),
        alt_text: asset.alt_text,
        width: asset.width,
        height: asset.height,
    });
    let og_image = cover
        .as_ref()
        .map(|cover| state.absolute_url(&cover.original_url))
        .transpose()?;
    let mut theme = state
        .theme_context(
            &path,
            title,
            Some(description),
            if content.kind == ContentKind::Post {
                "article"
            } else {
                "website"
            },
            ContentPage::from_content(&content, cover),
        )
        .await?;
    theme.meta.image_url = og_image;
    let mut response = state.render_html("public/content.html", theme)?;
    response.headers_mut().insert(
        header::ETAG,
        HeaderValue::from_str(&etag).map_err(WebError::header)?,
    );
    let modified: SystemTime = content.updated_at.into();
    response.headers_mut().insert(
        header::LAST_MODIFIED,
        HeaderValue::from_str(&httpdate::fmt_http_date(modified)).map_err(WebError::header)?,
    );
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("public, max-age=0, must-revalidate"),
    );
    Ok(response)
}

pub async fn canonical_archive() -> Response {
    canonical_redirect("/archive/")
}

pub async fn archive(State(state): State<AppState>) -> Result<Response, WebError> {
    archive_response(state, "/archive/", "Archive".into(), None).await
}

pub async fn canonical_tag(Path(slug): Path<String>) -> Response {
    canonical_redirect(&format!("/tag/{slug}/"))
}

pub async fn tag(
    State(state): State<AppState>,
    Path(raw_slug): Path<String>,
) -> Result<Response, WebError> {
    let Ok(slug) = Slug::parse(&raw_slug) else {
        return not_found(State(state)).await;
    };
    archive_response(
        state,
        &format!("/tag/{slug}/"),
        format!("#{slug}"),
        Some(slug),
    )
    .await
}

async fn archive_response(
    state: AppState,
    path: &str,
    heading: String,
    tag: Option<Slug>,
) -> Result<Response, WebError> {
    let now = state.clock.now();
    let posts = if let Some(tag) = tag {
        state.content.list_public_by_tag(&tag, now).await?
    } else {
        state.content.list_public_posts(now, 100, 0).await?
    };
    let context = state
        .theme_context(
            path,
            MetaTitle::Page(heading.clone()),
            Some(format!("Published writing in {heading}")),
            "website",
            ArchivePage {
                heading,
                posts: posts.into_iter().map(ContentCard::from).collect(),
            },
        )
        .await?;
    state.render_html("public/archive.html", context)
}

pub async fn feed(State(state): State<AppState>) -> Result<Response, WebError> {
    let now = state.clock.now();
    let settings = state.site.site_settings().await?;
    let posts = state.content.list_public_posts(now, 50, 0).await?;
    let updated = posts
        .first()
        .map_or(now, |post| post.updated_at)
        .to_rfc3339_opts(SecondsFormat::Secs, true);
    let site_url = state.absolute_url("/")?;
    let entries = posts
        .into_iter()
        .map(|post| {
            let publish_at = post.publication.publish_at().unwrap_or(post.created_at);
            Ok(FeedEntry {
                title: post.title,
                url: state.absolute_url(&format!("/{}/", post.slug))?,
                published: publish_at.to_rfc3339_opts(SecondsFormat::Secs, true),
                updated: post.updated_at.to_rfc3339_opts(SecondsFormat::Secs, true),
                summary: post.summary,
            })
        })
        .collect::<Result<Vec<_>, WebError>>()?;
    let xml = state.templates.render(
        "feed.xml",
        FeedContext {
            site_title: settings.site_title,
            site_url,
            feed_url: state.absolute_url("/feed.xml")?,
            updated,
            entries,
        },
    )?;
    Ok(xml_response(xml, "application/atom+xml; charset=utf-8"))
}

pub async fn sitemap(State(state): State<AppState>) -> Result<Response, WebError> {
    let contents = state.content.list_all_public(state.clock.now()).await?;
    let entries = contents
        .into_iter()
        .map(|content| {
            Ok(SitemapEntry {
                url: state.absolute_url(&format!("/{}/", content.slug))?,
                updated: content.updated_at.format("%Y-%m-%d").to_string(),
            })
        })
        .collect::<Result<Vec<_>, WebError>>()?;
    let xml = state.templates.render(
        "sitemap.xml",
        SitemapContext {
            site_url: state.absolute_url("/")?,
            entries,
        },
    )?;
    Ok(xml_response(xml, "application/xml; charset=utf-8"))
}

pub async fn robots(State(state): State<AppState>) -> Result<Response, WebError> {
    let body = format!(
        "User-agent: *\nAllow: /\nDisallow: /admin/\nSitemap: {}\n",
        state.absolute_url("/sitemap.xml")?
    );
    let mut response = body.into_response();
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/plain; charset=utf-8"),
    );
    Ok(response)
}

pub async fn theme_css() -> Response {
    let mut response = include_str!("../../static/theme.css").into_response();
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/css; charset=utf-8"),
    );
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("public, max-age=86400"),
    );
    response
}

pub async fn media_file(
    State(state): State<AppState>,
    Path(filename): Path<String>,
    headers: HeaderMap,
) -> Result<Response, WebError> {
    if filename.len() > 160
        || !filename.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'.' | b'w')
        })
    {
        return Ok(StatusCode::NOT_FOUND.into_response());
    }
    let media = state
        .media_repository
        .list_media()
        .await
        .map_err(WebError::media_repository)?;
    let mime = media.iter().find_map(|asset| {
        if asset.original_filename == filename {
            Some(asset.mime_type.as_str())
        } else if asset
            .variants
            .iter()
            .any(|variant| variant.filename == filename)
        {
            Some("image/webp")
        } else {
            None
        }
    });
    let Some(mime) = mime else {
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
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_str(mime).map_err(WebError::header)?,
    );
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

pub async fn health() -> Response {
    (
        [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
        "ok\n",
    )
        .into_response()
}

pub async fn not_found(State(state): State<AppState>) -> Result<Response, WebError> {
    let context = state
        .theme_context(
            "/404/",
            MetaTitle::Page("Page not found".into()),
            Some("The requested page could not be found.".into()),
            "website",
            (),
        )
        .await?;
    let mut response = state.render_html("public/not_found.html", context)?;
    *response.status_mut() = StatusCode::NOT_FOUND;
    Ok(response)
}

impl From<Content> for ContentCard {
    fn from(content: Content) -> Self {
        let published = content
            .publication
            .publish_at()
            .unwrap_or(content.created_at);
        Self {
            title: content.title,
            slug: content.slug.to_string(),
            summary: content.summary,
            publish_at: published.to_rfc3339_opts(SecondsFormat::Secs, true),
            date: published.format("%Y-%m-%d").to_string(),
            tags: content.tags,
        }
    }
}

impl ContentPage {
    fn from_content(content: &Content, cover: Option<CoverView>) -> Self {
        let published = match content.publication {
            Publication::Public { publish_at } => Some(publish_at),
            Publication::Draft => None,
        };
        Self {
            kind: content.kind.as_str(),
            title: content.title.clone(),
            summary: content.summary.clone(),
            body_html: content.body_html.clone(),
            publish_at: published.map(|date| date.to_rfc3339_opts(SecondsFormat::Secs, true)),
            date: published.map(|date| date.format("%Y-%m-%d").to_string()),
            tags: content.tags.clone(),
            cover,
        }
    }
}

fn canonical_redirect(location: &str) -> Response {
    redirect(StatusCode::PERMANENT_REDIRECT, location)
}

fn redirect(status: StatusCode, location: &str) -> Response {
    let mut response = status.into_response();
    if let Ok(value) = HeaderValue::from_str(location) {
        response.headers_mut().insert(header::LOCATION, value);
    }
    response
}

fn xml_response(xml: String, content_type: &'static str) -> Response {
    let mut response = xml.into_response();
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
    response
}

pub(super) fn meta(
    settings: &SiteSettings,
    canonical_url: String,
    title: Option<String>,
    description: Option<String>,
    og_type: &str,
) -> PageMeta {
    PageMeta {
        title: title.unwrap_or_else(|| settings.site_title.clone()),
        description: description.unwrap_or_else(|| settings.site_description.clone()),
        canonical_url,
        og_type: og_type.to_owned(),
        image_url: None,
    }
}
