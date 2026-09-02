use serde::{Deserialize, Serialize};
use thiserror::Error;
use url::Url;

use crate::domain::media::MediaId;

const MAX_NAVIGATION_ITEMS: usize = 16;
const MAX_CUSTOM_CSS_BYTES: usize = 64 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Locale {
    En,
    Ja,
    Zh,
}

impl Locale {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::En => "en",
            Self::Ja => "ja",
            Self::Zh => "zh",
        }
    }

    /// The Open Graph locale tag that social previews expect.
    #[must_use]
    pub const fn og_locale(self) -> &'static str {
        match self {
            Self::En => "en_US",
            Self::Ja => "ja_JP",
            Self::Zh => "zh_CN",
        }
    }
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum ThemeValidationError {
    #[error("site title must contain 1-120 characters")]
    SiteTitle,
    #[error("site description must contain at most 300 characters")]
    SiteDescription,
    #[error("custom CSS is too large or could escape its style element")]
    CustomCss,
    #[error("logo or favicon media ID is invalid")]
    MediaId,
    #[error("navigation may contain at most {MAX_NAVIGATION_ITEMS} items")]
    NavigationCount,
    #[error("navigation label must contain 1-80 characters")]
    NavigationLabel,
    #[error("navigation destination does not match its internal or external kind")]
    NavigationDestination,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SiteSettings {
    pub site_title: String,
    pub site_description: String,
    pub locale: Locale,
    pub logo_media_id: Option<String>,
    pub favicon_media_id: Option<String>,
    pub custom_css: String,
}

impl SiteSettings {
    pub fn validated(mut self) -> Result<Self, ThemeValidationError> {
        self.site_title = self.site_title.trim().to_owned();
        self.site_description = self.site_description.trim().to_owned();
        if self.site_title.is_empty() || self.site_title.chars().count() > 120 {
            return Err(ThemeValidationError::SiteTitle);
        }
        if self.site_description.chars().count() > 300 {
            return Err(ThemeValidationError::SiteDescription);
        }
        if self.custom_css.len() > MAX_CUSTOM_CSS_BYTES || self.custom_css.contains(['<', '>']) {
            return Err(ThemeValidationError::CustomCss);
        }
        for id in [&self.logo_media_id, &self.favicon_media_id]
            .into_iter()
            .flatten()
        {
            MediaId::parse(id).map_err(|_| ThemeValidationError::MediaId)?;
        }
        Ok(self)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NavigationItem {
    pub id: i64,
    pub label: String,
    pub destination: String,
    pub is_external: bool,
    pub position: u16,
}

pub fn validate_navigation(
    mut items: Vec<NavigationItem>,
) -> Result<Vec<NavigationItem>, ThemeValidationError> {
    if items.len() > MAX_NAVIGATION_ITEMS {
        return Err(ThemeValidationError::NavigationCount);
    }
    for (position, item) in items.iter_mut().enumerate() {
        item.label = item.label.trim().to_owned();
        item.destination = item.destination.trim().to_owned();
        if item.label.is_empty() || item.label.chars().count() > 80 {
            return Err(ThemeValidationError::NavigationLabel);
        }
        let valid_destination = if item.is_external {
            valid_external_destination(&item.destination)
        } else {
            valid_internal_destination(&item.destination)
        };
        if !valid_destination {
            return Err(ThemeValidationError::NavigationDestination);
        }
        item.position =
            u16::try_from(position).map_err(|_| ThemeValidationError::NavigationCount)?;
    }
    Ok(items)
}

fn valid_external_destination(value: &str) -> bool {
    Url::parse(value).is_ok_and(|url| {
        matches!(url.scheme(), "http" | "https")
            && url.host().is_some()
            && url.username().is_empty()
            && url.password().is_none()
    })
}

fn valid_internal_destination(value: &str) -> bool {
    value.starts_with('/')
        && !value.starts_with("//")
        && !value.contains(['\\', '?', '#'])
        && !value.chars().any(char::is_control)
        && value
            .split('/')
            .all(|segment| !matches!(segment, "." | ".."))
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct PageMeta {
    pub title: String,
    pub description: String,
    pub canonical_url: String,
    pub og_type: String,
    pub og_locale: String,
    pub image_url: Option<String>,
    /// RFC 3339 instants for `article:*` metadata; posts only.
    pub published_time: Option<String>,
    pub modified_time: Option<String>,
    pub article_tags: Vec<String>,
    /// Pagination neighbours for `<link rel="prev|next">`.
    pub prev_url: Option<String>,
    pub next_url: Option<String>,
    /// A feed specific to this page (a tag), on top of the site feed.
    pub alternate_feed: Option<AlternateFeed>,
    /// Serialized JSON-LD with `<`, `>` and `&` escaped, safe to inline.
    pub json_ld: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct AlternateFeed {
    pub href: String,
    pub title: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ThemeAssets {
    pub logo_url: Option<String>,
    pub favicon_url: Option<String>,
    /// Cache-busting fingerprint of the current custom CSS, used in the
    /// stylesheet URL so long-lived caches survive CSS edits.
    pub css_version: String,
    /// Fingerprint of the reader-preferences script, same caching scheme.
    pub prefs_js_version: String,
}

/// The only public-theme boundary. Templates never receive repositories or ad-hoc maps.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ThemeContext<T> {
    /// UI strings resolved for the site's locale (English fallback applied).
    pub t: std::collections::HashMap<String, String>,
    pub site: SiteSettings,
    pub assets: ThemeAssets,
    pub navigation: Vec<NavigationItem>,
    pub meta: PageMeta,
    pub page: T,
}
