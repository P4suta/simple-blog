//! Pure public-site compilation shared by every host adapter.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use chrono::{
    DateTime, Datelike, Duration, SecondsFormat, Utc,
    format::{Item, StrftimeItems},
};
use chrono_tz::Tz;
use serde::Serialize;
use thiserror::Error;

use crate::{
    application::{
        body_media::decorate_body_media,
        ports::ContentLink,
        static_search::{StaticSearchDocument, StaticSearchIndexV1},
        templates::{TemplateError, Templates},
    },
    domain::{
        content::{Content, ContentKind, Slug, Tag},
        media::MediaAsset,
        reading::{self, OutlineEntry},
        search,
        theme::{
            AlternateFeed, MetaImage, NavigationItem, PageMeta, SiteSettings, ThemeAssets,
            ThemeContext,
        },
    },
    i18n::{TranslationError, Translations},
    release::{PreparedRelease, ReleaseBuilder, ReleaseError, ReleaseManifest},
};

const LIKE_JS: &str = include_str!("../../static/like.js");
const ARTICLE_JS: &str = include_str!("../../static/article.js");
const PREFS_JS: &str = include_str!("../../static/prefs.js");
const SEARCH_JS: &str = include_str!("../../static/search.js");

/// Posts per home page; older pages live at `/page/N/`.
const HOME_PAGE_SIZE: usize = 20;
/// Entries in every Atom feed.
const FEED_ENTRY_LIMIT: usize = 50;
/// A table of contents appears only once it can orient the reader.
const OUTLINE_MIN_HEADINGS: usize = 3;
/// An "updated" date is shown only when it says something the publication
/// date does not.
const UPDATED_THRESHOLD_HOURS: i64 = 24;
/// Related posts under an article: those sharing the most tags, newest first.
const RELATED_LIMIT: usize = 3;
const JSON_FEED_VERSION: &str = "https://jsonfeed.org/version/1.1";

/// Renders instants the way the site's readers expect: in the site's zone,
/// with the patterns its language uses. Built once per compile from the
/// settings and the catalogs, so a broken pattern fails the build instead of
/// panicking inside a template.
struct SiteDates {
    zone: Tz,
    long: Vec<Item<'static>>,
    short: Vec<Item<'static>>,
    year: Vec<Item<'static>>,
}

impl SiteDates {
    fn new(
        translations: &Translations,
        settings: &SiteSettings,
    ) -> Result<Self, SiteCompilerError> {
        let locale = settings.locale;
        let pattern = |key: &'static str| {
            StrftimeItems::new(&translations.text(locale, key))
                .parse_to_owned()
                .map_err(|_| SiteCompilerError::DatePattern(key))
        };
        Ok(Self {
            zone: settings.time_zone(),
            long: pattern("public.date_long")?,
            short: pattern("public.date_short")?,
            year: pattern("public.year")?,
        })
    }

    fn local(&self, at: DateTime<Utc>) -> DateTime<Tz> {
        at.with_timezone(&self.zone)
    }

    fn render(&self, items: &[Item<'static>], at: DateTime<Utc>) -> String {
        self.local(at).format_with_items(items.iter()).to_string()
    }

    /// The full date on a page: "September 3, 2026", 2026年9月3日.
    fn long(&self, at: DateTime<Utc>) -> String {
        self.render(&self.long, at)
    }

    /// The date inside a year group: "Sep 3", 9月3日.
    fn short(&self, at: DateTime<Utc>) -> String {
        self.render(&self.short, at)
    }

    fn year_label(&self, at: DateTime<Utc>) -> String {
        self.render(&self.year, at)
    }

    fn year(&self, at: DateTime<Utc>) -> i32 {
        self.local(at).year()
    }

    /// `YYYY-MM-DD` in the site zone, for sitemaps.
    fn day(&self, at: DateTime<Utc>) -> String {
        self.local(at).format("%Y-%m-%d").to_string()
    }

    /// RFC 3339 with the site's offset; `Z` when that offset is zero.
    fn iso(&self, at: DateTime<Utc>) -> String {
        self.local(at).to_rfc3339_opts(SecondsFormat::Secs, true)
    }
}

/// Where a preview loads its stylesheet and reader-preferences script from:
/// the live admin copies rather than the fingerprinted release assets.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PreviewAssets {
    pub css_url: String,
    pub prefs_js_url: String,
}

impl PreviewAssets {
    fn apply(self, assets: &mut ThemeAssets, meta: &mut PageMeta) {
        assets.css_url = self.css_url;
        assets.prefs_js_url = self.prefs_js_url;
        meta.preview = true;
        meta.noindex = true;
    }
}

fn media_lookup(snapshot: &SiteSnapshotV1) -> HashMap<&str, &MediaAsset> {
    snapshot
        .media
        .iter()
        .map(|asset| (asset.id.as_str(), asset))
        .collect()
}

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

