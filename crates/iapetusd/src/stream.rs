//! The WebSocket JPEG-diff fallback encoder (PRD §6.3, §19.7).
//!
//! WebRTC is the default path and this is what runs where UDP is blocked
//! entirely — 5–10fps and 150–400ms, "usable but not smooth", which is why the
//! viewer shows a badge when it is in use (§6.3, V-12).
//!
//! The design question a fallback like this lives or dies on is **what it costs
//! while nothing is happening.** A Desktop sitting at an idle login screen is
//! the common case, and re-encoding a full 1920×1080 JPEG ten times a second to
//! transmit an identical picture would blow the per-Desktop encoding budget
//! §12.4 reserves. So the screen is divided into tiles, each is hashed, and
//! **only tiles whose contents actually changed are encoded at all.** A still
//! screen costs one hash pass and zero bytes.
//!
//! Tiles are also what make a keyframe cheap for the gateway: it can cache the
//! latest JPEG per position and hand a newly-connected viewer the whole mosaic
//! without ever decoding anything (§19.6).

use std::hash::{Hash, Hasher};

use crate::platform::{Frame, PlatformError, Result};

/// 64×64. Small enough that a blinking cursor costs one tile rather than a
/// band of the screen; large enough that a 1920×1080 frame is 510 tiles, so
/// the per-tile JPEG header overhead stays a few percent rather than dominating.
pub const TILE: u32 = 64;

/// §6.3 puts the fallback at 5–10fps. This is the ceiling; the loop sleeps out
/// the remainder when the screen changes faster.
pub const MAX_FPS: u32 = 10;

/// Quality for fallback tiles. Lower than a `screenshot`, because this is the
/// path chosen when bandwidth is already the problem.
pub const DEFAULT_QUALITY: u8 = 70;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tile {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
    pub jpeg: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrameUpdate {
    pub seq: u32,
    /// The full screen size, so a viewer can size its canvas before it has
    /// received every tile.
    pub width: u32,
    pub height: u32,
    /// True when every tile is present. A viewer joining mid-stream needs one
    /// of these before the diffs mean anything.
    pub keyframe: bool,
    pub tiles: Vec<Tile>,
}

impl FrameUpdate {
    /// Packs into the wire format the gateway relays verbatim.
    ///
    /// Binary rather than JSON with base64: base64 costs a third more bytes on
    /// the one path that exists because bandwidth is already constrained.
    #[must_use]
    pub fn to_bytes(&self) -> Vec<u8> {
        let total: usize = self.tiles.iter().map(|t| t.jpeg.len() + 12).sum();
        let mut out = Vec::with_capacity(total + 16);
        out.extend_from_slice(&self.seq.to_be_bytes());
        out.extend_from_slice(&(self.width as u16).to_be_bytes());
        out.extend_from_slice(&(self.height as u16).to_be_bytes());
        out.push(u8::from(self.keyframe));
        out.push(0); // reserved, keeps the tile records 4-byte aligned
        out.extend_from_slice(&(self.tiles.len() as u16).to_be_bytes());
        for t in &self.tiles {
            out.extend_from_slice(&(t.x as u16).to_be_bytes());
            out.extend_from_slice(&(t.y as u16).to_be_bytes());
            out.extend_from_slice(&(t.w as u16).to_be_bytes());
            out.extend_from_slice(&(t.h as u16).to_be_bytes());
            out.extend_from_slice(&(t.jpeg.len() as u32).to_be_bytes());
            out.extend_from_slice(&t.jpeg);
        }
        out
    }
}

/// Encodes successive frames, emitting only what changed.
pub struct TileEncoder {
    quality: u8,
    seq: u32,
    /// Per-tile content hashes from the previous frame, in row-major order.
    /// Empty until the first frame, which is therefore always a keyframe.
    hashes: Vec<u64>,
    grid: (u32, u32), // (cols, rows) the hashes correspond to
    size: (u32, u32),
}

impl TileEncoder {
    #[must_use]
    pub fn new(quality: u8) -> Self {
        Self {
            quality: quality.clamp(1, 100),
            seq: 0,
            hashes: Vec::new(),
            grid: (0, 0),
            size: (0, 0),
        }
    }

    /// Encodes `frame`, returning only the tiles that differ from the previous
    /// one — or every tile when `force_keyframe` is set or the geometry changed.
    ///
    /// A resolution change forces a keyframe because the tile grid it is
    /// diffing against no longer describes the same regions of the screen;
    /// comparing them by index would mark unchanged areas as changed and, worse,
    /// leave genuinely changed ones untouched.
    pub fn encode(&mut self, frame: &Frame, force_keyframe: bool) -> Result<FrameUpdate> {
        if frame.pixels.len() != frame.byte_len() {
            return Err(PlatformError::CaptureFailed(format!(
                "frame carries {} bytes but declares {}x{}",
                frame.pixels.len(),
                frame.width,
                frame.height
            )));
        }

        let cols = frame.width.div_ceil(TILE);
        let rows = frame.height.div_ceil(TILE);
        let resized = self.size != (frame.width, frame.height);
        let keyframe = force_keyframe || resized || self.hashes.len() != (cols * rows) as usize;

        let img = image::RgbaImage::from_raw(frame.width, frame.height, frame.pixels.clone())
            .ok_or_else(|| PlatformError::CaptureFailed("frame buffer is the wrong length".into()))?;

        let mut hashes = vec![0u64; (cols * rows) as usize];
        let mut tiles = Vec::new();

        for row in 0..rows {
            for col in 0..cols {
                let x = col * TILE;
                let y = row * TILE;
                // Clipped, not padded: the bottom row of a 1080px screen is 56
                // pixels tall, and padding it would encode 8 rows of garbage and
                // paint them over the viewer's canvas.
                let w = TILE.min(frame.width - x);
                let h = TILE.min(frame.height - y);

                let idx = (row * cols + col) as usize;
                let hash = hash_tile(&img, x, y, w, h);
                hashes[idx] = hash;

                if !keyframe && self.hashes[idx] == hash {
                    continue;
                }

                let sub = image::imageops::crop_imm(&img, x, y, w, h).to_image();
                let rgb = image::DynamicImage::ImageRgba8(sub).to_rgb8();
                let mut buf = std::io::Cursor::new(Vec::new());
                image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, self.quality)
                    .encode_image(&rgb)
                    .map_err(|e| PlatformError::CaptureFailed(format!("tile jpeg: {e}")))?;

                tiles.push(Tile { x, y, w, h, jpeg: buf.into_inner() });
            }
        }

        self.hashes = hashes;
        self.grid = (cols, rows);
        self.size = (frame.width, frame.height);
        self.seq = self.seq.wrapping_add(1);

        Ok(FrameUpdate {
            seq: self.seq,
            width: frame.width,
            height: frame.height,
            keyframe,
            tiles,
        })
    }

    /// Forgets the previous frame, so the next `encode` is a keyframe.
    ///
    /// Called when a viewer joins: the gateway can bootstrap from its own tile
    /// cache, but after a reconnect that cache may be gone, and diffs against a
    /// frame the viewer never saw paint nothing.
    pub fn invalidate(&mut self) {
        self.hashes.clear();
    }

    #[must_use]
    pub fn seq(&self) -> u32 {
        self.seq
    }
}

