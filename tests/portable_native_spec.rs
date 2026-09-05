use std::collections::BTreeMap;

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{TimeZone, Utc};
use simple_blog::{
    application::ports::{MarkdownRenderer, PortableRepository},
    domain::{
        content::{
            Content, ContentId, ContentKind, ContentRevision, Publication, SaveIntent, Slug,
        },
        theme::{Locale, NavigationItem, SiteSettings},
    },
    infrastructure::{markdown::ComrakMarkdownRenderer, sqlite::SqliteRepository},
    portable::{
        PortableContent, PortableEngagement, PortableOwner, PortablePasskey,
        PortablePublicationState, PortableRecoveryCode, PortableRedirect, PortableSettingsRevision,
        PortableSiteV1,
    },
};
use sqlx::Executor;
use uuid::Uuid;

fn site() -> PortableSiteV1 {
    let created_at = Utc.with_ymd_and_hms(2024, 3, 4, 5, 6, 7).unwrap();
    let updated_at = Utc.with_ymd_and_hms(2026, 9, 2, 12, 0, 0).unwrap();
    let id = ContentId::from_i64(41);
    PortableSiteV1 {
        format_version: 1,
        exported_at: updated_at,
        canonical_origin: "https://writing.example".into(),
        settings: SiteSettings {
            site_title: "Portable site".into(),
            site_description: "Every durable thing moves".into(),
            locale: Locale::Ja,
            logo_media_id: None,
            favicon_media_id: None,
            custom_css: "body { max-width: 40rem; }".into(),
            timezone: "UTC".into(),
            author_name: String::new(),
            custom_css_backup: None,
        },
        navigation: vec![NavigationItem {
            id: 9,
            label: "Archive".into(),
            destination: "/archive/".into(),
            is_external: false,
            position: 0,
        }],
        contents: portable_contents(id, created_at, updated_at),
        redirects: vec![PortableRedirect {
            old_slug: Slug::parse("before").unwrap(),
            content_id: id,
            created_at: updated_at,
        }],
        media: Vec::new(),
        engagement: BTreeMap::from([
            (
                id.as_i64(),
                PortableEngagement {
                    likes: 17,
                    views: 230,
                },
            ),
            (42, PortableEngagement { likes: 0, views: 0 }),
        ]),
        owner: Some(portable_owner(created_at, updated_at)),
        publication: PortablePublicationState {
            public_revision: 88,
            next_publish_at: None,
        },
        // The kept states of the settings move too, navigation and all;
        // their navigation items carry no identity of their own.
        settings_revisions: vec![PortableSettingsRevision {
            settings: SiteSettings {
                site_title: "Before the move".into(),
                site_description: String::new(),
                locale: Locale::Ja,
                logo_media_id: None,
                favicon_media_id: None,
                custom_css: String::new(),
                timezone: "UTC".into(),
                author_name: String::new(),
                custom_css_backup: None,
            },
            navigation: vec![NavigationItem {
                id: 0,
                label: "Archive".into(),
                destination: "/archive/".into(),
                is_external: false,
                position: 0,
            }],
            created_at,
        }],
    }
}

fn portable_contents(
    id: ContentId,
    created_at: chrono::DateTime<Utc>,
    updated_at: chrono::DateTime<Utc>,
) -> Vec<PortableContent> {
    let historical = Content {
        id,
        kind: ContentKind::Post,
        title: "Before".into(),
        slug: Slug::parse("before").unwrap(),
        summary: "Earlier summary".into(),
        body_markdown: "# Before".into(),
        body_html: "<script>must never be trusted</script>".into(),
        tags: Vec::new(),
        cover_media_id: None,
        seo_title: None,
        seo_description: None,
        publication: Publication::Draft,
        version: 1,
        created_at,
        updated_at: created_at,
        deleted_at: None,
    };
    let current = Content {
        id,
        kind: ContentKind::Post,
        title: "After".into(),
        slug: Slug::parse("after").unwrap(),
        summary: "Current summary".into(),
        body_markdown: "# After\n\nPortable source.".into(),
        body_html: "<script>also untrusted</script>".into(),
        tags: vec![simple_blog::domain::content::Tag {
            name: "Migration".into(),
            slug: Slug::parse("migration").unwrap(),
        }],
        cover_media_id: None,
        seo_title: Some("Portable SEO".into()),
        seo_description: Some("Moves without loss".into()),
        publication: Publication::Public {
            publish_at: updated_at,
        },
        version: 2,
        created_at,
        updated_at,
        deleted_at: None,
    };
    // A scheduled piece sitting in the trash: durable, exported, and never
    // counted by the publication clock (ADR 0014).
    let trashed = Content {
        id: ContentId::from_i64(42),
        kind: ContentKind::Post,
        title: "Shelved".into(),
        slug: Slug::parse("shelved").unwrap(),
        summary: String::new(),
        body_markdown: "# Shelved".into(),
        body_html: String::new(),
        tags: Vec::new(),
        cover_media_id: None,
        seo_title: None,
        seo_description: None,
        publication: Publication::Public {
            publish_at: updated_at + chrono::Duration::hours(1),
        },
        version: 1,
        created_at,
        updated_at,
        deleted_at: Some(updated_at),
    };
    vec![
        PortableContent {
            current,
            revisions: vec![ContentRevision {
                id: 73,
                content_id: id,
                intent: SaveIntent::Explicit,
                snapshot: historical,
                created_at,
            }],
        },
        PortableContent {
            current: trashed,
            revisions: Vec::new(),
        },
    ]
}

