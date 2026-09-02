use simple_blog::domain::theme::{Locale, NavigationItem, SiteSettings, validate_navigation};

fn settings() -> SiteSettings {
    SiteSettings {
        site_title: "  Quiet Notes  ".into(),
        site_description: "  Deliberate writing.  ".into(),
        locale: Locale::En,
        logo_media_id: None,
        favicon_media_id: None,
        custom_css: ".prose { text-wrap: pretty; }".into(),
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
    let post_nav_border = theme.rfind(".post-nav {").unwrap();
    let hairline_color = theme
        .rfind("border-color: color-mix(in srgb, currentcolor")
        .unwrap();
    assert!(hairline_color > post_nav_border);
    assert!(theme.contains("@media print"));
    assert!(theme.contains(".toc {"));
    assert!(theme.contains(".article-meta {"));
    assert!(theme.contains(".tag-list td:last-child"));
}

#[test]
fn locale_maps_to_open_graph_locale() {
    assert_eq!(Locale::En.og_locale(), "en_US");
    assert_eq!(Locale::Ja.og_locale(), "ja_JP");
    assert_eq!(Locale::Zh.og_locale(), "zh_CN");
}
