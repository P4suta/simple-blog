//! Imports Markdown files, so writing comes back from an export or arrives
//! from another tool without anyone writing SQL.
//!
//! Two shapes are understood: the front matter this program's own `export`
//! writes (JSON-quoted values, one key per line), and plain Markdown with no
//! front matter at all, which becomes a draft titled from its first heading
//! or its file name. Images under `media/` are stored the same way an upload
//! is, so a `cover_media_id` or `/media/…` reference keeps pointing at the
//! same content-addressed file. Files under `trash/` come back into the
//! trash, exactly as an export left them.

use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use chrono::{DateTime, Utc};

use crate::{
    application::{
        content::{ContentService, SaveIntent},
        ports::{ContentRepository, RepositoryError},
    },
    config::Config,
    domain::content::{ContentDraft, ContentKind, Publication, Slug},
    infrastructure::{
        markdown::ComrakMarkdownRenderer, media::LocalMediaService, sqlite::SqliteRepository,
    },
    operations::OperationError,
};

/// What an import did, for the person who ran it.
#[derive(Debug, Default)]
pub struct ImportReport {
    /// Slugs created or replaced, in the order the files were read.
    pub imported: Vec<String>,
    /// Files left alone, each with the reason.
    pub skipped: Vec<(String, String)>,
    /// Media files stored (duplicates of existing assets count too).
    pub media: usize,
}

pub struct Importer;

impl Importer {
    /// Reads `source/posts`, `source/pages`, `source/trash` and
    /// `source/media`. A piece whose slug is already taken is skipped unless
    /// `force`, which replaces the existing piece's text and metadata in
    /// place (keeping its history).
    pub async fn import(
        config: &Config,
        repository: &Arc<SqliteRepository>,
        source: &Path,
        force: bool,
        now: DateTime<Utc>,
    ) -> Result<ImportReport, OperationError> {
        if !source.is_dir() {
            return Err(OperationError::InvalidData(format!(
                "import source is not a directory: {}",
                source.display()
            )));
        }
        let mut report = ImportReport::default();
        Self::import_media(config, repository, &source.join("media"), &mut report).await?;
        let content = ContentService::new(
            repository.clone(),
            Arc::new(ComrakMarkdownRenderer::default()),
        );
        for (directory, kind, trashed) in [
            ("posts", ContentKind::Post, false),
            ("pages", ContentKind::Page, false),
            // A trashed file names its own kind in the front matter; a bare
            // one is a post, like any other bare file.
            ("trash", ContentKind::Post, true),
        ] {
            for path in markdown_files(&source.join(directory))? {
                Self::import_file(
                    &content,
                    repository,
                    &path,
                    kind,
                    trashed,
                    force,
                    now,
                    &mut report,
                )
                .await?;
            }
        }
        Ok(report)
    }

    async fn import_media(
        config: &Config,
        repository: &Arc<SqliteRepository>,
        directory: &Path,
        report: &mut ImportReport,
    ) -> Result<(), OperationError> {
        if !directory.is_dir() {
            return Ok(());
        }
        let media = LocalMediaService::new(
            config.media_dir(),
            repository.clone(),
            config.max_upload_bytes,
        );
        let mut files = std::fs::read_dir(directory)?
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.is_file())
            .collect::<Vec<_>>();
        files.sort();
        for path in files {
            let name = path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default();
            // Responsive variants written by an export are regenerated from
            // the original, so they need no separate import.
            if is_variant_name(&name) {
                continue;
            }
            let bytes = std::fs::read(&path)?;
            match media.store(&name, bytes, "", "", Utc::now()).await {
                Ok(_) => report.media += 1,
                Err(error) => report
                    .skipped
                    .push((format!("media/{name}"), error.to_string())),
            }
        }
        Ok(())
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "one call site; a struct would only rename the same eight facts"
    )]
    async fn import_file(
        content: &ContentService,
        repository: &Arc<SqliteRepository>,
        path: &Path,
        kind: ContentKind,
        trashed: bool,
        force: bool,
        now: DateTime<Utc>,
        report: &mut ImportReport,
    ) -> Result<(), OperationError> {
        let label = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let text = std::fs::read_to_string(path)?;
        let draft = match parse_document(&text, path, kind, now) {
            Ok(draft) => draft,
            Err(reason) => {
                report.skipped.push((label, reason));
                return Ok(());
            }
        };
        let slug = draft.slug.to_string();
        match content
            .create(draft.clone(), SaveIntent::Explicit, now)
            .await
        {
            Ok(created) => {
                if trashed {
                    content
                        .move_to_trash(created.id, created.version, now)
                        .await
                        .map_err(|error| OperationError::Database(error.to_string()))?;
                }
                report.imported.push(slug);
            }
            Err(RepositoryError::SlugTaken(_)) if force => {
                let existing = repository
                    .list_all_content()
                    .await
                    .map_err(|error| OperationError::Database(error.to_string()))?
                    .into_iter()
                    .find(|existing| existing.slug == draft.slug);
                match existing {
                    Some(existing) => {
                        content
                            .update(
                                existing.id,
                                existing.version,
                                draft,
                                SaveIntent::Explicit,
                                now,
                            )
                            .await
                            .map_err(|error| OperationError::Database(error.to_string()))?;
                        report.imported.push(slug);
                    }
                    None => report.skipped.push((
                        label,
                        "the slug is a historical address of another piece".into(),
                    )),
                }
            }
            Err(RepositoryError::SlugTaken(_)) => report.skipped.push((
                label,
                format!("slug {slug} already exists; pass --force to replace it"),
            )),
            Err(RepositoryError::Validation(reason)) => report.skipped.push((label, reason)),
            Err(error) => return Err(OperationError::Database(error.to_string())),
        }
        Ok(())
    }
}

