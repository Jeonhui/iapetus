//! Turns a captured frame into the bytes a `screenshot` response carries.
//!
//! §7.2 lets the caller pick a format, a quality, a region, and a scale. Three
//! of those interact in ways worth stating once here rather than rediscovering
//! at each call site:
//!
//! * **`scale` shrinks the transmitted image, never the coordinate frame.**
//!   `width`/`height` on the response describe the bytes; `display` describes
//!   the screen the agent computes clicks against. Conflating them puts every
//!   click at a fraction of where it was aimed.
//! * **`quality` only means something for JPEG.** PNG is lossless by
//!   definition, and the WebP encoder here is the pure-Rust lossless one, so a
//!   `quality` sent with either is accepted and has no effect. Rejecting it
//!   would fail requests that are asking for something perfectly sensible.
//! * **Out-of-range values are rejected, not clamped** (§8.2). A caller that
//!   asked to upscale has a bug, and silently downgrading it to 1.0 hides it.

use std::io::Cursor;

use iapetus_proto::limits;
use iapetus_proto::v1::ImageFormat;

use crate::platform::{Frame, PlatformError, Result};

/// The largest encoded image the guest will put on the wire.
///
/// §8.2's 256KB cap governs whether the *API* answers inline or with a URL —
/// a decision only the Control Plane can make, since only it can mint a
/// presigned URL. This is the separate limit on the guest→Control Plane hop
/// (§19.5), and it exists so an agent asking for a lossless PNG of a 4096×4096
/// screen gets told which knob to turn instead of a truncated message or a
/// stream reset.
pub const WIRE_MAX_BYTES: usize = 8 * 1024 * 1024;

pub struct Encoded {
    pub bytes: Vec<u8>,
    /// Dimensions of the encoded image, after `scale`.
    pub width: u32,
    pub height: u32,
}

/// Encodes `frame`, applying `scale` first.
///
/// `quality` is `1..=100`; `0` means "unset" and takes the default, because
/// proto3 cannot distinguish an omitted int from a zero one and rejecting zero
/// would fail every request that simply did not set the field.
pub fn encode(frame: &Frame, format: ImageFormat, quality: i32, scale: Option<f64>) -> Result<Encoded> {
    if frame.width > limits::SCREENSHOT_MAX_DIMENSION || frame.height > limits::SCREENSHOT_MAX_DIMENSION {
        return Err(PlatformError::CaptureFailed(format!(
            "{}x{} exceeds the {}px screenshot limit (§8.2)",
            frame.width,
            frame.height,
            limits::SCREENSHOT_MAX_DIMENSION
        )));
    }
    if frame.pixels.len() != frame.byte_len() {
        return Err(PlatformError::CaptureFailed(format!(
            "frame carries {} bytes but declares {}x{}",
            frame.pixels.len(),
            frame.width,
            frame.height
        )));
    }

    let quality = match quality {
        0 => 85, // a legible default for JPEG; ignored by the lossless formats
        q @ 1..=100 => q,
        q => {
            return Err(PlatformError::InputRejected(format!(
                "quality must be 1..=100, got {q}"
            )))
        }
    };

    let (w, h) = scaled_size(frame.width, frame.height, scale)?;

    let img = image::RgbaImage::from_raw(frame.width, frame.height, frame.pixels.clone())
        .ok_or_else(|| PlatformError::CaptureFailed("frame buffer is the wrong length".into()))?;

    let img = if (w, h) == (frame.width, frame.height) {
        image::DynamicImage::ImageRgba8(img)
    } else {
        // Triangle rather than Lanczos3: screenshots are mostly text and sharp
        // UI edges, where Lanczos ringing shows up as a halo the agent has to
        // read through. It is also markedly cheaper, which matters because
        // §12.4 budgets the host's CPU around encoding.
        image::DynamicImage::ImageRgba8(image::imageops::resize(
            &img,
            w,
            h,
            image::imageops::FilterType::Triangle,
        ))
    };

    let mut out = Cursor::new(Vec::new());
    match format {
        // Unspecified defaults to PNG. An agent reading text off the screen
        // should not get JPEG ringing around glyphs because it left the field
        // unset; a caller that wants the smaller image can ask for one.
        ImageFormat::Unspecified | ImageFormat::Png => {
            img.write_to(&mut out, image::ImageFormat::Png)
                .map_err(|e| PlatformError::CaptureFailed(format!("png encode: {e}")))?;
        }
        ImageFormat::Jpeg => {
            // JPEG has no alpha channel. Dropping it here is explicit; letting
            // the encoder decide would silently composite against black.
            let rgb = img.to_rgb8();
            image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, quality as u8)
                .encode_image(&rgb)
                .map_err(|e| PlatformError::CaptureFailed(format!("jpeg encode: {e}")))?;
        }
        ImageFormat::Webp => {
            img.write_to(&mut out, image::ImageFormat::WebP)
                .map_err(|e| PlatformError::CaptureFailed(format!("webp encode: {e}")))?;
        }
    }

    let bytes = out.into_inner();
    if bytes.len() > WIRE_MAX_BYTES {
        return Err(PlatformError::CaptureFailed(format!(
            "encoded image is {} bytes, over the {} byte transport limit; \
             ask for jpeg or a smaller scale",
            bytes.len(),
            WIRE_MAX_BYTES
        )));
    }

    Ok(Encoded { bytes, width: w, height: h })
}

