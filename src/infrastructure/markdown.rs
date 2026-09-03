use ammonia::Builder;
use comrak::{Options, markdown_to_html};

use crate::{
    application::ports::{MarkdownRenderer, RenderedMarkdown},
    domain::{html::push_escaped, search},
};

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
        // Prefix keeps user-authored ids from clobbering page DOM; in_href keeps
        // the generated anchors self-consistent with the prefixed ids.
        options.extension.header_id_prefix = Some("user-content-".into());
        options.extension.header_id_prefix_in_href = true;
        options.render.github_pre_lang = true;
        options.render.r#unsafe = false;

        Self { options }
    }
}

impl MarkdownRenderer for ComrakMarkdownRenderer {
    fn render(&self, markdown: &str) -> RenderedMarkdown {
        let untrusted_html = markdown_to_html(markdown, &self.options);
        let mut sanitizer = Builder::default();
        sanitizer
            .add_tags(["input", "section"])
            .add_tag_attributes("input", ["checked", "disabled", "type"])
            // Footnote and heading anchors need their ids and data attributes to
            // survive sanitization, or the in-page links point at nothing. All
            // generated ids carry a comrak prefix (user-content- / fn- / fnref-),
            // so allowing them cannot clobber page-level DOM.
            .add_tag_attributes(
                "a",
                [
                    "id",
                    "aria-label",
                    "data-footnote-ref",
                    "data-footnote-backref",
                    "data-footnote-backref-idx",
                    "data-heading-content",
                ],
            )
            .add_tag_attributes("li", ["id"])
            .add_tag_attributes("h1", ["id"])
            .add_tag_attributes("h2", ["id"])
            .add_tag_attributes("h3", ["id"])
            .add_tag_attributes("h4", ["id"])
            .add_tag_attributes("h5", ["id"])
            .add_tag_attributes("h6", ["id"])
            .add_tag_attributes("section", ["data-footnotes"])
            .add_allowed_classes("a", ["anchor", "footnote-backref"])
            .add_allowed_classes("sup", ["footnote-ref"])
            .add_allowed_classes("section", ["footnotes"]);
        let html = sanitizer.clean(&untrusted_html).to_string();
        let html = apply_aozora_ruby(&html);
        let html = highlight_code_blocks(&html);

        RenderedMarkdown { html }
    }
}

/// Aozora-bunko ruby notation:「漢字《かんじ》」rubies the preceding kanji
/// run;「｜親文字《るび》」(ASCII `|` works too) marks the base explicitly.
///
/// Markdown's Anglosphere heritage left 日本語 without ruby; this closes the
/// gap with the notation Japanese writing already uses, without opening the
/// raw-HTML door. It runs on the sanitizer's output — text is already
/// escaped, and the only markup added is written by this function itself.
/// Anything inside `pre` or `code` stays literal.
fn apply_aozora_ruby(html: &str) -> String {
    let mut output = String::with_capacity(html.len());
    let mut rest = html;
    let mut literal_depth = 0_usize;
    while let Some(open) = rest.find('<') {
        let (text, tail) = rest.split_at(open);
        if literal_depth == 0 {
            output.push_str(&ruby_in_text(text));
        } else {
            output.push_str(text);
        }
        let Some(close) = tail.find('>') else {
            output.push_str(tail);
            return output;
        };
        let tag = &tail[..=close];
        let name: String = tag[1..]
            .trim_start_matches('/')
            .chars()
            .take_while(char::is_ascii_alphanumeric)
            .collect();
        if matches!(name.as_str(), "pre" | "code") {
            if tag.starts_with("</") {
                literal_depth = literal_depth.saturating_sub(1);
            } else {
                literal_depth += 1;
            }
        }
        output.push_str(tag);
        rest = &tail[close + 1..];
    }
    if literal_depth == 0 {
        output.push_str(&ruby_in_text(rest));
    } else {
        output.push_str(rest);
    }
    output
}

/// Characters the marker-less form treats as a ruby base: kanji plus the
/// iteration and closing marks that ride along with them.
const fn is_ruby_base_char(character: char) -> bool {
    matches!(character,
        '\u{3400}'..='\u{4DBF}'
        | '\u{4E00}'..='\u{9FFF}'
        | '\u{F900}'..='\u{FAFF}'
        | '々' | '〆' | '〇' | 'ヶ')
}

