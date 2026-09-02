use simple_blog::application::static_search::{
    StaticSearchDocument, StaticSearchError, StaticSearchIndexV1,
};

fn document(id: i64, slug: &str, title: &str, body: &str) -> StaticSearchDocument {
    StaticSearchDocument::new(id, slug, title, "", body, "2026-09-02")
}

#[test]
fn static_search_preserves_cjk_width_kana_and_and_semantics() {
    let index = StaticSearchIndexV1::new(vec![
        document(1, "both", "東京でRust", "サーバーを作る"),
        document(2, "tokyo", "東京だけ", "散歩"),
    ]);

    assert_eq!(slugs(index.search("東京 rust", 50)), ["both"]);
    assert_eq!(slugs(index.search("ＲＵＳＴ", 50)), ["both"]);
    assert_eq!(slugs(index.search("さーばー", 50)), ["both"]);
    assert!(index.search("存在しない", 50).is_empty());
}

#[test]
fn title_hits_rank_above_body_hits_with_stable_source_order_as_tiebreaker() {
    let index = StaticSearchIndexV1::new(vec![
        document(1, "newer-body", "Newer", "検索エンジンの本文"),
        document(2, "older-title", "検索エンジン自作記", "短い本文"),
        document(3, "also-title", "検索エンジン運用記", "短い本文"),
    ]);

    assert_eq!(
        slugs(index.search("検索エンジン", 50)),
        ["older-title", "also-title", "newer-body"]
    );
    assert_eq!(
        slugs(index.search("検索エンジン", 2)),
        ["older-title", "also-title"]
    );
}

#[test]
fn index_bytes_are_versioned_round_trippable_and_do_not_interpret_hostile_input() {
    let index = StaticSearchIndexV1::new(vec![document(
        1,
        "safe",
        "Literal <script>",
        "100% _ a* NOT b",
    )]);
    let bytes = index.canonical_bytes().unwrap();
    let decoded = StaticSearchIndexV1::from_bytes(&bytes).unwrap();

    assert_eq!(decoded, index);
    assert_eq!(decoded.format_version, 1);
    for query in ["<script>", "100%", "_", "a* NOT b", "\"OR\" ("] {
        let _results = decoded.search(query, 50);
    }
}

#[test]
fn malformed_or_semantically_inconsistent_indexes_fail_closed() {
    assert!(matches!(
        StaticSearchIndexV1::from_bytes(b"not json"),
        Err(StaticSearchError::InvalidJson(_))
    ));

    let mut index = StaticSearchIndexV1::new(vec![document(1, "safe", "Title", "Body")]);
    index.format_version = 2;
    assert_eq!(
        index.canonical_bytes().unwrap_err(),
        StaticSearchError::UnsupportedFormat(2)
    );

    let mut index = StaticSearchIndexV1::new(vec![document(0, "safe", "Title", "Body")]);
    assert!(matches!(
        index.canonical_bytes(),
        Err(StaticSearchError::InvalidDocument(message))
            if message == "content identity must be positive"
    ));
    index.documents[0].id = 1;
    index.documents[0].slug = "../escape".into();
    assert!(matches!(
        index.canonical_bytes(),
        Err(StaticSearchError::InvalidDocument(_))
    ));
    index.documents[0].slug = "safe".into();
    index.documents[0].folded = "tampered".into();
    assert!(matches!(
        index.canonical_bytes(),
        Err(StaticSearchError::InvalidDocument(message)) if message.contains("does not match")
    ));
}

fn slugs(results: Vec<&StaticSearchDocument>) -> Vec<&str> {
    results
        .into_iter()
        .map(|document| document.slug.as_str())
        .collect()
}
