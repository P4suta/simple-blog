//! Embedded templates shared by the native and hosted adapters.

use std::{path::Path, sync::Arc};

use minijinja::{AutoEscape, Environment, value::Serde};
use serde::Serialize;
use thiserror::Error;

#[derive(Clone)]
pub struct Templates {
    environment: Arc<Environment<'static>>,
}

#[derive(Debug, Error)]
pub enum TemplateError {
    #[error("template setup failed: {0}")]
    Setup(String),
    #[error("template rendering failed: {0}")]
    Render(String),
}

/// Every template the binary embeds, by the name templates refer to each
/// other with. Adding a file means adding a line here.
const TEMPLATES: &[(&str, &str)] = &[
    (
        "public/base.html",
        include_str!("../../templates/public/base.html"),
    ),
    (
        "public/home.html",
        include_str!("../../templates/public/home.html"),
    ),
    (
        "public/content.html",
        include_str!("../../templates/public/content.html"),
    ),
    (
        "public/archive.html",
        include_str!("../../templates/public/archive.html"),
    ),
    (
        "public/tags.html",
        include_str!("../../templates/public/tags.html"),
    ),
    (
        "public/not_found.html",
        include_str!("../../templates/public/not_found.html"),
    ),
    (
        "public/search.html",
        include_str!("../../templates/public/search.html"),
    ),
    (
        "admin/base.html",
        include_str!("../../templates/admin/base.html"),
    ),
    (
        "admin/dashboard.html",
        include_str!("../../templates/admin/dashboard.html"),
    ),
    (
        "admin/editor.html",
        include_str!("../../templates/admin/editor.html"),
    ),
    (
        "admin/media.html",
        include_str!("../../templates/admin/media.html"),
    ),
    (
        "admin/share_link.html",
        include_str!("../../templates/admin/share_link.html"),
    ),
    (
        "admin/error.html",
        include_str!("../../templates/admin/error.html"),
    ),
    (
        "admin/conflict.html",
        include_str!("../../templates/admin/conflict.html"),
    ),
    (
        "admin/settings.html",
        include_str!("../../templates/admin/settings.html"),
    ),
    (
        "admin/recovery_codes.html",
        include_str!("../../templates/admin/recovery_codes.html"),
    ),
    (
        "admin/revision.html",
        include_str!("../../templates/admin/revision.html"),
    ),
    (
        "admin/login.html",
        include_str!("../../templates/admin/login.html"),
    ),
    (
        "admin/setup.html",
        include_str!("../../templates/admin/setup.html"),
    ),
    ("feed.xml", include_str!("../../templates/feed.xml")),
    ("sitemap.xml", include_str!("../../templates/sitemap.xml")),
];

impl Templates {
    pub fn embedded() -> Result<Self, TemplateError> {
        let mut environment = Environment::new();
        environment.set_auto_escape_callback(|name| {
            let escaped = Path::new(name)
                .extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| {
                    extension.eq_ignore_ascii_case("html") || extension.eq_ignore_ascii_case("xml")
                });
            if escaped {
                AutoEscape::Html
            } else {
                AutoEscape::None
            }
        });
        for (name, source) in TEMPLATES {
            environment
                .add_template(name, source)
                .map_err(|error| TemplateError::Setup(error.to_string()))?;
        }
        Ok(Self {
            environment: Arc::new(environment),
        })
    }

    pub fn render(&self, name: &str, context: impl Serialize) -> Result<String, TemplateError> {
        self.environment
            .get_template(name)
            .and_then(|template| template.render(Serde(context)))
            .map_err(|error| TemplateError::Render(error.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use serde::{Serialize, Serializer, ser::Error as _};

    use super::{TemplateError, Templates};

    #[derive(Serialize)]
    struct Feed<'a> {
        title: &'a str,
        entries: [Entry<'a>; 1],
    }

    #[derive(Serialize)]
    struct Entry<'a> {
        title: &'a str,
        content_html: &'a str,
    }

    struct BrokenContext;

    impl Serialize for BrokenContext {
        fn serialize<S: Serializer>(&self, _: S) -> Result<S::Ok, S::Error> {
            Err(S::Error::custom("template fixture serialization failed"))
        }
    }

    #[test]
    fn embedded_feed_escapes_nested_serializable_contexts() {
        let templates = Templates::embedded().expect("embedded templates compile");
        let context = Feed {
            title: "<unsafe>&",
            entries: [Entry {
                title: "Nested entry",
                content_html: "<script>&",
            }],
        };
        let output = templates
            .render("feed.xml", &context)
            .expect("serializable feed renders");
        assert!(output.contains("<title>&lt;unsafe&gt;&amp;</title>"));
        assert!(output.contains("<title>Nested entry</title>"));
        assert!(output.contains("&lt;script&gt;&amp;"));
        assert!(!output.contains("<script>"));
    }

    #[test]
    fn missing_templates_are_render_errors() {
        let templates = Templates::embedded().expect("embedded templates compile");
        assert!(matches!(
            templates.render("missing.html", ()),
            Err(TemplateError::Render(_))
        ));
    }

    #[test]
    fn context_serialization_failures_are_render_errors() {
        let templates = Templates::embedded().expect("embedded templates compile");
        let Err(TemplateError::Render(message)) = templates.render("feed.xml", BrokenContext)
        else {
            panic!("serialization failure must produce a render error");
        };
        assert!(message.contains("template fixture serialization failed"));
    }
}
