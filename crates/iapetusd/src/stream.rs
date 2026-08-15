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

use crate::platform::{Frame, PixelFormat, PlatformError, Result};

/// 64×64. Small enough that a blinking cursor costs one tile rather than a
/// band of the screen; large enough that a 1920×1080 frame is 510 tiles, so
/// the per-tile JPEG header overhead stays a few percent rather than dominating.
pub const TILE: u32 = 64;

/// §6.3 puts the fallback at 5–10fps. This is the ceiling; the loop sleeps out
/// the remainder when the screen changes faster.
pub const MAX_FPS: u32 = 10;

/// Above this fraction of changed tiles, the whole changed region is sent as
/// one image instead of many.
///
/// A 64×64 JPEG measured ~790 bytes here, of which roughly 600 is header and
/// Huffman tables — so a full-screen change spent more than half its bytes, and
/// 510 encoder setups, on overhead. One image over the bounding box pays that
/// once. The threshold is well below 1.0 because the crossover is about
/// per-image cost, not pixel count: re-encoding some unchanged pixels inside
/// the box is cheaper than 300 extra headers.
pub const COALESCE_ABOVE: f32 = 0.25;

/// How many threads may encode at once.
///
/// §12.4 reserves 1.8 vCPU per observed Desktop for encoding, so this is not
/// "use every core" — a host runs dozens of Desktops, and one greedy encoder
/// would take capacity the scheduler has already promised elsewhere. Four is
/// the ceiling; a machine with fewer cores gets fewer.
pub fn encode_threads() -> usize {
    std::thread::available_parallelism().map_or(1, |n| n.get().min(4))
}

/// Below this height a region is encoded whole rather than split across
/// threads. Splitting costs an extra JPEG header per band, which is only worth
/// paying when there is real work to divide.
const SPLIT_MIN_HEIGHT: u32 = 256;

/// Quality for fallback tiles. Lower than a `screenshot`, because this is the
/// path chosen when bandwidth is already the problem.
pub const DEFAULT_QUALITY: u8 = 70;

/// Where the time and the bytes went on one frame.
///
/// §12.4 budgets host capacity around encoding cost, so this is not curiosity:
/// without it, "the fallback is too slow" cannot be told apart from "capture is
/// too slow", and they have opposite fixes.
#[derive(Debug, Clone, Copy, Default)]
pub struct EncodeStats {
    /// Comparing every tile against the previous frame — paid on every frame,
    /// including still ones.
    pub hash: std::time::Duration,
    /// JPEG encoding — paid only for tiles that changed.
    pub jpeg: std::time::Duration,
    pub tiles_changed: usize,
    pub tiles_total: usize,
    pub bytes: usize,
}

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
    /// The previous frame's pixels.
    ///
    /// Kept whole rather than hashed. A hash would be smaller, but a collision
    /// is a tile that silently never updates — a wrong picture the viewer has
    /// no way to detect. `memcmp` on a slice is exact, is vectorised by the
    /// compiler, and measured faster here than SipHash over the same bytes, so
    /// the hash was costing accuracy and speed at once.
    ///
    /// Empty until the first frame, which is therefore always a keyframe.
    prev: Vec<u8>,
    grid: (u32, u32), // (cols, rows) the hashes correspond to
    size: (u32, u32),
    stats: EncodeStats,
}

impl TileEncoder {
    #[must_use]
    pub fn new(quality: u8) -> Self {
        Self {
            quality: quality.clamp(1, 100),
            seq: 0,
            prev: Vec::new(),
            grid: (0, 0),
            size: (0, 0),
            stats: EncodeStats::default(),
        }
    }

