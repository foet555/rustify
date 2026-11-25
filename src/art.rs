//! Album art: fetch Spotify's CDN images off the UI thread, decode each one
//! once, and keep the texture keyed by URL *and* size so scrolling, re-renders
//! and several widgets sharing a cover never refetch it.
//!
//! The generated gradient (`theme::art`) stays as the placeholder: it is what
//! every slot paints while a download is in flight, when a track has no cover
//! URL at all, and when a fetch fails. Nothing here invents artwork.
use std::collections::{HashMap, HashSet};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex, OnceLock};

use eframe::egui::{self, Color32, Pos2, Rect, TextureHandle, TextureOptions};

/// Sidebar thumbnails, tracklist rows and the player bar.
pub const THUMB: u32 = 64;
/// Cards, the banner and the Now Playing panel.
pub const TILE: u32 = 300;

type Key = (String, u32);
type Fetcher = Arc<dyn Fn(&str) -> Result<Vec<u8>, String> + Send + Sync>;

struct Entry {
    tex: Option<TextureHandle>,
    dominant: Option<Color32>,
    failed: bool,
}

pub struct Cache {
    fetch: Fetcher,
    tx: Sender<(Key, Result<Vec<u8>, String>)>,
    rx: Receiver<(Key, Result<Vec<u8>, String>)>,
    entries: HashMap<Key, Entry>,
    loading: HashSet<Key>,
}

impl Cache {
    pub fn new(fetch: Fetcher) -> Self {
        let (tx, rx) = channel();
        Self {
            fetch,
            tx,
            rx,
            entries: HashMap::new(),
            loading: HashSet::new(),
        }
    }

    /// The real pipeline: bytes come from i.scdn.co through curl.exe.
    pub fn with_curl() -> Self {
        Self::new(Arc::new(|url: &str| crate::spotify::fetch_bytes(url)))
    }

    /// The texture for this URL at this pixel size, if it is decoded already.
    /// A miss starts exactly one download and returns `None`, so the caller
    /// paints its placeholder this frame.
    pub fn texture(&mut self, ctx: &egui::Context, url: &str, size: u32) -> Option<TextureHandle> {
        self.pump(ctx);
        let key = (url.to_string(), size);
        if let Some(entry) = self.entries.get(&key) {
            // A URL that already failed keeps the placeholder and never asks
            // the CDN again.
            if entry.failed {
                return None;
            }
            return entry.tex.clone();
        }
        self.request(key);
        None
    }

    /// The average colour of the decoded artwork, for the page header. `None`
    /// until the image arrives so the caller can keep its own fallback.
    pub fn dominant(&mut self, ctx: &egui::Context, url: &str, size: u32) -> Option<Color32> {
        self.pump(ctx);
        let key = (url.to_string(), size);
        if let Some(entry) = self.entries.get(&key) {
            return entry.dominant;
        }
        self.request(key);
        None
    }

    fn request(&mut self, key: Key) {
        if self.loading.contains(&key) || self.entries.contains_key(&key) {
            return;
        }
        self.loading.insert(key.clone());
        let tx = self.tx.clone();
        let fetch = self.fetch.clone();
        let (url, _) = key.clone();
        // One short-lived thread per image: a slow CDN never stalls a frame,
        // and there is no pool to tune for a window that paints a few dozen
        // covers.
        std::thread::spawn(move || {
            let result = fetch(&url);
            let _ = tx.send((key, result));
        });
    }

    /// Turns finished downloads into textures. Cheap when nothing is pending:
    /// one non-blocking channel read.
    pub fn pump(&mut self, ctx: &egui::Context) {
        let mut ready = false;
        while let Ok((key, result)) = self.rx.try_recv() {
            self.loading.remove(&key);
            match result.and_then(|bytes| decode(&bytes, key.1)) {
                Ok((image, dominant)) => {
                    let tex = ctx.load_texture(
                        format!("art:{}:{}", key.1, key.0),
                        image,
                        TextureOptions::LINEAR,
                    );
                    self.entries.insert(
                        key,
                        Entry {
                            tex: Some(tex),
                            dominant: Some(dominant),
                            failed: false,
                        },
                    );
                }
                Err(_) => {
                    // A dead URL is remembered as dead: the placeholder stays
                    // and we never hammer the CDN for it again.
                    self.entries.insert(
                        key,
                        Entry {
                            tex: None,
                            dominant: None,
                            failed: true,
                        },
                    );
                }
            }
            ready = true;
        }
        if ready {
            ctx.request_repaint();
        }
    }

