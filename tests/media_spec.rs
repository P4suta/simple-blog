use std::{io::Cursor, path::Path, sync::Arc};

use async_trait::async_trait;
use chrono::Utc;
use image::{DynamicImage, ImageBuffer, ImageFormat, Rgb};
use simple_blog::{
    application::ports::{MediaRepository, MediaRepositoryError},
    domain::media::{MediaAsset, MediaId},
    infrastructure::{
        media::{LocalMediaService, MediaError},
        sqlite::SqliteRepository,
    },
};

struct FailingMediaRepository;

#[async_trait]
impl MediaRepository for FailingMediaRepository {
    async fn save_media(&self, _media: &MediaAsset) -> Result<MediaAsset, MediaRepositoryError> {
        Err(MediaRepositoryError::Storage("injected failure".into()))
    }

    async fn find_media(&self, _id: &MediaId) -> Result<Option<MediaAsset>, MediaRepositoryError> {
        Ok(None)
    }

    async fn list_media(&self) -> Result<Vec<MediaAsset>, MediaRepositoryError> {
        Ok(Vec::new())
    }
}

async fn harness(
    max_bytes: usize,
) -> (tempfile::TempDir, Arc<SqliteRepository>, LocalMediaService) {
    let temp = tempfile::tempdir().unwrap();
    let repository = Arc::new(
        SqliteRepository::connect(&temp.path().join("blog.sqlite3"))
            .await
            .unwrap(),
    );
    let media = LocalMediaService::new(temp.path().join("media"), repository.clone(), max_bytes);
    (temp, repository, media)
}

fn png(width: u32, height: u32) -> Vec<u8> {
    let image = DynamicImage::ImageRgb8(ImageBuffer::from_fn(width, height, |x, y| {
        Rgb([(x % 255) as u8, (y % 255) as u8, 120])
    }));
    let mut cursor = Cursor::new(Vec::new());
    image.write_to(&mut cursor, ImageFormat::Png).unwrap();
    cursor.into_inner()
}

fn has_extension(filename: &str, expected: &str) -> bool {
    Path::new(filename)
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case(expected))
}

#[tokio::test]
async fn verified_image_is_content_addressed_and_gets_responsive_webp_variants() {
    let (temp, repository, media) = harness(25 * 1024 * 1024).await;
    let bytes = png(1_200, 800);
    let asset = media
        .store(
            "cover-anything.exe",
            bytes.clone(),
            "A landscape",
            "Caption",
            Utc::now(),
        )
        .await
        .unwrap();

    assert_eq!(asset.mime_type, "image/png");
    assert_eq!((asset.width, asset.height), (1_200, 800));
    assert_eq!(asset.byte_size, bytes.len() as u64);
    assert_eq!(asset.id.as_str().len(), 64);
    assert!(has_extension(&asset.original_filename, "png"));
    assert!(
        temp.path()
            .join("media")
            .join(&asset.original_filename)
            .is_file()
    );
    assert_eq!(
        asset.variants.iter().map(|v| v.width).collect::<Vec<_>>(),
        [480, 960, 1_200]
    );
    assert!(asset.variants.iter().all(|variant| {
        has_extension(&variant.filename, "webp")
            && temp.path().join("media").join(&variant.filename).is_file()
    }));

    let stored = repository.find_media(&asset.id).await.unwrap().unwrap();
    assert_eq!(stored, asset);
}

#[tokio::test]
async fn spoofed_truncated_and_oversized_uploads_leave_no_files_or_rows() {
    let (temp, repository, media) = harness(128).await;
    for (name, bytes, expected) in [
        ("fake.png", b"<svg><script/></svg>".to_vec(), "unsupported"),
        ("broken.jpg", vec![0xff, 0xd8, 0xff, 0xdb], "decode"),
        ("large.png", vec![0_u8; 129], "limit"),
    ] {
        let error = media
            .store(name, bytes, "", "", Utc::now())
            .await
            .expect_err(name);
        match expected {
            "unsupported" => assert!(matches!(error, MediaError::UnsupportedType)),
            "decode" => assert!(matches!(error, MediaError::InvalidImage(_))),
            "limit" => assert!(matches!(error, MediaError::TooLarge { .. })),
            _ => unreachable!(),
        }
    }
    assert!(repository.list_media().await.unwrap().is_empty());
    assert!(
        !temp.path().join("media").exists()
            || std::fs::read_dir(temp.path().join("media"))
                .unwrap()
                .next()
                .is_none()
    );
}

#[tokio::test]
async fn duplicate_bytes_reuse_the_same_asset_identity() {
    let (_temp, repository, media) = harness(25 * 1024 * 1024).await;
    let bytes = png(32, 24);
    let first = media
        .store("one.png", bytes.clone(), "first", "", Utc::now())
        .await
        .unwrap();
    let second = media
        .store("two.png", bytes, "second", "", Utc::now())
        .await
        .unwrap();

    assert_eq!(first.id, second.id);
    assert_eq!(repository.list_media().await.unwrap().len(), 1);
    assert_eq!(second.alt_text, "second");
}

#[tokio::test]
async fn repository_failure_compensates_every_newly_installed_media_file() {
    let temp = tempfile::tempdir().unwrap();
    let directory = temp.path().join("media");
    let media = LocalMediaService::new(
        directory.clone(),
        Arc::new(FailingMediaRepository),
        25 * 1024 * 1024,
    );

    let error = media
        .store("failure.png", png(1_200, 800), "", "", Utc::now())
        .await
        .unwrap_err();

    assert!(matches!(error, MediaError::Repository(_)));
    assert!(directory.is_dir() && std::fs::read_dir(directory).unwrap().next().is_none());
}

#[tokio::test]
async fn compensation_never_removes_content_addressed_files_that_already_existed() {
    let temp = tempfile::tempdir().unwrap();
    let directory = temp.path().join("media");
    std::fs::create_dir_all(&directory).unwrap();
    let bytes = png(32, 24);
    let original = format!("{}.png", blake3::hash(&bytes).to_hex());
    std::fs::write(directory.join(&original), &bytes).unwrap();
    let media = LocalMediaService::new(
        directory.clone(),
        Arc::new(FailingMediaRepository),
        25 * 1024 * 1024,
    );

    let error = media
        .store("failure.png", bytes.clone(), "", "", Utc::now())
        .await
        .unwrap_err();

    assert!(matches!(error, MediaError::Repository(_)));
    assert_eq!(std::fs::read(directory.join(original)).unwrap(), bytes);
    assert_eq!(std::fs::read_dir(directory).unwrap().count(), 1);
}