    /// The breakdown for the most recent `encode`.
    #[must_use]
    pub fn stats(&self) -> EncodeStats {
        self.stats
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
        let keyframe = force_keyframe || resized || self.prev.len() != frame.pixels.len();

        if keyframe {
            self.prev = vec![0u8; frame.pixels.len()];
        }

        let stride = (frame.width * 4) as usize;
        let src = &frame.pixels[..];
        let mut stats = EncodeStats {
            tiles_total: (cols * rows) as usize,
            ..EncodeStats::default()
        };

        // Pass 1: which tiles moved. Cheap — a slice compare that stops at the
        // first differing byte — and knowing the total before encoding is what
        // makes the coalescing decision possible at all.
        let mut dirty: Vec<(u32, u32, u32, u32)> = Vec::new();
        let t_diff = std::time::Instant::now();
        for row in 0..rows {
            for col in 0..cols {
                let x = col * TILE;
                let y = row * TILE;
                // Clipped, not padded: the bottom row of a 1080px screen is 56
                // pixels tall, and padding it would encode 8 rows of garbage and
                // paint them over the viewer's canvas.
                let w = TILE.min(frame.width - x);
                let h = TILE.min(frame.height - y);

                if keyframe || tile_differs(src, &self.prev, stride, x, y, w, h) {
                    dirty.push((x, y, w, h));
                }
            }
        }
        stats.hash = t_diff.elapsed();
        stats.tiles_changed = dirty.len();

        // Pass 2: coalesce a widespread change into one image over its bounding
        // box, then encode whatever regions are left.
        let fraction = dirty.len() as f32 / (cols * rows).max(1) as f32;
        let regions: Vec<(u32, u32, u32, u32)> = if fraction > COALESCE_ABOVE && dirty.len() > 1 {
            let x0 = dirty.iter().map(|d| d.0).min().unwrap();
            let y0 = dirty.iter().map(|d| d.1).min().unwrap();
            let x1 = dirty.iter().map(|d| d.0 + d.2).max().unwrap();
            let y1 = dirty.iter().map(|d| d.1 + d.3).max().unwrap();
            vec![(x0, y0, x1 - x0, y1 - y0)]
        } else {
            dirty.clone()
        };

        // A large region is split into horizontal bands so the encode can run
        // on several threads. Bands are ordinary regions on the wire — the
        // format has always carried arbitrary rectangles — so the viewer needs
        // no idea this happened.
        let threads = encode_threads();
        let regions = split_for_threads(regions, threads);

        let t_jpeg = std::time::Instant::now();
        let quality = self.quality;
        let encoded: Vec<Result<Vec<u8>>> = if threads > 1 && regions.len() > 1 {
            std::thread::scope(|scope| {
                let handles: Vec<_> = regions
                    .chunks(regions.len().div_ceil(threads))
                    .map(|chunk| {
                        scope.spawn(move || {
                            chunk
                                .iter()
                                .map(|&(x, y, w, h)| encode_region(frame, stride, x, y, w, h, quality))
                                .collect::<Vec<_>>()
                        })
                    })
                    .collect();
                handles.into_iter().flat_map(|h| h.join().unwrap_or_default()).collect()
            })
        } else {
            regions
                .iter()
                .map(|&(x, y, w, h)| encode_region(frame, stride, x, y, w, h, quality))
                .collect()
        };

        let mut tiles = Vec::with_capacity(regions.len());
        for (&(x, y, w, h), jpeg) in regions.iter().zip(encoded) {
            let jpeg = jpeg?;
            stats.bytes += jpeg.len();
            tiles.push(Tile { x, y, w, h, jpeg });
        }
        stats.jpeg = t_jpeg.elapsed();

        // The reference frame records what was actually captured, not what was
        // sent. Copying the coalesced box instead would be equivalent here, but
        // copying per dirty tile keeps the cost proportional to real movement.
        for &(x, y, w, h) in &dirty {
            copy_tile(src, &mut self.prev, stride, x, y, w, h);
        }

        self.stats = stats;
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
        self.prev.clear();
    }

    #[must_use]
    pub fn seq(&self) -> u32 {
        self.seq
    }
}

/// Splits tall regions into horizontal bands, so there is work for every thread.
///
/// Only tall ones: a screen that changed in one small place has nothing worth
/// dividing, and every extra band is another JPEG header.
fn split_for_threads(
    regions: Vec<(u32, u32, u32, u32)>,
    threads: usize,
) -> Vec<(u32, u32, u32, u32)> {
    if threads <= 1 || regions.len() >= threads {
        return regions;
    }
    let mut out = Vec::with_capacity(threads);
    for (x, y, w, h) in regions {
        if h < SPLIT_MIN_HEIGHT {
            out.push((x, y, w, h));
            continue;
        }
        let bands = threads.min((h / (SPLIT_MIN_HEIGHT / 2)) as usize).max(1) as u32;
        let band_h = h.div_ceil(bands);
        let mut cursor = 0;
        while cursor < h {
            // The last band takes the remainder, so the split covers the region
            // exactly — a rounded-down band would leave a strip never sent.
            let this = band_h.min(h - cursor);
            out.push((x, y + cursor, w, this));
            cursor += this;
        }
    }
    out
}