    #[cfg(test)]
    fn loaded(&self, url: &str, size: u32) -> bool {
        self.entries
            .get(&(url.to_string(), size))
            .map(|e| e.tex.is_some())
            .unwrap_or(false)
    }

    #[cfg(test)]
    fn failed(&self, url: &str, size: u32) -> bool {
        self.entries
            .get(&(url.to_string(), size))
            .map(|e| e.failed)
            .unwrap_or(false)
    }
}

static CACHE: OnceLock<Mutex<Cache>> = OnceLock::new();

fn shared() -> &'static Mutex<Cache> {
    CACHE.get_or_init(|| Mutex::new(Cache::with_curl()))
}

/// Paints `url` into `rect` at `size`, or the generated gradient while the
/// download runs / when there is no URL. Every cover slot in the app goes
/// through here.
pub fn paint(p: &egui::Painter, rect: Rect, url: Option<&str>, size: u32, c1: Color32, c2: Color32) {
    let url = url.filter(|u| !u.trim().is_empty());
    if let Some(url) = url {
        let tex = shared()
            .lock()
            .ok()
            .and_then(|mut cache| cache.texture(p.ctx(), url, size));
        if let Some(tex) = tex {
            p.image(
                tex.id(),
                rect,
                Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                Color32::WHITE,
            );
            return;
        }
    }
    crate::theme::art(p, rect, c1, c2);
}

/// The artwork's own dominant colour, once decoded. Used for the page header
/// so the gradient matches the cover instead of a hash of its name.
pub fn dominant(ctx: &egui::Context, url: Option<&str>, size: u32) -> Option<Color32> {
    let url = url.filter(|u| !u.trim().is_empty())?;
    shared()
        .lock()
        .ok()
        .and_then(|mut cache| cache.dominant(ctx, url, size))
}

/// Decodes bytes into a square `size` texture image plus its average colour.
fn decode(bytes: &[u8], size: u32) -> Result<(egui::ColorImage, Color32), String> {
    let img = image::load_from_memory(bytes).map_err(|e| format!("not an image: {e}"))?;
    let rgba = img.to_rgba8();
    let dominant = dominant_of(&rgba);
    let (w, h) = rgba.dimensions();
    let side = w.min(h).max(1);
    let crop = image::imageops::crop_imm(&rgba, (w - side) / 2, (h - side) / 2, side, side).to_image();
    let side = size.clamp(16, 1024).min(side.max(1));
    let scaled = image::imageops::resize(
        &crop,
        side,
        side,
        image::imageops::FilterType::Triangle,
    );
    Ok((
        egui::ColorImage::from_rgba_unmultiplied(
            [scaled.width() as usize, scaled.height() as usize],
            scaled.as_raw(),
        ),
        dominant,
    ))
}

