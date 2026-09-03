//! Decorates a released body so images load the way readers deserve.
//!
//! The editor stores exactly what Markdown produced; this pass runs on the
//! compiled page only. Every `<img>` that points at a known asset gains its
//! intrinsic size (no layout shift), lazy loading, and the responsive WebP
//! variants the upload already generated. An image that stands alone in its
//! paragraph becomes a `<figure>`, with the Markdown title as its caption.
//!
//! Interpretation: a figure inside a paragraph is invalid HTML, so an image
//! that shares its paragraph with text or sits inside a link keeps the
//! paragraph and gains only the picture and the attributes. Content inside
//! `<pre>` and `<code>` is never touched, and an image already wrapped in a
//! `<picture>` is left alone, so the pass is idempotent.

use std::collections::HashMap;

use crate::domain::{
    html::push_escaped,
    media::{MediaAsset, media_id_from_path},
};

/// The same slot the cover uses, so both scale together with the measure.
pub const BODY_IMAGE_SIZES: &str = "(max-width: 700px) 100vw, 640px";

#[must_use]
pub fn decorate_body_media(html: &str, media: &HashMap<&str, &MediaAsset>) -> String {
    let mut output = String::with_capacity(html.len() + html.len() / 4);
    let mut rest = html;
    let mut literal_depth = 0_usize;
    let mut picture_depth = 0_usize;
    while let Some(open) = rest.find('<') {
        let (text, tail) = rest.split_at(open);
        output.push_str(text);
        let Some(close) = tail.find('>') else {
            output.push_str(tail);
            return output;
        };
        let tag = &tail[..=close];
        let closing = tag.starts_with("</");
        let name: String = tag[if closing { 2 } else { 1 }..]
            .chars()
            .take_while(char::is_ascii_alphanumeric)
            .collect();
        match name.as_str() {
            "pre" | "code" => literal_depth = nest(literal_depth, closing),
            "picture" => picture_depth = nest(picture_depth, closing),
            "img" if !closing && literal_depth == 0 && picture_depth == 0 => {
                if let Some(consumed) = decorate_image(tag, &tail[close + 1..], &mut output, media)
                {
                    rest = &tail[close + 1 + consumed..];
                    continue;
                }
            }
            _ => {}
        }
        output.push_str(tag);
        rest = &tail[close + 1..];
    }
    output.push_str(rest);
    output
}

const fn nest(depth: usize, closing: bool) -> usize {
    if closing {
        depth.saturating_sub(1)
    } else {
        depth + 1
    }
}

/// Rewrites one `<img>` tag into `output`. Answers how many bytes after the
/// tag were consumed (the closing `</p>` of a figure), or `None` when the
/// image is not one of ours and must be copied verbatim by the caller.
fn decorate_image(
    tag: &str,
    after: &str,
    output: &mut String,
    media: &HashMap<&str, &MediaAsset>,
) -> Option<usize> {
    let attributes = attributes(tag);
    let src = attributes
        .iter()
        .find_map(|(name, value)| (*name == "src").then_some(*value))?;
    let asset = media.get(media_id_from_path(src)?.as_str())?;

    // A sole image in a paragraph: the paragraph becomes the figure.
    let paragraph_open = output.trim_end().len();
    let sole = output[..paragraph_open].ends_with("<p>") && {
        let following = after.trim_start_matches([' ', '\n', '\r', '\t']);
        following.starts_with("</p>")
    };
    let caption = sole
        .then(|| {
            attributes
                .iter()
                .find_map(|(name, value)| (*name == "title" && !value.is_empty()).then_some(*value))
        })
        .flatten();
    if sole {
        output.truncate(paragraph_open - "<p>".len());
        output.push_str("<figure>");
    }
    let responsive = !asset.variants.is_empty();
    if responsive {
        output.push_str("<picture><source type=\"image/webp\" srcset=\"");
        for (index, variant) in asset.variants.iter().enumerate() {
            if index > 0 {
                output.push_str(", ");
            }
            output.push_str("/media/");
            push_escaped(output, &variant.filename);
            output.push(' ');
            output.push_str(&variant.width.to_string());
            output.push('w');
        }
        output.push_str("\" sizes=\"");
        output.push_str(BODY_IMAGE_SIZES);
        output.push_str("\">");
    }
    push_image(output, &attributes, asset, sole);
    if responsive {
        output.push_str("</picture>");
    }
    if sole {
        if let Some(caption) = caption {
            // Attribute text is already entity-escaped by the sanitizer and
            // stays valid as element text.
            output.push_str("<figcaption>");
            output.push_str(caption);
            output.push_str("</figcaption>");
        }
        output.push_str("</figure>");
        let whitespace = after.len() - after.trim_start_matches([' ', '\n', '\r', '\t']).len();
        return Some(whitespace + "</p>".len());
    }
    Some(0)
}