/// Encodes one rectangle of the frame as JPEG.
///
/// The RGB buffer is filled with the channel order resolved once, outside the
/// loop. Reading it per pixel through the frame's accessor re-tested the format
/// four million times a second for an answer that cannot change mid-frame.
fn encode_region(
    frame: &Frame,
    stride: usize,
    x: u32,
    y: u32,
    w: u32,
    h: u32,
    quality: u8,
) -> Result<Vec<u8>> {
    let src = &frame.pixels[..];
    let mut rgb = vec![0u8; (w as usize) * (h as usize) * 3];
    let bgr = frame.format == PixelFormat::Bgrx;

    for r in 0..h as usize {
        let row = ((y as usize) + r) * stride + (x as usize) * 4;
        let dst = &mut rgb[r * (w as usize) * 3..][..(w as usize) * 3];
        let s_row = &src[row..row + (w as usize) * 4];
        if bgr {
            for (px, out) in s_row.chunks_exact(4).zip(dst.chunks_exact_mut(3)) {
                out[0] = px[2];
                out[1] = px[1];
                out[2] = px[0];
            }
        } else {
            for (px, out) in s_row.chunks_exact(4).zip(dst.chunks_exact_mut(3)) {
                out.copy_from_slice(&px[..3]);
            }
        }
    }

    // A SIMD encoder with 4:2:0 chroma subsampling rather than the `image`
    // crate's scalar 4:2:2. This is the hot loop of the whole path — a
    // full-screen change is two million pixels, ten times a second — and the
    // two choices are worth roughly a factor of five between them.
    //
    // 4:2:0 halves the chroma work. On a desktop screen that is nearly free
    // visually: text is a luma edge, and this is the path already chosen
    // because bandwidth is short.
    let mut out = Vec::with_capacity((w as usize) * (h as usize) / 4);
    let mut enc = jpeg_encoder::Encoder::new(&mut out, quality);
    enc.set_sampling_factor(jpeg_encoder::SamplingFactor::F_2_2);
    enc.encode(&rgb, w as u16, h as u16, jpeg_encoder::ColorType::Rgb)
        .map_err(|e| PlatformError::CaptureFailed(format!("region jpeg: {e}")))?;
    Ok(out)
}

/// Whether any pixel of the tile differs from the previous frame.
///
/// Row-at-a-time slice comparison: `memcmp` under the hood, so it is vectorised
/// and it stops at the first difference, which is the common case for a tile
/// that did change.
fn tile_differs(cur: &[u8], prev: &[u8], stride: usize, x: u32, y: u32, w: u32, h: u32) -> bool {
    let row_bytes = (w as usize) * 4;
    for r in 0..h as usize {
        let off = ((y as usize) + r) * stride + (x as usize) * 4;
        if cur[off..off + row_bytes] != prev[off..off + row_bytes] {
            return true;
        }
    }
    false
}

