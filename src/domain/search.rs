//! CJK-first search text processing.
//!
//! Everything here exists to make search work as well for 日本語 as for
//! English, which the usual "split on whitespace and stem" pipeline does not:
//!
//! - Text is NFKC-normalized (full-width ＡＢＣ１２３ meet their ASCII
//!   selves, ﾊﾝｶｸｶﾅ becomes full-width) and then *folded* one character at a
//!   time — ASCII lowercased, katakana mapped to hiragana — so「サーバ」,
//!   「さーば」, "RUST" and "rust" all land on the same index terms.
//! - Folding is strictly one-char-to-one-char on already-normalized text, so
//!   a character index found while scanning the folded text is directly
//!   usable in the displayable text. Snippets highlight the original.
//! - The FTS index uses the trigram tokenizer, which handles CJK substrings
//!   of three or more characters. Two-character queries — the bread and
//!   butter of Japanese (東京, 紅茶, 検索…) — cannot match a trigram index,
//!   so terms are split into FTS terms and LIKE terms and the repository
//!   combines both.

use unicode_normalization::UnicodeNormalization;

/// Queries longer than this are truncated; a search box is not an essay box.
const MAX_QUERY_CHARS: usize = 120;
/// At most this many distinct terms take part in a query.
const MAX_TERMS: usize = 8;
/// A term with at least this many characters can match the trigram index.
const TRIGRAM_MIN_CHARS: usize = 3;

/// NFKC normalization: the canonical form both stored and displayed.
#[must_use]
pub fn normalize(text: &str) -> String {
    text.nfkc().collect()
}

/// One-character fold on top of NFKC: ASCII lowercase, katakana → hiragana.
/// The one-to-one guarantee is what lets snippet positions transfer from the
/// folded haystack back to the displayable one.
#[must_use]
pub const fn fold_char(character: char) -> char {
    match character {
        'A'..='Z' => character.to_ascii_lowercase(),
        // ァ (30A1) through ヶ (30F6), including ヴ, sit exactly 0x60 above
        // their hiragana counterparts. ー and ・ are shared and stay put.
        '\u{30A1}'..='\u{30F6}' => {
            // Safety of the unwrap-free path: the range maps into 3041..=3096,
            // all assigned hiragana.
            match char::from_u32(character as u32 - 0x60) {
                Some(folded) => folded,
                None => character,
            }
        }
        _ => character,
    }
}

/// Folds an already-normalized string. Same character count as the input.
#[must_use]
pub fn fold(normalized: &str) -> String {
    normalized.chars().map(fold_char).collect()
}

/// A parsed search query: terms that can use the trigram FTS index and terms
/// that must fall back to substring scans.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SearchTerms {
    /// Terms of three or more characters, already folded.
    pub fts: Vec<String>,
    /// One- and two-character terms, already folded.
    pub like: Vec<String>,
}

impl SearchTerms {
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.fts.is_empty() && self.like.is_empty()
    }

    /// Every term, for highlighting.
    #[must_use]
    pub fn all(&self) -> Vec<&str> {
        self.fts
            .iter()
            .chain(self.like.iter())
            .map(String::as_str)
            .collect()
    }
}

/// Splits a raw query into folded terms. NFKC also converts the ideographic
/// space (U+3000) to an ASCII space, so Japanese input segments without any
/// special casing.
#[must_use]
pub fn parse_query(raw: &str) -> SearchTerms {
    let folded = fold(&normalize(raw));
    let clipped: String = folded.chars().take(MAX_QUERY_CHARS).collect();
    let mut terms = SearchTerms::default();
    let mut seen: Vec<&str> = Vec::new();
    for term in clipped.split_whitespace() {
        if seen.contains(&term) {
            continue;
        }
        seen.push(term);
        if seen.len() > MAX_TERMS {
            break;
        }
        if term.chars().count() >= TRIGRAM_MIN_CHARS {
            terms.fts.push(term.to_owned());
        } else {
            terms.like.push(term.to_owned());
        }
    }
    terms
}

/// Escapes a folded term for `LIKE ... ESCAPE '\'`.
#[must_use]
pub fn escape_like(term: &str) -> String {
    let mut escaped = String::with_capacity(term.len());
    for character in term.chars() {
        if matches!(character, '%' | '_' | '\\') {
            escaped.push('\\');
        }
        escaped.push(character);
    }
    escaped
}

/// Quotes a folded term for an FTS5 MATCH expression. Inside double quotes
/// only the double quote itself is special.
#[must_use]
pub fn quote_fts(term: &str) -> String {
    format!("\"{}\"", term.replace('"', "\"\""))
}