fn ruby_in_text(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut output = String::with_capacity(text.len());
    // The text run currently waiting to be emitted; a ruby base is carved
    // off its tail when a《…》annotation arrives.
    let mut pending: Vec<char> = Vec::new();
    let mut index = 0;
    while index < chars.len() {
        let character = chars[index];
        if character != '《' {
            pending.push(character);
            index += 1;
            continue;
        }
        let Some(length) = chars[index + 1..].iter().position(|&c| c == '》') else {
            pending.extend(&chars[index..]);
            break;
        };
        let annotation: String = chars[index + 1..index + 1 + length].iter().collect();
        // The marker itself, when present, is consumed with the base.
        let base_start = pending
            .iter()
            .rposition(|&c| c == '｜' || c == '|')
            .unwrap_or_else(|| {
                let mut start = pending.len();
                while start > 0 && is_ruby_base_char(pending[start - 1]) {
                    start -= 1;
                }
                start
            });
        let marked = matches!(pending.get(base_start), Some('｜' | '|'));
        let base: String = pending[base_start + usize::from(marked)..].iter().collect();
        if annotation.is_empty() || base.is_empty() {
            pending.push('《');
            index += 1;
            continue;
        }
        pending.truncate(base_start);
        let kept: String = pending.iter().collect();
        output.push_str(&kept);
        pending.clear();
        output.push_str("<ruby>");
        output.push_str(&base);
        output.push_str("<rp>（</rp><rt>");
        output.push_str(&annotation);
        output.push_str("</rt><rp>）</rp></ruby>");
        index += length + 2;
    }
    let kept: String = pending.iter().collect();
    output.push_str(&kept);
    output
}

/// Server-side syntax highlighting over the sanitizer's output, so the public
/// site stays JavaScript-free. Only this renderer's own well-formed
/// `<pre lang="…"><code>…</code></pre>` shape is rewritten; the highlighted
/// spans are generated (and escaped) by the dependency-free lexer below,
/// never taken from input. It intentionally recognizes a small stable token
/// contract rather than loading executable or serialized grammar bundles.
fn highlight_code_blocks(html: &str) -> String {
    let mut output = String::with_capacity(html.len());
    let mut rest = html;
    while let Some(start) = rest.find("<pre lang=\"") {
        let (before, tail) = rest.split_at(start);
        output.push_str(before);
        let Some(highlighted) = try_highlight_block(tail) else {
            output.push_str("<pre");
            rest = &tail["<pre".len()..];
            continue;
        };
        output.push_str(&highlighted.html);
        rest = &tail[highlighted.consumed..];
    }
    output.push_str(rest);
    output
}

struct HighlightedBlock {
    html: String,
    consumed: usize,
}

fn try_highlight_block(tail: &str) -> Option<HighlightedBlock> {
    let lang_start = "<pre lang=\"".len();
    let lang_end = lang_start + tail[lang_start..].find('"')?;
    let lang = &tail[lang_start..lang_end];
    let code_open = "<code>";
    let code_start = lang_end + tail[lang_end..].find(code_open)? + code_open.len();
    let code_end = code_start + tail[code_start..].find("</code></pre>")?;
    let consumed = code_end + "</code></pre>".len();

    let profile = language_profile(lang)?;
    let raw_code = search::decode_entities(&tail[code_start..code_end]);
    let spans = highlight_tokens(&raw_code, profile);
    Some(HighlightedBlock {
        html: format!("<pre lang=\"{lang}\"><code>{spans}</code></pre>"),
        consumed,
    })
}

#[derive(Clone, Copy)]
enum LanguageProfile {
    CLike,
    HashComment,
    Sql,
    Markup,
    Data,
}

fn language_profile(language: &str) -> Option<LanguageProfile> {
    match language {
        "rust" | "rs" | "javascript" | "js" | "typescript" | "ts" | "go" | "java" | "c" | "cpp"
        | "c++" | "csharp" | "cs" | "css" => Some(LanguageProfile::CLike),
        "python" | "py" | "ruby" | "rb" | "bash" | "sh" | "shell" | "toml" | "yaml" | "yml" => {
            Some(LanguageProfile::HashComment)
        }
        "sql" => Some(LanguageProfile::Sql),
        "html" | "xml" | "markdown" | "md" => Some(LanguageProfile::Markup),
        "json" => Some(LanguageProfile::Data),
        _ => None,
    }
}

fn highlight_tokens(code: &str, profile: LanguageProfile) -> String {
    let mut output = String::with_capacity(code.len());
    let mut index = 0;
    while index < code.len() {
        let rest = &code[index..];
        if let Some(end) = comment_end(rest, profile) {
            highlighted(&mut output, "hl-comment", &rest[..end]);
            index += end;
            continue;
        }
        let byte = code.as_bytes()[index];
        if matches!(byte, b'\'' | b'"' | b'`') {
            let end = string_end(rest, byte);
            highlighted(&mut output, "hl-string", &rest[..end]);
            index += end;
            continue;
        }
        if byte.is_ascii_alphabetic() || byte == b'_' {
            let end = rest
                .bytes()
                .take_while(|candidate| candidate.is_ascii_alphanumeric() || *candidate == b'_')
                .count();
            let word = &rest[..end];
            if let Some(class) = word_class(word, profile) {
                highlighted(&mut output, class, word);
            } else {
                push_escaped(&mut output, word);
            }
            index += end;
            continue;
        }
        if byte.is_ascii_digit() {
            let end = rest
                .bytes()
                .take_while(|candidate| {
                    candidate.is_ascii_alphanumeric() || matches!(candidate, b'_' | b'.')
                })
                .count();
            highlighted(&mut output, "hl-constant hl-numeric", &rest[..end]);
            index += end;
            continue;
        }
        let length = rest.chars().next().map_or(1, char::len_utf8);
        push_escaped(&mut output, &rest[..length]);
        index += length;
    }
    output
}

