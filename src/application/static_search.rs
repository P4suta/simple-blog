//! Portable static-search index and reference query semantics.
//!
//! Browser and host adapters may implement the query locally, but this module
//! is the compatibility oracle used by fixtures and release conformance tests.

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::domain::{content::Slug, search};

pub const STATIC_SEARCH_FORMAT_VERSION: u8 = 1;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct StaticSearchIndexV1 {
    pub format_version: u8,
    pub documents: Vec<StaticSearchDocument>,
}

impl StaticSearchIndexV1 {
    #[must_use]
    pub const fn new(documents: Vec<StaticSearchDocument>) -> Self {
        Self {
            format_version: STATIC_SEARCH_FORMAT_VERSION,
            documents,
        }
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, StaticSearchError> {
        let index: Self = serde_json::from_slice(bytes)
            .map_err(|error| StaticSearchError::InvalidJson(error.to_string()))?;
        index.validate()?;
        Ok(index)
    }

    pub fn canonical_bytes(&self) -> Result<Vec<u8>, StaticSearchError> {
        self.validate()?;
        serde_json::to_vec(self).map_err(|error| StaticSearchError::InvalidJson(error.to_string()))
    }

    #[must_use]
    pub fn search(&self, query: &str, limit: usize) -> Vec<&StaticSearchDocument> {
        let parsed = search::parse_query(query);
        let terms = parsed.all();
        if terms.is_empty() || limit == 0 {
            return Vec::new();
        }
        let mut matches = self
            .documents
            .iter()
            .enumerate()
            .filter(|(_, document)| terms.iter().all(|term| document.folded.contains(*term)))
            .map(|(source_position, document)| (document.score(&terms), source_position, document))
            .collect::<Vec<_>>();
        matches.sort_by(|left, right| right.0.cmp(&left.0).then_with(|| left.1.cmp(&right.1)));
        matches
            .into_iter()
            .take(limit)
            .map(|(_, _, document)| document)
            .collect()
    }

    fn validate(&self) -> Result<(), StaticSearchError> {
        if self.format_version != STATIC_SEARCH_FORMAT_VERSION {
            return Err(StaticSearchError::UnsupportedFormat(self.format_version));
        }
        for document in &self.documents {
            document.validate()?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct StaticSearchDocument {
    pub id: i64,
    pub slug: String,
    pub title: String,
    pub summary: String,
    pub body: String,
    pub folded: String,
    pub published: String,
}

impl StaticSearchDocument {
    #[must_use]
    pub fn new(
        id: i64,
        slug: &str,
        title: &str,
        summary: &str,
        body: &str,
        published: &str,
    ) -> Self {
        Self {
            id,
            slug: slug.to_owned(),
            title: title.to_owned(),
            summary: summary.to_owned(),
            body: body.to_owned(),
            folded: folded_document(title, summary, body),
            published: published.to_owned(),
        }
    }

    fn score(&self, terms: &[&str]) -> u32 {
        let title = search::fold(&search::normalize(&self.title));
        let summary = search::fold(&search::normalize(&self.summary));
        let body = search::fold(&search::normalize(&self.body));
        terms.iter().fold(0_u32, |score, term| {
            score
                .saturating_add(u32::from(title.contains(*term)) * 100)
                .saturating_add(u32::from(summary.contains(*term)) * 10)
                .saturating_add(u32::from(body.contains(*term)))
        })
    }

    fn validate(&self) -> Result<(), StaticSearchError> {
        Slug::parse(&self.slug)
            .map_err(|error| StaticSearchError::InvalidDocument(error.to_string()))?;
        if self.id <= 0 {
            return Err(StaticSearchError::InvalidDocument(
                "content identity must be positive".into(),
            ));
        }
        let expected = folded_document(&self.title, &self.summary, &self.body);
        if self.folded != expected {
            return Err(StaticSearchError::InvalidDocument(format!(
                "folded text does not match content {}",
                self.id
            )));
        }
        Ok(())
    }
}

fn folded_document(title: &str, summary: &str, body: &str) -> String {
    search::fold(&search::normalize(&format!("{title} {summary} {body}")))
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum StaticSearchError {
    #[error("unsupported static search format version: {0}")]
    UnsupportedFormat(u8),
    #[error("invalid static search JSON: {0}")]
    InvalidJson(String),
    #[error("invalid static search document: {0}")]
    InvalidDocument(String),
}
