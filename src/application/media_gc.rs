//! Media is deleted the moment nothing current references it. The reference
//! set is computed from live content and site settings only — revision
//! snapshots deliberately do not keep media alive.

use std::collections::HashSet;

use crate::domain::{content::Content, theme::SiteSettings};

const MEDIA_ID_LENGTH: usize = 64;
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
        collect_body_references(&content.body_markdown, &mut referenced);
    }
    referenced
}

fn collect_body_references(markdown: &str, referenced: &mut HashSet<String>) {
    for (index, _) in markdown.match_indices(MEDIA_URL_PREFIX) {
        let candidate = &markdown.as_bytes()[index + MEDIA_URL_PREFIX.len()..];
        let Some(id) = candidate.get(..MEDIA_ID_LENGTH) else {
            continue;
        };
        if id
            .iter()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        {
            referenced.insert(id.iter().map(|byte| char::from(*byte)).collect());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn body_reference_scanner_accepts_only_complete_lowercase_content_ids() {
        let valid = "a".repeat(MEDIA_ID_LENGTH);
        let uppercase = "B".repeat(MEDIA_ID_LENGTH);
        let short = "c".repeat(MEDIA_ID_LENGTH - 1);
        let markdown = format!(
            "![valid](/media/{valid}) ![uppercase](/media/{uppercase}) ![short](/media/{short})"
        );
        let mut referenced = HashSet::new();

        collect_body_references(&markdown, &mut referenced);

        assert_eq!(referenced, HashSet::from([valid]));
    }

    #[test]
    fn body_reference_scanner_never_slices_through_utf8() {
        let markdown = format!("/media/{}é", "a".repeat(MEDIA_ID_LENGTH - 1));
        let mut referenced = HashSet::new();

        collect_body_references(&markdown, &mut referenced);

        assert!(referenced.is_empty());
    }
}
