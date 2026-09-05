use std::sync::{Arc, Barrier};
use std::{
    collections::BTreeMap,
    fs::{File, OpenOptions},
    io::{Cursor, Write},
};

use chrono::{DateTime, TimeZone, Utc};
use serde::Serialize;
use simple_blog::{
    domain::{
        content::{Content, ContentId, ContentKind, Publication, Slug},
        theme::{Locale, SiteSettings},
    },
    portable::{
        PortableArchive, PortableArchiveError, PortableContent, PortableEngagement,
        PortablePackage, PortablePublicationState, PortableSettingsRevision, PortableSiteV1,
    },
};

fn package() -> PortablePackage {
    let at = Utc.with_ymd_and_hms(2026, 9, 2, 12, 0, 0).unwrap();
    let content = Content {
        id: ContentId::from_i64(7),
        kind: ContentKind::Post,
        title: "Portable".into(),
        slug: Slug::parse("portable").unwrap(),
        summary: "Leaves any host".into(),
        body_markdown: "# Canonical".into(),
        body_html: "<h1>Canonical</h1>".into(),
        tags: Vec::new(),
        cover_media_id: None,
        seo_title: None,
        seo_description: None,
        publication: Publication::Public { publish_at: at },
        version: 3,
        created_at: at,
        updated_at: at,
        deleted_at: None,
    };
    PortablePackage {
        site: PortableSiteV1 {
            format_version: 1,
            exported_at: at,
            canonical_origin: "https://writing.example".into(),
            settings: SiteSettings {
                site_title: "Portable site".into(),
                site_description: String::new(),
                locale: Locale::En,
                logo_media_id: None,
                favicon_media_id: None,
                custom_css: String::new(),
                timezone: "UTC".into(),
                author_name: String::new(),
                custom_css_backup: None,
            },
            navigation: Vec::new(),
            contents: vec![PortableContent {
                current: content,
                revisions: Vec::new(),
            }],
            redirects: Vec::new(),
            media: Vec::new(),
            engagement: BTreeMap::from([(7, PortableEngagement { likes: 2, views: 9 })]),
            owner: None,
            publication: PortablePublicationState {
                public_revision: 12,
                next_publish_at: None,
            },
            settings_revisions: Vec::new(),
        },
        media_files: BTreeMap::new(),
    }
}

#[test]
fn the_portable_site_contract_reads_writes_and_travels_unchanged() {
    // A CRLF checkout must not change what the test proves.
    let fixture = include_str!("../contracts/portable-site-v1.json").replace("\r\n", "\n");
    let site: PortableSiteV1 = serde_json::from_str(&fixture).unwrap();
    site.validate().unwrap();

    // Field order and omissions are part of the contract: a second
    // implementation must produce these bytes from this site.
    let written = serde_json::to_string_pretty(&site).unwrap() + "\n";
    assert_eq!(written, fixture);

    // Through an archive and back, the site is the same site.
    let temp = tempfile::tempdir().unwrap();
    let archive = temp.path().join("contract.simple-blog");
    let package = PortablePackage {
        site: site.clone(),
        media_files: BTreeMap::new(),
    };
    PortableArchive::write(&package, &archive).unwrap();
    assert_eq!(PortableArchive::read(&archive).unwrap().site, site);
}

#[test]
fn settings_history_travels_and_an_empty_one_leaves_older_archives_untouched() {
    let without = serde_json::to_string(&package().site).unwrap();
    assert!(
        !without.contains("settings_revisions"),
        "an empty history must not change the bytes of an archive"
    );

    let mut site = package().site;
    let earlier = PortableSettingsRevision {
        settings: SiteSettings {
            site_title: "Before the rename".into(),
            ..site.settings.clone()
        },
        navigation: Vec::new(),
        created_at: site.exported_at - chrono::Duration::hours(1),
    };
    let current = PortableSettingsRevision {
        settings: site.settings.clone(),
        navigation: Vec::new(),
        created_at: site.exported_at,
    };
    site.settings_revisions = vec![earlier.clone(), current.clone()];
    site.validate().unwrap();
    let json = serde_json::to_string(&site).unwrap();
    assert_eq!(serde_json::from_str::<PortableSiteV1>(&json).unwrap(), site);

    // A history a conforming host could not have written is refused: out of
    // time order, or settings it would have normalized on save.
    let mut disordered = site.clone();
    disordered.settings_revisions = vec![current, earlier];
    assert!(disordered.validate().is_err());
    let mut unnormalized = site;
    unnormalized.settings_revisions[0].settings.site_title = "  padded  ".into();
    assert!(unnormalized.validate().is_err());
}

