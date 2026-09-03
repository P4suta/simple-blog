use std::{collections::HashSet, fmt::Write as _, sync::Arc};

use chrono::{DateTime, Utc};

use crate::{
    application::ports::{
        ContentRepository, MarkdownRenderer, PreparedContent, RenderedMarkdown, RepositoryError,
    },
    domain::content::{
        Content, ContentDraft, ContentId, SaveIntent as DomainSaveIntent, Slug, Tag,
    },
    domain::media::MediaId,
};

const MAX_MARKDOWN_BYTES: usize = 2 * 1024 * 1024;

pub use crate::domain::content::SaveIntent;

#[derive(Clone)]
pub struct ContentService {
    repository: Arc<dyn ContentRepository>,
    markdown: Arc<dyn MarkdownRenderer>,
}

impl ContentService {
    pub fn new(
        repository: Arc<dyn ContentRepository>,
        markdown: Arc<dyn MarkdownRenderer>,
    ) -> Self {
        Self {
            repository,
            markdown,
        }
    }

    pub async fn create(
        &self,
        draft: ContentDraft,
        intent: DomainSaveIntent,
        now: DateTime<Utc>,
    ) -> Result<Content, RepositoryError> {
        let content = self.prepare(draft)?;
        self.repository.create(content, intent, now).await
    }

    pub async fn update(
        &self,
        id: ContentId,
        expected_version: i64,
        draft: ContentDraft,
        intent: DomainSaveIntent,
        now: DateTime<Utc>,
    ) -> Result<Content, RepositoryError> {
        let content = self.prepare(draft)?;
        self.repository
            .update(id, expected_version, content, intent, now)
            .await
    }

    #[tracing::instrument(
        name = "content.restore_revision",
        skip(self),
        fields(content_id = %id, revision_id, expected_version),
        err
    )]
    pub async fn restore_revision(
        &self,
        id: ContentId,
        revision_id: i64,
        expected_version: i64,
        now: DateTime<Utc>,
    ) -> Result<Content, RepositoryError> {
        let revision = self
            .repository
            .find_revision(id, revision_id)
            .await?
            .ok_or(RepositoryError::NotFound)?;
        self.update(
            id,
            expected_version,
            revision.snapshot.to_draft(),
            DomainSaveIntent::Explicit,
            now,
        )
        .await
    }

    #[tracing::instrument(name = "content.trash", skip(self), fields(content_id = %id), err)]
    pub async fn move_to_trash(
        &self,
        id: ContentId,
        expected_version: i64,
        now: DateTime<Utc>,
    ) -> Result<Content, RepositoryError> {
        self.repository
            .move_to_trash(id, expected_version, now)
            .await
    }

    #[tracing::instrument(name = "content.restore", skip(self), fields(content_id = %id), err)]
    pub async fn restore_from_trash(
        &self,
        id: ContentId,
        now: DateTime<Utc>,
    ) -> Result<Content, RepositoryError> {
        self.repository.restore_from_trash(id, now).await
    }

    #[tracing::instrument(name = "content.delete", skip(self), fields(content_id = %id), err)]
    pub async fn delete_permanently(&self, id: ContentId) -> Result<(), RepositoryError> {
        self.repository.delete_permanently(id).await
    }

    pub fn preview(&self, markdown: &str) -> Result<RenderedMarkdown, RepositoryError> {
        if markdown.len() > MAX_MARKDOWN_BYTES {
            return Err(RepositoryError::Validation(
                "Markdown exceeds the 2 MiB limit".into(),
            ));
        }
        Ok(self.markdown.render(markdown))
    }

    fn prepare(&self, mut draft: ContentDraft) -> Result<PreparedContent, RepositoryError> {
        draft.title = draft.title.trim().to_owned();
        draft.summary = draft.summary.trim().to_owned();
        draft.seo_title = clean_optional(draft.seo_title);
        draft.seo_description = clean_optional(draft.seo_description);

        if draft.title.is_empty() || draft.title.chars().count() > 200 {
            return Err(RepositoryError::Validation(
                "title must contain 1-200 characters".into(),
            ));
        }
        if draft.summary.chars().count() > 500 {
            return Err(RepositoryError::Validation(
                "summary must contain at most 500 characters".into(),
            ));
        }
        if draft.body_markdown.len() > MAX_MARKDOWN_BYTES {
            return Err(RepositoryError::Validation(
                "Markdown exceeds the 2 MiB limit".into(),
            ));
        }
        if draft
            .seo_title
            .as_ref()
            .is_some_and(|value| value.chars().count() > 70)
            || draft
                .seo_description
                .as_ref()
                .is_some_and(|value| value.chars().count() > 200)
        {
            return Err(RepositoryError::Validation(
                "SEO title or description is too long".into(),
            ));
        }
        if let Some(id) = &draft.cover_media_id {
            MediaId::parse(id).map_err(|error| RepositoryError::Validation(error.to_string()))?;
        }

        let tags = normalize_tags(&draft.tags)?;
        draft.tags = tags.iter().map(|tag| tag.name.clone()).collect();
        let body_html = self.markdown.render(&draft.body_markdown).html;

        Ok(PreparedContent {
            draft,
            body_html,
            tags,
        })
    }
}

fn clean_optional(value: Option<String>) -> Option<String> {
    value.and_then(|value| {
        let value = value.trim();
        (!value.is_empty()).then(|| value.to_owned())
    })
}

fn normalize_tags(values: &[String]) -> Result<Vec<Tag>, RepositoryError> {
    let mut seen = HashSet::new();
    let mut tags = Vec::new();
    for value in values {
        let name = value.trim();
        if name.is_empty() {
            continue;
        }
        if name.chars().count() > 50 {
            return Err(RepositoryError::Validation(
                "tag names must contain at most 50 characters".into(),
            ));
        }
        let candidate = slug::slugify(name);
        let candidate = if candidate.is_empty() {
            let mut codepoints = String::new();
            for character in name.chars() {
                write!(&mut codepoints, "{:x}", u32::from(character))
                    .map_err(|error| RepositoryError::Validation(error.to_string()))?;
            }
            format!("u{codepoints}")
        } else {
            candidate
        };
        let slug = Slug::parse(candidate).map_err(|_| {
            RepositoryError::Validation(format!("tag {name:?} cannot form a safe slug"))
        })?;
        if seen.insert(slug.clone()) {
            tags.push(Tag {
                name: name.to_owned(),
                slug,
            });
        }
        if tags.len() > 20 {
            return Err(RepositoryError::Validation(
                "content may contain at most 20 tags".into(),
            ));
        }
    }
    Ok(tags)
}