fn markdown_files(directory: &Path) -> Result<Vec<PathBuf>, OperationError> {
    if !directory.is_dir() {
        return Ok(Vec::new());
    }
    let mut files = std::fs::read_dir(directory)?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_file()
                && path
                    .extension()
                    .is_some_and(|extension| extension.eq_ignore_ascii_case("md"))
        })
        .collect::<Vec<_>>();
    files.sort();
    Ok(files)
}

fn is_variant_name(name: &str) -> bool {
    // Bytes, not a string slice: a multi-byte character crossing byte 64
    // must answer "no", not panic.
    let bytes = name.as_bytes();
    bytes.len() > 64 + "-w.webp".len()
        && name.ends_with("w.webp")
        && bytes[..64].iter().all(u8::is_ascii_hexdigit)
        && bytes.get(64) == Some(&b'-')
}

/// A file as this program exports it, or plain Markdown. The front matter
/// values are JSON strings (so titles with colons survive); bare values are
/// accepted too for files written by hand.
fn parse_document(
    text: &str,
    path: &Path,
    kind: ContentKind,
    now: DateTime<Utc>,
) -> Result<ContentDraft, String> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let (fields, body) = split_front_matter(text)?;
    let field = |name: &str| {
        fields
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    };
    let title = match field("title") {
        Some(title) if !title.trim().is_empty() => title.trim().to_owned(),
        _ => title_from_body(body, path),
    };
    let stem_slug = path
        .file_stem()
        .and_then(|stem| Slug::parse(stem.to_string_lossy().to_lowercase()).ok());
    let slug = match field("slug") {
        Some(slug) => Slug::parse(slug.trim()).map_err(|error| error.to_string())?,
        None => stem_slug.unwrap_or_else(|| Slug::from_title(&title, now)),
    };
    let kind = match field("kind") {
        Some(kind) => kind
            .trim()
            .parse()
            .map_err(|_| format!("unknown kind {kind:?}"))?,
        None => kind,
    };
    let publish_at = match field("publish_at") {
        Some(value) => Some(
            DateTime::parse_from_rfc3339(value.trim())
                .map(|at| at.with_timezone(&Utc))
                .map_err(|_| format!("publish_at is not an RFC 3339 instant: {value:?}"))?,
        ),
        None => None,
    };
    let publication = match field("status").map(str::trim) {
        Some("public") => Publication::Public {
            publish_at: publish_at.unwrap_or(now),
        },
        Some("draft") | None => Publication::Draft,
        Some(other) => return Err(format!("unknown status {other:?}")),
    };
    let tags = match field("tags") {
        Some(value) => serde_json::from_str::<Vec<String>>(value.trim()).or_else(|_| {
            Ok::<_, String>(value.split(',').map(|tag| tag.trim().to_owned()).collect())
        })?,
        None => Vec::new(),
    };
    Ok(ContentDraft {
        kind,
        title,
        slug,
        summary: field("summary").map(str::to_owned).unwrap_or_default(),
        body_markdown: body.to_owned(),
        tags: tags.into_iter().filter(|tag| !tag.is_empty()).collect(),
        cover_media_id: field("cover_media_id").map(str::to_owned),
        seo_title: field("seo_title").map(str::to_owned),
        seo_description: field("seo_description").map(str::to_owned),
        publication,
    })
}

/// Key/value pairs from a front matter block, in file order.
type FrontMatter = Vec<(String, String)>;

