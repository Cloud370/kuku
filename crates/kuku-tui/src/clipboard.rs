use std::fs::File;
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};

use base64::Engine;
use image::codecs::jpeg::JpegEncoder;
use image::{DynamicImage, ImageDecoder, ImageFormat, ImageReader, RgbaImage};
use kuku_client::MAX_MODEL_IMAGE_BYTES;

const MAX_SOURCE_BYTES: u64 = 32 * 1024 * 1024;
const MAX_SOURCE_PIXELS: usize = 32 * 1024 * 1024;

pub(crate) enum PasteSource {
    Clipboard { image_only: bool },
    ImagePath(String),
}

impl PasteSource {
    pub(crate) fn read(self, workspace: &Path) -> Result<Pasted, String> {
        match self {
            Self::Clipboard { image_only } => read_clipboard(workspace, image_only),
            Self::ImagePath(text) => image_from_text(workspace, text),
        }
    }
}

pub(crate) enum Pasted {
    Text(String),
    Image(ClipboardImage),
}

pub(crate) struct ClipboardImage {
    pub(crate) pixels: DynamicImage,
    pub(crate) encoded: Option<Vec<u8>>,
    pub(crate) transparent: bool,
}

impl std::fmt::Debug for ClipboardImage {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ClipboardImage")
            .field("width", &self.pixels.width())
            .field("height", &self.pixels.height())
            .field("transparent", &self.transparent)
            .finish()
    }
}

impl ClipboardImage {
    pub(crate) fn encode(&self, original: bool) -> Result<String, String> {
        if original && let Some(bytes) = &self.encoded {
            if bytes.len() > MAX_MODEL_IMAGE_BYTES {
                return Err("Original image exceeds 1 MiB".into());
            }
            return Ok(base64::engine::general_purpose::STANDARD.encode(bytes));
        }

        let mut pixels = std::borrow::Cow::Borrowed(&self.pixels);
        loop {
            let bytes = if original || self.transparent {
                let mut encoded = Cursor::new(Vec::new());
                pixels
                    .write_to(&mut encoded, ImageFormat::Png)
                    .map_err(|error| error.to_string())?;
                encoded.into_inner()
            } else {
                let mut encoded = Vec::new();
                JpegEncoder::new_with_quality(&mut encoded, 85)
                    .encode_image(pixels.as_ref())
                    .map_err(|error| error.to_string())?;
                encoded
            };
            if bytes.len() <= MAX_MODEL_IMAGE_BYTES {
                return Ok(base64::engine::general_purpose::STANDARD.encode(bytes));
            }
            if original || (pixels.width() == 1 && pixels.height() == 1) {
                return Err("Original image exceeds 1 MiB".into());
            }
            let width = (pixels.width() * 3 / 4).max(1);
            let height = (pixels.height() * 3 / 4).max(1);
            pixels = std::borrow::Cow::Owned(pixels.resize(
                width,
                height,
                image::imageops::FilterType::Triangle,
            ));
        }
    }
}

fn supported_path(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            ["png", "jpg", "jpeg"]
                .iter()
                .any(|format| extension.eq_ignore_ascii_case(format))
        })
}

pub(crate) fn image_path_candidate(text: &str) -> Option<PathBuf> {
    let text = text.trim();
    if text.is_empty() || text.contains(['\r', '\n', '\0']) {
        return None;
    }
    let text = text
        .strip_prefix('"')
        .and_then(|text| text.strip_suffix('"'))
        .or_else(|| {
            text.strip_prefix('\'')
                .and_then(|text| text.strip_suffix('\''))
        })
        .unwrap_or(text);
    let path = if text.starts_with("file:") {
        url::Url::parse(text).ok()?.to_file_path().ok()?
    } else {
        PathBuf::from(text)
    };
    supported_path(&path).then_some(path)
}

fn image_from_text(workspace: &Path, text: String) -> Result<Pasted, String> {
    let Some(path) = image_path_candidate(&text) else {
        return Ok(Pasted::Text(text));
    };
    let path = resolve_image_path(workspace, path);
    if path.is_file() {
        read_file(&path).map(Pasted::Image)
    } else {
        Ok(Pasted::Text(text))
    }
}

fn resolve_image_path(workspace: &Path, path: PathBuf) -> PathBuf {
    #[cfg(target_os = "linux")]
    let path = wsl_image_path(&path).unwrap_or(path);
    workspace.join(path)
}

#[cfg(target_os = "linux")]
fn wsl_image_path(path: &Path) -> Option<PathBuf> {
    let text = path.to_str()?;
    let [drive, b':', b'\\' | b'/', ..] = text.as_bytes() else {
        return None;
    };
    if !drive.is_ascii_alphabetic() {
        return None;
    }
    let output = std::process::Command::new("wslpath")
        .args(["-u", text])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let path = PathBuf::from(
        std::str::from_utf8(&output.stdout)
            .ok()?
            .trim_end_matches(['\r', '\n']),
    );
    path.is_absolute().then_some(path)
}

fn read_clipboard(workspace: &Path, image_only: bool) -> Result<Pasted, String> {
    let native = read_native_clipboard(workspace, image_only);
    #[cfg(target_os = "linux")]
    if (native.is_err() || matches!(&native, Ok(Pasted::Text(text)) if text.is_empty()))
        && let Some(result) = read_windows_bitmap()
    {
        return result.map(Pasted::Image);
    }
    native
}

