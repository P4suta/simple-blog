//! Pure public-site compilation shared by every host adapter.

use std::collections::{BTreeMap, HashMap};

use chrono::{DateTime, SecondsFormat, Utc};
use serde::Serialize;
use thiserror::Error;

use crate::{
    application::{
        ports::ContentLink,
        static_search::{StaticSearchDocument, StaticSearchIndexV1},
        templates::{TemplateError, Templates},
    },
    domain::{
        content::{Content, ContentKind, Slug, Tag},
        media::MediaAsset,
        search,
        theme::{NavigationItem, PageMeta, SiteSettings, ThemeAssets, ThemeContext},
    },
    i18n::{TranslationError, Translations},
    release::{PreparedRelease, ReleaseBuilder, ReleaseError, ReleaseManifest},
};

const LIKE_JS: &str = include_str!("../../static/like.js");
const PREFS_JS: &str = include_str!("../../static/prefs.js");
const SEARCH_JS: &str = include_str!("../../static/search.js");

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PublicRedirect {
    pub from: Slug,
    pub to: Slug,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SiteSnapshotV1 {
    pub public_revision: u64,
    pub effective_at: DateTime<Utc>,
    pub settings: SiteSettings,
    pub navigation: Vec<NavigationItem>,
    pub contents: Vec<Content>,
    pub redirects: Vec<PublicRedirect>,
    pub media: Vec<MediaAsset>,
}

#[derive(Clone)]
pub struct SiteCompiler {
    templates: Templates,
    translations: Translations,
}

impl SiteCompiler {
    pub fn embedded() -> Result<Self, SiteCompilerError> {
        Ok(Self {
            templates: Templates::embedded()?,
            translations: Translations::embedded()?,
        })
    }

    #[tracing::instrument(
        name = "site.compile",
        skip_all,
        fields(
            public_revision = snapshot.public_revision,
            content_candidates = snapshot.contents.len(),
            incremental = previous.is_some()
        )
    )]
    pub fn compile(
        &self,
        snapshot: &SiteSnapshotV1,
        canonical_origin: &str,
        previous: Option<&ReleaseManifest>,
    ) -> Result<PreparedRelease, SiteCompilerError> {
        tracing::info!(event = "site.compile.started");
        let mut public = snapshot
            .contents
            .iter()
            .filter(|content| content.publication.is_visible_at(snapshot.effective_at))
            .cloned()
            .collect::<Vec<_>>();
        public.sort_by(public_order);
        let posts = public
            .iter()
            .filter(|content| content.kind == ContentKind::Post)
            .cloned()
            .collect::<Vec<_>>();
        let media = snapshot
            .media
            .iter()
            .map(|asset| (asset.id.as_str(), asset))
            .collect::<HashMap<_, _>>();

        let mut builder = match previous {
            Some(previous) => {
                ReleaseBuilder::incremental(snapshot.public_revision, canonical_origin, previous)?
            }
            None => ReleaseBuilder::clean(snapshot.public_revision, canonical_origin)?,
        };
        let origin = builder.canonical_origin().to_owned();

        // Historical redirects are installed first; canonical content routes
        // below take precedence if corrupt imported data ever overlaps them.
        let mut redirects = snapshot.redirects.clone();
        redirects.sort_by(|left, right| left.from.as_str().cmp(right.from.as_str()));
        for redirect in redirects {
            builder = builder
                .redirect(
                    &format!("/{}/", redirect.from),
                    &format!("/{}/", redirect.to),
                    301,
                )?
                .redirect(
                    &format!("/{}", redirect.from),
                    &format!("/{}/", redirect.from),
                    308,
                )?;
        }

        builder = self.add_home(builder, snapshot, &origin, &posts, &media)?;
        builder = self.add_archives(builder, snapshot, &origin, &public, &posts, &media)?;
        builder = self.add_contents(builder, snapshot, &origin, &public, &posts, &media)?;
        builder = self.add_machine_files(builder, snapshot, &origin, &public, &posts)?;
        builder = self.add_static_assets(builder, snapshot, &origin, &media)?;

        let release = builder.finish()?;
        let pruned_route_count = previous.map_or(0, |previous| {
            previous
                .routes
                .keys()
                .filter(|path| !release.manifest.routes.contains_key(*path))
                .count()
        });
        tracing::info!(
            event = "site.compile.completed",
            release_id = %release.id,
            route_count = release.manifest.routes.len(),
            staged_object_count = release.objects.len(),
            pruned_route_count
        );
        Ok(release)
    }

    fn add_home(
        &self,
        builder: ReleaseBuilder,
        snapshot: &SiteSnapshotV1,
        origin: &str,
        posts: &[Content],
        media: &HashMap<&str, &MediaAsset>,
    ) -> Result<ReleaseBuilder, SiteCompilerError> {
        let context = self.theme_context(
            snapshot,
            origin,
            media,
            PagePresentation {
                path: "/",
                title: MetaTitle::Site,
                description: None,
                og_type: "website",
            },
            CardsPage {
                posts: posts.iter().take(25).map(ContentCard::from).collect(),
            },
        );
        let html = self.templates.render("public/home.html", context)?;
        Ok(builder.asset("/", html.into_bytes(), "text/html; charset=utf-8", None)?)
    }

    fn add_archives(
        &self,
        mut builder: ReleaseBuilder,
        snapshot: &SiteSnapshotV1,
        origin: &str,
        public: &[Content],
        posts: &[Content],
        media: &HashMap<&str, &MediaAsset>,
    ) -> Result<ReleaseBuilder, SiteCompilerError> {
        builder = builder.redirect("/archive", "/archive/", 308)?;
        let archive_heading = self
            .translations
            .text(snapshot.settings.locale, "public.archive");
        builder = self.add_archive_page(
            builder,
            snapshot,
            origin,
            media,
            ArchivePresentation {
                path: "/archive/",
                heading: archive_heading,
            },
            posts.iter(),
        )?;

        let mut tags: BTreeMap<String, (String, Vec<&Content>)> = BTreeMap::new();
        for content in public {
            for tag in &content.tags {
                let entry = tags
                    .entry(tag.slug.to_string())
                    .or_insert_with(|| (tag.name.clone(), Vec::new()));
                entry.1.push(content);
            }
        }
        for (slug, (name, contents)) in tags {
            let path = format!("/tag/{slug}/");
            builder = builder.redirect(&format!("/tag/{slug}"), &path, 308)?;
            builder = self.add_archive_page(
                builder,
                snapshot,
                origin,
                media,
                ArchivePresentation {
                    path: &path,
                    heading: format!("#{name}"),
                },
                contents.into_iter(),
            )?;
        }
        Ok(builder)
    }

    fn add_archive_page<'a>(
        &self,
        builder: ReleaseBuilder,
        snapshot: &SiteSnapshotV1,
        origin: &str,
        media: &HashMap<&str, &MediaAsset>,
        presentation: ArchivePresentation<'_>,
        contents: impl Iterator<Item = &'a Content>,
    ) -> Result<ReleaseBuilder, SiteCompilerError> {
        let ArchivePresentation { path, heading } = presentation;
        let mut years: Vec<ArchiveYear> = Vec::new();
        for content in contents {
            let card = ContentCard::from(content);
            let year = card.date.get(..4).unwrap_or("").to_owned();
            match years.last_mut() {
                Some(group) if group.year == year => group.posts.push(card),
                _ => years.push(ArchiveYear {
                    year,
                    posts: vec![card],
                }),
            }
        }
        let context = self.theme_context(
            snapshot,
            origin,
            media,
            PagePresentation {
                path,
                title: MetaTitle::Page(heading.clone()),
                description: Some(format!("Published writing in {heading}")),
                og_type: "website",
            },
            ArchivePage { heading, years },
        );
        let html = self.templates.render("public/archive.html", context)?;
        Ok(builder.asset(path, html.into_bytes(), "text/html; charset=utf-8", None)?)
    }

    fn add_contents(
        &self,
        mut builder: ReleaseBuilder,
        snapshot: &SiteSnapshotV1,
        origin: &str,
        public: &[Content],
        posts: &[Content],
        media: &HashMap<&str, &MediaAsset>,
    ) -> Result<ReleaseBuilder, SiteCompilerError> {
        let post_positions = posts
            .iter()
            .enumerate()
            .map(|(index, post)| (post.id.as_i64(), index))
            .collect::<HashMap<_, _>>();
        for content in public {
            let (older, newer) =
                post_positions
                    .get(&content.id.as_i64())
                    .map_or((None, None), |index| {
                        let newer = index.checked_sub(1).and_then(|at| posts.get(at));
                        let older = posts.get(index + 1);
                        (older.map(content_link), newer.map(content_link))
                    });
            let cover = content
                .cover_media_id
                .as_deref()
                .and_then(|id| media.get(id).copied())
                .map(CoverView::from);
            let path = format!("/{}/", content.slug);
            let title = content.seo_title.clone().map_or_else(
                || MetaTitle::Page(content.title.clone()),
                MetaTitle::Override,
            );
            let description = content
                .seo_description
                .clone()
                .unwrap_or_else(|| content.summary.clone());
            let mut context = self.theme_context(
                snapshot,
                origin,
                media,
                PagePresentation {
                    path: &path,
                    title,
                    description: Some(description),
                    og_type: if content.kind == ContentKind::Post {
                        "article"
                    } else {
                        "website"
                    },
                },
                ContentPage::from_content(content, cover.clone(), older, newer),
            );
            context.meta.image_url = cover.map(|cover| format!("{origin}{}", cover.original_url));
            let html = self.templates.render("public/content.html", context)?;
            builder = builder
                .asset_with_metadata(
                    &path,
                    html.into_bytes(),
                    "text/html; charset=utf-8",
                    Some(content.id.as_i64()),
                    200,
                    Some(content.updated_at),
                )?
                .redirect(&format!("/{}", content.slug), &path, 308)?;
        }
        Ok(builder)
    }

    fn add_machine_files(
        &self,
        mut builder: ReleaseBuilder,
        snapshot: &SiteSnapshotV1,
        origin: &str,
        public: &[Content],
        posts: &[Content],
    ) -> Result<ReleaseBuilder, SiteCompilerError> {
        let updated = posts
            .iter()
            .map(|post| post.updated_at)
            .max()
            .unwrap_or(snapshot.effective_at);
        let entries = posts
            .iter()
            .take(50)
            .map(|post| {
                let published = post.publication.publish_at().unwrap_or(post.created_at);
                FeedEntry {
                    title: post.title.clone(),
                    url: format!("{origin}/{}/", post.slug),
                    published: published.to_rfc3339_opts(SecondsFormat::Secs, true),
                    updated: post.updated_at.to_rfc3339_opts(SecondsFormat::Secs, true),
                    summary: post.summary.clone(),
                    content_html: post.body_html.clone(),
                }
            })
            .collect();
        let feed = self.templates.render(
            "feed.xml",
            FeedContext {
                site_title: snapshot.settings.site_title.clone(),
                site_url: format!("{origin}/"),
                feed_url: format!("{origin}/feed.xml"),
                updated: updated.to_rfc3339_opts(SecondsFormat::Secs, true),
                entries,
            },
        )?;
        builder = builder.asset(
            "/feed.xml",
            feed.into_bytes(),
            "application/atom+xml; charset=utf-8",
            None,
        )?;

        let sitemap = self.templates.render(
            "sitemap.xml",
            SitemapContext {
                site_url: format!("{origin}/"),
                entries: public
                    .iter()
                    .map(|content| SitemapEntry {
                        url: format!("{origin}/{}/", content.slug),
                        updated: content.updated_at.format("%Y-%m-%d").to_string(),
                    })
                    .collect(),
            },
        )?;
        builder = builder.asset(
            "/sitemap.xml",
            sitemap.into_bytes(),
            "application/xml; charset=utf-8",
            None,
        )?;

        let robots =
            format!("User-agent: *\nAllow: /\nDisallow: /admin/\nSitemap: {origin}/sitemap.xml\n");
        builder = builder.asset(
            "/robots.txt",
            robots.into_bytes(),
            "text/plain; charset=utf-8",
            None,
        )?;

        let documents = public
            .iter()
            .map(|content| {
                StaticSearchDocument::new(
                    content.id.as_i64(),
                    content.slug.as_str(),
                    &content.title,
                    &content.summary,
                    &search::html_to_text(&content.body_html),
                    &content
                        .publication
                        .publish_at()
                        .unwrap_or(content.created_at)
                        .format("%Y-%m-%d")
                        .to_string(),
                )
            })
            .collect::<Vec<_>>();
        let index = StaticSearchIndexV1::new(documents)
            .canonical_bytes()
            .map_err(|error| SiteCompilerError::SearchIndex(error.to_string()))?;
        builder = builder.asset(
            "/assets/search-index.json",
            index,
            "application/json; charset=utf-8",
            None,
        )?;
        Ok(builder)
    }

    fn add_static_assets(
        &self,
        mut builder: ReleaseBuilder,
        snapshot: &SiteSnapshotV1,
        origin: &str,
        media: &HashMap<&str, &MediaAsset>,
    ) -> Result<ReleaseBuilder, SiteCompilerError> {
        builder = builder
            .asset(
                "/assets/site.css",
                snapshot.settings.custom_css.as_bytes().to_vec(),
                "text/css; charset=utf-8",
                None,
            )?
            .asset(
                "/assets/like.js",
                LIKE_JS.as_bytes().to_vec(),
                "text/javascript; charset=utf-8",
                None,
            )?
            .asset(
                "/assets/prefs.js",
                PREFS_JS.as_bytes().to_vec(),
                "text/javascript; charset=utf-8",
                None,
            )?
            .asset(
                "/assets/search.js",
                SEARCH_JS.as_bytes().to_vec(),
                "text/javascript; charset=utf-8",
                None,
            )?
            .redirect("/search", "/search/", 308)?;

        let search_context = self.theme_context(
            snapshot,
            origin,
            media,
            PagePresentation {
                path: "/search/",
                title: MetaTitle::Page(
                    self.translations
                        .text(snapshot.settings.locale, "public.search"),
                ),
                description: None,
                og_type: "website",
            },
            StaticSearchPage {
                query: "",
                searched: false,
                results: Vec::<StaticSearchResult>::new(),
                static_search: true,
                search_js_version: fingerprint(SEARCH_JS),
            },
        );
        let search = self
            .templates
            .render("public/search.html", search_context)?;
        builder = builder.asset(
            "/search/",
            search.into_bytes(),
            "text/html; charset=utf-8",
            None,
        )?;

        let not_found_context = self.theme_context(
            snapshot,
            origin,
            media,
            PagePresentation {
                path: "/404/",
                title: MetaTitle::Page("Page not found".into()),
                description: Some("The requested page could not be found.".into()),
                og_type: "website",
            },
            (),
        );
        let not_found = self
            .templates
            .render("public/not_found.html", not_found_context)?;
        Ok(builder.asset_with_metadata(
            "/404/",
            not_found.into_bytes(),
            "text/html; charset=utf-8",
            None,
            404,
            None,
        )?)
    }

    fn theme_context<T>(
        &self,
        snapshot: &SiteSnapshotV1,
        origin: &str,
        media: &HashMap<&str, &MediaAsset>,
        presentation: PagePresentation<'_>,
        page: T,
    ) -> ThemeContext<T> {
        let PagePresentation {
            path,
            title,
            description,
            og_type,
        } = presentation;
        let title = match title {
            MetaTitle::Site => snapshot.settings.site_title.clone(),
            MetaTitle::Page(title) => format!("{title} — {}", snapshot.settings.site_title),
            MetaTitle::Override(title) => title,
        };
        let media_url = |id: Option<&str>| {
            id.and_then(|id| media.get(id))
                .map(|asset| format!("/media/{}", asset.original_filename))
        };
        ThemeContext {
            t: self
                .translations
                .for_locale(snapshot.settings.locale)
                .clone(),
            site: snapshot.settings.clone(),
            assets: ThemeAssets {
                logo_url: media_url(snapshot.settings.logo_media_id.as_deref()),
                favicon_url: media_url(snapshot.settings.favicon_media_id.as_deref()),
                css_version: fingerprint(&snapshot.settings.custom_css),
                prefs_js_version: fingerprint(PREFS_JS),
            },
            navigation: snapshot.navigation.clone(),
            meta: PageMeta {
                title,
                description: description
                    .unwrap_or_else(|| snapshot.settings.site_description.clone()),
                canonical_url: format!("{origin}{path}"),
                og_type: og_type.to_owned(),
                image_url: None,
            },
            page,
        }
    }
}