fn copy_tile(cur: &[u8], prev: &mut [u8], stride: usize, x: u32, y: u32, w: u32, h: u32) {
    let row_bytes = (w as usize) * 4;
    for r in 0..h as usize {
        let off = ((y as usize) + r) * stride + (x as usize) * 4;
        prev[off..off + row_bytes].copy_from_slice(&cur[off..off + row_bytes]);
    }
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
            format: PixelFormat::Rgba,
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

    /// Asserts the regions tile the given area exactly — no gap, no overlap.
    ///
    /// A gap is a hole in the viewer's canvas; an overlap is bytes spent twice
    /// on the one path that exists because bandwidth is short. The count is
    /// deliberately not checked: how the encoder divides the work is its own
    /// business, and pinning it turns every future improvement into a test
    /// failure.
    fn assert_covers(u: &FrameUpdate, w: u32, h: u32) {
        let area: u32 = u.tiles.iter().map(|t| t.w * t.h).sum();
        assert_eq!(area, w * h, "regions do not add up to {w}x{h}");
        for t in &u.tiles {
            assert!(t.x + t.w <= w && t.y + t.h <= h, "region {t:?} runs off the screen");
        }
        let mut seen: Vec<(u32, u32)> = u.tiles.iter().map(|t| (t.x, t.y)).collect();
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(seen.len(), u.tiles.len(), "a position was emitted twice");
    }

    #[test]
    fn the_first_frame_is_a_keyframe_covering_the_whole_screen_exactly_once() {
        let mut e = TileEncoder::new(DEFAULT_QUALITY);
        let u = e.encode(&frame(256, 128, 0), false).unwrap();

        assert!(u.keyframe);
        assert_covers(&u, 256, 128);
    }

    #[test]
    fn a_widespread_change_is_sent_as_one_image_rather_than_many() {
        // A 64x64 JPEG is ~790 bytes here, roughly 600 of it header and Huffman
        // tables. Sending a full-screen change tile by tile spends more than
        // half its bytes, and one encoder setup per tile, on that overhead.
        let mut e = TileEncoder::new(DEFAULT_QUALITY);
        let a = frame(256, 128, 0);
        e.encode(&a, false).unwrap();

        let b = frame(256, 128, 200); // every pixel differs
        let u = e.encode(&b, false).unwrap();

        assert!(!u.keyframe);
        assert_eq!(u.tiles.len(), 1, "a full-screen change was still sent tile by tile");
        assert_eq!((u.tiles[0].x, u.tiles[0].y, u.tiles[0].w, u.tiles[0].h), (0, 0, 256, 128));
    }

    #[test]
    fn splitting_for_threads_still_covers_the_region_exactly() {
        // A band rounded down leaves a strip that is never sent, and the viewer
        // keeps showing stale pixels there until something else dirties it.
        for h in [255u32, 256, 257, 1000, 1080] {
            for threads in 1..=4usize {
                let out = split_for_threads(vec![(0, 0, 1920, h)], threads);
                let area: u32 = out.iter().map(|r| r.2 * r.3).sum();
                assert_eq!(area, 1920 * h, "h={h} threads={threads} lost coverage");

                // Contiguous and in order, so the bands tile rather than overlap.
                let mut y = 0;
                for r in &out {
                    assert_eq!(r.1, y, "band gap or overlap at h={h} threads={threads}");
                    y += r.3;
                }
                assert_eq!(y, h);
            }
        }
    }

    #[test]
    fn a_small_region_is_not_split_across_threads() {
        // Every band costs another JPEG header; there is nothing to win by
        // dividing a cursor-sized change four ways.
        let out = split_for_threads(vec![(10, 10, 64, 64)], 4);
        assert_eq!(out, vec![(10, 10, 64, 64)]);
    }

    #[test]
    fn a_sparse_change_stays_split_so_untouched_pixels_are_not_resent() {
        // The mirror of the test above: coalescing everything would send the
        // whole screen every time a cursor blinked.
        let mut e = TileEncoder::new(DEFAULT_QUALITY);
        let mut f = frame(512, 256, 0); // 8x4 = 32 tiles
        e.encode(&f, false).unwrap();

        set_pixel(&mut f, 10, 10, 255);
        set_pixel(&mut f, 400, 200, 255);
        let u = e.encode(&f, false).unwrap();

        assert_eq!(u.tiles.len(), 2, "two distant pixels produced {} regions", u.tiles.len());
        let area: u32 = u.tiles.iter().map(|t| t.w * t.h).sum();
        assert!(area <= 64 * 64 * 2, "the change was coalesced into {area} pixels");
    }

    #[test]
    fn a_bgrx_frame_is_tiled_with_its_channels_the_right_way_round() {
        // The stream reads the capture backend's native layout to avoid a
        // whole-frame conversion. If it read it as RGBA the viewer would show
        // every screen with red and blue swapped.
        let mut px = Vec::new();
        for _ in 0..(8 * 8) {
            px.extend_from_slice(&[10u8, 20, 200, 0x00]); // B G R X
        }
        let f = Frame {
            width: 8,
            height: 8,
            pixels: px,
            format: PixelFormat::Bgrx,
            captured_at: SystemTime::now(),
        };

        let mut e = TileEncoder::new(100);
        let u = e.encode(&f, false).unwrap();
        let img = image::load_from_memory(&u.tiles[0].jpeg).unwrap().to_rgb8();
        let p = img.get_pixel(4, 4).0;
        assert!(p[0] > 150 && p[2] < 60, "channels look swapped: {p:?}");
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
        // encode eight rows that do not exist and paint them onto the viewer.
        let mut e = TileEncoder::new(DEFAULT_QUALITY);
        let mut f = frame(200, 100, 0); // 4x2 grid; the corner tile is 8x36
        assert_covers(&e.encode(&f, false).unwrap(), 200, 100);

        // Dirty only the clipped corner, so it is sent on its own.
        set_pixel(&mut f, 195, 90, 255);
        let u = e.encode(&f, false).unwrap();
        assert_eq!(u.tiles.len(), 1);
        assert_eq!(
            (u.tiles[0].x, u.tiles[0].y, u.tiles[0].w, u.tiles[0].h),
            (192, 64, 8, 36),
            "the corner region was padded past the edge of the screen"
        );
    }

    #[test]
    fn every_tile_decodes_to_its_declared_size() {
        // A tile whose bytes disagree with its header lands in the wrong place
        // on the canvas and corrupts the region around it.
        let mut e = TileEncoder::new(DEFAULT_QUALITY);
        let mut f = frame(200, 100, 33);
        e.encode(&f, false).unwrap();
        set_pixel(&mut f, 10, 10, 99);
        set_pixel(&mut f, 195, 95, 99);
        let u = e.encode(&f, false).unwrap();
        assert!(!u.tiles.is_empty());
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
        assert_covers(&u, 320, 192);
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
        assert_covers(&u, 128, 128);
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
            format: PixelFormat::Rgba,
            captured_at: SystemTime::now(),
        };
        assert!(e.encode(&bad, false).is_err());
    }
}