fn portable_owner(
    created_at: chrono::DateTime<Utc>,
    updated_at: chrono::DateTime<Utc>,
) -> PortableOwner {
    PortableOwner {
        user_handle: Uuid::from_u128(0x1234),
        created_at,
        passkeys: vec![PortablePasskey {
            credential_id: URL_SAFE_NO_PAD.encode([1_u8, 2, 3, 4]),
            name: "Laptop".into(),
            passkey_json: "{\"counter\":9}".into(),
            created_at,
            last_used_at: Some(updated_at),
        }],
        recovery_codes: vec![PortableRecoveryCode {
            code_hash: "ab".repeat(32),
            consumed_at: None,
            created_at,
        }],
    }
}

fn normalized(mut site: PortableSiteV1) -> PortableSiteV1 {
    let renderer = ComrakMarkdownRenderer::default();
    for content in &mut site.contents {
        content.current.body_html = renderer.render(&content.current.body_markdown).html;
        for revision in &mut content.revisions {
            revision.snapshot.body_html = renderer.render(&revision.snapshot.body_markdown).html;
        }
    }
    site
}

#[tokio::test]
async fn sqlite_portable_round_trip_preserves_all_durable_state_and_rebuilds_html() {
    let temp = tempfile::tempdir().unwrap();
    let repository = SqliteRepository::connect(&temp.path().join("destination.sqlite3"))
        .await
        .unwrap();
    repository
        .pool()
        .execute(
            "INSERT INTO setup_tokens (token_hash, purpose, expires_at) \
             VALUES (x'01', 'setup', '2099-01-01T00:00:00Z'); \
             INSERT INTO sessions (token_hash, csrf_token_hash, created_at, expires_at, \
                                    last_seen_at, reauthenticated_at) \
             VALUES (x'02', x'03', '2026-01-01T00:00:00Z', '2099-01-01T00:00:00Z', \
                     '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
        )
        .await
        .unwrap();
    let expected = normalized(site());

    repository
        .replace_portable_site(&site(), &ComrakMarkdownRenderer::default())
        .await
        .unwrap();
    let actual = repository
        .portable_site("https://writing.example", expected.exported_at)
        .await
        .unwrap();

    assert_eq!(actual, expected);
    for (table, sql) in [
        ("sessions", "SELECT COUNT(*) FROM sessions"),
        ("setup_tokens", "SELECT COUNT(*) FROM setup_tokens"),
    ] {
        let count: i64 = sqlx::query_scalar(sql)
            .fetch_one(repository.pool())
            .await
            .unwrap();
        assert_eq!(count, 0, "{table} must not cross a host boundary");
    }
}

#[tokio::test]
async fn sqlite_portable_import_rolls_back_every_change_after_a_mid_transaction_failure() {
    let temp = tempfile::tempdir().unwrap();
    let repository = SqliteRepository::connect(&temp.path().join("destination.sqlite3"))
        .await
        .unwrap();
    let renderer = ComrakMarkdownRenderer::default();
    repository
        .replace_portable_site(&site(), &renderer)
        .await
        .unwrap();
    let before = repository
        .portable_site("https://writing.example", site().exported_at)
        .await
        .unwrap();
    repository
        .pool()
        .execute(
            "CREATE TRIGGER reject_portable_content BEFORE INSERT ON contents \
             WHEN NEW.title = 'Injected failure' \
             BEGIN SELECT RAISE(ABORT, 'portable test failure'); END",
        )
        .await
        .unwrap();
    let mut replacement = site();
    replacement.contents[0].current.title = "Injected failure".into();

    assert!(
        repository
            .replace_portable_site(&replacement, &renderer)
            .await
            .is_err()
    );
    assert_eq!(
        repository
            .portable_site("https://writing.example", site().exported_at)
            .await
            .unwrap(),
        before
    );
}