#[derive(Debug, Error)]
pub enum SiteCompilerError {
    #[error(transparent)]
    Template(#[from] TemplateError),
    #[error(transparent)]
    Translation(#[from] TranslationError),
    #[error(transparent)]
    Release(#[from] ReleaseError),
    #[error("search index generation failed: {0}")]
    SearchIndex(String),
}

#[derive(Clone)]
enum MetaTitle {
    Site,
    Page(String),
    Override(String),
}

struct PagePresentation<'a> {
    path: &'a str,
    title: MetaTitle,
    description: Option<String>,
    og_type: &'a str,
}

struct ArchivePresentation<'a> {
    path: &'a str,
    heading: String,
}

#[derive(Serialize)]
struct CardsPage {
    posts: Vec<ContentCard>,
}

#[derive(Serialize)]
struct ArchivePage {
    heading: String,
    years: Vec<ArchiveYear>,
}

#[derive(Serialize)]
struct ArchiveYear {
    year: String,
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

impl From<&Content> for ContentCard {
    fn from(content: &Content) -> Self {
        let published = content
            .publication
            .publish_at()
            .unwrap_or(content.created_at);
        Self {
            title: content.title.clone(),
            slug: content.slug.to_string(),
            summary: content.summary.clone(),
            publish_at: published.to_rfc3339_opts(SecondsFormat::Secs, true),
            date: published.format("%Y-%m-%d").to_string(),
            tags: content.tags.clone(),
        }
    }
}

#[derive(Serialize)]
struct ContentPage {
    id: i64,
    kind: &'static str,
    title: String,
    summary: String,
    body_html: String,
    publish_at: Option<String>,
    date: Option<String>,
    tags: Vec<Tag>,
    cover: Option<CoverView>,
    like_js_version: String,
    older: Option<NeighborView>,
    newer: Option<NeighborView>,
}

impl ContentPage {
    fn from_content(
        content: &Content,
        cover: Option<CoverView>,
        older: Option<ContentLink>,
        newer: Option<ContentLink>,
    ) -> Self {
        let published = content.publication.publish_at();
        Self {
            id: content.id.as_i64(),
            kind: content.kind.as_str(),
            title: content.title.clone(),
            summary: content.summary.clone(),
            body_html: content.body_html.clone(),
            publish_at: published.map(|date| date.to_rfc3339_opts(SecondsFormat::Secs, true)),
            date: published.map(|date| date.format("%Y-%m-%d").to_string()),
            tags: content.tags.clone(),
            cover,
            like_js_version: fingerprint(LIKE_JS),
            older: older.map(NeighborView::from),
            newer: newer.map(NeighborView::from),
        }
    }
}

#[derive(Clone, Serialize)]
struct CoverView {
    original_url: String,
    srcset: String,
    alt_text: String,
    width: u32,
    height: u32,
}

impl From<&MediaAsset> for CoverView {
    fn from(asset: &MediaAsset) -> Self {
        Self {
            original_url: format!("/media/{}", asset.original_filename),
            srcset: asset
                .variants
                .iter()
                .map(|variant| format!("/media/{} {}w", variant.filename, variant.width))
                .collect::<Vec<_>>()
                .join(", "),
            alt_text: asset.alt_text.clone(),
            width: asset.width,
            height: asset.height,
        }
    }
}

#[derive(Serialize)]
struct NeighborView {
    slug: String,
    title: String,
}

impl From<ContentLink> for NeighborView {
    fn from(link: ContentLink) -> Self {
        Self {
            slug: link.slug.to_string(),
            title: link.title,
        }
    }
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
    content_html: String,
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

#[derive(Serialize)]
struct StaticSearchPage<'a> {
    query: &'a str,
    searched: bool,
    results: Vec<StaticSearchResult>,
    static_search: bool,
    search_js_version: String,
}

#[derive(Serialize)]
struct StaticSearchResult;

fn public_order(left: &Content, right: &Content) -> std::cmp::Ordering {
    let left_at = left.publication.publish_at().unwrap_or(left.created_at);
    let right_at = right.publication.publish_at().unwrap_or(right.created_at);
    right_at
        .cmp(&left_at)
        .then_with(|| right.id.as_i64().cmp(&left.id.as_i64()))
}

fn content_link(content: &Content) -> ContentLink {
    ContentLink {
        id: content.id,
        slug: content.slug.clone(),
        title: content.title.clone(),
    }
}

fn fingerprint(value: &str) -> String {
    blake3::hash(value.as_bytes()).to_hex()[..8].to_owned()
}