#[test]
fn portable_archive_round_trips_all_logical_data_and_is_byte_deterministic() {
    let temp = tempfile::tempdir().unwrap();
    let first = temp.path().join("first.simple-blog");
    let second = temp.path().join("second.simple-blog");
    let package = package();

    let first_report = PortableArchive::write(&package, &first).unwrap();
    let second_report = PortableArchive::write(&package, &second).unwrap();

    assert_eq!(first_report.archive_id, second_report.archive_id);
    assert_eq!(
        std::fs::read(&first).unwrap(),
        std::fs::read(&second).unwrap()
    );
    assert_eq!(PortableArchive::read(&first).unwrap(), package);
    assert_eq!(first_report.entry_count, 1);
}

#[test]
fn portable_archive_refuses_overwrite_and_detects_any_byte_corruption() {
    let temp = tempfile::tempdir().unwrap();
    let archive = temp.path().join("site.simple-blog");
    PortableArchive::write(&package(), &archive).unwrap();
    assert!(matches!(
        PortableArchive::write(&package(), &archive).unwrap_err(),
        PortableArchiveError::OutputExists(_)
    ));

    let mut bytes = std::fs::read(&archive).unwrap();
    let middle = bytes.len() / 2;
    bytes[middle] ^= 0x40;
    let corrupt = temp.path().join("corrupt.simple-blog");
    std::fs::write(&corrupt, bytes).unwrap();
    assert!(PortableArchive::read(&corrupt).is_err());
}

#[test]
fn archive_frame_is_checksummed_and_trailing_bytes_are_rejected() {
    let temp = tempfile::tempdir().unwrap();
    let archive = temp.path().join("checksummed.simple-blog");
    PortableArchive::write(&package(), &archive).unwrap();
    let bytes = std::fs::read(&archive).unwrap();

    assert_eq!(&bytes[..4], &[0x28, 0xb5, 0x2f, 0xfd]);
    assert_ne!(bytes[4] & 0x04, 0, "zstd content checksum flag is required");

    let trailing = temp.path().join("trailing.simple-blog");
    std::fs::copy(&archive, &trailing).unwrap();
    OpenOptions::new()
        .append(true)
        .open(&trailing)
        .unwrap()
        .write_all(b"untrusted trailing bytes")
        .unwrap();
    assert!(matches!(
        PortableArchive::read(&trailing).unwrap_err(),
        PortableArchiveError::InvalidArchive(message) if message.contains("trailing")
    ));
}

#[test]
fn archive_manifest_schema_and_site_identity_must_match_exactly() {
    let temp = tempfile::tempdir().unwrap();
    let mismatched = temp.path().join("mismatched.simple-blog");
    let site = package().site;
    let other_time = Utc.with_ymd_and_hms(2026, 9, 2, 13, 0, 0).unwrap();
    write_identity_archive(&mismatched, &site, other_time, ManifestMutation::None);
    assert!(matches!(
        PortableArchive::read(&mismatched).unwrap_err(),
        PortableArchiveError::InvalidArchive(message) if message.contains("site identity")
    ));

    let unknown = temp.path().join("unknown-manifest-field.simple-blog");
    write_identity_archive(
        &unknown,
        &site,
        site.exported_at,
        ManifestMutation::UnknownField,
    );
    assert!(matches!(
        PortableArchive::read(&unknown).unwrap_err(),
        PortableArchiveError::InvalidArchive(message) if message.contains("manifest fields")
    ));

    let duplicate = temp.path().join("duplicate-manifest-field.simple-blog");
    write_identity_archive(
        &duplicate,
        &site,
        site.exported_at,
        ManifestMutation::DuplicateArchiveId,
    );
    assert!(matches!(
        PortableArchive::read(&duplicate).unwrap_err(),
        PortableArchiveError::InvalidArchive(message) if message.contains("duplicate JSON field")
    ));
}

#[test]
fn package_validation_rejects_missing_or_unexpected_media_bytes() {
    let mut package = package();
    package
        .media_files
        .insert("orphan.webp".into(), b"orphan".to_vec());

    let error = package.validate().unwrap_err();

    assert!(error.to_string().contains("unexpected media file"));
}

