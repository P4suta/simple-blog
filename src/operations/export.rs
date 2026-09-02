use std::{
    fmt::Write as _,
    path::{Path, PathBuf},
};

use chrono::{DateTime, SecondsFormat, Utc};
use uuid::Uuid;

use crate::{
    application::ports::{ContentRepository, MediaRepository},
    config::Config,
    domain::content::{Content, ContentKind, Publication},
    infrastructure::sqlite::SqliteRepository,
    operations::OperationError,
};

pub struct Exporter;

impl Exporter {
    pub async fn export(
        config: &Config,
        repository: &SqliteRepository,
        output: &Path,
        _now: DateTime<Utc>,
    ) -> Result<PathBuf, OperationError> {
        if output.exists() {
            return Err(OperationError::ExportExists(output.display().to_string()));
        }
        let parent = output.parent().unwrap_or_else(|| Path::new("."));
        std::fs::create_dir_all(parent)?;
        let staging = parent.join(format!(".simple-blog-export-{}", Uuid::new_v4()));
        std::fs::create_dir(&staging)?;
        let result = Self::write(config, repository, &staging).await;
        if let Err(error) = result {
            let _ = std::fs::remove_dir_all(&staging);
            return Err(error);
        }
        std::fs::rename(&staging, output)?;
        Ok(output.to_owned())
    }

    async fn write(
        config: &Config,
        repository: &SqliteRepository,
        staging: &Path,
    ) -> Result<(), OperationError> {
        std::fs::create_dir(staging.join("posts"))?;
        std::fs::create_dir(staging.join("pages"))?;
        std::fs::create_dir(staging.join("media"))?;
        for content in repository
            .list_all_content()
            .await
            .map_err(|error| OperationError::Database(error.to_string()))?
            .into_iter()
            .filter(|content| !content.is_trashed())
        {
            let directory = match content.kind {
                ContentKind::Post => "posts",
                ContentKind::Page => "pages",
            };
            std::fs::write(
                staging.join(directory).join(format!("{}.md", content.slug)),
                markdown_export(&content)?,
            )?;
        }
        for asset in repository
            .list_media()
            .await
            .map_err(|error| OperationError::Database(error.to_string()))?
        {
            copy(
                &config.media_dir().join(&asset.original_filename),
                &staging.join("media").join(&asset.original_filename),
            )?;
            for variant in asset.variants {
                copy(
                    &config.media_dir().join(&variant.filename),
                    &staging.join("media").join(&variant.filename),
                )?;
            }
        }
        Ok(())
    }
}

fn markdown_export(content: &Content) -> Result<String, OperationError> {
    let json = |value: &str| {
        serde_json::to_string(value).map_err(|error| OperationError::InvalidData(error.to_string()))
    };
    let tags = content
        .tags
        .iter()
        .map(|tag| tag.name.as_str())
        .collect::<Vec<_>>();
    let tags = serde_json::to_string(&tags)
        .map_err(|error| OperationError::InvalidData(error.to_string()))?;
    let (status, publish_at) = match content.publication {
        Publication::Draft => ("draft", String::new()),
        Publication::Public { publish_at } => (
            "public",
            format!(
                "publish_at: {}\n",
                json(&publish_at.to_rfc3339_opts(SecondsFormat::Secs, true))?
            ),
        ),
    };
    let mut front_matter = format!(
        "---\ntitle: {}\nslug: {}\nkind: {}\nstatus: {status}\n{publish_at}summary: {}\ntags: {tags}\n",
        json(&content.title)?,
        content.slug,
        content.kind,
        json(&content.summary)?,
    );
    if let Some(cover) = &content.cover_media_id {
        writeln!(&mut front_matter, "cover_media_id: {}", json(cover)?)
            .map_err(|error| OperationError::InvalidData(error.to_string()))?;
    }
    if let Some(title) = &content.seo_title {
        writeln!(&mut front_matter, "seo_title: {}", json(title)?)
            .map_err(|error| OperationError::InvalidData(error.to_string()))?;
    }
    if let Some(description) = &content.seo_description {
        writeln!(&mut front_matter, "seo_description: {}", json(description)?)
            .map_err(|error| OperationError::InvalidData(error.to_string()))?;
    }
    front_matter.push_str("---\n");
    front_matter.push_str(&content.body_markdown);
    if !front_matter.ends_with('\n') {
        front_matter.push('\n');
    }
    Ok(front_matter)
}

fn copy(source: &Path, destination: &Path) -> Result<(), OperationError> {
    if !source.is_file() {
        return Err(OperationError::InvalidData(format!(
            "media file is missing: {}",
            source.display()
        )));
    }
    std::fs::copy(source, destination)?;
    Ok(())
}
