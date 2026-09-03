use simple_blog::domain::theme::{
    Locale, NavigationItem, SiteSettings, ThemeValidationError, timezone_choices,
    validate_navigation,
};

fn settings() -> SiteSettings {
    SiteSettings {
        site_title: "  Quiet Notes  ".into(),
        site_description: "  Deliberate writing.  ".into(),
        locale: Locale::En,
        logo_media_id: None,
        favicon_media_id: None,
        custom_css: ".prose { text-wrap: pretty; }".into(),
        timezone: "UTC".into(),
        author_name: String::new(),
        custom_css_backup: None,
    }
}

#[test]
fn site_settings_are_canonicalized_at_the_domain_boundary() {
    let validated = settings().validated().unwrap();
    assert_eq!(validated.site_title, "Quiet Notes");
    assert_eq!(validated.site_description, "Deliberate writing.");
}

#[test]
fn unsafe_theme_values_are_rejected() {
    let mut dangerous = settings();
    dangerous.custom_css = "</style><script>alert(1)</script>".into();
    assert!(dangerous.validated().is_err());
}

#[test]
fn navigation_is_single_level_ordered_and_has_explicit_url_kinds() {
    let items = validate_navigation(vec![
        NavigationItem {
            id: 42,
            label: "  Archive ".into(),
            destination: "/archive/".into(),
            is_external: false,
            position: 99,
        },
        NavigationItem {
            id: 7,
            label: "Rust".into(),
            destination: "https://www.rust-lang.org/".into(),
            is_external: true,
            position: 99,
        },
    ])
    .unwrap();

    assert_eq!(items[0].label, "Archive");
    assert_eq!(items[0].position, 0);
    assert_eq!(items[1].position, 1);
    assert!(
        validate_navigation(vec![NavigationItem {
            id: 0,
            label: "Bad".into(),
            destination: "javascript:alert(1)".into(),
            is_external: true,
            position: 0,
        }])
        .is_err()
    );
    assert!(
        validate_navigation(vec![NavigationItem {
            id: 0,
            label: "Bad".into(),
            destination: "//evil.example/".into(),
            is_external: false,
            position: 0,
        }])
        .is_err()
    );
}

#[test]
fn embedded_styles_preserve_font_names_and_apply_hairline_color_last() {
    let admin = include_str!("../static/admin.css");
    for family in [
        "\"Meiryo\"",
        "\"SFMono-Regular\"",
        "\"Menlo\"",
        "\"Consolas\"",
    ] {
        assert!(admin.contains(family), "unquoted font family {family}");
    }

    let theme = include_str!("../static/default-theme.css");
    // A checkout with CRLF endings must not change what the test proves.
    let theme = theme.replace("\r\n", "\n");
    let theme = theme.as_str();
    let post_nav_border = theme.rfind(".post-nav {").unwrap();
    let hairline_color = theme
        .rfind("border-color: color-mix(in srgb, currentcolor")
        .unwrap();
    assert!(hairline_color > post_nav_border);
    assert!(theme.contains("@media print"));
    assert!(theme.contains(".toc {"));
    assert!(theme.contains(".article-meta {"));
    assert!(theme.contains(".tag-list td:last-child"));

    // Typography, figures, related posts, focus, and the copy button.
    for family in ["\"Hiragino Sans\"", "\"Noto Sans JP\"", "\"PingFang SC\""] {
        assert!(theme.contains(family), "theme lacks {family}");
    }
    for rule in [
        "ruby {",
        ":lang(ja) body {",
        ".prose figure {",
        ".related {",
        ":focus-visible {",
        ".copy-code {",
    ] {
        assert!(theme.contains(rule), "theme lacks {rule}");
    }
    let hairline_block = &theme[theme.rfind("hr,\nthead th").unwrap()..hairline_color];
    assert!(hairline_block.contains(".related,"));
    let body_start = theme.find("\nbody {").unwrap();
    let body_block = &theme[body_start..theme[body_start..].find('}').unwrap() + body_start];
    assert!(body_block.contains("overflow-wrap: break-word"));
    assert!(!body_block.contains("anywhere"));
    let print_block = &theme[theme.find("@media print").unwrap()..];
    assert!(print_block.contains(".copy-code"));
    assert!(
        !theme.contains(['<', '>']),
        "a stylesheet with angle brackets could never be stored"
    );
}

#[test]
fn locale_maps_to_open_graph_locale() {
    assert_eq!(Locale::En.og_locale(), "en_US");
    assert_eq!(Locale::Ja.og_locale(), "ja_JP");
    assert_eq!(Locale::Zh.og_locale(), "zh_CN");
}

#[test]
fn site_timezone_must_be_a_known_iana_zone_and_is_normalized() {
    let mut tokyo = settings();
    tokyo.timezone = " Asia/Tokyo ".into();
    let validated = tokyo.validated().unwrap();
    assert_eq!(validated.timezone, "Asia/Tokyo");
    assert_eq!(validated.time_zone(), chrono_tz::Tz::Asia__Tokyo);

    for bad in ["Mars/Olympus", ""] {
        let mut invalid = settings();
        invalid.timezone = bad.into();
        assert_eq!(
            invalid.validated().unwrap_err(),
            ThemeValidationError::Timezone,
            "{bad:?}"
        );
    }
}

#[test]
fn author_name_falls_back_to_the_site_title() {
    assert_eq!(settings().validated().unwrap().author(), "Quiet Notes");
    let mut named = settings();
    named.author_name = " Ryo ".into();
    assert_eq!(named.validated().unwrap().author(), "Ryo");
    let mut long = settings();
    long.author_name = "x".repeat(121);
    assert_eq!(
        long.validated().unwrap_err(),
        ThemeValidationError::AuthorName
    );
}

#[test]
fn theme_backup_obeys_the_custom_css_contract() {
    let mut dangerous = settings();
    dangerous.custom_css_backup = Some("</style><script>alert(1)</script>".into());
    assert_eq!(
        dangerous.validated().unwrap_err(),
        ThemeValidationError::CustomCss
    );
}

#[test]
fn timezone_choices_are_grouped_by_region_without_legacy_aliases() {
    let groups = timezone_choices();
    assert_eq!(groups[0].region, "UTC");
    assert_eq!(groups[0].zones, ["UTC"]);
    let asia = groups.iter().find(|group| group.region == "Asia").unwrap();
    assert!(asia.zones.iter().any(|zone| zone == "Asia/Tokyo"));
    assert!(
        asia.zones.windows(2).all(|pair| pair[0] <= pair[1]),
        "sorted"
    );
    assert!(
        groups.iter().flat_map(|group| &group.zones).all(|zone| {
            !zone.starts_with("US/") && !zone.starts_with("Etc/") && zone != "Japan"
        })
    );
}
