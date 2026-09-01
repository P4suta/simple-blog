//! The CJK-first guarantees of the search text pipeline, tested at the seams
//! where anglocentric search implementations usually fail.

use simple_blog::domain::search::{
    self, Segment, excerpt, fold, html_to_text, normalize, parse_query,
};

#[test]
fn folding_maps_katakana_to_hiragana_one_to_one() {
    let folded = fold("サーバーとゔぁヴァ");
    assert_eq!(folded, "さーばーとゔぁゔぁ");
    // The one-to-one guarantee that snippet positions rely on.
    assert_eq!(folded.chars().count(), "サーバーとゔぁヴァ".chars().count());
}

#[test]
fn normalization_reconciles_width_variants() {
    // Full-width Latin and half-width katakana both come from real input
    // methods; both must meet their canonical forms.
    assert_eq!(normalize("Ｒｕｓｔ　２０２６"), "Rust 2026");
    assert_eq!(normalize("ｻｰﾊﾞｰ"), "サーバー");
    assert_eq!(fold(&normalize("ｻｰﾊﾞｰ")), "さーばー");
}

#[test]
fn query_terms_split_by_trigram_reach() {
    // 東京 (2 chars) cannot match a trigram index and must go the LIKE path;
    // タワー (3 chars) can use FTS. The ideographic space separates terms.
    let terms = parse_query("東京　タワー Rust");
    assert_eq!(terms.like, vec!["東京"]);
    assert_eq!(terms.fts, vec!["たわー", "rust"]);
}

#[test]
fn ruby_markup_indexes_as_base_and_reading() {
    // A ruby-annotated word must be findable by either its base or its
    // reading; rp parentheses keep the plain text readable.
    let text = html_to_text("<p><ruby>漢字<rp>（</rp><rt>かんじ</rt><rp>）</rp></ruby>の話。</p>");
    assert_eq!(text, "漢字（かんじ）の話。");
}

#[test]
fn html_reduces_to_text_without_splitting_cjk_words() {
    let text = html_to_text(
        "<p>これは<em>重要</em>な話。</p><p>Second &amp; last &#39;paragraph&#39;.</p>",
    );
    // Inline emphasis must not inject spaces into the middle of a sentence;
    // block boundaries must separate words.
    assert_eq!(text, "これは重要な話。 Second & last 'paragraph'.");
}

#[test]
fn entity_decoding_never_slices_through_following_cjk_text() {
    assert_eq!(
        html_to_text("<p>紅茶 &amp;日本語の話。</p>"),
        "紅茶 &日本語の話。"
    );
}

#[test]
fn excerpt_highlights_folded_matches_in_the_display_text() {
    let display = "静的サイトのサーバーをRustで書き直した記録。";
    let terms = ["さーばー".to_string(), "rust".to_string()];
    let term_refs: Vec<&str> = terms.iter().map(String::as_str).collect();
    let (segments, clipped_start, clipped_end) = excerpt(display, &term_refs, 100);
    assert!(!clipped_start && !clipped_end);
    let hits: Vec<&Segment> = segments.iter().filter(|segment| segment.hit).collect();
    // The katakana in the display text is what gets highlighted, found via
    // its hiragana fold; the ASCII match is case-folded the same way.
    assert_eq!(hits[0].text, "サーバー");
    assert_eq!(hits[1].text, "Rust");
    let rebuilt: String = segments
        .iter()
        .map(|segment| segment.text.as_str())
        .collect();
    assert_eq!(rebuilt, display);
}

#[test]
fn excerpt_windows_long_text_around_the_first_hit() {
    let padding = "あ".repeat(300);
    let display = format!("{padding}検索エンジン{padding}");
    let terms = ["検索えんじん".to_string()];
    let term_refs: Vec<&str> = terms.iter().map(String::as_str).collect();
    let (segments, clipped_start, clipped_end) = excerpt(&display, &term_refs, 60);
    assert!(clipped_start && clipped_end);
    let window: String = segments
        .iter()
        .map(|segment| segment.text.as_str())
        .collect();
    assert_eq!(window.chars().count(), 60);
    assert!(segments.iter().any(|segment| segment.hit));
}

#[test]
fn like_and_fts_escaping_neutralize_special_characters() {
    assert_eq!(search::escape_like("100%_\\"), "100\\%\\_\\\\");
    assert_eq!(search::quote_fts("say \"hi\""), "\"say \"\"hi\"\"\"");
}