/// What every page of one compile shares: the snapshot, the canonical
/// origin, the media lookup, and the date formatter.
#[derive(Clone, Copy)]
struct Scope<'a> {
    snapshot: &'a SiteSnapshotV1,
    origin: &'a str,
    media: &'a HashMap<&'a str, &'a MediaAsset>,
    dates: &'a SiteDates,
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
        let media = media_lookup(snapshot);

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

        let tags = tag_groups(&public);
        let dates = SiteDates::new(&self.translations, &snapshot.settings)?;
        let search_index = Self::search_index(&public, &dates)?;
        let search_index_version = fingerprint(&search_index);
        let scope = Scope {
            snapshot,
            origin: &origin,
            media: &media,
            dates: &dates,
        };
        builder = self.add_home(builder, &scope, &posts)?;
        builder = self.add_archives(builder, &scope, &posts, &tags)?;
        builder = self.add_contents(builder, &scope, &public, &posts)?;
        builder = self.add_machine_files(builder, &scope, &public, &posts, &tags)?;
        builder = builder.asset(
            "/assets/search-index.json",
            search_index,
            "application/json; charset=utf-8",
            None,
        )?;
        builder = self.add_static_assets(builder, &scope, &search_index_version)?;

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

    /// One piece through the public templates with the live stylesheet, as the
    /// owner (or a preview-link holder) sees it before publication. Neighbours
    /// and reader interactions are left out; the page is marked `noindex`.
    pub fn render_content_preview(
        &self,
        snapshot: &SiteSnapshotV1,
        content: &Content,
        origin: &str,
        assets: PreviewAssets,
    ) -> Result<String, SiteCompilerError> {
        let media = media_lookup(snapshot);
        let dates = SiteDates::new(&self.translations, &snapshot.settings)?;
        let scope = Scope {
            snapshot,
            origin,
            media: &media,
            dates: &dates,
        };
        let mut context = self.content_context(&scope, content, None, None, Vec::new());
        assets.apply(&mut context.assets, &mut context.meta);
        Ok(self.templates.render("public/content.html", context)?)
    }

    /// The first home page with the live stylesheet, for the theme editor.
    pub fn render_home_preview(
        &self,
        snapshot: &SiteSnapshotV1,
        origin: &str,
        assets: PreviewAssets,
    ) -> Result<String, SiteCompilerError> {
        let mut public = snapshot
            .contents
            .iter()
            .filter(|content| content.publication.is_visible_at(snapshot.effective_at))
            .cloned()
            .collect::<Vec<_>>();
        public.sort_by(public_order);
        let posts = public
            .into_iter()
            .filter(|content| content.kind == ContentKind::Post)
            .collect::<Vec<_>>();
        let media = media_lookup(snapshot);
        let dates = SiteDates::new(&self.translations, &snapshot.settings)?;
        let scope = Scope {
            snapshot,
            origin,
            media: &media,
            dates: &dates,
        };
        let page_count = posts.len().div_ceil(HOME_PAGE_SIZE).max(1);
        let mut context = self.home_context(&scope, &posts, 1, page_count);
        assets.apply(&mut context.assets, &mut context.meta);
        Ok(self.templates.render("public/home.html", context)?)
    }

    fn add_home(
        &self,
        mut builder: ReleaseBuilder,
        scope: &Scope<'_>,
        posts: &[Content],
    ) -> Result<ReleaseBuilder, SiteCompilerError> {
        let page_count = posts.len().div_ceil(HOME_PAGE_SIZE).max(1);
        for number in 1..=page_count {
            let path = home_path(number);
            let context = self.home_context(scope, posts, number, page_count);
            let html = self.templates.render("public/home.html", context)?;
            builder = builder.asset(&path, html.into_bytes(), "text/html; charset=utf-8", None)?;
            if number > 1 {
                builder = builder.redirect(&format!("/page/{number}"), &path, 308)?;
            }
        }
        // The first page is the home page; its numbered address is a courtesy.
        builder = builder
            .redirect("/page/1/", "/", 308)?
            .redirect("/page/1", "/", 308)?;
        Ok(builder)
    }

    /// One page of the home list: twenty cards and the pager around them.
    fn home_context(
        &self,
        scope: &Scope<'_>,
        posts: &[Content],
        number: usize,
        page_count: usize,
    ) -> ThemeContext<CardsPage> {
        let Scope {
            snapshot,
            origin,
            dates,
            ..
        } = *scope;
        let locale = snapshot.settings.locale;
        let start = (number - 1) * HOME_PAGE_SIZE;
        let end = (start + HOME_PAGE_SIZE).min(posts.len());
        let path = home_path(number);
        let newer_url = match number {
            1 => None,
            2 => Some("/".to_owned()),
            other => Some(home_path(other - 1)),
        };
        let older_url = (number < page_count).then(|| home_path(number + 1));
        let number_text = number.to_string();
        let count_text = page_count.to_string();
        let title = if number == 1 {
            MetaTitle::Site
        } else {
            MetaTitle::Page(self.translations.format(
                locale,
                "public.page_number",
                &[("number", &number_text)],
            ))
        };
        let mut context = self.theme_context(
            scope,
            PagePresentation {
                path: &path,
                title,
                description: None,
                og_type: "website",
            },
            CardsPage {
                posts: posts[start..end]
                    .iter()
                    .map(|post| ContentCard::new(post, dates))
                    .collect(),
                pager: PagerView {
                    number,
                    count: page_count,
                    newer_url: newer_url.clone(),
                    older_url: older_url.clone(),
                    archive_url: (number == page_count).then(|| "/archive/".to_owned()),
                    status: (page_count > 1).then(|| {
                        self.translations.format(
                            locale,
                            "public.page_of",
                            &[("number", &number_text), ("count", &count_text)],
                        )
                    }),
                },
            },
        );
        context.meta.prev_url = newer_url;
        context.meta.next_url = older_url;
        if number == 1 {
            context.meta.json_ld = Some(site_json_ld(snapshot, origin));
        }
        context
    }

    fn add_archives(
        &self,
        mut builder: ReleaseBuilder,
        scope: &Scope<'_>,
        posts: &[Content],
        tags: &BTreeMap<String, TagGroup<'_>>,
    ) -> Result<ReleaseBuilder, SiteCompilerError> {
        let Scope {
            snapshot, origin, ..
        } = *scope;
        let locale = snapshot.settings.locale;
        builder = builder.redirect("/archive", "/archive/", 308)?;
        builder = self.add_archive_page(
            builder,
            scope,
            ArchivePresentation {
                path: "/archive/",
                heading: self.translations.text(locale, "public.archive"),
                description: self.translations.text(locale, "public.archive_description"),
                feed: None,
            },
            posts.iter(),
        )?;

        builder = self.add_tag_index(builder, scope, tags)?;
        for (slug, group) in tags {
            let path = format!("/tag/{slug}/");
            let feed_href = format!("/tag/{slug}/feed.xml");
            builder = builder.redirect(&format!("/tag/{slug}"), &path, 308)?;
            builder = self.add_archive_page(
                builder,
                scope,
                ArchivePresentation {
                    path: &path,
                    heading: format!("#{}", group.name),
                    description: self.translations.format(
                        locale,
                        "public.tag_description",
                        &[("name", &group.name)],
                    ),
                    feed: Some(AlternateFeed {
                        href: feed_href.clone(),
                        title: self.translations.format(
                            locale,
                            "public.tag_feed",
                            &[("name", &group.name)],
                        ),
                    }),
                },
                group
                    .contents
                    .iter()
                    .copied()
                    .filter(|content| content.kind == ContentKind::Post),
            )?;
            let tag_posts = group
                .contents
                .iter()
                .copied()
                .filter(|content| content.kind == ContentKind::Post)
                .collect::<Vec<_>>();
            let feed = self.render_feed(
                snapshot,
                FeedPresentation {
                    title: format!("{} — #{}", snapshot.settings.site_title, group.name),
                    subtitle: None,
                    page_url: format!("{origin}{path}"),
                    feed_url: format!("{origin}{feed_href}"),
                },
                &tag_posts,
            )?;
            builder = builder.asset(
                &feed_href,
                feed.into_bytes(),
                "application/atom+xml; charset=utf-8",
                None,
            )?;
        }
        Ok(builder)
    }

    fn add_tag_index(
        &self,
        builder: ReleaseBuilder,
        scope: &Scope<'_>,
        tags: &BTreeMap<String, TagGroup<'_>>,
    ) -> Result<ReleaseBuilder, SiteCompilerError> {
        let Scope { snapshot, .. } = *scope;
        let locale = snapshot.settings.locale;
        let mut summaries = tags
            .iter()
            .map(|(slug, group)| TagSummary {
                name: group.name.clone(),
                slug: slug.clone(),
                count: group
                    .contents
                    .iter()
                    .filter(|content| content.kind == ContentKind::Post)
                    .count(),
            })
            .collect::<Vec<_>>();
        summaries.sort_by(|left, right| {
            right
                .count
                .cmp(&left.count)
                .then_with(|| left.name.cmp(&right.name))
        });
        let context = self.theme_context(
            scope,
            PagePresentation {
                path: "/tag/",
                title: MetaTitle::Page(self.translations.text(locale, "public.tags")),
                description: Some(
                    self.translations
                        .text(locale, "public.tag_index_description"),
                ),
                og_type: "website",
            },
            TagIndexPage { tags: summaries },
        );
        let html = self.templates.render("public/tags.html", context)?;
        Ok(builder.redirect("/tag", "/tag/", 308)?.asset(
            "/tag/",
            html.into_bytes(),
            "text/html; charset=utf-8",
            None,
        )?)
    }

    fn add_archive_page<'a>(
        &self,
        builder: ReleaseBuilder,
        scope: &Scope<'_>,
        presentation: ArchivePresentation<'_>,
        contents: impl Iterator<Item = &'a Content>,
    ) -> Result<ReleaseBuilder, SiteCompilerError> {
        let Scope { dates, .. } = *scope;
        let ArchivePresentation {
            path,
            heading,
            description,
            feed,
        } = presentation;
        let mut years: Vec<ArchiveYear> = Vec::new();
        for content in contents {
            let card = ContentCard::new(content, dates);
            let published = card_instant(content);
            let year = dates.year(published);
            match years.last_mut() {
                Some(group) if group.year_number == year => group.posts.push(card),
                _ => years.push(ArchiveYear {
                    year_number: year,
                    year: dates.year_label(published),
                    posts: vec![card],
                }),
            }
        }
        let mut context = self.theme_context(
            scope,
            PagePresentation {
                path,
                title: MetaTitle::Page(heading.clone()),
                description: Some(description),
                og_type: "website",
            },
            ArchivePage { heading, years },
        );
        context.meta.alternate_feed = feed;
        let html = self.templates.render("public/archive.html", context)?;
        Ok(builder.asset(path, html.into_bytes(), "text/html; charset=utf-8", None)?)
    }

    fn add_contents(
        &self,
        mut builder: ReleaseBuilder,
        scope: &Scope<'_>,
        public: &[Content],
        posts: &[Content],
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
            let path = format!("/{}/", content.slug);
            let related = post_positions
                .get(&content.id.as_i64())
                .map_or_else(Vec::new, |index| related_posts(posts, *index, scope.dates));
            let context = self.content_context(scope, content, older, newer, related);
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

    /// Everything one content page carries: the body view plus the metadata
    /// in its head. Shared by the release and the owner preview.
    fn content_context(
        &self,
        scope: &Scope<'_>,
        content: &Content,
        older: Option<ContentLink>,
        newer: Option<ContentLink>,
        related: Vec<RelatedView>,
    ) -> ThemeContext<ContentPage> {
        let Scope {
            snapshot,
            origin,
            media,
            dates,
        } = *scope;
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
        let is_post = content.kind == ContentKind::Post;
        let mut context = self.theme_context(
            scope,
            PagePresentation {
                path: &path,
                title,
                description: Some(description),
                og_type: if is_post { "article" } else { "website" },
            },
            self.content_page(scope, content, cover.clone(), older, newer, related),
        );
        let image = cover.map(|cover| MetaImage {
            url: format!("{origin}{}", cover.original_url),
            width: cover.width,
            height: cover.height,
            alt: cover.alt_text,
        });
        context.meta.image.clone_from(&image);
        if is_post {
            let published = content.publication.publish_at();
            context.meta.published_time = published.map(|at| dates.iso(at));
            context.meta.modified_time = Some(dates.iso(content.updated_at));
            context.meta.article_tags = content.tags.iter().map(|tag| tag.name.clone()).collect();
        }
        context.meta.json_ld = Some(content_json_ld(
            snapshot,
            origin,
            content,
            image.as_ref().map(|image| image.url.as_str()),
            dates,
        ));
        context
    }

    fn content_page(
        &self,
        scope: &Scope<'_>,
        content: &Content,
        cover: Option<CoverView>,
        older: Option<ContentLink>,
        newer: Option<ContentLink>,
        related: Vec<RelatedView>,
    ) -> ContentPage {
        let Scope {
            snapshot,
            media,
            dates,
            ..
        } = *scope;
        let locale = snapshot.settings.locale;
        let published = content.publication.publish_at();
        let updated = published
            .filter(|published| {
                content.updated_at - *published > Duration::hours(UPDATED_THRESHOLD_HOURS)
            })
            .map(|_| DateView::new(dates, content.updated_at));
        let minutes = reading::reading_minutes(&search::html_to_text(&content.body_html));
        let reading_label = (minutes > 0).then(|| {
            self.translations.format(
                locale,
                "public.reading_time",
                &[("minutes", &minutes.to_string())],
            )
        });
        let mut outline = reading::outline(&content.body_html);
        if outline.iter().map(OutlineEntry::size).sum::<usize>() < OUTLINE_MIN_HEADINGS {
            outline.clear();
        }
        ContentPage {
            id: content.id.as_i64(),
            kind: content.kind.as_str(),
            title: content.title.clone(),
            summary: content.summary.clone(),
            body_html: decorate_body_media(&content.body_html, media),
            publish_at: published.map(|at| dates.iso(at)),
            date: published.map(|at| dates.long(at)),
            updated,
            reading_label,
            outline,
            tags: content.tags.clone(),
            cover,
            like_js_version: fingerprint(LIKE_JS),
            article_js_version: fingerprint(ARTICLE_JS),
            older: older.map(NeighborView::from),
            newer: newer.map(NeighborView::from),
            related,
        }
    }

    fn render_feed(
        &self,
        snapshot: &SiteSnapshotV1,
        presentation: FeedPresentation,
        posts: &[&Content],
    ) -> Result<String, SiteCompilerError> {
        let updated = posts
            .iter()
            .map(|post| post.updated_at)
            .max()
            .unwrap_or(snapshot.effective_at);
        let origin = presentation.page_url.trim_end_matches('/').to_owned();
        let entries = posts
            .iter()
            .take(FEED_ENTRY_LIMIT)
            .map(|post| {
                let published = post.publication.publish_at().unwrap_or(post.created_at);
                FeedEntry {
                    title: post.title.clone(),
                    url: format!("{}/{}/", site_origin(&origin), post.slug),
                    published: iso(published),
                    updated: iso(post.updated_at),
                    summary: post.summary.clone(),
                    content_html: post.body_html.clone(),
                    tags: post.tags.clone(),
                }
            })
            .collect();
        Ok(self.templates.render(
            "feed.xml",
            FeedContext {
                title: presentation.title,
                subtitle: presentation.subtitle,
                author: snapshot.settings.author().to_owned(),
                page_url: presentation.page_url,
                feed_url: presentation.feed_url,
                updated: iso(updated),
                entries,
            },
        )?)
    }

    /// The client-side search corpus: every visible piece, folded once here
    /// so the browser never has to.
    fn search_index(public: &[Content], dates: &SiteDates) -> Result<Vec<u8>, SiteCompilerError> {
        let documents = public
            .iter()
            .map(|content| {
                StaticSearchDocument::new(
                    content.id.as_i64(),
                    content.slug.as_str(),
                    &content.title,
                    &content.summary,
                    &search::html_to_text(&content.body_html),
                    &dates.long(card_instant(content)),
                )
            })
            .collect::<Vec<_>>();
        StaticSearchIndexV1::new(documents)
            .canonical_bytes()
            .map_err(|error| SiteCompilerError::SearchIndex(error.to_string()))
    }

    fn add_machine_files(
        &self,
        mut builder: ReleaseBuilder,
        scope: &Scope<'_>,
        public: &[Content],
        posts: &[Content],
        tags: &BTreeMap<String, TagGroup<'_>>,
    ) -> Result<ReleaseBuilder, SiteCompilerError> {
        let Scope {
            snapshot,
            origin,
            dates,
            ..
        } = *scope;
        let post_refs = posts.iter().collect::<Vec<_>>();
        let feed = self.render_feed(
            snapshot,
            FeedPresentation {
                title: snapshot.settings.site_title.clone(),
                subtitle: (!snapshot.settings.site_description.is_empty())
                    .then(|| snapshot.settings.site_description.clone()),
                page_url: format!("{origin}/"),
                feed_url: format!("{origin}/feed.xml"),
            },
            &post_refs,
        )?;
        builder = builder.asset(
            "/feed.xml",
            feed.into_bytes(),
            "application/atom+xml; charset=utf-8",
            None,
        )?;
        builder = builder.asset(
            "/feed.json",
            json_feed(snapshot, origin, posts, dates)?,
            "application/feed+json; charset=utf-8",
            None,
        )?;

        let day = |at: DateTime<Utc>| dates.day(at);
        let latest = public.iter().map(|content| content.updated_at).max();
        let mut entries = Vec::new();
        for number in 2..=posts.len().div_ceil(HOME_PAGE_SIZE) {
            entries.push(SitemapEntry {
                url: format!("{origin}{}", home_path(number)),
                updated: latest.map(day),
            });
        }
        entries.push(SitemapEntry {
            url: format!("{origin}/archive/"),
            updated: latest.map(day),
        });
        entries.push(SitemapEntry {
            url: format!("{origin}/tag/"),
            updated: latest.map(day),
        });
        for (slug, group) in tags {
            entries.push(SitemapEntry {
                url: format!("{origin}/tag/{slug}/"),
                updated: group
                    .contents
                    .iter()
                    .map(|content| content.updated_at)
                    .max()
                    .map(day),
            });
        }
        entries.extend(public.iter().map(|content| SitemapEntry {
            url: format!("{origin}/{}/", content.slug),
            updated: Some(day(content.updated_at)),
        }));
        let sitemap = self.templates.render(
            "sitemap.xml",
            SitemapContext {
                site_url: format!("{origin}/"),
                site_updated: latest.map(day),
                entries,
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

        Ok(builder)
    }

    fn add_static_assets(
        &self,
        mut builder: ReleaseBuilder,
        scope: &Scope<'_>,
        search_index_version: &str,
    ) -> Result<ReleaseBuilder, SiteCompilerError> {
        let Scope { snapshot, .. } = *scope;
        let locale = snapshot.settings.locale;
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
                "/assets/article.js",
                ARTICLE_JS.as_bytes().to_vec(),
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

        let mut search_context = self.theme_context(
            scope,
            PagePresentation {
                path: "/search/",
                title: MetaTitle::Page(self.translations.text(locale, "public.search")),
                description: Some(self.translations.text(locale, "public.search_description")),
                og_type: "website",
            },
            StaticSearchPage {
                query: "",
                searched: false,
                results: Vec::<StaticSearchResult>::new(),
                static_search: true,
                search_js_version: fingerprint(SEARCH_JS),
                // The index is served immutable for a year; a versioned URL is
                // the only thing that keeps a returning reader's search fresh.
                search_index_url: format!("/assets/search-index.json?v={search_index_version}"),
            },
        );
        search_context.meta.noindex = true;
        let search = self
            .templates
            .render("public/search.html", search_context)?;
        builder = builder.asset(
            "/search/",
            search.into_bytes(),
            "text/html; charset=utf-8",
            None,
        )?;

        let mut not_found_context = self.theme_context(
            scope,
            PagePresentation {
                path: "/404/",
                title: MetaTitle::Page(self.translations.text(locale, "public.not_found_title")),
                description: Some(
                    self.translations
                        .text(locale, "public.not_found_description"),
                ),
                og_type: "website",
            },
            (),
        );
        not_found_context.meta.noindex = true;
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
        scope: &Scope<'_>,
        presentation: PagePresentation<'_>,
        page: T,
    ) -> ThemeContext<T> {
        let Scope {
            snapshot,
            origin,
            media,
            ..
        } = *scope;
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
                css_url: format!(
                    "/assets/site.css?v={}",
                    fingerprint(&snapshot.settings.custom_css)
                ),
                prefs_js_url: format!("/assets/prefs.js?v={}", fingerprint(PREFS_JS)),
            },
            navigation: snapshot.navigation.clone(),
            meta: PageMeta {
                title,
                description: description
                    .unwrap_or_else(|| snapshot.settings.site_description.clone()),
                canonical_url: format!("{origin}{path}"),
                og_type: og_type.to_owned(),
                og_locale: snapshot.settings.locale.og_locale().to_owned(),
                image: None,
                noindex: false,
                preview: false,
                published_time: None,
                modified_time: None,
                article_tags: Vec::new(),
                prev_url: None,
                next_url: None,
                alternate_feed: None,
                json_ld: None,
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
    #[error("the date pattern in catalog key {0} is not a valid strftime string")]
    DatePattern(&'static str),
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
    description: String,
    feed: Option<AlternateFeed>,
}

struct FeedPresentation {
    title: String,
    subtitle: Option<String>,
    page_url: String,
    feed_url: String,
}

/// Every public piece filed under one tag, keyed by tag slug for a stable
/// order.
struct TagGroup<'a> {
    name: String,
    contents: Vec<&'a Content>,
}

fn tag_groups(public: &[Content]) -> BTreeMap<String, TagGroup<'_>> {
    let mut groups: BTreeMap<String, TagGroup<'_>> = BTreeMap::new();
    for content in public {
        for tag in &content.tags {
            groups
                .entry(tag.slug.to_string())
                .or_insert_with(|| TagGroup {
                    name: tag.name.clone(),
                    contents: Vec::new(),
                })
                .contents
                .push(content);
        }
    }
    groups
}

fn home_path(number: usize) -> String {
    if number <= 1 {
        "/".to_owned()
    } else {
        format!("/page/{number}/")
    }
}

/// `https://site.example` from any page URL on the site.
fn site_origin(page_url: &str) -> &str {
    let scheme_end = page_url.find("://").map_or(0, |at| at + 3);
    page_url[scheme_end..]
        .find('/')
        .map_or(page_url, |at| &page_url[..scheme_end + at])
}

fn iso(at: DateTime<Utc>) -> String {
    at.to_rfc3339_opts(SecondsFormat::Secs, true)
}

/// Serializes structured data so it can sit inside a `<script>` data block:
/// the characters that could end the element or start a tag are written as
/// JSON escapes, which every JSON parser reads back unchanged.
fn json_ld(value: &serde_json::Value) -> String {
    serde_json::to_string(value)
        .unwrap_or_default()
        .replace('<', "\\u003c")
        .replace('>', "\\u003e")
        .replace('&', "\\u0026")
}

fn site_json_ld(snapshot: &SiteSnapshotV1, origin: &str) -> String {
    json_ld(&serde_json::json!({
        "@context": "https://schema.org",
        "@type": "WebSite",
        "name": snapshot.settings.site_title,
        "url": format!("{origin}/"),
        "description": snapshot.settings.site_description,
        "inLanguage": snapshot.settings.locale.as_str(),
        "potentialAction": {
            "@type": "SearchAction",
            "target": format!("{origin}/search/?q={{search_term_string}}"),
            "query-input": "required name=search_term_string"
        }
    }))
}

fn content_json_ld(
    snapshot: &SiteSnapshotV1,
    origin: &str,
    content: &Content,
    image_url: Option<&str>,
    dates: &SiteDates,
) -> String {
    let url = format!("{origin}/{}/", content.slug);
    let mut value = serde_json::json!({
        "@context": "https://schema.org",
        "@type": if content.kind == ContentKind::Post { "BlogPosting" } else { "WebPage" },
        "headline": content.title,
        "description": content.seo_description.clone().unwrap_or_else(|| content.summary.clone()),
        "url": url,
        "mainEntityOfPage": url,
        "dateModified": dates.iso(content.updated_at),
        "inLanguage": snapshot.settings.locale.as_str(),
        "author": { "@type": "Person", "name": snapshot.settings.author() },
        "publisher": { "@type": "Organization", "name": snapshot.settings.author() },
    });
    if let Some(published) = content.publication.publish_at() {
        value["datePublished"] = serde_json::Value::String(dates.iso(published));
    }
    if !content.tags.is_empty() {
        value["keywords"] = serde_json::Value::Array(
            content
                .tags
                .iter()
                .map(|tag| serde_json::Value::String(tag.name.clone()))
                .collect(),
        );
    }
    if let Some(image_url) = image_url {
        value["image"] = serde_json::Value::String(image_url.to_owned());
    }
    json_ld(&value)
}

#[derive(Serialize)]
struct CardsPage {
    posts: Vec<ContentCard>,
    pager: PagerView,
}

#[derive(Serialize)]
struct PagerView {
    number: usize,
    count: usize,
    newer_url: Option<String>,
    older_url: Option<String>,
    /// On the last page the "older" slot points at the archive instead.
    archive_url: Option<String>,
    status: Option<String>,
}

#[derive(Serialize)]
struct ArchivePage {
    heading: String,
    years: Vec<ArchiveYear>,
}

#[derive(Serialize)]
struct ArchiveYear {
    /// Grouping key in the site zone; the label is what readers see.
    #[serde(skip)]
    year_number: i32,
    year: String,
    posts: Vec<ContentCard>,
}

#[derive(Serialize)]
struct TagIndexPage {
    tags: Vec<TagSummary>,
}

#[derive(Serialize)]
struct TagSummary {
    name: String,
    slug: String,
    count: usize,
}

#[derive(Serialize)]
struct ContentCard {
    title: String,
    slug: String,
    summary: String,
    publish_at: String,
    date: String,
    date_short: String,
    tags: Vec<Tag>,
}

/// The instant a card is dated by: publication, or creation for the rare
/// visible piece without one.
fn card_instant(content: &Content) -> DateTime<Utc> {
    content
        .publication
        .publish_at()
        .unwrap_or(content.created_at)
}

impl ContentCard {
    fn new(content: &Content, dates: &SiteDates) -> Self {
        let published = card_instant(content);
        Self {
            title: content.title.clone(),
            slug: content.slug.to_string(),
            summary: content.summary.clone(),
            publish_at: dates.iso(published),
            date: dates.long(published),
            date_short: dates.short(published),
            tags: content.tags.clone(),
        }
    }
}

#[derive(Serialize)]
struct DateView {
    iso: String,
    date: String,
}

impl DateView {
    fn new(dates: &SiteDates, at: DateTime<Utc>) -> Self {
        Self {
            iso: dates.iso(at),
            date: dates.long(at),
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
    updated: Option<DateView>,
    reading_label: Option<String>,
    outline: Vec<OutlineEntry>,
    tags: Vec<Tag>,
    cover: Option<CoverView>,
    like_js_version: String,
    article_js_version: String,
    older: Option<NeighborView>,
    newer: Option<NeighborView>,
    related: Vec<RelatedView>,
}

#[derive(Serialize)]
struct RelatedView {
    slug: String,
    title: String,
    publish_at: String,
    date: String,
}

/// Posts sharing the most tags with `posts[index]`, ties broken by the
/// list's own order (newest first). Pages are neither source nor candidate.
fn related_posts(posts: &[Content], index: usize, dates: &SiteDates) -> Vec<RelatedView> {
    let own = posts[index]
        .tags
        .iter()
        .map(|tag| tag.slug.as_str())
        .collect::<BTreeSet<_>>();
    let mut scored = posts
        .iter()
        .enumerate()
        .filter(|(position, _)| *position != index)
        .filter_map(|(position, candidate)| {
            let shared = candidate
                .tags
                .iter()
                .filter(|tag| own.contains(tag.slug.as_str()))
                .count();
            (shared > 0).then_some((shared, position, candidate))
        })
        .collect::<Vec<_>>();
    scored.sort_by(|left, right| right.0.cmp(&left.0).then(left.1.cmp(&right.1)));
    scored
        .into_iter()
        .take(RELATED_LIMIT)
        .map(|(_, _, candidate)| RelatedView {
            slug: candidate.slug.to_string(),
            title: candidate.title.clone(),
            publish_at: dates.iso(card_instant(candidate)),
            date: dates.long(card_instant(candidate)),
        })
        .collect()
}

#[derive(Serialize)]
struct JsonFeed {
    version: &'static str,
    title: String,
    home_page_url: String,
    feed_url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    language: &'static str,
    authors: Vec<JsonFeedAuthor>,
    items: Vec<JsonFeedItem>,
}

#[derive(Serialize)]
struct JsonFeedAuthor {
    name: String,
}

#[derive(Serialize)]
struct JsonFeedItem {
    id: String,
    url: String,
    title: String,
    content_html: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    summary: String,
    date_published: String,
    date_modified: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tags: Vec<String>,
}

/// JSON Feed 1.1 with the same entries as the Atom feed. Built from structs,
/// so field order is fixed and no template escaping is involved.
fn json_feed(
    snapshot: &SiteSnapshotV1,
    origin: &str,
    posts: &[Content],
    dates: &SiteDates,
) -> Result<Vec<u8>, SiteCompilerError> {
    let settings = &snapshot.settings;
    let feed = JsonFeed {
        version: JSON_FEED_VERSION,
        title: settings.site_title.clone(),
        home_page_url: format!("{origin}/"),
        feed_url: format!("{origin}/feed.json"),
        description: (!settings.site_description.is_empty())
            .then(|| settings.site_description.clone()),
        language: settings.locale.as_str(),
        authors: vec![JsonFeedAuthor {
            name: settings.author().to_owned(),
        }],
        items: posts
            .iter()
            .take(FEED_ENTRY_LIMIT)
            .map(|post| {
                let url = format!("{origin}/{}/", post.slug);
                JsonFeedItem {
                    id: url.clone(),
                    url,
                    title: post.title.clone(),
                    content_html: post.body_html.clone(),
                    summary: post.summary.clone(),
                    date_published: dates.iso(card_instant(post)),
                    date_modified: dates.iso(post.updated_at),
                    tags: post.tags.iter().map(|tag| tag.name.clone()).collect(),
                }
            })
            .collect(),
    };
    serde_json::to_vec(&feed).map_err(|error| SiteCompilerError::SearchIndex(error.to_string()))
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
    title: String,
    subtitle: Option<String>,
    author: String,
    page_url: String,
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
    tags: Vec<Tag>,
}

#[derive(Serialize)]
struct SitemapContext {
    site_url: String,
    site_updated: Option<String>,
    entries: Vec<SitemapEntry>,
}

#[derive(Serialize)]
struct SitemapEntry {
    url: String,
    updated: Option<String>,
}

#[derive(Serialize)]
struct StaticSearchPage<'a> {
    query: &'a str,
    searched: bool,
    results: Vec<StaticSearchResult>,
    static_search: bool,
    search_js_version: String,
    search_index_url: String,
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

fn fingerprint(value: impl AsRef<[u8]>) -> String {
    blake3::hash(value.as_ref()).to_hex()[..8].to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_ld_escapes_every_character_that_could_close_the_script_element() {
        let value = serde_json::json!({ "headline": "</script><script>alert(1)</script> & co" });
        let serialized = json_ld(&value);
        assert!(!serialized.contains('<'));
        assert!(!serialized.contains('>'));
        assert!(!serialized.contains('&'));
        let parsed: serde_json::Value = serde_json::from_str(&serialized).unwrap();
        assert_eq!(parsed, value);
    }

    #[test]
    fn site_origin_strips_the_path_from_any_page_url() {
        assert_eq!(
            site_origin("https://writing.example/"),
            "https://writing.example"
        );
        assert_eq!(
            site_origin("https://writing.example/tag/rust/"),
            "https://writing.example"
        );
        assert_eq!(
            site_origin("http://localhost:8080/"),
            "http://localhost:8080"
        );
    }

    #[test]
    fn home_paths_number_from_two() {
        assert_eq!(home_path(1), "/");
        assert_eq!(home_path(2), "/page/2/");
        assert_eq!(home_path(10), "/page/10/");
    }
}
