use simple_blog::domain::theme::{
    ColorScheme, FontPreset, Locale, NavigationItem, SiteSettings, validate_navigation,
};

fn settings() -> SiteSettings {
    SiteSettings {
        site_title: "  Quiet Notes  ".into(),
        site_description: "  Deliberate writing.  ".into(),
        locale: Locale::En,
        logo_media_id: None,
        favicon_media_id: None,
        accent_color: "#A1B2C3".into(),
        font_preset: FontPreset::Serif,
        content_width: 720,
        color_scheme: ColorScheme::System,
        custom_css: ".prose { text-wrap: pretty; }".into(),
    }
}

#[test]
fn site_settings_are_canonicalized_at_the_domain_boundary() {
    let validated = settings().validated().unwrap();
    assert_eq!(validated.site_title, "Quiet Notes");
    assert_eq!(validated.site_description, "Deliberate writing.");
    assert_eq!(validated.accent_color, "#a1b2c3");
}

#[test]
fn unsafe_or_out_of_range_theme_values_are_rejected() {
    let mut dangerous = settings();
    dangerous.custom_css = "</style><script>alert(1)</script>".into();
    assert!(dangerous.validated().is_err());

    let mut color = settings();
    color.accent_color = "red; background: url(//tracker.invalid)".into();
    assert!(color.validated().is_err());

    let mut width = settings();
    width.content_width = 1_200;
    assert!(width.validated().is_err());
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
