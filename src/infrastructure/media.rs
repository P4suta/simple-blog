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
    AnimationDecoder, DynamicImage, GenericImageView, ImageFormat, codecs::gif::GifDecoder,
    imageops::FilterType,
};
use thiserror::Error;
use uuid::Uuid;

use crate::{
    application::ports::{MediaRepository, MediaRepositoryError},
    domain::media::{MediaAsset, MediaId, MediaVariant},
};

const MAX_PIXELS: u64 = 100_000_000;
const RESPONSIVE_WIDTHS: [u32; 3] = [480, 960, 1_440];

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

fn process(
    original_name: String,
    bytes: Vec<u8>,
    alt_text: String,
    caption: String,
    now: DateTime<Utc>,
) -> Result<ProcessedUpload, MediaError> {
    let kind = infer::get(&bytes).ok_or(MediaError::UnsupportedType)?;
    let (mime_type, extension, format) = match kind.mime_type() {
        "image/jpeg" => ("image/jpeg", "jpg", ImageFormat::Jpeg),
        "image/png" => ("image/png", "png", ImageFormat::Png),
        "image/webp" => ("image/webp", "webp", ImageFormat::WebP),
        "image/gif" => ("image/gif", "gif", ImageFormat::Gif),
        _ => return Err(MediaError::UnsupportedType),
    };
    let decoded = image::load_from_memory_with_format(&bytes, format)
        .map_err(|error| MediaError::InvalidImage(error.to_string()))?;
    let oriented = apply_exif_orientation(decoded, &bytes, format);
    let (width, height) = oriented.dimensions();
    if u64::from(width) * u64::from(height) > MAX_PIXELS {
        return Err(MediaError::PixelLimit);
    }
    let animated = format == ImageFormat::Gif && gif_is_animated(&bytes);
    let digest = blake3::hash(&bytes).to_hex().to_string();
    let byte_size =
        u64::try_from(bytes.len()).map_err(|error| MediaError::InvalidImage(error.to_string()))?;
    let id =
        MediaId::parse(&digest).map_err(|error| MediaError::InvalidImage(error.to_string()))?;
    let original_filename = format!("{id}.{extension}");
    let mut files = vec![(original_filename.clone(), bytes)];
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
        let mut encoded = Cursor::new(Vec::new());
        resized
            .write_to(&mut encoded, ImageFormat::WebP)
            .map_err(|error| MediaError::InvalidImage(error.to_string()))?;
        let encoded = encoded.into_inner();
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
            mime_type: mime_type.into(),
            extension: extension.into(),
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
