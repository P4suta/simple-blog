use chrono::{Duration, TimeZone, Utc};
use proptest::prelude::*;
use proptest::test_runner::FileFailurePersistence;
use scraper::{Html, Selector};
use simple_blog::{
    application::{
        auth::{AuthRateLimiter, RateLimitDecision},
        ports::MarkdownRenderer,
    },
    domain::content::{Publication, Slug},
    infrastructure::markdown::ComrakMarkdownRenderer,
};

proptest! {
    #![proptest_config(ProptestConfig::with_failure_persistence(
        FileFailurePersistence::Direct("tests/proptest-regressions/property_spec.txt")
    ))]

    #[test]
    fn accepted_slugs_always_preserve_the_canonical_url_invariants(value in any::<String>()) {
        if let Ok(slug) = Slug::parse(&value) {
            let canonical = slug.as_str();
            prop_assert_eq!(canonical, value);
            prop_assert!((1..=120).contains(&canonical.len()));
            prop_assert!(canonical.is_ascii());
            let safe_bytes = canonical.bytes().all(|byte| {
                byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-'
            });
            prop_assert!(safe_bytes);
            prop_assert!(!canonical.starts_with('-'));
            prop_assert!(!canonical.ends_with('-'));
            prop_assert!(!canonical.contains("--"));
            let serialized = serde_json::to_string(&slug).unwrap();
            let restored: Slug = serde_json::from_str(&serialized).unwrap();
            prop_assert_eq!(restored, slug);
        }
    }

    #[test]
    fn public_visibility_is_monotonic_after_publish_at(
        publish_offset in -100_000_i64..100_000,
        later_offset in 0_i64..100_000,
    ) {
        let base = Utc.timestamp_opt(1_800_000_000, 0).unwrap();
        let publish_at = base + Duration::seconds(publish_offset);
        let publication = Publication::Public { publish_at };
        let observed = publish_at + Duration::seconds(later_offset);

        prop_assert!(publication.is_visible_at(observed));
        prop_assert!(publication.is_visible_at(observed + Duration::days(365)));
        prop_assert!(!publication.is_visible_at(publish_at - Duration::nanoseconds(1)));
    }

    #[test]
    fn rate_limit_window_has_an_exact_deterministic_boundary(
        maximum in 1_usize..32,
        window_seconds in 1_i64..600,
    ) {
        let now = Utc.timestamp_opt(1_800_000_000, 0).unwrap();
        let window = Duration::seconds(window_seconds);
        let limiter = AuthRateLimiter::new(maximum, window);
        for _ in 0..maximum {
            prop_assert_eq!(limiter.check("client", now), RateLimitDecision::Allowed);
        }
        prop_assert_eq!(
            limiter.check("client", now),
            RateLimitDecision::Limited {
                retry_after: window_seconds.unsigned_abs(),
            }
        );
        prop_assert_eq!(
            limiter.check("client", now + window),
            RateLimitDecision::Allowed
        );
    }

    #[test]
    fn arbitrary_markdown_cannot_create_active_script_or_event_attributes(
        characters in proptest::collection::vec(any::<char>(), 0..512)
    ) {
        let markdown: String = characters.into_iter().collect();
        let rendered = ComrakMarkdownRenderer::default().render(&markdown);
        let document = Html::parse_fragment(&rendered.html);
        let scripts = Selector::parse("script").unwrap();
        prop_assert_eq!(document.select(&scripts).count(), 0);

        let elements = Selector::parse("*").unwrap();
        for element in document.select(&elements) {
            for (name, value) in element.value().attrs() {
                let name = name.to_ascii_lowercase();
                let value = value.trim().to_ascii_lowercase();
                prop_assert!(!name.starts_with("on"));
                if matches!(name.as_str(), "href" | "src" | "xlink:href") {
                    prop_assert!(!value.starts_with("javascript:"));
                    prop_assert!(!value.starts_with("vbscript:"));
                    prop_assert!(!value.starts_with("data:text/html"));
                }
            }
        }
    }
}