#[cfg(target_os = "linux")]
fn read_windows_bitmap() -> Option<Result<ClipboardImage, String>> {
    let release = std::fs::read_to_string("/proc/sys/kernel/osrelease").ok()?;
    if !release.to_ascii_lowercase().contains("microsoft") {
        return None;
    }
    let script = r#"
        $img = Get-Clipboard -Format Image
        if ($null -eq $img -or ([int64]$img.Width * $img.Height) -gt 33554432) { exit 1 }
        $stream = [System.IO.MemoryStream]::new()
        try {
            $img.Save($stream, [System.Drawing.Imaging.ImageFormat]::Png)
            if ($stream.Length -gt 33554432) { exit 1 }
            $bytes = $stream.ToArray()
            [Console]::OpenStandardOutput().Write($bytes, 0, $bytes.Length)
        } finally {
            $stream.Dispose()
            $img.Dispose()
        }
    "#;
    let output = std::process::Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-STA", "-Command", script])
        .output()
        .ok()?;
    (output.status.success() && output.stdout.starts_with(b"\x89PNG\r\n\x1a\n"))
        .then(|| decode_image(output.stdout))
}

fn read_native_clipboard(workspace: &Path, image_only: bool) -> Result<Pasted, String> {
    let mut clipboard = arboard::Clipboard::new().map_err(|error| error.to_string())?;
    if let Ok(paths) = clipboard.get().file_list()
        && let Some(path) = paths
            .into_iter()
            .filter(|path| supported_path(path))
            .map(|path| resolve_image_path(workspace, path))
            .find(|path| path.is_file())
    {
        return read_file(&path).map(Pasted::Image);
    }
    match clipboard.get_image() {
        Ok(bitmap) => {
            if bitmap.width.saturating_mul(bitmap.height) > MAX_SOURCE_PIXELS {
                return Err("Clipboard image exceeds the pixel limit".into());
            }
            let width = u32::try_from(bitmap.width).map_err(|error| error.to_string())?;
            let height = u32::try_from(bitmap.height).map_err(|error| error.to_string())?;
            let rgba = RgbaImage::from_raw(width, height, bitmap.bytes.into_owned())
                .ok_or_else(|| "Clipboard image has invalid dimensions".to_owned())?;
            let transparent = rgba.pixels().any(|pixel| pixel.0[3] != 255);
            Ok(Pasted::Image(ClipboardImage {
                pixels: DynamicImage::ImageRgba8(rgba),
                encoded: None,
                transparent,
            }))
        }
        Err(arboard::Error::ContentNotAvailable) if image_only => Ok(Pasted::Text(String::new())),
        Err(arboard::Error::ContentNotAvailable) => {
            let text = clipboard.get_text().map_err(|error| error.to_string())?;
            image_from_text(workspace, text)
        }
        Err(error) => Err(error.to_string()),
    }
}

fn read_file(path: &Path) -> Result<ClipboardImage, String> {
    let metadata = path.metadata().map_err(|error| error.to_string())?;
    if !metadata.is_file() || metadata.len() > MAX_SOURCE_BYTES {
        return Err("Clipboard file must be a regular image file up to 32 MiB".into());
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    File::open(path)
        .map_err(|error| error.to_string())?
        .take(MAX_SOURCE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    if bytes.len() as u64 > MAX_SOURCE_BYTES {
        return Err("Clipboard image file exceeds 32 MiB".into());
    }
    decode_image(bytes)
}

pub(crate) fn decode_image(bytes: Vec<u8>) -> Result<ClipboardImage, String> {
    let reader = ImageReader::new(Cursor::new(&bytes))
        .with_guessed_format()
        .map_err(|error| error.to_string())?;
    if !matches!(reader.format(), Some(ImageFormat::Png | ImageFormat::Jpeg)) {
        return Err("Clipboard file must be PNG or JPEG".into());
    }
    let mut decoder = reader.into_decoder().map_err(|error| error.to_string())?;
    let (width, height) = decoder.dimensions();
    if (width as usize).saturating_mul(height as usize) > MAX_SOURCE_PIXELS {
        return Err("Clipboard image exceeds the pixel limit".into());
    }
    let orientation = decoder.orientation().map_err(|error| error.to_string())?;
    let mut pixels = DynamicImage::from_decoder(decoder).map_err(|error| error.to_string())?;
    pixels.apply_orientation(orientation);
    let transparent =
        pixels.color().has_alpha() && pixels.to_rgba8().pixels().any(|pixel| pixel.0[3] != 255);
    Ok(ClipboardImage {
        pixels,
        encoded: Some(bytes),
        transparent,
    })
}

#[cfg(test)]
mod tests {
    use super::{PasteSource, Pasted, image_path_candidate};

    #[test]
    fn a_readable_pasted_path_retains_the_original_file() {
        let workspace = tempfile::tempdir().unwrap();
        let path = workspace.path().join("screen shot.png");
        image::RgbaImage::from_pixel(4, 2, image::Rgba([20, 40, 60, 80]))
            .save(&path)
            .unwrap();
        let original = std::fs::read(path).unwrap();
        let Pasted::Image(image) = PasteSource::ImagePath("'screen shot.png'".into())
            .read(workspace.path())
            .unwrap()
        else {
            panic!("readable image path was kept as text");
        };
        assert_eq!(image.encoded.as_deref(), Some(original.as_slice()));
        assert!(image.transparent);

        let prompt = "missing.png";
        let pasted = PasteSource::ImagePath(prompt.into())
            .read(workspace.path())
            .unwrap();
        assert!(matches!(pasted, Pasted::Text(text) if text == prompt));
        assert!(image_path_candidate("two lines\nmissing.png").is_none());
    }
}
