//! Pure-Rust image transform engine. Decodes an input image (bounded against decode
//! bombs), applies a named preset (resize/fit/format/quality), and returns the
//! encoded bytes + content-type. No C dependencies; only the accepted formats are
//! compiled in (see Cargo.toml `image` features).

use std::io::Cursor;

use image::{imageops::FilterType, DynamicImage, GenericImageView, ImageFormat, ImageReader};
use serde::{Deserialize, Serialize};

use crate::error::{AppError, AppResult};

/// Max input pixels we will decode — guards against decode bombs. 40 MP.
const MAX_INPUT_PIXELS: u64 = 40_000_000;
/// Max width/height for a decoded input (hard cap during decode).
const MAX_INPUT_DIM: u32 = 20_000;
/// Max output dimension a preset may request.
pub const MAX_OUTPUT_DIM: u32 = 8_192;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Fit {
    /// Scale to fill w×h, center-cropping the overflow (default).
    #[default]
    Cover,
    /// Scale to fit within w×h, preserving aspect (no crop). Aka "contain".
    Inside,
    /// Stretch to exactly w×h, ignoring aspect ratio.
    Fill,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OutFormat {
    Jpeg,
    Png,
    Webp,
}

impl OutFormat {
    pub fn content_type(self) -> &'static str {
        match self {
            OutFormat::Jpeg => "image/jpeg",
            OutFormat::Png => "image/png",
            OutFormat::Webp => "image/webp",
        }
    }
}

/// A named, per-policy transform preset (bounded output). Serialized to/from the
/// `policies.transforms` JSONB map.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Preset {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub w: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub h: Option<u32>,
    #[serde(default)]
    pub fit: Fit,
    pub fmt: OutFormat,
    /// JPEG/WebP quality 1..=100 (ignored for PNG).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub q: Option<u8>,
}

impl Preset {
    /// Validate the preset bounds (at catalog registration).
    pub fn validate(&self, key: &str) -> AppResult<()> {
        if self.w.is_none() && self.h.is_none() {
            return Err(AppError::BadRequest(format!(
                "transform '{key}': at least one of w/h is required"
            )));
        }
        for dim in [self.w, self.h].into_iter().flatten() {
            if dim == 0 || dim > MAX_OUTPUT_DIM {
                return Err(AppError::BadRequest(format!(
                    "transform '{key}': w/h must be 1..={MAX_OUTPUT_DIM}"
                )));
            }
        }
        if self.fit == Fit::Fill && (self.w.is_none() || self.h.is_none()) {
            return Err(AppError::BadRequest(format!(
                "transform '{key}': fit=fill requires both w and h"
            )));
        }
        if let Some(q) = self.q {
            if q == 0 || q > 100 {
                return Err(AppError::BadRequest(format!(
                    "transform '{key}': q must be 1..=100"
                )));
            }
        }
        Ok(())
    }

    /// Stable signature used to content-address the rendered variant. Changing any
    /// field changes the cache key, so a redefined preset re-renders automatically.
    pub fn cache_signature(&self) -> String {
        format!(
            "w={:?};h={:?};fit={:?};fmt={:?};q={:?}",
            self.w, self.h, self.fit, self.fmt, self.q
        )
    }
}

/// Decode `input`, apply `preset`, return `(encoded_bytes, content_type)`.
pub fn render(input: &[u8], preset: &Preset) -> AppResult<(Vec<u8>, &'static str)> {
    // Cheap header-only dimension check first (reject decode bombs before decoding).
    let dims = ImageReader::new(Cursor::new(input))
        .with_guessed_format()
        .map_err(|e| AppError::BadRequest(format!("unreadable image: {e}")))?
        .into_dimensions()
        .map_err(|e| AppError::BadRequest(format!("cannot read image header: {e}")))?;
    if (dims.0 as u64) * (dims.1 as u64) > MAX_INPUT_PIXELS {
        return Err(AppError::BadRequest("image exceeds the max input pixel budget".into()));
    }

    let mut reader = ImageReader::new(Cursor::new(input))
        .with_guessed_format()
        .map_err(|e| AppError::BadRequest(format!("unreadable image: {e}")))?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_INPUT_DIM);
    limits.max_image_height = Some(MAX_INPUT_DIM);
    reader.limits(limits);
    let img = reader
        .decode()
        .map_err(|e| AppError::BadRequest(format!("cannot decode image: {e}")))?;

    let (iw, ih) = img.dimensions();
    let resized = apply_fit(&img, preset, iw, ih);

    let mut out = Vec::new();
    match preset.fmt {
        OutFormat::Jpeg => {
            let q = preset.q.unwrap_or(82).clamp(1, 100);
            let rgb = resized.to_rgb8();
            image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, q)
                .encode_image(&rgb)
                .map_err(|e| AppError::Internal(format!("jpeg encode: {e}")))?;
        }
        OutFormat::Png => {
            resized
                .write_to(&mut Cursor::new(&mut out), ImageFormat::Png)
                .map_err(|e| AppError::Internal(format!("png encode: {e}")))?;
        }
        OutFormat::Webp => {
            // image-webp encodes lossless WebP (q does not apply).
            resized
                .write_to(&mut Cursor::new(&mut out), ImageFormat::WebP)
                .map_err(|e| AppError::Internal(format!("webp encode: {e}")))?;
        }
    }
    Ok((out, preset.fmt.content_type()))
}

