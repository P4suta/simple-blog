use ammonia::Builder;
use comrak::{Options, markdown_to_html};

use crate::application::ports::{MarkdownRenderer, RenderedMarkdown};

/// GFM-compatible renderer with raw HTML disabled and a second sanitization pass.
#[derive(Clone, Debug)]
pub struct ComrakMarkdownRenderer {
    options: Options<'static>,
}

impl Default for ComrakMarkdownRenderer {
    fn default() -> Self {
        let mut options = Options::default();
        options.extension.autolink = true;
        options.extension.footnotes = true;
        options.extension.strikethrough = true;
        options.extension.table = true;
        options.extension.tagfilter = true;
        options.extension.tasklist = true;
        options.render.github_pre_lang = true;
        options.render.unsafe_ = false;

        Self { options }
    }
}

impl MarkdownRenderer for ComrakMarkdownRenderer {
    fn render(&self, markdown: &str) -> RenderedMarkdown {
        let untrusted_html = markdown_to_html(markdown, &self.options);
        let mut sanitizer = Builder::default();
        sanitizer
            .add_tags(["input"])
            .add_tag_attributes("input", ["checked", "disabled", "type"]);
        let html = sanitizer.clean(&untrusted_html).to_string();

        RenderedMarkdown { html }
    }
}