/// `(fields, body)`: the front matter block between `---` lines, if any.
fn split_front_matter(text: &str) -> Result<(FrontMatter, &str), String> {
    let Some(rest) = text
        .strip_prefix("---\n")
        .or_else(|| text.strip_prefix("---\r\n"))
    else {
        return Ok((Vec::new(), text));
    };
    let mut fields = Vec::new();
    let mut offset = 0;
    for line in rest.split_inclusive('\n') {
        offset += line.len();
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if trimmed == "---" {
            return Ok((fields, &rest[offset..]));
        }
        let Some((key, value)) = trimmed.split_once(':') else {
            return Err(format!("front matter line without a colon: {trimmed:?}"));
        };
        let value = value.trim();
        let value = serde_json::from_str::<String>(value).unwrap_or_else(|_| value.to_owned());
        fields.push((key.trim().to_owned(), value));
    }
    Err("front matter never closes".into())
}

/// The first `# Heading`, else the file name.
fn title_from_body(body: &str, path: &Path) -> String {
    body.lines()
        .map(str::trim)
        .find_map(|line| line.strip_prefix("# ").map(|title| title.trim().to_owned()))
        .filter(|title| !title.is_empty())
        .unwrap_or_else(|| {
            path.file_stem()
                .map(|stem| stem.to_string_lossy().replace(['-', '_'], " "))
                .unwrap_or_else(|| "Untitled".into())
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exported_front_matter_round_trips_and_plain_files_become_drafts() {
        let now = Utc::now();
        let exported = "---\ntitle: \"Hello: World\"\nslug: hello-world\nkind: post\nstatus: public\npublish_at: \"2026-09-02T23:30:00Z\"\nsummary: \"A summary\"\ntags: [\"Rust\",\"Writing\"]\nseo_title: \"SEO\"\n---\n# Hello\n\nBody.\n";
        let draft = parse_document(
            exported,
            Path::new("posts/hello-world.md"),
            ContentKind::Page,
            now,
        )
        .unwrap();
        assert_eq!(draft.title, "Hello: World");
        assert_eq!(draft.slug.as_str(), "hello-world");
        assert_eq!(draft.kind, ContentKind::Post);
        assert_eq!(draft.tags, vec!["Rust".to_owned(), "Writing".to_owned()]);
        assert_eq!(draft.seo_title.as_deref(), Some("SEO"));
        assert_eq!(draft.body_markdown, "# Hello\n\nBody.\n");
        assert!(
            matches!(draft.publication, Publication::Public { publish_at } if publish_at.to_rfc3339().starts_with("2026-09-02T23:30:00"))
        );

        let plain = "# From a heading\n\nText.\n";
        let draft = parse_document(
            plain,
            Path::new("posts/2026-09-03-memo.md"),
            ContentKind::Post,
            now,
        )
        .unwrap();
        assert_eq!(draft.title, "From a heading");
        assert_eq!(
            draft.slug.as_str(),
            "2026-09-03-memo",
            "a slug-shaped file name wins"
        );
        assert_eq!(draft.publication, Publication::Draft);

        let nameless = "Just text.\n";
        let draft = parse_document(
            nameless,
            Path::new("pages/About Me.md"),
            ContentKind::Page,
            now,
        )
        .unwrap();
        assert_eq!(draft.title, "About Me");
        assert_eq!(draft.slug.as_str(), "about-me");
        assert_eq!(draft.kind, ContentKind::Page);

        let bom = "\u{feff}---\ntitle: \"Marked\"\n---\nbody\n";
        assert_eq!(
            parse_document(bom, Path::new("posts/x.md"), ContentKind::Post, now)
                .unwrap()
                .title,
            "Marked"
        );
        assert!(
            parse_document(
                "---\ntitle: never closed\n",
                Path::new("posts/x.md"),
                ContentKind::Post,
                now
            )
            .is_err()
        );
        assert!(
            parse_document(
                "---\nstatus: weird\n---\nbody",
                Path::new("posts/x.md"),
                ContentKind::Post,
                now
            )
            .is_err()
        );
    }

    #[test]
    fn variant_file_names_are_recognised() {
        let id = "a".repeat(64);
        assert!(is_variant_name(&format!("{id}-480w.webp")));
        assert!(!is_variant_name(&format!("{id}.webp")));
        assert!(!is_variant_name("photo-480w.webp"));
        let crossing = format!("{}\u{e9}-480w.webp", "a".repeat(63));
        assert!(
            !is_variant_name(&crossing),
            "a multi-byte name is not a variant"
        );
    }
}
