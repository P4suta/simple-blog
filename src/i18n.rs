//! UI strings live in `locales/*.json`, not in code.
//!
//! English is the base: every other locale is resolved at startup by
//! overlaying its translations on the English set, so a missing key falls
//! back to English.

use std::collections::HashMap;

use crate::domain::theme::Locale;

#[derive(Clone, Debug)]
pub struct Translations {
    en: HashMap<String, String>,
    ja: HashMap<String, String>,
    zh: HashMap<String, String>,
}

#[derive(Debug, thiserror::Error)]
#[error("locale file {file} is invalid: {message}")]
pub struct TranslationError {
    file: &'static str,
    message: String,
}

impl Translations {
    pub fn embedded() -> Result<Self, TranslationError> {
        let en = parse("locales/en.json", include_str!("../locales/en.json"))?;
        let ja = overlay(
            &en,
            parse("locales/ja.json", include_str!("../locales/ja.json"))?,
        );
        let zh = overlay(
            &en,
            parse("locales/zh.json", include_str!("../locales/zh.json"))?,
        );
        Ok(Self { en, ja, zh })
    }

    #[must_use]
    pub const fn for_locale(&self, locale: Locale) -> &HashMap<String, String> {
        match locale {
            Locale::En => &self.en,
            Locale::Ja => &self.ja,
            Locale::Zh => &self.zh,
        }
    }

    /// Looks up one string; a key absent even from English comes back as the
    /// key itself, so a typo is visible instead of silent.
    #[must_use]
    pub fn text(&self, locale: Locale, key: &str) -> String {
        self.for_locale(locale)
            .get(key)
            .cloned()
            .unwrap_or_else(|| key.to_owned())
    }
}

fn parse(file: &'static str, source: &str) -> Result<HashMap<String, String>, TranslationError> {
    serde_json::from_str(source).map_err(|error| TranslationError {
        file,
        message: error.to_string(),
    })
}

fn overlay(
    base: &HashMap<String, String>,
    mut own: HashMap<String, String>,
) -> HashMap<String, String> {
    for (key, value) in base {
        own.entry(key.clone()).or_insert_with(|| value.clone());
    }
    own
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn malformed_locale_reports_its_source_file() {
        let error = parse("broken.json", "{").unwrap_err();
        assert_eq!(error.file, "broken.json");
        assert!(error.to_string().contains("broken.json"));
    }

    #[test]
    fn locale_overlay_falls_back_without_overwriting_translations() {
        let base = HashMap::from([
            ("only_base".into(), "Base".into()),
            ("shared".into(), "English".into()),
        ]);
        let own = HashMap::from([("shared".into(), "日本語".into())]);

        let merged = overlay(&base, own);

        assert_eq!(merged.get("only_base").map(String::as_str), Some("Base"));
        assert_eq!(merged.get("shared").map(String::as_str), Some("日本語"));
    }
}