/// Reduces sanitized HTML to plain text for indexing and snippets.
///
/// The input is this application's own ammonia output, so tags are
/// well-formed. Block boundaries become spaces; inline tags vanish without
/// splitting words — CJK prose must not grow stray spaces around every `<em>`.
#[must_use]
pub fn html_to_text(html: &str) -> String {
    const BLOCK_TAGS: &[&str] = &[
        "p",
        "div",
        "li",
        "ul",
        "ol",
        "dl",
        "dt",
        "dd",
        "h1",
        "h2",
        "h3",
        "h4",
        "h5",
        "h6",
        "blockquote",
        "pre",
        "table",
        "tr",
        "td",
        "th",
        "figure",
        "figcaption",
        "section",
        "br",
        "hr",
        "summary",
        "details",
    ];
    let mut text = String::with_capacity(html.len());
    let mut rest = html;
    while let Some(open) = rest.find('<') {
        text.push_str(&rest[..open]);
        let Some(close) = rest[open..].find('>') else {
            rest = "";
            break;
        };
        let tag = &rest[open + 1..open + close];
        let name: String = tag
            .trim_start_matches('/')
            .chars()
            .take_while(char::is_ascii_alphanumeric)
            .collect();
        if BLOCK_TAGS.contains(&name.to_ascii_lowercase().as_str()) {
            text.push(' ');
        }
        rest = &rest[open + close + 1..];
    }
    text.push_str(rest);
    let decoded = decode_entities(&text);
    // Collapse whitespace runs; the sources are HTML where runs carry no meaning.
    let mut collapsed = String::with_capacity(decoded.len());
    let mut in_space = true;
    for character in decoded.chars() {
        if character.is_whitespace() {
            if !in_space {
                collapsed.push(' ');
            }
            in_space = true;
        } else {
            collapsed.push(character);
            in_space = false;
        }
    }
    collapsed.trim_end().to_owned()
}

/// Decodes the named and numeric entities this application's own sanitized
/// HTML can contain.
///
/// Also used to recover raw code text before highlighting.
#[must_use]
pub fn decode_entities(text: &str) -> String {
    let mut decoded = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find('&') {
        decoded.push_str(&rest[..start]);
        let tail = &rest[start..];
        let Some(end) = tail[..tail.len().min(12)].find(';') else {
            decoded.push('&');
            rest = &tail[1..];
            continue;
        };
        let entity = &tail[1..end];
        let replacement = match entity {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            _ => entity.strip_prefix('#').and_then(|digits| {
                let code = digits.strip_prefix(['x', 'X']).map_or_else(
                    || digits.parse::<u32>().ok(),
                    |hex| u32::from_str_radix(hex, 16).ok(),
                )?;
                char::from_u32(code)
            }),
        };
        if let Some(replacement) = replacement {
            decoded.push(replacement);
            rest = &tail[end + 1..];
        } else {
            decoded.push('&');
            rest = &tail[1..];
        }
    }
    decoded.push_str(rest);
    decoded
}

/// One run of snippet text; `hit` marks a matched term to highlight.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Segment {
    pub text: String,
    pub hit: bool,
}

/// A snippet of `display` centered on the first term match.
///
/// Every term occurrence inside the window is marked. `display` must be
/// normalized text and `terms` folded terms; matching happens on a fold of
/// `display`, and the one-to-one fold guarantee carries the positions back.
#[must_use]
pub fn excerpt(display: &str, terms: &[&str], window: usize) -> (Vec<Segment>, bool, bool) {
    let display_chars: Vec<char> = display.chars().collect();
    let folded_chars: Vec<char> = display_chars.iter().map(|&c| fold_char(c)).collect();
    let term_chars: Vec<Vec<char>> = terms
        .iter()
        .filter(|term| !term.is_empty())
        .map(|term| term.chars().collect())
        .collect();

    let first_hit = term_chars
        .iter()
        .filter_map(|term| find_from(&folded_chars, term, 0))
        .min();

    let center = first_hit.unwrap_or(0);
    let start = center.saturating_sub(window / 3);
    let end = (start + window).min(display_chars.len());
    let start = end.saturating_sub(window).min(start);

    let mut segments = Vec::new();
    let mut plain_start = start;
    let mut index = start;
    while index < end {
        // Longest match first so「検索エンジン」wins over「検索」at the same spot.
        let hit_length = term_chars
            .iter()
            .filter(|term| folded_chars[index..].starts_with(term))
            .map(Vec::len)
            .max();
        if let Some(length) = hit_length {
            let length = length.min(end - index);
            if plain_start < index {
                segments.push(Segment {
                    text: display_chars[plain_start..index].iter().collect(),
                    hit: false,
                });
            }
            segments.push(Segment {
                text: display_chars[index..index + length].iter().collect(),
                hit: true,
            });
            index += length;
            plain_start = index;
        } else {
            index += 1;
        }
    }
    if plain_start < end {
        segments.push(Segment {
            text: display_chars[plain_start..end].iter().collect(),
            hit: false,
        });
    }
    (segments, start > 0, end < display_chars.len())
}

fn find_from(haystack: &[char], needle: &[char], from: usize) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    (from..=haystack.len() - needle.len()).find(|&i| haystack[i..].starts_with(needle))
}
