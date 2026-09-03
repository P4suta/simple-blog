use std::{
    collections::BTreeSet,
    fs::OpenOptions,
    io::{Cursor, Write},
    path::{Path, PathBuf},
    sync::Arc,
};

use chrono::{DateTime, Utc};
use exif::{In, Reader as ExifReader, Tag};
use image::{
    AnimationDecoder, DynamicImage, GenericImageView, ImageFormat, RgbaImage,
    codecs::gif::GifDecoder,
    imageops::{self, FilterType},
};
use thiserror::Error;
use uuid::Uuid;
use webpkit::{
    AnimationEncoder, BlendMode, Dimensions, DisposalMode, Encoder, FrameMeta, ImageRef,
    PixelLayout, lossless::HasFrames,
};

use crate::{
    application::ports::{MediaRepository, MediaRepositoryError},
    domain::media::{MediaAsset, MediaId, MediaVariant},
};

const MAX_PIXELS: u64 = 100_000_000;
const RESPONSIVE_WIDTHS: [u32; 3] = [480, 960, 1_440];
const LOSSY_QUALITY: u8 = 85;
// WebP's coded dimension limit; larger images cannot be represented at all.
const MAX_WEBP_SIDE: u32 = 16_383;

#[derive(Clone)]
pub struct LocalMediaService {
    directory: PathBuf,
    repository: Arc<dyn MediaRepository>,
    max_bytes: usize,
}

#[derive(Debug, Error)]
pub enum MediaError {
    #[error("upload is empty")]
    Empty,
    #[error("upload exceeds the {limit}-byte limit (received {actual} bytes)")]
    TooLarge { limit: usize, actual: usize },
    #[error("only JPEG, PNG, WebP, and GIF images are accepted")]
    UnsupportedType,
    #[error("image could not be decoded: {0}")]
    InvalidImage(String),
    #[error("image dimensions exceed the safety limit")]
    PixelLimit,
    #[error("image could not be encoded: {0}")]
    Encode(String),
    #[error("invalid media metadata: {0}")]
    InvalidMetadata(String),
    #[error("media file operation failed: {0}")]
    File(String),
    #[error(transparent)]
    Repository(#[from] MediaRepositoryError),
    #[error("media processing task failed: {0}")]
    Task(String),
}

struct ProcessedUpload {
    asset: MediaAsset,
    files: Vec<(String, Vec<u8>)>,
}

impl LocalMediaService {
    pub fn new(directory: PathBuf, repository: Arc<dyn MediaRepository>, max_bytes: usize) -> Self {
        Self {
            directory,
            repository,
            max_bytes,
        }
    }

    #[must_use]
    pub fn directory(&self) -> &Path {
        &self.directory
    }

    /// Deletes every media asset whose ID is not in `referenced`: rows first,
    /// then files. A crash in between leaves only stray files, which the
    /// doctor's orphan check surfaces.
    pub async fn collect_garbage(
        &self,
        referenced: &std::collections::HashSet<String>,
    ) -> Result<usize, MediaError> {
        let mut removed = 0;
        for asset in self.repository.list_media().await? {
            if referenced.contains(asset.id.as_str()) {
                continue;
            }
            self.remove_asset(&asset).await?;
            removed += 1;
        }
        Ok(removed)
    }

    /// Deletes one asset the owner chose to drop; `false` when it does not
    /// exist. The caller decides whether anything still shows it.
    pub async fn delete_asset(&self, id: &MediaId) -> Result<bool, MediaError> {
        let Some(asset) = self.repository.find_media(id).await? else {
            return Ok(false);
        };
        self.remove_asset(&asset).await?;
        Ok(true)
    }

    async fn remove_asset(&self, asset: &MediaAsset) -> Result<(), MediaError> {
        self.repository.delete_media(&asset.id).await?;
        let filenames = std::iter::once(&asset.original_filename)
            .chain(asset.variants.iter().map(|variant| &variant.filename));
        for filename in filenames {
            match std::fs::remove_file(self.directory.join(filename)) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(MediaError::File(error.to_string())),
            }
        }
        Ok(())
    }