fn comment_end(code: &str, profile: LanguageProfile) -> Option<usize> {
    let line = match profile {
        LanguageProfile::CLike if code.starts_with("//") => Some(2),
        LanguageProfile::HashComment if code.starts_with('#') => Some(1),
        LanguageProfile::Sql if code.starts_with("--") => Some(2),
        _ => None,
    };
    if let Some(prefix) = line {
        return Some(
            code.find('\n')
                .map_or(code.len(), |newline| newline + 1)
                .max(prefix),
        );
    }
    let block_end = match profile {
        LanguageProfile::CLike if code.starts_with("/*") => code.find("*/").map(|end| end + 2),
        LanguageProfile::Markup if code.starts_with("<!--") => code.find("-->").map(|end| end + 3),
        _ => None,
    };
    block_end.or_else(|| {
        matches!(profile, LanguageProfile::CLike | LanguageProfile::Markup)
            .then_some(code.len())
            .filter(|_| code.starts_with("/*") || code.starts_with("<!--"))
    })
}

fn string_end(code: &str, quote: u8) -> usize {
    let mut index = 1;
    let mut escaped = false;
    while index < code.len() {
        let rest = &code[index..];
        let character = rest.chars().next().unwrap_or_default();
        let length = character.len_utf8();
        if character == '\n' {
            return index;
        }
        if !escaped && character as u32 == u32::from(quote) {
            return index + length;
        }
        escaped = character == '\\' && !escaped;
        index += length;
    }
    code.len()
}

fn word_class(word: &str, profile: LanguageProfile) -> Option<&'static str> {
    if matches!(profile, LanguageProfile::Sql)
        && ["select", "from", "where", "insert", "update", "delete"]
            .iter()
            .any(|keyword| word.eq_ignore_ascii_case(keyword))
    {
        Some("hl-keyword")
    } else if matches!(
        word,
        "fn" | "let"
            | "const"
            | "static"
            | "var"
            | "class"
            | "struct"
            | "enum"
            | "interface"
            | "type"
            | "def"
            | "function"
    ) {
        Some("hl-storage")
    } else if matches!(
        word,
        "if" | "else"
            | "for"
            | "while"
            | "loop"
            | "match"
            | "return"
            | "break"
            | "continue"
            | "async"
            | "await"
            | "use"
            | "mod"
            | "pub"
            | "impl"
            | "trait"
            | "where"
            | "from"
            | "import"
            | "in"
    ) {
        Some("hl-keyword")
    } else if matches!(
        word,
        "true" | "false" | "null" | "None" | "Some" | "Ok" | "Err"
    ) {
        Some("hl-constant")
    } else if matches!(
        word,
        "bool" | "char" | "str" | "String" | "int" | "float" | "number" | "void"
    ) {
        Some("hl-entity")
    } else {
        None
    }
}

fn highlighted(output: &mut String, class: &str, value: &str) {
    output.push_str("<span class=\"");
    output.push_str(class);
    output.push_str("\">");
    push_escaped(output, value);
    output.push_str("</span>");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lexer_handles_comments_literals_numbers_and_unterminated_input() {
        let c_like = highlight_tokens(
            "// note\nconst answer: bool = 42; /* tail",
            LanguageProfile::CLike,
        );
        assert!(c_like.contains("hl-comment"));
        assert!(c_like.contains("hl-storage"));
        assert!(c_like.contains("hl-entity"));
        assert!(c_like.contains("hl-numeric"));

        let python = highlight_tokens("# note\nreturn 'it\\\'s", LanguageProfile::HashComment);
        assert!(python.contains("hl-comment"));
        assert!(python.contains("hl-keyword"));
        assert!(python.contains("hl-string"));

        let markup = highlight_tokens("<!-- never closes", LanguageProfile::Markup);
        assert!(markup.contains("hl-comment"));
        assert_eq!(
            word_class("SeLeCt", LanguageProfile::Sql),
            Some("hl-keyword")
        );
        assert_eq!(
            word_class("true", LanguageProfile::Data),
            Some("hl-constant")
        );
    }
}