fn push_image(output: &mut String, attributes: &[(&str, &str)], asset: &MediaAsset, figure: bool) {
    output.push_str("<img");
    let mut has_alt = false;
    for (name, value) in attributes {
        match *name {
            "title" if figure => continue,
            "alt" => {
                has_alt = true;
                output.push_str(" alt=\"");
                if value.is_empty() {
                    push_escaped(output, &asset.alt_text);
                } else {
                    output.push_str(value);
                }
                output.push('"');
            }
            _ => {
                output.push(' ');
                output.push_str(name);
                output.push_str("=\"");
                output.push_str(value);
                output.push('"');
            }
        }
    }
    if !has_alt {
        output.push_str(" alt=\"");
        push_escaped(output, &asset.alt_text);
        output.push('"');
    }
    let has = |wanted: &str| attributes.iter().any(|(name, _)| *name == wanted);
    if !has("width") && !has("height") {
        output.push_str(&format!(
            " width=\"{}\" height=\"{}\"",
            asset.width, asset.height
        ));
    }
    if !has("loading") {
        output.push_str(" loading=\"lazy\"");
    }
    if !has("decoding") {
        output.push_str(" decoding=\"async\"");
    }
    output.push('>');
}

/// The `name="value"` pairs of a sanitized tag. The sanitizer always quotes
/// values with double quotes and escapes any quote inside them, so a plain
/// scan is exact for the markup this pass receives.
fn attributes(tag: &str) -> Vec<(&str, &str)> {
    let mut pairs = Vec::new();
    let mut rest = tag
        .strip_prefix("<img")
        .unwrap_or(tag)
        .trim_end_matches('>')
        .trim_end_matches('/');
    loop {
        rest = rest.trim_start();
        let Some(equals) = rest.find('=') else {
            break;
        };
        let name = rest[..equals].trim();
        let Some(quoted) = rest[equals + 1..].strip_prefix('"') else {
            break;
        };
        let Some(end) = quoted.find('"') else {
            break;
        };
        if !name.is_empty() {
            pairs.push((name, &quoted[..end]));
        }
        rest = &quoted[end + 1..];
    }
    pairs
}

#[cfg(test)]
mod tests {
    use chrono::Utc;

    use super::*;
    use crate::domain::media::{MediaId, MediaVariant};

    fn asset() -> MediaAsset {
        let id = MediaId::parse("a".repeat(64)).unwrap();
        MediaAsset {
            original_name: "sunset.png".into(),
            original_filename: format!("{id}.webp"),
            mime_type: "image/webp".into(),
            extension: "webp".into(),
            width: 1600,
            height: 900,
            byte_size: 1,
            alt_text: "Blue & calm".into(),
            caption: String::new(),
            animated: false,
            variants: vec![
                MediaVariant {
                    width: 480,
                    height: 270,
                    byte_size: 1,
                    filename: format!("{id}-480w.webp"),
                },
                MediaVariant {
                    width: 960,
                    height: 540,
                    byte_size: 1,
                    filename: format!("{id}-960w.webp"),
                },
            ],
            created_at: Utc::now(),
            id,
        }
    }