fn apply_fit(img: &DynamicImage, preset: &Preset, iw: u32, ih: u32) -> DynamicImage {
    let f = FilterType::Lanczos3;
    match (preset.w, preset.h, preset.fit) {
        (Some(w), Some(h), Fit::Cover) => img.resize_to_fill(w, h, f),
        (Some(w), Some(h), Fit::Fill) => img.resize_exact(w, h, f),
        (Some(w), Some(h), Fit::Inside) => img.resize(w, h, f),
        // Single dimension: preserve aspect, bound by the given side.
        (Some(w), None, _) => img.resize(w, u32::MAX, f),
        (None, Some(h), _) => img.resize(u32::MAX, h, f),
        // Validated out (at least one of w/h is required).
        (None, None, _) => {
            let _ = (iw, ih);
            img.clone()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source_png(w: u32, h: u32) -> Vec<u8> {
        let img = DynamicImage::ImageRgb8(image::RgbImage::from_fn(w, h, |x, _| {
            image::Rgb([(x % 256) as u8, 128, 200])
        }));
        let mut buf = Vec::new();
        img.write_to(&mut Cursor::new(&mut buf), ImageFormat::Png).unwrap();
        buf
    }

    fn out_dims(bytes: &[u8]) -> (u32, u32) {
        image::load_from_memory(bytes).unwrap().dimensions()
    }

    #[test]
    fn cover_crops_to_exact_dimensions() {
        let src = source_png(200, 100);
        let preset = Preset { w: Some(64), h: Some(64), fit: Fit::Cover, fmt: OutFormat::Jpeg, q: Some(80) };
        let (bytes, ct) = render(&src, &preset).unwrap();
        assert_eq!(ct, "image/jpeg");
        assert_eq!(out_dims(&bytes), (64, 64));
    }

    #[test]
    fn inside_preserves_aspect_within_box() {
        let src = source_png(200, 100);
        let preset = Preset { w: Some(100), h: Some(100), fit: Fit::Inside, fmt: OutFormat::Png, q: None };
        let (bytes, _) = render(&src, &preset).unwrap();
        // 2:1 image fit inside 100×100 -> 100×50.
        assert_eq!(out_dims(&bytes), (100, 50));
    }

    #[test]
    fn width_only_scales_by_aspect() {
        let src = source_png(200, 100);
        let preset = Preset { w: Some(50), h: None, fit: Fit::Cover, fmt: OutFormat::Png, q: None };
        let (bytes, _) = render(&src, &preset).unwrap();
        assert_eq!(out_dims(&bytes), (50, 25));
    }

    #[test]
    fn webp_encode_works() {
        let src = source_png(80, 80);
        let preset = Preset { w: Some(32), h: Some(32), fit: Fit::Cover, fmt: OutFormat::Webp, q: None };
        let (bytes, ct) = render(&src, &preset).unwrap();
        assert_eq!(ct, "image/webp");
        assert_eq!(out_dims(&bytes), (32, 32));
    }

    #[test]
    fn rejects_non_image() {
        let preset = Preset { w: Some(10), h: Some(10), fit: Fit::Cover, fmt: OutFormat::Png, q: None };
        assert!(render(b"not an image at all", &preset).is_err());
    }

    #[test]
    fn validate_catches_bad_presets() {
        assert!(Preset { w: None, h: None, fit: Fit::Cover, fmt: OutFormat::Png, q: None }.validate("x").is_err());
        assert!(Preset { w: Some(0), h: None, fit: Fit::Cover, fmt: OutFormat::Png, q: None }.validate("x").is_err());
        assert!(Preset { w: Some(100), h: None, fit: Fit::Fill, fmt: OutFormat::Png, q: None }.validate("x").is_err());
        assert!(Preset { w: Some(64), h: Some(64), fit: Fit::Cover, fmt: OutFormat::Jpeg, q: Some(80) }.validate("x").is_ok());
    }
}