fn hash_tile(img: &image::RgbaImage, x: u32, y: u32, w: u32, h: u32) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    // Hash row slices rather than pixel by pixel: same result, far fewer calls,
    // and this runs over every tile of every frame.
    for row in 0..h {
        let start = (((y + row) * img.width() + x) * 4) as usize;
        let end = start + (w * 4) as usize;
        img.as_raw()[start..end].hash(&mut hasher);
    }
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::SystemTime;

    fn frame(w: u32, h: u32, fill: u8) -> Frame {
        Frame {
            width: w,
            height: h,
            pixels: vec![fill; (w as usize) * (h as usize) * 4],
            captured_at: SystemTime::now(),
        }
    }

    fn set_pixel(f: &mut Frame, x: u32, y: u32, v: u8) {
        let i = (((y * f.width) + x) * 4) as usize;
        f.pixels[i] = v;
        f.pixels[i + 1] = v.wrapping_add(40);
        f.pixels[i + 2] = v.wrapping_add(80);
        f.pixels[i + 3] = 0xFF;
    }

    #[test]
    fn the_first_frame_is_a_keyframe_covering_the_whole_screen_exactly_once() {
        let mut e = TileEncoder::new(DEFAULT_QUALITY);
        let u = e.encode(&frame(256, 128, 0), false).unwrap();

        assert!(u.keyframe);
        assert_eq!(u.tiles.len(), 4 * 2, "256x128 at 64px is 4x2 tiles");

        // Every pixel covered, none twice: a gap shows as a hole in the viewer
        // and an overlap wastes the bandwidth this path exists to save.
        let area: u32 = u.tiles.iter().map(|t| t.w * t.h).sum();
        assert_eq!(area, 256 * 128);
        let mut seen: Vec<(u32, u32)> = u.tiles.iter().map(|t| (t.x, t.y)).collect();
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(seen.len(), u.tiles.len(), "a tile position was emitted twice");
    }

    #[test]
    fn a_still_screen_costs_nothing_after_the_keyframe() {
        // The whole reason for tiling. An idle Desktop re-encoding a full frame
        // ten times a second would consume the §12.4 encoding reservation to
        // transmit an identical picture.
        let mut e = TileEncoder::new(DEFAULT_QUALITY);
        let f = frame(256, 128, 7);
        assert!(!e.encode(&f, false).unwrap().tiles.is_empty());

        let u = e.encode(&f, false).unwrap();
        assert!(!u.keyframe);
        assert!(u.tiles.is_empty(), "an unchanged screen emitted {} tiles", u.tiles.len());
        assert_eq!(u.to_bytes().len(), 12, "an empty update should be a bare header");
    }

    #[test]
    fn one_changed_pixel_costs_exactly_one_tile_at_the_right_place() {
        let mut e = TileEncoder::new(DEFAULT_QUALITY);
        let mut f = frame(256, 128, 0);
        e.encode(&f, false).unwrap();

        // Inside the tile at column 2, row 1 — origin (128, 64).
        set_pixel(&mut f, 130, 70, 200);
        let u = e.encode(&f, false).unwrap();

        assert_eq!(u.tiles.len(), 1, "a single pixel dirtied {} tiles", u.tiles.len());
        assert_eq!((u.tiles[0].x, u.tiles[0].y), (128, 64));
        assert!(!u.keyframe);
    }

    #[test]
    fn edge_tiles_are_clipped_rather_than_padded() {
        // 1080 is not a multiple of 64: the bottom row is 56px. Padding it would
        // encode eight rows of garbage and paint them onto the viewer.
        let mut e = TileEncoder::new(DEFAULT_QUALITY);
        let u = e.encode(&frame(200, 100, 0), false).unwrap();

        let last = u.tiles.iter().find(|t| t.x == 192 && t.y == 64).expect("no corner tile");
        assert_eq!((last.w, last.h), (8, 36), "corner tile was padded");

        let area: u32 = u.tiles.iter().map(|t| t.w * t.h).sum();
        assert_eq!(area, 200 * 100);
    }

    #[test]
    fn every_tile_decodes_to_its_declared_size() {
        // A tile whose bytes disagree with its header lands in the wrong place
        // on the canvas and corrupts the region around it.
        let mut e = TileEncoder::new(DEFAULT_QUALITY);
        let u = e.encode(&frame(200, 100, 33), false).unwrap();
        for t in &u.tiles {
            let img = image::load_from_memory(&t.jpeg).expect("tile did not decode");
            assert_eq!((img.width(), img.height()), (t.w, t.h));
        }
    }

    #[test]
    fn a_resolution_change_forces_a_keyframe() {
        // The old grid no longer describes the same regions. Diffing by index
        // across it would mark unchanged areas as dirty and, worse, leave
        // genuinely changed ones untouched.
        let mut e = TileEncoder::new(DEFAULT_QUALITY);
        e.encode(&frame(256, 128, 0), false).unwrap();

        let u = e.encode(&frame(320, 192, 0), false).unwrap();
        assert!(u.keyframe, "a resize did not force a keyframe");
        assert_eq!(u.tiles.len(), 5 * 3);
    }

    #[test]
    fn invalidate_makes_the_next_frame_a_keyframe() {
        // A viewer that joined after the gateway lost its tile cache needs a
        // full frame; diffs against something it never saw paint nothing.
        let mut e = TileEncoder::new(DEFAULT_QUALITY);
        let f = frame(128, 128, 5);
        e.encode(&f, false).unwrap();
        assert!(e.encode(&f, false).unwrap().tiles.is_empty());

        e.invalidate();
        let u = e.encode(&f, false).unwrap();
        assert!(u.keyframe);
        assert_eq!(u.tiles.len(), 4);
    }

    #[test]
    fn the_sequence_number_advances_even_when_nothing_changed() {
        // A viewer uses it to notice it missed an update. Holding it still on
        // empty frames would make a dropped update indistinguishable from an
        // idle screen.
        let mut e = TileEncoder::new(DEFAULT_QUALITY);
        let f = frame(64, 64, 1);
        let a = e.encode(&f, false).unwrap().seq;
        let b = e.encode(&f, false).unwrap().seq;
        assert_eq!(b, a + 1);
    }

    #[test]
    fn the_wire_format_round_trips() {
        let mut e = TileEncoder::new(DEFAULT_QUALITY);
        let u = e.encode(&frame(200, 100, 9), false).unwrap();
        let bytes = u.to_bytes();

        assert_eq!(u32::from_be_bytes(bytes[0..4].try_into().unwrap()), u.seq);
        assert_eq!(u16::from_be_bytes(bytes[4..6].try_into().unwrap()), 200);
        assert_eq!(u16::from_be_bytes(bytes[6..8].try_into().unwrap()), 100);
        assert_eq!(bytes[8], 1, "keyframe flag");
        assert_eq!(u16::from_be_bytes(bytes[10..12].try_into().unwrap()) as usize, u.tiles.len());

        // Walk the records and confirm the declared lengths reach exactly the end.
        let mut off = 12;
        for t in &u.tiles {
            let len = u32::from_be_bytes(bytes[off + 8..off + 12].try_into().unwrap()) as usize;
            assert_eq!(len, t.jpeg.len());
            assert_eq!(&bytes[off + 12..off + 12 + len], &t.jpeg[..]);
            off += 12 + len;
        }
        assert_eq!(off, bytes.len(), "trailing bytes after the last tile");
    }

    #[test]
    fn a_frame_whose_buffer_disagrees_with_its_dimensions_is_refused() {
        let mut e = TileEncoder::new(DEFAULT_QUALITY);
        let bad = Frame {
            width: 64,
            height: 64,
            pixels: vec![0; 10],
            captured_at: SystemTime::now(),
        };
        assert!(e.encode(&bad, false).is_err());
    }
}