#[test]
fn concurrent_writers_can_never_overwrite_a_portable_archive() {
    let temp = tempfile::tempdir().unwrap();
    let output = Arc::new(temp.path().join("contended.simple-blog"));
    let package = Arc::new(package());
    let barrier = Arc::new(Barrier::new(12));
    let mut writers = Vec::new();
    for _ in 0..12 {
        let output = output.clone();
        let package = package.clone();
        let barrier = barrier.clone();
        writers.push(std::thread::spawn(move || {
            barrier.wait();
            PortableArchive::write(&package, &output)
        }));
    }
    let results = writers
        .into_iter()
        .map(|writer| writer.join().unwrap())
        .collect::<Vec<_>>();

    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    assert!(results.iter().filter(|result| result.is_err()).all(|result| {
        matches!(result, Err(PortableArchiveError::OutputExists(path)) if path == output.as_ref())
    }));
    assert_eq!(PortableArchive::read(&output).unwrap(), *package);
}

#[test]
fn versioned_portable_json_rejects_fields_an_adapter_cannot_preserve() {
    let mut json = serde_json::to_value(&package().site).unwrap();
    json.as_object_mut().unwrap().insert(
        "future_host_state".into(),
        serde_json::json!({"lost": true}),
    );

    let error = serde_json::from_value::<PortableSiteV1>(json).unwrap_err();

    assert!(error.to_string().contains("unknown field"));
}

#[test]
fn package_validation_rejects_inconsistent_revision_identity_and_unstorable_counters() {
    let mut invalid_revision = package();
    let current = invalid_revision.site.contents[0].current.clone();
    invalid_revision.site.contents[0].revisions.push(
        simple_blog::domain::content::ContentRevision {
            id: 1,
            content_id: current.id,
            intent: simple_blog::domain::content::SaveIntent::Explicit,
            snapshot: Content {
                id: ContentId::from_i64(999),
                ..current
            },
            created_at: invalid_revision.site.exported_at,
        },
    );
    assert!(invalid_revision.validate().is_err());

    let mut overflowing_counter = package();
    overflowing_counter
        .site
        .engagement
        .get_mut(&7)
        .unwrap()
        .views = u64::MAX;
    assert!(overflowing_counter.validate().is_err());
}

#[test]
fn archive_reader_rejects_special_and_duplicate_entries_before_extraction() {
    let temp = tempfile::tempdir().unwrap();
    let special = temp.path().join("special.simple-blog");
    write_test_archive(&special, |archive| {
        let mut header = tar::Header::new_gnu();
        header.set_entry_type(tar::EntryType::Symlink);
        header.set_size(0);
        header.set_mode(0o777);
        header.set_cksum();
        archive
            .append_data(&mut header, "media/link", Cursor::new(Vec::new()))
            .unwrap();
    });
    assert!(matches!(
        PortableArchive::read(&special).unwrap_err(),
        PortableArchiveError::UnsafeEntry(_)
    ));

    let duplicate = temp.path().join("duplicate.simple-blog");
    write_test_archive(&duplicate, |archive| {
        for body in [b"first".as_slice(), b"second".as_slice()] {
            let mut header = tar::Header::new_gnu();
            header.set_entry_type(tar::EntryType::Regular);
            header.set_size(u64::try_from(body.len()).unwrap());
            header.set_mode(0o600);
            header.set_cksum();
            archive.append_data(&mut header, "site.json", body).unwrap();
        }
    });
    assert!(matches!(
        PortableArchive::read(&duplicate).unwrap_err(),
        PortableArchiveError::UnsafeEntry(message) if message.contains("duplicate")
    ));
}

