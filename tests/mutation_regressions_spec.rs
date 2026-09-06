//! Small public-contract counterexamples found by hosted mutation testing.
use simple_blog::{
    domain::{content::Slug, search::parse_query},
    release::{ReleaseId, ReleaseManifest},
};

#[test]
fn empty_queries_and_the_eighth_distinct_search_term_keep_their_meaning() {
    for query in ["", " \t\n", "　"] {
        assert!(parse_query(query).is_empty());
    }
    for query in ["a", "東京", "long"] {
        assert!(!parse_query(query).is_empty());
    }
    for count in [7, 8, 9, 10] {
        let terms = (0..count)
            .map(|index| format!("term{index}"))
            .collect::<Vec<_>>();
        let parsed = parse_query(&terms.join(" "));
        assert_eq!(parsed.fts, terms[..count.min(8)]);
        assert!(parsed.like.is_empty());
    }
}

#[test]
fn timestamp_shaped_addresses_require_digits_on_both_sides_of_the_separator() {
    for value in [
        "abcdefgh-1234",
        "20260906-abcd",
        "2026090612345",
        "20260906-12345",
    ] {
        assert!(!Slug::parse(value).unwrap().is_timestamped());
    }
    for value in ["20260906-1234", "20260906-123456"] {
        assert!(Slug::parse(value).unwrap().is_timestamped());
    }
}

#[test]
fn release_identifiers_require_both_exact_length_and_lowercase_hex() {
    for value in [
        "a".repeat(63),
        "a".repeat(65),
        "g".repeat(64),
        "A".repeat(64),
    ] {
        assert!(ReleaseId::parse(value).is_err());
    }
    assert!(ReleaseId::parse("0123456789abcdef".repeat(4)).is_ok());
}

#[test]
fn parsing_a_manifest_validates_all_metadata_and_route_variants() {
    let valid = serde_json::json!({
        "format_version": 1, "compiler_version": "synthetic-compiler", "public_revision": 1,
        "canonical_origin": "https://writing.example", "routes": {
            "/": {"kind":"asset", "object_id":"a".repeat(64), "content_type":"text/html",
                "cache_control":"public, max-age=60", "status":200},
            "/old/": {"kind":"redirect", "status":301, "location":"/"}
        }
    });
    assert!(ReleaseManifest::from_bytes(&serde_json::to_vec(&valid).unwrap()).is_ok());
    for (pointer, value) in [
        ("/format_version", serde_json::json!(2)),
        ("/compiler_version", serde_json::json!(" \t")),
        ("/canonical_origin", serde_json::json!("invalid")),
        ("/routes/~1/status", serde_json::json!(201)),
        ("/routes/~1/object_id", serde_json::json!("g".repeat(64))),
        (
            "/routes/~1/content_type",
            serde_json::json!("text/html\r\nInjected: value"),
        ),
        (
            "/routes/~1/cache_control",
            serde_json::json!("public\nInjected: value"),
        ),
        ("/routes/~1old~1/status", serde_json::json!(200)),
        (
            "/routes/~1old~1/location",
            serde_json::json!("https://outside.example/"),
        ),
    ] {
        let mut invalid = valid.clone();
        *invalid.pointer_mut(pointer).unwrap() = value;
        assert!(
            ReleaseManifest::from_bytes(&serde_json::to_vec(&invalid).unwrap()).is_err(),
            "{pointer}"
        );
    }
}
