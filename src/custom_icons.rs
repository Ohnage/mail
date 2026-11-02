//! Bounded, content-addressed image staging. No settings are written here.
use anyhow::{Context, Result, bail};
use image::{ImageFormat, ImageReader, Limits};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{Cursor, Read, Write},
    path::Path,
    sync::atomic::{AtomicU64, Ordering},
};

const MAX_SOURCE_BYTES: u64 = 16 * 1024 * 1024;
const MAX_DECODED_PIXELS: u64 = 32 * 1024 * 1024;
static NEXT_STAGE: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy)]
pub(crate) enum IconKind {
    Account,
    Tab,
}
impl IconKind {
    fn directory(self) -> &'static str {
        match self {
            Self::Account => "account-icons",
            Self::Tab => "tab-icons",
        }
    }
}

/// Return a data-root-relative PNG path, ready to commit with settings.
/// A cancelled or failed settings write may leave an unreferenced immutable
/// PNG, but cannot replace the previous icon or change its settings entry.
pub(crate) fn import_icon(data_root: &Path, source: &Path, kind: IconKind) -> Result<String> {
    let mut bytes = Vec::new();
    fs::File::open(source)
        .context("Open icon")?
        .take(MAX_SOURCE_BYTES + 1)
        .read_to_end(&mut bytes)
        .context("Read icon")?;
    if bytes.len() as u64 > MAX_SOURCE_BYTES {
        bail!("Choose an image smaller than 16 MiB");
    }
    let png = normalize_icon(&bytes)?;
    let digest = format!("{:x}", Sha256::digest(&png));
    let relative = format!("{}/{digest}.png", kind.directory());
    let destination = data_root.join(&relative);
    let directory = data_root.join(kind.directory());
    fs::create_dir_all(&directory).context("Create icon directory")?;
    if destination.is_file() && fs::read(&destination)? == png {
        return Ok(relative);
    }
    let stage = directory.join(format!(
        ".{digest}.{}.{}.tmp",
        std::process::id(),
        NEXT_STAGE.fetch_add(1, Ordering::Relaxed)
    ));
    let result = (|| -> Result<()> {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&stage)?;
        file.write_all(&png)?;
        file.sync_all()?;
        drop(file);
        match fs::rename(&stage, &destination) {
            Ok(()) => Ok(()),
            Err(_) if destination.is_file() && fs::read(&destination)? == png => Ok(()),
            Err(error) => Err(error.into()),
        }
    })();
    let _ = fs::remove_file(&stage);
    result.context("Store normalized icon")?;
    Ok(relative)
}

fn normalize_icon(bytes: &[u8]) -> Result<Vec<u8>> {
    let (width, height) = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()?
        .into_dimensions()
        .context("Read image dimensions")?;
    if width == 0
        || height == 0
        || width > 16_384
        || height > 16_384
        || u64::from(width) * u64::from(height) > MAX_DECODED_PIXELS
    {
        bail!("Choose an image with at most 32 million pixels and sides below 16384 pixels");
    }
    let mut limits = Limits::default();
    limits.max_image_width = Some(16_384);
    limits.max_image_height = Some(16_384);
    limits.max_alloc = Some(128 * 1024 * 1024);
    let mut reader = ImageReader::new(Cursor::new(bytes)).with_guessed_format()?;
    reader.limits(limits);
    let image = reader.decode().context("Decode image")?;
    let image = if width > 512 || height > 512 {
        image.thumbnail(512, 512)
    } else {
        image
    };
    let mut output = Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(image.into_rgba8()).write_to(&mut output, ImageFormat::Png)?;
    Ok(output.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn png(width: u32, height: u32) -> Vec<u8> {
        let image = image::RgbaImage::from_pixel(width, height, image::Rgba([40, 80, 160, 96]));
        let mut output = Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(image)
            .write_to(&mut output, ImageFormat::Png)
            .unwrap();
        output.into_inner()
    }
    #[test]
    fn normalization_preserves_aspect_ratio_and_transparency() {
        let normalized = normalize_icon(&png(1200, 600)).unwrap();
        let result = image::load_from_memory(&normalized).unwrap().into_rgba8();
        assert_eq!(result.dimensions(), (512, 256));
        assert_eq!(result.get_pixel(200, 100).0[3], 96);
    }
    #[test]
    fn small_icons_are_not_upscaled_and_invalid_images_are_rejected() {
        let result = image::load_from_memory(&normalize_icon(&png(24, 12)).unwrap()).unwrap();
        assert_eq!((result.width(), result.height()), (24, 12));
        assert!(normalize_icon(b"not an image").is_err());
    }
    #[test]
    fn staged_icons_are_content_addressed_and_do_not_change_after_failure() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("source.png");
        fs::write(&source, png(1200, 600)).unwrap();
        let relative = import_icon(root.path(), &source, IconKind::Account).unwrap();
        assert!(relative.starts_with("account-icons/"));
        let original = fs::read(root.path().join(&relative)).unwrap();
        assert_eq!(
            import_icon(root.path(), &source, IconKind::Account).unwrap(),
            relative
        );
        fs::write(&source, b"invalid image").unwrap();
        assert!(import_icon(root.path(), &source, IconKind::Account).is_err());
        assert_eq!(fs::read(root.path().join(&relative)).unwrap(), original);
        assert_eq!(
            fs::read_dir(root.path().join("account-icons"))
                .unwrap()
                .count(),
            1
        );
    }

    #[test]
    fn decoded_bounds_are_checked_before_allocation() {
        let mut bytes = png(1, 1);
        bytes[16..20].copy_from_slice(&8192u32.to_be_bytes());
        bytes[20..24].copy_from_slice(&8192u32.to_be_bytes());
        let mut crc = 0xffff_ffffu32;
        for byte in &bytes[12..29] {
            crc ^= u32::from(*byte);
            for _ in 0..8 {
                crc = if crc & 1 != 0 {
                    (crc >> 1) ^ 0xedb8_8320
                } else {
                    crc >> 1
                };
            }
        }
        bytes[29..33].copy_from_slice(&(!crc).to_be_bytes());
        let error = normalize_icon(&bytes).unwrap_err();
        assert!(error.to_string().contains("32 million pixels"), "{error}");
    }
}
