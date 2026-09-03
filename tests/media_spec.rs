use std::{io::Cursor, path::Path, sync::Arc};

use async_trait::async_trait;
use chrono::Utc;
use image::{
    Delay, DynamicImage, Frame, ImageBuffer, ImageFormat, Rgb, RgbaImage, codecs::gif::GifEncoder,
};
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

    async fn delete_media(&self, _id: &MediaId) -> Result<(), MediaRepositoryError> {
        Ok(())
    }

    async fn mime_type_for_filename(
        &self,
        _filename: &str,
    ) -> Result<Option<String>, MediaRepositoryError> {
        Ok(None)
    }

    async fn update_media_alt_text(
        &self,
        _id: &MediaId,
        _alt_text: &str,
        _now: chrono::DateTime<Utc>,
    ) -> Result<bool, MediaRepositoryError> {
        Ok(false)
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

fn jpeg(width: u32, height: u32) -> Vec<u8> {
    let image = DynamicImage::ImageRgb8(ImageBuffer::from_fn(width, height, |x, y| {
        Rgb([(x % 255) as u8, (y % 255) as u8, 120])
    }));
    let mut cursor = Cursor::new(Vec::new());
    image.write_to(&mut cursor, ImageFormat::Jpeg).unwrap();
    cursor.into_inner()
}

fn webp(width: u32, height: u32) -> Vec<u8> {
    let image = DynamicImage::ImageRgb8(ImageBuffer::from_fn(width, height, |x, y| {
        Rgb([(x % 255) as u8, (y % 255) as u8, 120])
    }));
    let mut cursor = Cursor::new(Vec::new());
    image.write_to(&mut cursor, ImageFormat::WebP).unwrap();
    cursor.into_inner()
}

fn animated_gif(width: u32, height: u32, frames: u32) -> Vec<u8> {
    let mut bytes = Vec::new();
    {
        let mut encoder = GifEncoder::new(&mut bytes);
        for index in 0..frames {
            let shade = u8::try_from((index * 40) % 255).unwrap();
            let buffer = RgbaImage::from_pixel(width, height, image::Rgba([shade, 0, 0, 255]));
            let frame = Frame::from_parts(buffer, 0, 0, Delay::from_numer_denom_ms(100, 1));
            encoder.encode_frame(frame).unwrap();
        }
    }
    bytes
}

fn stored_bytes(temp: &tempfile::TempDir, filename: &str) -> Vec<u8> {
    std::fs::read(temp.path().join("media").join(filename)).unwrap()
}

fn is_webp(bytes: &[u8]) -> bool {
    bytes.get(..4) == Some(b"RIFF") && bytes.get(8..12) == Some(b"WEBP")
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

    assert_eq!(asset.mime_type, "image/webp");
    assert_eq!((asset.width, asset.height), (1_200, 800));
    assert_eq!(asset.id.as_str().len(), 64);
    assert!(has_extension(&asset.original_filename, "webp"));
    let original = stored_bytes(&temp, &asset.original_filename);
    assert!(is_webp(&original));
    // The stored (converted) bytes are the identity: filename hash and
    // byte_size describe the file on disk, which is what the doctor verifies.
    assert_eq!(
        blake3::hash(&original).to_hex().to_string(),
        *asset.id.as_str()
    );
    assert_eq!(asset.byte_size, original.len() as u64);
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
async fn jpeg_uploads_are_reencoded_as_lossy_webp() {
    let (temp, _repository, media) = harness(25 * 1024 * 1024).await;
    let asset = media
        .store("photo.jpg", jpeg(600, 400), "photo", "", Utc::now())
        .await
        .unwrap();

    assert_eq!(asset.mime_type, "image/webp");
    let original = stored_bytes(&temp, &asset.original_filename);
    // Lossy WebP carries the "VP8 " fourcc; lossless would be "VP8L".
    assert_eq!(&original[12..16], b"VP8 ");
    assert_eq!(
        blake3::hash(&original).to_hex().to_string(),
        *asset.id.as_str()
    );
}

#[tokio::test]
async fn webp_uploads_pass_through_without_reencoding() {
    let (temp, _repository, media) = harness(25 * 1024 * 1024).await;
    let bytes = webp(300, 200);
    let asset = media
        .store("already.webp", bytes.clone(), "", "", Utc::now())
        .await
        .unwrap();

    assert_eq!(asset.mime_type, "image/webp");
    assert_eq!(stored_bytes(&temp, &asset.original_filename), bytes);
    assert_eq!(
        blake3::hash(&bytes).to_hex().to_string(),
        *asset.id.as_str()
    );
}

#[tokio::test]
async fn animated_gifs_become_animated_webp() {
    let (temp, _repository, media) = harness(25 * 1024 * 1024).await;
    let asset = media
        .store("loop.gif", animated_gif(64, 48, 3), "", "", Utc::now())
        .await
        .unwrap();

    assert!(asset.animated);
    assert_eq!(asset.mime_type, "image/webp");
    let original = stored_bytes(&temp, &asset.original_filename);
    assert!(is_webp(&original));
    assert!(webpkit::is_animated(&original).unwrap());
    // Variants remain still images derived from the first frame.
    assert!(asset.variants.iter().all(|variant| {
        !webpkit::is_animated(&stored_bytes(&temp, &variant.filename)).unwrap()
    }));
}

#[tokio::test]
async fn reencoding_is_deterministic_so_duplicates_share_identity() {
    let (_temp, repository, media) = harness(25 * 1024 * 1024).await;
    let bytes = jpeg(120, 90);
    let first = media
        .store("a.jpg", bytes.clone(), "", "", Utc::now())
        .await
        .unwrap();
    let second = media
        .store("b.jpg", bytes, "", "", Utc::now())
        .await
        .unwrap();

    assert_eq!(first.id, second.id);
    assert_eq!(repository.list_media().await.unwrap().len(), 1);
}

#[tokio::test]
async fn images_wider_than_webp_allows_are_rejected() {
    let (_temp, repository, media) = harness(64 * 1024 * 1024).await;
    let error = media
        .store("wide.png", png(17_000, 2), "", "", Utc::now())
        .await
        .unwrap_err();

    assert!(matches!(error, MediaError::PixelLimit));
    assert!(repository.list_media().await.unwrap().is_empty());
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
    // A WebP upload passes through unchanged, so its stored name is the hash
    // of the uploaded bytes and can be planted ahead of the failing store.
    let bytes = webp(32, 24);
    let original = format!("{}.webp", blake3::hash(&bytes).to_hex());
    std::fs::write(directory.join(&original), &bytes).unwrap();
    let media = LocalMediaService::new(
        directory.clone(),
        Arc::new(FailingMediaRepository),
        25 * 1024 * 1024,
    );

    let error = media
        .store("failure.webp", bytes.clone(), "", "", Utc::now())
        .await
        .unwrap_err();

    assert!(matches!(error, MediaError::Repository(_)));
    assert_eq!(std::fs::read(directory.join(original)).unwrap(), bytes);
    assert_eq!(std::fs::read_dir(directory).unwrap().count(), 1);
}
