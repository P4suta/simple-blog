#[path = "support/parser_oracles.rs"]
mod oracles;

#[test]
fn saved_parser_cases_are_replayed_in_every_normal_test_run() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus");
    for (name, oracle) in [
        ("search", oracles::search as fn(&[u8])),
        ("release_manifest", oracles::release_manifest),
        ("portable_site", oracles::portable_site),
    ] {
        let mut count = 0;
        for entry in std::fs::read_dir(root.join(name)).unwrap() {
            oracle(&std::fs::read(entry.unwrap().path()).unwrap());
            count += 1;
        }
        assert!(count > 0, "each parser needs a saved corpus");
    }
}

proptest::proptest! {
    #[test]
    fn unicode_queries_preserve_search_invariants(text in ".{0,512}") {
        oracles::search(text.as_bytes());
    }
}

#[test]
fn portable_corpus_reaches_successful_validation() {
    let site: simple_blog::portable::PortableSiteV1 =
        serde_json::from_str(include_str!("corpus/portable_site/valid-empty.json")).unwrap();
    site.validate().unwrap();
}
