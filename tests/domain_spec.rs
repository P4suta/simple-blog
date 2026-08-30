use chrono::{Duration, TimeZone, Utc};
use simple_blog::{
    application::ports::MarkdownRenderer,
    domain::content::{ContentKind, Publication, Slug},
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
}

#[test]
fn markdown_is_gfm_like_and_raw_html_is_never_trusted() {
    let renderer = ComrakMarkdownRenderer::default();
    let output = renderer.render(
        "# Hello\n\n- [x] done\n\n<script>alert('xss')</script>\n\n[bad](javascript:alert(1))",
    );

    assert!(output.html.contains("<h1>Hello</h1>"));
    assert!(output.html.contains("type=\"checkbox\""));
    assert!(!output.html.contains("<script"));
    assert!(!output.html.contains("javascript:"));
}
