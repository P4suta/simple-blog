//! Reader-facing derivations from sanitized article HTML: an estimated
//! reading time and a table of contents. Both are pure so they compile
//! deterministically into releases on every host adapter.

use serde::Serialize;

use crate::domain::search;

/// Characters per minute for scripts that carry meaning per character.
const CJK_CHARS_PER_MINUTE: u64 = 500;
/// Words per minute for scripts that separate words with spaces.
const WORDS_PER_MINUTE: u64 = 200;

/// Estimated minutes to read `text`, counting CJK code points and Latin
/// words separately and rounding up. Empty text is zero minutes; anything
/// else is at least one.
#[must_use]
pub fn reading_minutes(text: &str) -> u32 {
    let mut cjk = 0_u64;
    let mut words = 0_u64;
    let mut in_word = false;
    for character in text.chars() {
        if is_cjk(character) {
            cjk += 1;
            in_word = false;
        } else if character.is_alphanumeric() {
            if !in_word {
                words += 1;
                in_word = true;
            }
        } else {
            in_word = false;
        }
    }
    if cjk == 0 && words == 0 {
        return 0;
    }
    // minutes = cjk / 500 + words / 200, kept in integers: scale both to
    // thousandths of a minute and round up once.
    let thousandths = cjk * (1_000 / CJK_CHARS_PER_MINUTE) + words * (1_000 / WORDS_PER_MINUTE);
    u32::try_from(thousandths.div_ceil(1_000).max(1)).unwrap_or(u32::MAX)
}

const fn is_cjk(character: char) -> bool {
    matches!(
        character,
        '\u{1100}'..='\u{11FF}'   // Hangul Jamo
        | '\u{2E80}'..='\u{2FDF}' // CJK radicals
        | '\u{3040}'..='\u{309F}' // Hiragana
        | '\u{30A0}'..='\u{30FF}' // Katakana
        | '\u{3130}'..='\u{318F}' // Hangul compatibility Jamo
        | '\u{31F0}'..='\u{31FF}' // Katakana extensions
        | '\u{3400}'..='\u{4DBF}' // CJK extension A
        | '\u{4E00}'..='\u{9FFF}' // CJK unified ideographs
        | '\u{AC00}'..='\u{D7AF}' // Hangul syllables
        | '\u{F900}'..='\u{FAFF}' // CJK compatibility ideographs
        | '\u{FF00}'..='\u{FFEF}' // Halfwidth and fullwidth forms
        | '\u{20000}'..='\u{2FA1F}' // CJK extensions B–F
    )
}

/// One heading of the article outline. Second-level headings nest their
/// third-level followers; anything else is left out of the table of contents.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct OutlineEntry {
    pub id: String,
    pub text: String,
    pub children: Vec<Self>,
}

impl OutlineEntry {
    /// The entry itself plus every nested child.
    #[must_use]
    pub fn size(&self) -> usize {
        1 + self.children.iter().map(Self::size).sum::<usize>()
    }
}

/// Extracts `h2`/`h3` headings with ids from sanitized article HTML. The
/// renderer prefixes every id and wraps the heading text in a self-link
/// anchor; only the visible text survives here.
#[must_use]
pub fn outline(html: &str) -> Vec<OutlineEntry> {
    let mut entries: Vec<OutlineEntry> = Vec::new();
    let mut rest = html;
    while let Some(start) = rest.find("<h") {
        let tail = &rest[start..];
        let Some((level, after_tag)) = heading_open(tail) else {
            rest = &tail[2..];
            continue;
        };
        let Some(id) = attribute(after_tag, "id") else {
            rest = &tail[2..];
            continue;
        };
        let close = format!("</h{level}>");
        let Some(inner_start) = after_tag.find('>') else {
            break;
        };
        let inner = &after_tag[inner_start + 1..];
        let Some(inner_end) = inner.find(&close) else {
            break;
        };
        let text = search::html_to_text(&inner[..inner_end]).trim().to_owned();
        rest = &inner[inner_end + close.len()..];
        if text.is_empty() {
            continue;
        }
        let entry = OutlineEntry {
            id,
            text,
            children: Vec::new(),
        };
        match (level, entries.last_mut()) {
            (3, Some(parent)) => parent.children.push(entry),
            _ => entries.push(entry),
        }
    }
    entries
}

/// Recognizes `<h2 ` / `<h3 ` (and the bare `<h2>` forms) at the start of
/// `tail`, returning the level and the text after the tag name.
fn heading_open(tail: &str) -> Option<(u8, &str)> {
    let level = match tail.as_bytes().get(2)? {
        b'2' => 2,
        b'3' => 3,
        _ => return None,
    };
    let after = &tail[3..];
    after
        .starts_with([' ', '>', '\n', '\t'])
        .then_some((level, after))
}

fn attribute(tag: &str, name: &str) -> Option<String> {
    let end = tag.find('>')?;
    let inside = &tag[..end];
    let marker = format!("{name}=\"");
    let start = inside.find(&marker)? + marker.len();
    let value_end = inside[start..].find('"')?;
    Some(search::decode_entities(&inside[start..start + value_end]))
}