/// Applies `scale`, rejecting anything outside `(0, 1]`.
fn scaled_size(width: u32, height: u32, scale: Option<f64>) -> Result<(u32, u32)> {
    let Some(s) = scale else { return Ok((width, height)) };
    if !s.is_finite() || s <= 0.0 || s > 1.0 {
        return Err(PlatformError::InputRejected(format!(
            "scale must be in (0, 1]; upscaling is rejected, got {s}"
        )));
    }
    // At least one pixel each way: a scale small enough to round to zero would
    // otherwise produce an image no decoder will accept.
    let w = ((f64::from(width) * s).round() as u32).max(1);
    let h = ((f64::from(height) * s).round() as u32).max(1);
    Ok((w, h))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::SystemTime;

    fn frame(w: u32, h: u32) -> Frame {
        // A gradient rather than a flat fill: a solid colour compresses to
        // almost nothing, which would hide a size regression and make the
        // scaling assertions pass for the wrong reason.
        let mut pixels = Vec::with_capacity((w * h * 4) as usize);
        for y in 0..h {
            for x in 0..w {
                pixels.extend_from_slice(&[(x % 256) as u8, (y % 256) as u8, ((x + y) % 256) as u8, 0xFF]);
            }
        }
        Frame { width: w, height: h, pixels, captured_at: SystemTime::now() }
    }

    fn decode(bytes: &[u8]) -> (u32, u32) {
        let img = image::load_from_memory(bytes).expect("the encoded bytes did not decode");
        (img.width(), img.height())
    }

    #[test]
    fn every_declared_format_produces_a_decodable_image() {
        // §8.2 names jpeg, png, and webp. A format the schema advertises and
        // the guest cannot produce is worse than one it never offered.
        for f in [ImageFormat::Png, ImageFormat::Jpeg, ImageFormat::Webp, ImageFormat::Unspecified] {
            let e = encode(&frame(64, 48), f, 0, None).unwrap_or_else(|err| panic!("{f:?}: {err}"));
            assert_eq!(decode(&e.bytes), (64, 48), "{f:?} decoded to the wrong size");
            assert_eq!((e.width, e.height), (64, 48));
        }
    }

    #[test]
    fn scale_shrinks_the_image_and_is_reported() {
        let e = encode(&frame(200, 100), ImageFormat::Png, 0, Some(0.5)).unwrap();
        assert_eq!((e.width, e.height), (100, 50));
        assert_eq!(decode(&e.bytes), (100, 50), "the reported size disagrees with the bytes");
    }

    #[test]
    fn upscaling_is_rejected_rather_than_clamped() {
        // §8.2 rejects out-of-range values. Clamping to 1.0 would hide a caller
        // bug that is about to misplace every click it computes.
        let f = frame(32, 32);
        assert!(encode(&f, ImageFormat::Png, 0, Some(1.5)).is_err());
        assert!(encode(&f, ImageFormat::Png, 0, Some(0.0)).is_err());
        assert!(encode(&f, ImageFormat::Png, 0, Some(-0.5)).is_err());
        assert!(encode(&f, ImageFormat::Png, 0, Some(f64::NAN)).is_err());
        assert!(encode(&f, ImageFormat::Png, 0, Some(1.0)).is_ok(), "1.0 is in range");
    }

    #[test]
    fn a_scale_that_would_round_to_zero_still_yields_one_pixel() {
        let e = encode(&frame(10, 10), ImageFormat::Png, 0, Some(0.01)).unwrap();
        assert_eq!((e.width, e.height), (1, 1));
        assert_eq!(decode(&e.bytes), (1, 1));
    }

    #[test]
    fn quality_is_range_checked_and_zero_means_unset() {
        let f = frame(32, 32);
        assert!(encode(&f, ImageFormat::Jpeg, 0, None).is_ok(), "proto3 sends 0 for an unset field");
        assert!(encode(&f, ImageFormat::Jpeg, 1, None).is_ok());
        assert!(encode(&f, ImageFormat::Jpeg, 100, None).is_ok());
        assert!(encode(&f, ImageFormat::Jpeg, 101, None).is_err());
        assert!(encode(&f, ImageFormat::Jpeg, -1, None).is_err());
    }

    #[test]
    fn jpeg_quality_actually_changes_the_output() {
        // Accepting the parameter and ignoring it would let a caller tune a
        // knob that does nothing, and conclude the format is not the problem.
        let f = frame(128, 128);
        let low = encode(&f, ImageFormat::Jpeg, 10, None).unwrap();
        let high = encode(&f, ImageFormat::Jpeg, 95, None).unwrap();
        assert!(
            low.bytes.len() < high.bytes.len(),
            "quality 10 produced {} bytes, quality 95 produced {}",
            low.bytes.len(),
            high.bytes.len()
        );
    }

    #[test]
    fn png_is_lossless_so_the_pixels_survive_the_round_trip() {
        // The freshness work in §6.3 is pointless if the image an agent reads
        // is not the image that was captured.
        let f = frame(16, 16);
        let e = encode(&f, ImageFormat::Png, 0, None).unwrap();
        let back = image::load_from_memory(&e.bytes).unwrap().to_rgba8();
        assert_eq!(back.as_raw(), &f.pixels, "png round trip altered the pixels");
    }

    #[test]
    fn an_oversized_frame_is_refused() {
        // §8.2 caps a screenshot at 4096 on a side.
        let over = limits::SCREENSHOT_MAX_DIMENSION + 1;
        let f = Frame {
            width: over,
            height: 1,
            pixels: vec![0; (over as usize) * 4],
            captured_at: SystemTime::now(),
        };
        assert!(encode(&f, ImageFormat::Png, 0, None).is_err());
    }

    #[test]
    fn a_frame_whose_buffer_disagrees_with_its_dimensions_is_refused() {
        // Trusting the header here would hand the encoder a short buffer and
        // turn a driver bug into a panic inside a dependency.
        let f = Frame {
            width: 64,
            height: 64,
            pixels: vec![0; 10],
            captured_at: SystemTime::now(),
        };
        assert!(encode(&f, ImageFormat::Png, 0, None).is_err());
    }
}
