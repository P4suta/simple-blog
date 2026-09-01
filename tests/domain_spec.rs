use chrono::{Duration, TimeZone, Utc};
use simple_blog::{
    application::auth::hash_secret,
    application::ports::MarkdownRenderer,
    domain::{
        auth::{SecretToken, SetupPurpose},
        content::{ContentKind, Publication, Slug},
    },
    infrastructure::markdown::ComrakMarkdownRenderer,
};

#[test]
fn slug_accepts_a_stable_url_segment() {
    let slug = Slug::parse("hello-rust-2026").expect("valid slug");

    assert_eq!(slug.as_str(), "hello-rust-2026");
    assert_eq!(slug.to_string(), "hello-rust-2026");
}

#[test]
fn slug_rejects_reserved_or_unsafe_segments() {
    for invalid in ["", "Admin", "two words", "../escape", "archive", "feed.xml"] {
        assert!(
            Slug::parse(invalid).is_err(),
            "{invalid:?} must be rejected"
        );
    }
}

#[test]
fn scheduled_publication_becomes_visible_without_a_job() {
    let now = Utc.with_ymd_and_hms(2026, 8, 30, 12, 0, 0).unwrap();
    let publication = Publication::Public {
        publish_at: now + Duration::minutes(5),
    };

    assert!(!publication.is_visible_at(now));
    assert!(publication.is_visible_at(now + Duration::minutes(5)));
    assert!(!Publication::Draft.is_visible_at(now + Duration::days(1)));
}

#[test]
fn post_and_page_share_the_same_kind_boundary() {
    assert_eq!(ContentKind::Post.as_str(), "post");
    assert_eq!(ContentKind::Page.as_str(), "page");
    assert_eq!("page".parse::<ContentKind>().unwrap(), ContentKind::Page);
    assert!("unknown".parse::<ContentKind>().is_err());
}

#[test]
fn bearer_capabilities_are_redacted_and_purpose_parsing_fails_closed() {
    let token = SecretToken::new("must-never-be-rendered".into());
    let hash = hash_secret(token.expose());

    assert_eq!(token.to_string(), "[REDACTED]");
    assert_eq!(format!("{hash:?}"), "SecretHash([REDACTED])");
    assert_eq!(hash.as_bytes().len(), 32);
    assert_eq!(SetupPurpose::parse("setup"), Some(SetupPurpose::Initial));
    assert_eq!(SetupPurpose::parse("recover"), Some(SetupPurpose::Recovery));
    assert_eq!(SetupPurpose::parse("recovery"), None);
}

#[test]
fn markdown_is_gfm_like_and_raw_html_is_never_trusted() {
    let renderer = ComrakMarkdownRenderer::default();
    let output = renderer.render(
        "# Hello\n\n- [x] done\n\n<script>alert('xss')</script>\n\n[bad](javascript:alert(1))",
    );

    assert!(output.html.contains("<h1 id=\"user-content-hello\">"));
    assert!(output.html.contains("type=\"checkbox\""));
    assert!(!output.html.contains("<script"));
    assert!(!output.html.contains("javascript:"));
}

#[test]
fn timestamped_slugs_are_valid_and_collision_resistant() {
    let now = Utc.with_ymd_and_hms(2026, 8, 31, 21, 45, 7).unwrap();

    let minute = Slug::timestamped(now);
    let second = Slug::timestamped_precise(now);
    assert_eq!(minute.as_str(), "20260831-2145");
    assert_eq!(second.as_str(), "20260831-214507");
    assert!(minute.is_timestamped());
    assert!(second.is_timestamped());
    assert!(!Slug::parse("hello-rust-2026").unwrap().is_timestamped());
    // Round-trips through the same validation every hand-typed slug gets.
    assert!(Slug::parse(minute.as_str()).is_ok());
    assert!(Slug::parse(second.as_str()).is_ok());
}

#[test]
fn heading_anchors_and_footnote_links_survive_sanitization() {
    let renderer = ComrakMarkdownRenderer::default();
    let output = renderer.render("# Section\n\nBody with a note.[^1]\n\n[^1]: The note text.");

    // Heading: id and its self-link agree, both carrying the clobber prefix.
    assert!(output.html.contains("id=\"user-content-section\""));
    assert!(output.html.contains("href=\"#user-content-section\""));
    // Footnote: the reference points at a list item that still has its id,
    // and the backref returns to the reference.
    assert!(output.html.contains("href=\"#fn-1\""));
    assert!(output.html.contains("id=\"fn-1\""));
    assert!(output.html.contains("href=\"#fnref-1\""));
    assert!(output.html.contains("id=\"fnref-1\""));
    assert!(output.html.contains("class=\"footnotes\""));
}

