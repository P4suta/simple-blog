//! Media is deleted only once nothing can show it any more.
//!
//! The live reference set comes from current content (trashed pieces
//! included, since the trash is recoverable) and site settings. Revision
//! snapshots keep their images alive too: restoring an older version must
//! never bring back a broken picture, so history is part of the survivor set.

use std::collections::HashSet;

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
    fn body_reference_scanner_never_slices_through_utf8() {
        let markdown = format!("/media/{}é", "a".repeat(MEDIA_ID_LENGTH - 1));
        let mut referenced = HashSet::new();

        collect_media_references(&markdown, &mut referenced);

        assert!(referenced.is_empty());
    }
}
