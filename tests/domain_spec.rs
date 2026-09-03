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
    for invalid in [
        "",
        "Admin",
        "two words",
        "../escape",
        "archive",
        "feed.xml",
        "page",
        "tag",
    ] {
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
fn unterminated_code_strings_do_not_consume_later_lines() {
    let renderer = ComrakMarkdownRenderer::default();
    let output = renderer.render("```rust\nlet value = \"unfinished\nfn main() {}\n```");

    assert!(
        output
            .html
            .contains("unfinished</span>\n<span class=\"hl-storage\">fn</span>")
    );
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
#[test]
fn is_scheduled_at_is_true_only_for_future_public_entries() {
    let now = Utc.with_ymd_and_hms(2026, 9, 3, 12, 0, 0).unwrap();
    let scheduled = Publication::Public {
        publish_at: now + Duration::minutes(1),
    };

    assert!(scheduled.is_scheduled_at(now));
    assert!(!scheduled.is_scheduled_at(now + Duration::minutes(1)));
    assert!(!Publication::Public { publish_at: now }.is_scheduled_at(now));
    assert!(!Publication::Draft.is_scheduled_at(now));
}

#[test]
fn revision_snapshots_predating_the_trash_column_deserialize_and_live_content_omits_it() {
    use simple_blog::domain::content::Content;

    let legacy = serde_json::json!({
        "id": 5,
        "kind": "post",
        "title": "Before the trash existed",
        "slug": "legacy",
        "summary": "",
        "body_markdown": "# Legacy",
        "body_html": "<h1>Legacy</h1>",
        "tags": [],
        "cover_media_id": null,
        "seo_title": null,
        "seo_description": null,
        "publication": { "state": "draft" },
        "version": 1,
        "created_at": "2026-09-01T00:00:00Z",
        "updated_at": "2026-09-01T00:00:00Z"
    });
    let content: Content = serde_json::from_value(legacy).expect("older snapshot still reads");
    assert_eq!(content.deleted_at, None);
    assert!(!content.is_trashed());

    let live = serde_json::to_value(&content).unwrap();
    assert!(
        live.get("deleted_at").is_none(),
        "live content must serialize exactly as before"
    );

    let trashed = Content {
        deleted_at: Some(Utc.with_ymd_and_hms(2026, 9, 3, 0, 0, 0).unwrap()),
        ..content
    };
    let json = serde_json::to_value(&trashed).unwrap();
    assert_eq!(json["deleted_at"], "2026-09-03T00:00:00Z");
    let round_trip: Content = serde_json::from_value(json).unwrap();
    assert!(round_trip.is_trashed());
}
#[test]
fn reading_time_counts_cjk_characters_and_latin_words() {
    use simple_blog::domain::reading::reading_minutes;

    assert_eq!(reading_minutes(""), 0);
    assert_eq!(reading_minutes("   \n "), 0);
    assert_eq!(reading_minutes("one short line"), 1);
    assert_eq!(reading_minutes(&"日".repeat(1_000)), 2);
    assert_eq!(reading_minutes(&"word ".repeat(400)), 2);
    assert_eq!(
        reading_minutes(&format!("{} {}", "文".repeat(500), "word ".repeat(200))),
        2
    );
    assert_eq!(reading_minutes(&"word ".repeat(450)), 3);
    assert_eq!(reading_minutes("今日は、良い天気です。"), 1);
}

#[test]
fn outline_nests_h3_under_the_preceding_h2_and_keeps_prefixed_ids() {
    use simple_blog::domain::reading::{OutlineEntry, outline};

    let html = ComrakMarkdownRenderer::default()
        .render("## Intro\n\n### Detail *one*\n\n## Second\n\n# Ignored h1\n\n#### Ignored h4\n\n### Trailing")
        .html;
    let entries = outline(&html);

    assert_eq!(
        entries,
        vec![
            OutlineEntry {
                id: "user-content-intro".into(),
                text: "Intro".into(),
                children: vec![OutlineEntry {
                    id: "user-content-detail-one".into(),
                    text: "Detail one".into(),
                    children: Vec::new(),
                }],
            },
            OutlineEntry {
                id: "user-content-second".into(),
                text: "Second".into(),
                children: vec![OutlineEntry {
                    id: "user-content-trailing".into(),
                    text: "Trailing".into(),
                    children: Vec::new(),
                }],
            },
        ]
    );
    assert_eq!(entries.iter().map(OutlineEntry::size).sum::<usize>(), 4);

    let orphan = outline("<h3 id=\"user-content-alone\">Alone</h3><h2>No id</h2>");
    assert_eq!(orphan.len(), 1);
    assert_eq!(orphan[0].text, "Alone");
    assert!(outline("<p>no headings</p>").is_empty());
}
#[test]
fn line_diff_marks_only_what_a_restore_would_change() {
    use simple_blog::domain::diff::{DiffKind, diff_lines};

    let before = "# Title\n\nkept\nremoved line\nalso kept\n";
    let after = "# Title\n\nkept\nadded line\nalso kept\ntrailing\n";
    let lines = diff_lines(before, after);
    let shape: Vec<(DiffKind, &str)> = lines
        .iter()
        .map(|line| (line.kind, line.text.as_str()))
        .collect();

    assert_eq!(
        shape,
        vec![
            (DiffKind::Same, "# Title"),
            (DiffKind::Same, ""),
            (DiffKind::Same, "kept"),
            (DiffKind::Removed, "removed line"),
            (DiffKind::Added, "added line"),
            (DiffKind::Same, "also kept"),
            (DiffKind::Added, "trailing"),
        ]
    );
    assert!(diff_lines("", "").is_empty());
    assert!(
        diff_lines("same\n", "same\n")
            .iter()
            .all(|line| line.kind == DiffKind::Same)
    );
    let huge = "x\n".repeat(2_500);
    let fallback = diff_lines(&huge, "y\n");
    assert_eq!(fallback.len(), 2_501);
    assert_eq!(fallback[0].kind, DiffKind::Removed);
    assert_eq!(fallback[2_500].kind, DiffKind::Added);
}

#[test]
fn slugs_come_from_titles_when_they_can_and_from_the_clock_otherwise() {
    let now = Utc.with_ymd_and_hms(2026, 9, 3, 8, 12, 0).unwrap();
    assert_eq!(
        Slug::from_title("  Hello, World!  ", now).as_str(),
        "hello-world"
    );
    assert_eq!(
        Slug::from_title("Café à la Crème", now).as_str(),
        "cafe-a-la-creme"
    );
    assert_eq!(
        Slug::from_title("Archive", now).as_str(),
        "archive-post",
        "a reserved word gains a suffix instead of colliding with a route"
    );
    let japanese = Slug::from_title("今日の記事", now);
    assert!(japanese.is_timestamped(), "{japanese}");
    let mixed = Slug::from_title("Rust と 所有権", now);
    assert!(mixed.is_timestamped(), "{mixed}");
    assert!(Slug::from_title("한국어 제목", now).is_timestamped());
    assert!(Slug::from_title("!!!", now).is_timestamped());
    let long = Slug::from_title(&"word ".repeat(60), now);
    assert!(long.as_str().len() <= 120);
    assert!(!long.as_str().ends_with('-'));

    let base = Slug::parse("hello-world").unwrap();
    assert_eq!(base.numbered(2).as_str(), "hello-world-2");
    let near_limit = Slug::parse("a".repeat(119)).unwrap();
    assert!(near_limit.numbered(12).as_str().len() <= 120);
    assert!(near_limit.numbered(12).as_str().ends_with("-12"));
}