/// Average colour of the pixels that carry the most colour: greys are damped
/// so a cover that is mostly black with a green logo reports green.
pub fn dominant_of(img: &image::RgbaImage) -> Color32 {
    let (mut r, mut g, mut b, mut weight) = (0f64, 0f64, 0f64, 0f64);
    for px in img.pixels() {
        if px.0[3] < 8 {
            continue;
        }
        let (rr, gg, bb) = (px.0[0] as f64, px.0[1] as f64, px.0[2] as f64);
        let max = rr.max(gg).max(bb);
        let min = rr.min(gg).min(bb);
        let sat = if max <= 0.0 { 0.0 } else { (max - min) / max };
        // Saturation plus brightness: mid-greys barely count, dark mud none.
        let w = (0.25 + sat) * (max / 255.0);
        r += rr * w;
        g += gg * w;
        b += bb * w;
        weight += w;
    }
    if weight <= 0.0 {
        return Color32::from_rgb(0x33, 0x33, 0x33);
    }
    Color32::from_rgb(
        (r / weight).round().clamp(0.0, 255.0) as u8,
        (g / weight).round().clamp(0.0, 255.0) as u8,
        (b / weight).round().clamp(0.0, 255.0) as u8,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A real i.scdn.co cover, no auth needed. The bytes are also recorded next
    /// to the source so the pipeline test still proves decoding when the
    /// machine is offline.
    const REAL_URL: &str = "https://i.scdn.co/image/ab67616d00001e028863bc11d2aa12b54f5aeb36";
    const RECORDED: &str = "assets/test-cover.jpg";

    fn real_bytes() -> Vec<u8> {
        match crate::spotify::fetch_bytes(REAL_URL) {
            Ok(bytes) => bytes,
            Err(_) => std::fs::read(RECORDED).expect("recorded cover fixture"),
        }
    }

    fn wait_for(cache: &mut Cache, ctx: &egui::Context, url: &str, size: u32) {
        for _ in 0..300 {
            cache.pump(ctx);
            if cache.loaded(url, size) || cache.failed(url, size) {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        panic!("artwork for {url} at {size}px never finished decoding");
    }

    #[test]
    fn recorded_bytes_are_a_real_jpeg_from_the_cdn() {
        let bytes = std::fs::read(RECORDED).expect("recorded cover fixture");
        // JPEG magic: whatever happens with the network, this file is a real
        // image and not a text stub.
        assert_eq!(&bytes[..2], &[0xff, 0xd8], "fixture must be a JPEG");
        assert!(bytes.len() > 4000, "fixture is only {} bytes", bytes.len());
    }

    #[test]
    fn real_cover_decodes_into_a_texture_and_is_cached_by_url_and_size() {
        let calls = Arc::new(AtomicUsize::new(0));
        let counted = calls.clone();
        let bytes = real_bytes();
        let mut cache = Cache::new(Arc::new(move |_url: &str| {
            counted.fetch_add(1, Ordering::SeqCst);
            Ok(bytes.clone())
        }));
        let ctx = egui::Context::default();

        // First request: no texture yet, exactly one download in flight.
        assert!(cache.texture(&ctx, REAL_URL, THUMB).is_none());
        wait_for(&mut cache, &ctx, REAL_URL, THUMB);
        let first = cache.texture(&ctx, REAL_URL, THUMB).expect("decoded texture");
        let again = cache.texture(&ctx, REAL_URL, THUMB).expect("cached texture");
        assert_eq!(first.id(), again.id(), "same URL and size must reuse the texture");
        assert_eq!(calls.load(Ordering::SeqCst), 1, "one fetch serves every request");

        // A different slot size is its own cache entry, not a silent blur.
        assert!(cache.texture(&ctx, REAL_URL, TILE).is_none());
        wait_for(&mut cache, &ctx, REAL_URL, TILE);
        let tile = cache.texture(&ctx, REAL_URL, TILE).expect("decoded tile");
        assert_ne!(tile.id(), first.id());
        assert_eq!(calls.load(Ordering::SeqCst), 2);

        // The header colour comes from the same decoded pixels.
        let dominant = cache.dominant(&ctx, REAL_URL, THUMB).expect("dominant colour");
        assert!(
            dominant != Color32::from_rgb(0x33, 0x33, 0x33),
            "dominant colour must come from the image, not the fallback"
        );
    }

    #[test]
    fn dominant_colour_follows_the_pixels_that_carry_colour() {
        let mut img = image::RgbaImage::from_pixel(40, 40, image::Rgba([12, 12, 12, 255]));
        for x in 10..30 {
            for y in 10..30 {
                img.put_pixel(x, y, image::Rgba([0x1d, 0xb9, 0x54, 255]));
            }
        }
        let c = dominant_of(&img);
        assert!(c.g() > 120, "green channel should dominate: {c:?}");
        assert!(c.g() > c.r() + 40 && c.g() > c.b() + 20, "{c:?}");

        // All black is still answered, with the neutral fallback.
        let black = image::RgbaImage::from_pixel(8, 8, image::Rgba([0, 0, 0, 255]));
        assert_eq!(dominant_of(&black), Color32::from_rgb(0x33, 0x33, 0x33));
    }

    #[test]
    fn garbage_bytes_fail_once_and_are_not_retried() {
        let calls = Arc::new(AtomicUsize::new(0));
        let counted = calls.clone();
        let mut cache = Cache::new(Arc::new(move |_url: &str| {
            counted.fetch_add(1, Ordering::SeqCst);
            Ok(b"not an image at all".to_vec())
        }));
        let ctx = egui::Context::default();
        assert!(cache.texture(&ctx, "https://i.scdn.co/image/dead", THUMB).is_none());
        for _ in 0..100 {
            cache.pump(&ctx);
            if cache.failed("https://i.scdn.co/image/dead", THUMB) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(cache.failed("https://i.scdn.co/image/dead", THUMB));
        // The slot keeps the placeholder instead of looping on the CDN.
        assert!(cache.texture(&ctx, "https://i.scdn.co/image/dead", THUMB).is_none());
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn square_crop_keeps_the_middle_of_a_wide_image() {
        let (image, _) = decode(&std::fs::read(RECORDED).unwrap(), THUMB).unwrap();
        assert_eq!(image.size, [THUMB as usize, THUMB as usize]);
    }
}