#[test]
fn external_and_internal_links_render_clickable() {
    let renderer = ComrakMarkdownRenderer::default();
    let output = renderer.render("[out](https://example.com/x) and [in](/my-post/)");

    assert!(output.html.contains("href=\"https://example.com/x\""));
    assert!(output.html.contains("href=\"/my-post/\""));
}

#[test]
fn code_fences_are_highlighted_server_side_without_colors_of_their_own() {
    let renderer = ComrakMarkdownRenderer::default();
    let output = renderer.render("```rust\nfn main() {}\n```\n\n```unknownlang\nx\n```");

    // Known language: classed spans, generated server-side.
    assert!(output.html.contains("<pre lang=\"rust\">"));
    assert!(output.html.contains("class=\"hl-storage"));
    // No inline styles: the theme alone decides appearance.
    assert!(!output.html.contains("style="));
    // Unknown language: left as plain escaped code.
    assert!(output.html.contains("<pre lang=\"unknownlang\"><code>x"));
}

#[test]
fn code_highlighting_has_stable_language_neutral_token_classes() {
    let renderer = ComrakMarkdownRenderer::default();
    let output = renderer.render(
        "```python\n# note\ndef answer():\n    return 42\n```\n\n\
         ```sql\n-- note\nselect true from posts where id = 7\n```\n\n\
         ```html\n<!-- note -->\n<div>safe</div>\n```\n\n\
         ```json\n{\"enabled\": true, \"count\": 3}\n```",
    );

    for class in [
        "hl-comment",
        "hl-storage",
        "hl-keyword",
        "hl-constant",
        "hl-numeric",
        "hl-string",
    ] {
        assert!(output.html.contains(class), "missing token class {class}");
    }
    assert!(
        output
            .html
            .contains("<span class=\"hl-keyword\">select</span>"),
        "SQL keywords must be case-insensitive"
    );
    assert!(!output.html.contains("style="));
}

#[test]
fn highlighted_code_cannot_smuggle_markup() {
    let renderer = ComrakMarkdownRenderer::default();
    let output =
        renderer.render("```rust\nlet x = \"</code></pre><script>alert(1)</script>\";\n```");

    assert!(!output.html.contains("<script"));
}

#[test]
fn aozora_ruby_notation_becomes_ruby_markup() {
    let renderer = ComrakMarkdownRenderer::default();

    // Marker-less: the preceding kanji run (iteration marks included) is the base.
    let output = renderer.render("日々《にちにち》の振り仮名は漢字《かんじ》に付く。");
    assert!(
        output
            .html
            .contains("<ruby>日々<rp>（</rp><rt>にちにち</rt><rp>）</rp></ruby>")
    );
    assert!(
        output
            .html
            .contains("<ruby>漢字<rp>（</rp><rt>かんじ</rt><rp>）</rp></ruby>")
    );
    assert!(!output.html.contains('《'));

    // Explicit base with ｜ (or ASCII |) can cover any characters.
    let output = renderer.render("｜クリスマス・イヴ《聖夜》と|Tokyo《とうきょう》。");
    assert!(
        output
            .html
            .contains("<ruby>クリスマス・イヴ<rp>（</rp><rt>聖夜</rt><rp>）</rp></ruby>")
    );
    assert!(
        output
            .html
            .contains("<ruby>Tokyo<rp>（</rp><rt>とうきょう</rt><rp>）</rp></ruby>")
    );
}

#[test]
fn ruby_notation_stays_literal_in_code_and_without_a_base() {
    let renderer = ComrakMarkdownRenderer::default();

    let output = renderer.render("`漢字《かんじ》`\n\n```\n漢字《かんじ》\n```");
    assert!(!output.html.contains("<ruby>"));
    assert!(output.html.contains("《かんじ》"));

    // Kana cannot be an implicit base; the notation passes through untouched.
    let output = renderer.render("これ《これ》");
    assert!(!output.html.contains("<ruby>"));
    assert!(output.html.contains("これ《これ》"));
}