fn write_test_archive(
    path: &std::path::Path,
    write: impl FnOnce(&mut tar::Builder<zstd::Encoder<'static, File>>),
) {
    let file = File::create(path).unwrap();
    let encoder = zstd::Encoder::new(file, 1).unwrap();
    let mut archive = tar::Builder::new(encoder);
    write(&mut archive);
    let encoder = archive.into_inner().unwrap();
    encoder.finish().unwrap();
}

#[derive(Serialize)]
struct TestArchiveEntry {
    checksum: String,
    byte_size: u64,
}

#[derive(Serialize)]
struct TestArchiveIdentity {
    archive_format_version: u16,
    site_format_version: u16,
    producer_version: String,
    exported_at: DateTime<Utc>,
    entries: BTreeMap<String, TestArchiveEntry>,
}

#[derive(Serialize)]
struct TestArchiveManifest {
    archive_id: String,
    #[serde(flatten)]
    identity: TestArchiveIdentity,
}

fn write_identity_archive(
    path: &std::path::Path,
    site: &PortableSiteV1,
    identity_time: DateTime<Utc>,
    mutation: ManifestMutation,
) {
    let site_bytes = serde_json::to_vec(site).unwrap();
    let identity = TestArchiveIdentity {
        archive_format_version: 1,
        site_format_version: 1,
        producer_version: "contract-test".into(),
        exported_at: identity_time,
        entries: BTreeMap::from([(
            "site.json".into(),
            TestArchiveEntry {
                checksum: blake3::hash(&site_bytes).to_hex().to_string(),
                byte_size: u64::try_from(site_bytes.len()).unwrap(),
            },
        )]),
    };
    let identity_bytes = serde_json::to_vec(&identity).unwrap();
    let manifest = TestArchiveManifest {
        archive_id: blake3::hash(&identity_bytes).to_hex().to_string(),
        identity,
    };
    let manifest_bytes = match mutation {
        ManifestMutation::None => serde_json::to_vec(&manifest).unwrap(),
        ManifestMutation::UnknownField => {
            let mut value = serde_json::to_value(&manifest).unwrap();
            value
                .as_object_mut()
                .unwrap()
                .insert("future_adapter_state".into(), serde_json::json!(true));
            serde_json::to_vec(&value).unwrap()
        }
        ManifestMutation::DuplicateArchiveId => {
            let encoded = serde_json::to_string(&manifest).unwrap();
            format!(
                "{{\"archive_id\":\"{}\",{}",
                manifest.archive_id,
                &encoded[1..]
            )
            .into_bytes()
        }
    };
    write_test_archive(path, |archive| {
        append_test_entry(archive, "manifest.json", &manifest_bytes);
        append_test_entry(archive, "site.json", &site_bytes);
    });
}

#[derive(Clone, Copy)]
enum ManifestMutation {
    None,
    UnknownField,
    DuplicateArchiveId,
}

fn append_test_entry(
    archive: &mut tar::Builder<zstd::Encoder<'static, File>>,
    path: &str,
    bytes: &[u8],
) {
    let mut header = tar::Header::new_gnu();
    header.set_entry_type(tar::EntryType::Regular);
    header.set_size(u64::try_from(bytes.len()).unwrap());
    header.set_mode(0o600);
    header.set_cksum();
    archive.append_data(&mut header, path, bytes).unwrap();
}
#[test]
fn trashed_scheduled_content_does_not_participate_in_the_publication_clock() {
    let mut trashed = package();
    let later = trashed.site.exported_at + chrono::Duration::hours(2);
    trashed.site.contents[0].current.publication = Publication::Public { publish_at: later };
    trashed.site.contents[0].current.deleted_at = Some(trashed.site.exported_at);
    trashed.site.publication.next_publish_at = None;
    assert!(
        trashed.validate().is_ok(),
        "a trashed entry must not be expected to hold the clock"
    );

    let mut live = package();
    live.site.contents[0].current.publication = Publication::Public { publish_at: later };
    live.site.publication.next_publish_at = None;
    assert!(
        live.validate().is_err(),
        "a live scheduled entry must still be reflected by the clock"
    );

    let mut impossible = package();
    impossible.site.contents[0].current.deleted_at =
        Some(impossible.site.exported_at - chrono::Duration::days(400));
    assert!(
        impossible.validate().is_err(),
        "trashed before it was created is not a valid history"
    );
}

#[test]
fn archives_without_locale_settings_parse_and_defaults_serialize_without_them() {
    let site = package().site;
    let json = serde_json::to_value(&site).unwrap();
    let settings = json["settings"].as_object().unwrap();
    for key in ["timezone", "author_name", "custom_css_backup"] {
        assert!(
            !settings.contains_key(key),
            "{key} must be omitted at its default"
        );
    }
    assert_eq!(
        serde_json::from_value::<PortableSiteV1>(json).unwrap(),
        site
    );

    let mut tokyo = package();
    tokyo.site.settings.timezone = "Asia/Tokyo".into();
    tokyo.site.settings.author_name = "Ryo".into();
    let json = serde_json::to_value(&tokyo.site).unwrap();
    assert_eq!(json["settings"]["timezone"], "Asia/Tokyo");
    assert_eq!(json["settings"]["author_name"], "Ryo");
    assert_eq!(
        serde_json::from_value::<PortableSiteV1>(json).unwrap(),
        tokyo.site
    );
}

#[test]
fn packages_with_an_unknown_zone_fail_validation() {
    let mut invalid = package();
    invalid.site.settings.timezone = "Nowhere/Land".into();
    assert!(invalid.validate().is_err());
}