    fn decorate(html: &str) -> String {
        let asset = asset();
        let media = HashMap::from([(asset.id.as_str(), &asset)]);
        decorate_body_media(html, &media)
    }

    fn src() -> String {
        format!("/media/{}.webp", "a".repeat(64))
    }

    #[test]
    fn sole_image_paragraph_becomes_a_figure_with_a_caption_from_title() {
        let html = format!("<p><img src=\"{}\" alt=\"\" title=\"Sunset\"></p>", src());
        let decorated = decorate(&html);
        let id = "a".repeat(64);
        assert_eq!(
            decorated,
            format!(
                "<figure><picture><source type=\"image/webp\" srcset=\"/media/{id}-480w.webp 480w, /media/{id}-960w.webp 960w\" sizes=\"{BODY_IMAGE_SIZES}\"><img src=\"/media/{id}.webp\" alt=\"Blue &amp; calm\" width=\"1600\" height=\"900\" loading=\"lazy\" decoding=\"async\"></picture><figcaption>Sunset</figcaption></figure>"
            )
        );
    }

    #[test]
    fn image_inside_a_link_or_amid_text_gets_a_picture_but_no_figure() {
        let linked = format!(
            "<p><a href=\"/x/\"><img src=\"{}\" alt=\"Link\"></a></p>",
            src()
        );
        let decorated = decorate(&linked);
        assert!(decorated.starts_with("<p><a href=\"/x/\"><picture>"));
        assert!(decorated.ends_with("</picture></a></p>"));
        assert!(!decorated.contains("<figure>"));
        assert!(decorated.contains("alt=\"Link\""));

        let inline = format!("<p>Look: <img src=\"{}\" alt=\"Inline\"> here.</p>", src());
        let decorated = decorate(&inline);
        assert!(decorated.starts_with("<p>Look: <picture>"));
        assert!(!decorated.contains("<figure>"));
    }

    #[test]
    fn unknown_media_and_literal_blocks_are_left_untouched() {
        let other = "<p><img src=\"/media/ffff.webp\" alt=\"\"></p>";
        assert_eq!(decorate(other), other);
        let external = "<p><img src=\"https://example.com/a.png\" alt=\"\"></p>";
        assert_eq!(decorate(external), external);
        let literal = format!("<pre><code>&lt;img src=\"{}\"&gt;</code></pre>", src());
        assert_eq!(decorate(&literal), literal);
        let raw_in_code = format!("<p><code><img src=\"{}\" alt=\"\"></code></p>", src());
        assert_eq!(decorate(&raw_in_code), raw_in_code);
    }

    #[test]
    fn existing_dimensions_are_kept_and_decorating_twice_is_idempotent() {
        let sized = format!(
            "<p><img src=\"{}\" alt=\"x\" width=\"800\" height=\"450\" loading=\"eager\"></p>",
            src()
        );
        let once = decorate(&sized);
        assert!(once.contains("width=\"800\" height=\"450\""));
        assert!(once.contains("loading=\"eager\""));
        assert!(!once.contains("loading=\"lazy\""));
        assert_eq!(decorate(&once), once);
    }

    #[test]
    fn assets_without_variants_keep_a_plain_image() {
        let mut plain = asset();
        plain.variants.clear();
        let media = HashMap::from([(plain.id.as_str(), &plain)]);
        let decorated =
            decorate_body_media(&format!("<p><img src=\"{}\" alt=\"\"></p>", src()), &media);
        assert!(decorated.starts_with("<figure><img "));
        assert!(!decorated.contains("<picture>"));
        assert!(decorated.ends_with("></figure>"));
    }

    #[test]
    fn malformed_tails_never_panic() {
        for html in [
            "<img",
            "<p><img src=\"",
            "<img src=\"/media/",
            "<p><img src=\"x\" alt=\"y",
        ] {
            let _ = decorate(html);
        }
        assert_eq!(decorate("<p><img"), "<p><img");
    }
}
