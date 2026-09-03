//! Media is deleted only once nothing can show it any more.
//!
//! The live reference set comes from current content (trashed pieces
//! included, since the trash is recoverable) and site settings. Revision
//! snapshots keep their images alive too: restoring an older version must
//! never bring back a broken picture, so history is part of the survivor set.

use std::collections::{HashMap, HashSet};

use crate::domain::{content::Content, media::media_id_from_path, theme::SiteSettings};

const MEDIA_URL_PREFIX: &str = "/media/";

#[must_use]
pub fn referenced_media_ids(contents: &[Content], settings: &SiteSettings) -> HashSet<String> {
    let mut referenced = HashSet::new();
    for id in [&settings.logo_media_id, &settings.favicon_media_id]
        .into_iter()
        .flatten()
    {
        referenced.insert(id.clone());
    }
    for content in contents {
        if let Some(id) = &content.cover_media_id {
            referenced.insert(id.clone());
        }
        collect_media_references(&content.body_markdown, &mut referenced);
    }
    referenced
}

/// How one asset is used, for the media page and the delete guard.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct MediaUsage {
    /// Pieces (cover or body) that show the asset now.
    pub pieces: usize,
    /// The site logo or favicon.
    pub settings: bool,
    /// Only stored revisions still mention it.
    pub history_only: bool,
}

impl MediaUsage {
    /// Something readers can reach today still needs the asset.
    #[must_use]
    pub const fn is_current(self) -> bool {
        self.pieces > 0 || self.settings
    }
}

/// Usage of every referenced asset; an asset absent from the map is unused.
#[must_use]
pub fn media_usage(
    contents: &[Content],
    settings: &SiteSettings,
    revision_references: &HashSet<String>,
) -> HashMap<String, MediaUsage> {
    let mut usage: HashMap<String, MediaUsage> = HashMap::new();
    for content in contents {
        let mut own = HashSet::new();
        if let Some(id) = &content.cover_media_id {
            own.insert(id.clone());
        }
        collect_media_references(&content.body_markdown, &mut own);
        for id in own {
            usage.entry(id).or_default().pieces += 1;
        }
    }
    for id in [&settings.logo_media_id, &settings.favicon_media_id]
        .into_iter()
        .flatten()
    {
        usage.entry(id.clone()).or_default().settings = true;
    }
    for id in revision_references {
        let entry = usage.entry(id.clone()).or_default();
        entry.history_only = !entry.is_current();
    }
    usage
}

/// Everything a sweep must keep: live references plus what stored revisions
/// still point at.
#[must_use]
pub fn gc_survivors(
    contents: &[Content],
    settings: &SiteSettings,
    revision_references: &HashSet<String>,
) -> HashSet<String> {
    let mut survivors = referenced_media_ids(contents, settings);
    survivors.extend(revision_references.iter().cloned());
    survivors
}

/// Adds every complete media identity mentioned as `/media/<id>` in `text`.
/// Works on Markdown as well as on the JSON a revision snapshot stores,
/// because neither escapes the slash.
pub fn collect_media_references(text: &str, referenced: &mut HashSet<String>) {
    for (index, _) in text.match_indices(MEDIA_URL_PREFIX) {
        if let Some(id) = media_id_from_path(&text[index..]) {
            referenced.insert(id.into());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MEDIA_ID_LENGTH: usize = 64;

    #[test]
    fn body_reference_scanner_accepts_only_complete_lowercase_content_ids() {
        let valid = "a".repeat(MEDIA_ID_LENGTH);
        let uppercase = "B".repeat(MEDIA_ID_LENGTH);
        let short = "c".repeat(MEDIA_ID_LENGTH - 1);
        let markdown = format!(
            "![valid](/media/{valid}) ![uppercase](/media/{uppercase}) ![short](/media/{short})"
        );
        let mut referenced = HashSet::new();

        collect_media_references(&markdown, &mut referenced);

        assert_eq!(referenced, HashSet::from([valid]));
    }

    #[test]
    fn survivors_include_revision_references_alongside_live_ones() {
        let live = "b".repeat(MEDIA_ID_LENGTH);
        let historical = "c".repeat(MEDIA_ID_LENGTH);
        let settings = SiteSettings {
            site_title: "t".into(),
            site_description: String::new(),
            locale: crate::domain::theme::Locale::En,
            logo_media_id: Some(live.clone()),
            favicon_media_id: None,
            custom_css: String::new(),
            timezone: "UTC".into(),
            author_name: String::new(),
            custom_css_backup: None,
        };
        let revisions = HashSet::from([historical.clone()]);

        let survivors = gc_survivors(&[], &settings, &revisions);

        assert_eq!(survivors, HashSet::from([live, historical]));
    }

    #[test]
    fn usage_counts_pieces_and_settings_and_flags_history_only_assets() {
        let cover = "d".repeat(MEDIA_ID_LENGTH);
        let inline = "e".repeat(MEDIA_ID_LENGTH);
        let historical = "f".repeat(MEDIA_ID_LENGTH);
        let logo = "1".repeat(MEDIA_ID_LENGTH);
        let mut piece = crate::domain::content::Content {
            id: crate::domain::content::ContentId::from_i64(1),
            kind: crate::domain::content::ContentKind::Post,
            title: "t".into(),
            slug: crate::domain::content::Slug::parse("t").unwrap(),
            summary: String::new(),
            body_markdown: format!("![](/media/{inline}.webp) ![](/media/{inline}.webp)"),
            body_html: String::new(),
            tags: Vec::new(),
            cover_media_id: Some(cover.clone()),
            seo_title: None,
            seo_description: None,
            publication: crate::domain::content::Publication::Draft,
            version: 1,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            deleted_at: None,
        };
        let settings = SiteSettings {
            site_title: "t".into(),
            site_description: String::new(),
            locale: crate::domain::theme::Locale::En,
            logo_media_id: Some(logo.clone()),
            favicon_media_id: None,
            custom_css: String::new(),
            timezone: "UTC".into(),
            author_name: String::new(),
            custom_css_backup: None,
        };
        let revisions = HashSet::from([historical.clone(), inline.clone()]);

        let usage = media_usage(std::slice::from_ref(&piece), &settings, &revisions);

        assert_eq!(usage[&cover].pieces, 1);
        assert_eq!(usage[&inline].pieces, 1, "one piece, however many times");
        assert!(!usage[&inline].history_only);
        assert!(usage[&logo].settings);
        assert!(usage[&historical].history_only);
        assert!(!usage[&historical].is_current());
        piece.cover_media_id = None;
        assert!(!media_usage(&[piece], &settings, &revisions).contains_key(&cover));
    }

    #[test]
    fn body_reference_scanner_never_slices_through_utf8() {
        let markdown = format!("/media/{}é", "a".repeat(MEDIA_ID_LENGTH - 1));
        let mut referenced = HashSet::new();

        collect_media_references(&markdown, &mut referenced);

        assert!(referenced.is_empty());
    }
}