    /// Replaces an asset's alternative text after the same validation the
    /// upload applied; `false` when no such asset exists.
    pub async fn update_alt_text(
        &self,
        id: &MediaId,
        alt_text: &str,
        now: DateTime<Utc>,
    ) -> Result<bool, MediaError> {
        let alt_text = clean_text(alt_text, 500, "alt text")?;
        self.repository
            .update_media_alt_text(id, &alt_text, now)
            .await
            .map_err(MediaError::from)
    }

    pub async fn store(
        &self,
        original_name: &str,
        bytes: Vec<u8>,
        alt_text: &str,
        caption: &str,
        now: DateTime<Utc>,
    ) -> Result<MediaAsset, MediaError> {
        if bytes.is_empty() {
            return Err(MediaError::Empty);
        }
        if bytes.len() > self.max_bytes {
            return Err(MediaError::TooLarge {
                limit: self.max_bytes,
                actual: bytes.len(),
            });
        }
        let original_name = clean_name(original_name)?;
        let alt_text = clean_text(alt_text, 500, "alt text")?;
        let caption = clean_text(caption, 2_000, "caption")?;
        let processed = tokio::task::spawn_blocking(move || {
            process(original_name, bytes, alt_text, caption, now)
        })
        .await
        .map_err(|error| MediaError::Task(error.to_string()))??;

        std::fs::create_dir_all(&self.directory)
            .map_err(|error| MediaError::File(error.to_string()))?;
        let mut installation = FileInstallation::default();
        for (filename, contents) in &processed.files {
            if atomic_write(&self.directory, filename, contents)? {
                installation.track(self.directory.join(filename));
            }
        }
        let asset = self
            .repository
            .save_media(&processed.asset)
            .await
            .map_err(MediaError::from)?;
        installation.commit();
        Ok(asset)
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum SourceFormat {
    Jpeg,
    Png,
    Gif,
    WebP,
}

impl SourceFormat {
    const fn image_format(self) -> ImageFormat {
        match self {
            Self::Jpeg => ImageFormat::Jpeg,
            Self::Png => ImageFormat::Png,
            Self::Gif => ImageFormat::Gif,
            Self::WebP => ImageFormat::WebP,
        }
    }
}

/// Every upload is stored as WebP: JPEG re-encodes lossy, PNG and still GIF
/// lossless, animated GIF becomes an animated WebP, and a WebP upload passes
/// through untouched. The media ID is the BLAKE3 digest of the stored bytes,
/// which keeps the doctor's "filename hash matches file content" check intact.
fn process(
    original_name: String,
    bytes: Vec<u8>,
    alt_text: String,
    caption: String,
    now: DateTime<Utc>,
) -> Result<ProcessedUpload, MediaError> {
    let kind = infer::get(&bytes).ok_or(MediaError::UnsupportedType)?;
    let source = match kind.mime_type() {
        "image/jpeg" => SourceFormat::Jpeg,
        "image/png" => SourceFormat::Png,
        "image/webp" => SourceFormat::WebP,
        "image/gif" => SourceFormat::Gif,
        _ => return Err(MediaError::UnsupportedType),
    };
    let decoded = image::load_from_memory_with_format(&bytes, source.image_format())
        .map_err(|error| MediaError::InvalidImage(error.to_string()))?;
    let oriented = apply_exif_orientation(decoded, &bytes, source.image_format());
    let (width, height) = oriented.dimensions();
    if u64::from(width) * u64::from(height) > MAX_PIXELS || width.max(height) > MAX_WEBP_SIDE {
        return Err(MediaError::PixelLimit);
    }

    let (canonical, animated, lossy) = match source {
        SourceFormat::WebP => {
            let animated = webpkit::is_animated(&bytes).unwrap_or(false);
            let lossy = webp_is_lossy(&bytes);
            (bytes, animated, lossy)
        }
        SourceFormat::Gif if gif_is_animated(&bytes) => {
            (encode_animation(&bytes, width, height)?, true, false)
        }
        SourceFormat::Jpeg => (encode_still(&oriented, true)?, false, true),
        SourceFormat::Png | SourceFormat::Gif => (encode_still(&oriented, false)?, false, false),
    };

    let digest = blake3::hash(&canonical).to_hex().to_string();
    let byte_size = u64::try_from(canonical.len())
        .map_err(|error| MediaError::InvalidImage(error.to_string()))?;
    let id =
        MediaId::parse(&digest).map_err(|error| MediaError::InvalidImage(error.to_string()))?;
    let original_filename = format!("{id}.webp");
    let mut files = vec![(original_filename.clone(), canonical)];
    let mut widths: BTreeSet<u32> = RESPONSIVE_WIDTHS
        .into_iter()
        .filter(|candidate| *candidate < width)
        .collect();
    widths.insert(width.min(1_440));
    let mut variants = Vec::with_capacity(widths.len());
    for variant_width in widths {
        let variant_height = scaled_height(width, height, variant_width);
        let resized = if variant_width == width {
            oriented.clone()
        } else {
            oriented.resize_exact(variant_width, variant_height, FilterType::Lanczos3)
        };
        let encoded = encode_still(&resized, lossy)?;
        let filename = format!("{id}-{variant_width}w.webp");
        variants.push(MediaVariant {
            width: variant_width,
            height: variant_height,
            byte_size: u64::try_from(encoded.len())
                .map_err(|error| MediaError::InvalidImage(error.to_string()))?,
            filename: filename.clone(),
        });
        files.push((filename, encoded));
    }

    Ok(ProcessedUpload {
        asset: MediaAsset {
            id,
            original_name,
            original_filename,
            mime_type: "image/webp".into(),
            extension: "webp".into(),
            width,
            height,
            byte_size,
            alt_text,
            caption,
            animated,
            variants,
            created_at: now,
        },
        files,
    })
}

fn encode_still(image: &DynamicImage, lossy: bool) -> Result<Vec<u8>, MediaError> {
    let source = webpkit::Image::try_from(image).map_err(|error| encode_error(&error))?;
    let encoded = if lossy {
        Encoder::lossy().quality(LOSSY_QUALITY).encode(&source)
    } else {
        Encoder::lossless().encode(&source)
    };
    encoded.map_err(|error| encode_error(&error))
}

/// Re-encodes an animated GIF as an animated WebP. Frames arrive from the
/// image crate already composited onto the full canvas, so each becomes an
/// overwrite frame; GIF loop counts are not exposed, so loops are infinite.
fn encode_animation(gif_bytes: &[u8], width: u32, height: u32) -> Result<Vec<u8>, MediaError> {
    let canvas = Dimensions::new(width, height).map_err(|error| encode_error(&error))?;
    let decoder = GifDecoder::new(Cursor::new(gif_bytes))
        .map_err(|error| MediaError::InvalidImage(error.to_string()))?;
    let mut encoder: Option<AnimationEncoder<HasFrames>> = None;
    let mut total_pixels: u64 = 0;
    for frame in decoder.into_frames() {
        let frame = frame.map_err(|error| MediaError::InvalidImage(error.to_string()))?;
        total_pixels = total_pixels.saturating_add(canvas.pixel_count());
        if total_pixels > MAX_PIXELS {
            return Err(MediaError::PixelLimit);
        }
        let (numerator, denominator) = frame.delay().numer_denom_ms();
        let duration_ms = numerator.checked_div(denominator).unwrap_or(numerator);
        let left = frame.left();
        let top = frame.top();
        let buffer = if frame.buffer().dimensions() == (width, height) && left == 0 && top == 0 {
            frame.into_buffer()
        } else {
            let mut full = RgbaImage::new(width, height);
            imageops::overlay(&mut full, frame.buffer(), i64::from(left), i64::from(top));
            full
        };
        let image_ref = ImageRef::new(canvas, PixelLayout::Rgba8, buffer.as_raw())
            .map_err(|error| encode_error(&error))?;
        let meta = FrameMeta {
            x: 0,
            y: 0,
            dimensions: canvas,
            duration_ms,
            blend: BlendMode::Overwrite,
            dispose: DisposalMode::Keep,
        };
        encoder = Some(match encoder.take() {
            None => AnimationEncoder::new(canvas)
                .with_loop_count(0)
                .add_frame(image_ref, meta)
                .map_err(|error| encode_error(&error))?,
            Some(started) => started
                .add_frame(image_ref, meta)
                .map_err(|error| encode_error(&error))?,
        });
    }
    encoder
        .map(AnimationEncoder::finish)
        .ok_or_else(|| MediaError::InvalidImage("animated GIF yielded no decodable frames".into()))
}

/// Whether a WebP file's primary bitstream is lossy (`VP8 `). `VP8X` extended
/// files (including animations) count as lossless here, which only means their
/// still variants are re-encoded losslessly.
fn webp_is_lossy(bytes: &[u8]) -> bool {
    bytes.get(12..16).is_some_and(|fourcc| fourcc == b"VP8 ")
}

fn encode_error(error: &webpkit::Error) -> MediaError {
    MediaError::Encode(error.to_string())
}

fn apply_exif_orientation(image: DynamicImage, bytes: &[u8], format: ImageFormat) -> DynamicImage {
    if format != ImageFormat::Jpeg {
        return image;
    }
    let orientation = ExifReader::new()
        .read_from_container(&mut Cursor::new(bytes))
        .ok()
        .and_then(|exif| {
            exif.get_field(Tag::Orientation, In::PRIMARY)
                .and_then(|field| field.value.get_uint(0))
        })
        .unwrap_or(1);
    match orientation {
        2 => image.fliph(),
        3 => image.rotate180(),
        4 => image.flipv(),
        5 => image.rotate90().fliph(),
        6 => image.rotate90(),
        7 => image.rotate270().fliph(),
        8 => image.rotate270(),
        _ => image,
    }
}

fn gif_is_animated(bytes: &[u8]) -> bool {
    GifDecoder::new(Cursor::new(bytes))
        .is_ok_and(|decoder| decoder.into_frames().take(2).count() > 1)
}

fn scaled_height(width: u32, height: u32, target_width: u32) -> u32 {
    let numerator = u64::from(height) * u64::from(target_width);
    u32::try_from((numerator + u64::from(width) / 2) / u64::from(width))
        .unwrap_or(1)
        .max(1)
}

fn clean_name(value: &str) -> Result<String, MediaError> {
    let name = Path::new(value)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("upload")
        .trim();
    let name: String = name
        .chars()
        .filter(|character| !character.is_control())
        .collect();
    if name.chars().count() > 255 {
        return Err(MediaError::InvalidMetadata(
            "original filename exceeds 255 characters".into(),
        ));
    }
    Ok(if name.is_empty() {
        "upload".into()
    } else {
        name
    })
}

fn clean_text(value: &str, limit: usize, field: &str) -> Result<String, MediaError> {
    let value = value.trim();
    if value.chars().count() > limit {
        return Err(MediaError::InvalidMetadata(format!(
            "{field} exceeds {limit} characters"
        )));
    }
    Ok(value.to_owned())
}

fn atomic_write(directory: &Path, filename: &str, bytes: &[u8]) -> Result<bool, MediaError> {
    let destination = directory.join(filename);
    if destination.is_file() {
        return Ok(false);
    }
    let temporary = directory.join(format!(".upload-{}.tmp", Uuid::new_v4()));
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        std::fs::rename(&temporary, &destination)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
        .map(|()| true)
        .map_err(|error| MediaError::File(error.to_string()))
}

#[derive(Default)]
struct FileInstallation {
    paths: Vec<PathBuf>,
    committed: bool,
}

impl FileInstallation {
    fn track(&mut self, path: PathBuf) {
        self.paths.push(path);
    }

    fn commit(mut self) {
        self.committed = true;
    }
}

impl Drop for FileInstallation {
    fn drop(&mut self) {
        if self.committed {
            return;
        }
        let mut failures = 0_usize;
        for path in &self.paths {
            if std::fs::remove_file(path).is_err() {
                failures += 1;
            }
        }
        if !self.paths.is_empty() {
            tracing::warn!(
                event = "media.installation.compensated",
                removed = self.paths.len() - failures,
                failures
            );
        }
    }
}
