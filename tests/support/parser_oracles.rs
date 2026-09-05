pub fn search(bytes: &[u8]) {
    use simple_blog::domain::content::Slug;
    use simple_blog::domain::search::{fold, normalize, parse_query};
    if let Ok(text) = std::str::from_utf8(bytes) {
        if let Ok(slug) = Slug::parse(text) {
            assert_eq!(Slug::parse(slug.as_str()).unwrap(), slug);
            assert!(!slug.as_str().contains(['/', '\\', '?', '#']));
        }
        let normalized = normalize(text);
        assert_eq!(normalize(&normalized), normalized);
        assert_eq!(
            fold(&normalized).chars().count(),
            normalized.chars().count()
        );
        let terms = parse_query(text);
        assert!(terms.all().len() <= 8);
        assert!(terms.fts.iter().all(|term| term.chars().count() >= 3));
        assert!(
            terms
                .like
                .iter()
                .all(|term| (1..3).contains(&term.chars().count()))
        );
        for term in terms.all() {
            assert!(fold(&normalized).contains(term));
        }
    }
}

pub fn release_manifest(bytes: &[u8]) {
    use simple_blog::release::ReleaseManifest;
    if let Ok(manifest) = ReleaseManifest::from_bytes(bytes) {
        let canonical = manifest.canonical_bytes().unwrap();
        let reparsed = ReleaseManifest::from_bytes(&canonical).unwrap();
        assert_eq!(manifest, reparsed);
        assert_eq!(manifest.id().unwrap(), reparsed.id().unwrap());
    }
}

pub fn portable_site(bytes: &[u8]) {
    use simple_blog::portable::PortableSiteV1;
    if let Ok(site) = serde_json::from_slice::<PortableSiteV1>(bytes) {
        let encoded = serde_json::to_vec(&site).unwrap();
        let reparsed: PortableSiteV1 = serde_json::from_slice(&encoded).unwrap();
        assert_eq!(site, reparsed);
        assert_eq!(site.validate().is_ok(), reparsed.validate().is_ok());
    }
}
