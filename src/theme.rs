// Palettes and painting helpers. Every theme is a full colour set so the whole
// app can be re-skinned at runtime.
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use eframe::egui::{self, Align2, Color32, CornerRadius, FontId, Painter, Pos2, Rect, Shape, Stroke, Vec2};

/// Interface-wide knobs set by the theme file: corner radius and the font
/// family every painted label uses. Globals rather than parameters because
/// every widget already reaches for these helpers.
static RADIUS_SCALE: AtomicU32 = AtomicU32::new(0x3f80_0000); // 1.0f32
static DENSITY: AtomicU32 = AtomicU32::new(0x3f80_0000); // 1.0f32
static MONO: AtomicBool = AtomicBool::new(false);
/// Whole-interface scale from the theme file.
static ZOOM: AtomicU32 = AtomicU32::new(0x3f80_0000);

pub fn set_zoom(z: f32) {
    ZOOM.store(z.clamp(0.8, 1.3).to_bits(), Ordering::Relaxed);
}

pub fn zoom() -> f32 {
    f32::from_bits(ZOOM.load(Ordering::Relaxed))
}

/// Spacing multiplier for the Rustify-only Compact/Cozy/Spacious option.
pub fn set_density(d: f32) {
    DENSITY.store(d.clamp(0.8, 1.2).to_bits(), Ordering::Relaxed);
}

pub fn density() -> f32 {
    f32::from_bits(DENSITY.load(Ordering::Relaxed))
}

pub fn set_radius_scale(s: f32) {
    RADIUS_SCALE.store(s.clamp(0.0, 6.0).to_bits(), Ordering::Relaxed);
}

pub fn radius_scale() -> f32 {
    f32::from_bits(RADIUS_SCALE.load(Ordering::Relaxed))
}

pub fn set_mono(on: bool) {
    MONO.store(on, Ordering::Relaxed);
}

pub fn mono() -> bool {
    MONO.load(Ordering::Relaxed)
}

pub fn rgb(hex: u32) -> Color32 {
    Color32::from_rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

/// Same colour with an explicit alpha, for panels that let a background image
/// or gradient show through.
pub fn with_alpha(c: Color32, a: u8) -> Color32 {
    Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), a)
}

pub fn opaque(c: Color32) -> Color32 {
    with_alpha(c, 255)
}

/// Blends `b` into `a`. Used to derive the six shades a palette needs from the
/// two colours a custom theme actually asks for.
fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let f = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgba_unmultiplied(f(a.r(), b.r()), f(a.g(), b.g()), f(a.b(), b.b()), a.a())
}

pub fn lum(c: Color32) -> f32 {
    (0.299 * c.r() as f32 + 0.587 * c.g() as f32 + 0.114 * c.b() as f32) / 255.0
}


/// The logo is the one fixed colour in the app: it stays the same orange in
/// every theme, the way a brand mark should.
pub fn brand() -> Color32 {
    rgb(0xff7a00)
}

/// The Rustify mark: a black gear on an orange disc. The gear is deliberately
/// generic - no sound waves, no Spotify silhouette - so the brand cannot be
/// mistaken for anyone else's mark.
pub fn logo(p: &Painter, center: Pos2, radius: f32, circle: Color32) {
    p.circle_filled(center, radius, circle);
    gear(p, center, radius, Color32::BLACK, circle);
}

/// Eight teeth around a solid body, with the disc colour showing through the
/// hub. Shared by the on-screen marks and the rasterized app icon.
fn gear(p: &Painter, center: Pos2, radius: f32, ink: Color32, hole: Color32) {
    const TEETH: usize = 8;
    let body = radius * 0.72;
    let hub = radius * 0.28;
    for i in 0..TEETH {
        let angle = i as f32 / TEETH as f32 * std::f32::consts::TAU;
        let at = |offset: f32, distance: f32| {
            center + Vec2::new((angle + offset).cos(), (angle + offset).sin()) * distance
        };
        p.add(Shape::convex_polygon(
            vec![
                at(-0.20, body),
                at(0.20, body),
                at(0.15, radius),
                at(-0.15, radius),
            ],
            ink,
            Stroke::NONE,
        ));
    }
    p.circle_filled(center, body, ink);
    p.circle_filled(center, hub, hole);
}

/// The same gear rasterized for the window and taskbar. Drawn at 4x and
/// averaged down, so the teeth stay smooth without shipping an image.
pub fn icon(size: u32) -> eframe::egui::IconData {
    const SS: u32 = 4;
    let px = (size * SS).max(1);
    let mut hi = vec![0u8; (px * px * 4) as usize];
    let c = px as f32 / 2.0;
    let radius = c * 0.94;
    let orange = brand();
    let tau = std::f32::consts::TAU;

    for y in 0..px {
        for x in 0..px {
            let dx = (x as f32 + 0.5 - c) / radius;
            let dy = (y as f32 + 0.5 - c) / radius;
            let rr = dx * dx + dy * dy;
            if rr > 1.0 {
                continue;
            }
            let r = rr.sqrt();
            let sector = (dy.atan2(dx) + std::f32::consts::PI) / tau * 8.0;
            let frac = sector - sector.floor();
            let high = ((r - 0.66) / 0.34).clamp(0.0, 1.0);
            let half = 0.20 + (0.15 - 0.20) * high;
            let tooth = r >= 0.66 && (frac - 0.5).abs() <= half;
            let color = if r <= 0.28 {
                (orange.r(), orange.g(), orange.b())
            } else if r <= 0.72 || tooth {
                (0, 0, 0)
            } else {
                (orange.r(), orange.g(), orange.b())
            };
            let i = ((y * px + x) * 4) as usize;
            hi[i] = color.0;
            hi[i + 1] = color.1;
            hi[i + 2] = color.2;
            hi[i + 3] = 255;
        }
    }

    let mut rgba = vec![0u8; (size * size * 4) as usize];
    let cell = SS * SS;
    for y in 0..size {
        for x in 0..size {
            let (mut r, mut g, mut b, mut a) = (0u32, 0u32, 0u32, 0u32);
            for sy in 0..SS {
                for sx in 0..SS {
                    let i = ((((y * SS + sy) * px) + x * SS + sx) * 4) as usize;
                    let alpha = hi[i + 3] as u32;
                    r += hi[i] as u32 * alpha;
                    g += hi[i + 1] as u32 * alpha;
                    b += hi[i + 2] as u32 * alpha;
                    a += alpha;
                }
            }
            let o = ((y * size + x) * 4) as usize;
            if a > 0 {
                rgba[o] = ((r + a / 2) / a) as u8;
                rgba[o + 1] = ((g + a / 2) / a) as u8;
                rgba[o + 2] = ((b + a / 2) / a) as u8;
            }
            rgba[o + 3] = ((a + cell / 2) / cell) as u8;
        }
    }
    eframe::egui::IconData {
        rgba,
        width: size,
        height: size,
    }
}

#[derive(Clone)]
pub struct Theme {
    pub name: &'static str,
    pub bg: Color32,
    pub sidebar: Color32,
    pub card: Color32,
    pub card_hover: Color32,
    pub surface: Color32,
    pub player: Color32,
    pub accent: Color32,
    pub on_accent: Color32,
    pub text: Color32,
    pub muted: Color32,
    pub faint: Color32,
    pub rail: Color32,
    /// Set for gradient and custom themes: painted behind every panel.
    pub gradient: Option<(Color32, Color32)>,
}

impl Theme {
    /// The flat colour behind the gradient, so a translucent panel never shows
    /// whatever was on screen before this frame.
    pub fn base(&self) -> Color32 {
        match self.gradient {
            Some((top, _)) => opaque(top),
            None => opaque(self.bg),
        }
    }
}

/// Everything a custom theme needs. Only four colours and two knobs, so the
/// rest of the palette can be derived and the picker stays readable.
#[derive(Clone, PartialEq, Debug)]
pub struct Custom {
    pub top: u32,
    pub bottom: u32,
    pub accent: u32,
    pub text: u32,
    /// Optional background photo. Empty means the gradient alone.
    pub image: String,
    /// 0 = sharp, 1 = very soft. Applied when the photo is loaded.
    pub blur: f32,
    /// 0 = photo at full strength, 1 = almost black.
    pub dim: f32,
    /// How much of the two-colour gradient is laid over the photo. 0 leaves the
    /// photo untouched, 1 gives it the full themed wash.
    pub wash: f32,
}

impl Default for Custom {
    fn default() -> Self {
        Self {
            top: 0x141220,
            bottom: 0x3a1030,
            accent: 0xff7a00,
            text: 0xffffff,
            image: String::new(),
            blur: 0.55,
            dim: 0.45,
            wash: 0.55,
        }
    }
}

impl Custom {
    /// Turns four colours into a full palette. Panels keep some transparency so
    /// the gradient or photo behind them stays visible.
    pub fn theme(&self) -> Theme {
        let top = rgb(self.top);
        let bottom = rgb(self.bottom);
        let text = rgb(self.text);
        let accent = rgb(self.accent);
        let mid = mix(top, bottom, 0.5);
        Theme {
            name: "Custom",
            bg: with_alpha(top, 130),
            sidebar: with_alpha(top, 180),
            card: with_alpha(mid, 200),
            card_hover: with_alpha(bottom, 235),
            surface: with_alpha(mid, 190),
            player: with_alpha(top, 215),
            accent,
            on_accent: if lum(accent) > 0.55 {
                Color32::BLACK
            } else {
                Color32::WHITE
            },
            text,
            muted: mix(text, mid, 0.35),
            faint: mix(text, mid, 0.55),
            rail: mix(text, mid, 0.72),
            gradient: Some((top, bottom)),
        }
    }
}

pub const BUILTIN: usize = 11;

/// The last entry of `all()` is always the user's custom palette.
pub fn custom_index() -> usize {
    BUILTIN
}

/// Built-in palettes with the user's own palette in the last slot.
pub fn with_custom(custom: &Custom) -> Vec<Theme> {
    let mut list = all();
    if let Some(last) = list.last_mut() {
        *last = custom.theme();
    }
    list
}

pub fn all() -> Vec<Theme> {
    vec![
        Theme {
            name: "Spotify Dark",
            bg: rgb(0x121212),
            sidebar: rgb(0x000000),
            card: rgb(0x181818),
            card_hover: rgb(0x2a2a2a),
            surface: rgb(0x262626),
            player: rgb(0x0b0b0b),
            accent: rgb(0x1db954),
            on_accent: Color32::BLACK,
            text: rgb(0xffffff),
            muted: rgb(0xb3b3b3),
            faint: rgb(0x7c7c7c),
            rail: rgb(0x484848),
            gradient: None,
        },
        Theme {
            name: "Midnight",
            bg: rgb(0x0c1220),
            sidebar: rgb(0x060a14),
            card: rgb(0x121a2c),
            card_hover: rgb(0x1e2a42),
            surface: rgb(0x1a2438),
            player: rgb(0x090e1a),
            accent: rgb(0x5aa0ff),
            on_accent: rgb(0x04121e),
            text: rgb(0xeef4ff),
            muted: rgb(0xa0b0c8),
            faint: rgb(0x708098),
            rail: rgb(0x3a4862),
            gradient: None,
        },
        Theme {
            name: "Sunset",
            bg: rgb(0x1a1016),
            sidebar: rgb(0x10090d),
            card: rgb(0x26161e),
            card_hover: rgb(0x38202a),
            surface: rgb(0x2e1c24),
            player: rgb(0x120a0f),
            accent: rgb(0xff7a59),
            on_accent: rgb(0x1e0a04),
            text: rgb(0xfff0eb),
            muted: rgb(0xc6a4a0),
            faint: rgb(0x966c72),
            rail: rgb(0x56363e),
            gradient: None,
        },
        Theme {
            name: "Neon",
            bg: rgb(0x0e0a1a),
            sidebar: rgb(0x080610),
            card: rgb(0x161028),
            card_hover: rgb(0x241a3e),
            surface: rgb(0x1e1634),
            player: rgb(0x0a0714),
            accent: rgb(0xd95aff),
            on_accent: rgb(0x100418),
            text: rgb(0xf4ecff),
            muted: rgb(0xb0a0d0),
            faint: rgb(0x8070a0),
            rail: rgb(0x403060),
            gradient: None,
        },
        Theme {
            name: "Daylight",
            bg: rgb(0xf6f6f6),
            sidebar: rgb(0xe8e8e8),
            card: rgb(0xeeeeee),
            card_hover: rgb(0xdedede),
            surface: rgb(0xe2e2e2),
            player: rgb(0xfafafa),
            accent: rgb(0x1d9646),
            on_accent: rgb(0xffffff),
            text: rgb(0x141414),
            muted: rgb(0x5a5a5a),
            faint: rgb(0x828282),
            rail: rgb(0xc4c4c4),
            gradient: None,
        },
        gradient_theme("Aurora", 0x062029, 0x1b2a6b, 0x2ee6c9, 0xffffff),
        gradient_theme("Ember", 0x1a0a06, 0x4a1608, 0xff7a00, 0xfff4ec),
        gradient_theme("Ocean", 0x04141f, 0x0b3a5c, 0x57c8ff, 0xeaf7ff),
        gradient_theme("Bubblegum", 0x2a0a22, 0x5e1a52, 0xff6ec7, 0xffeefa),
        gradient_theme("Graphite", 0x141414, 0x2e2e2e, 0xd8d8d8, 0xf2f2f2),
        gradient_theme("Mint", 0xdff3e6, 0xb9e3cf, 0x1a8f5a, 0x14231c),
        Custom::default().theme(),
    ]
}

/// Built-in gradient themes are just a custom palette with the name fixed, so
/// they age together and there is only one palette builder to get right.
fn gradient_theme(name: &'static str, top: u32, bottom: u32, accent: u32, text: u32) -> Theme {
    let mut t = Custom {
        top,
        bottom,
        accent,
        text,
        ..Default::default()
    }
    .theme();
    t.name = name;
    t
}

/// Filled rounded rect. The radius is scaled by the theme file and capped at
/// half the short side, so a pill stays a pill no matter what the file says.
pub fn rr(p: &Painter, rect: Rect, r: u8, fill: Color32) {
    let cap = (rect.width().min(rect.height()).max(0.0) * 0.5).min(255.0);
    let radius = ((r as f32) * radius_scale()).round().clamp(0.0, cap) as u8;
    p.rect_filled(rect, CornerRadius::same(radius), fill);
}

pub fn text(p: &Painter, pos: Pos2, align: Align2, s: impl ToString, size: f32, color: Color32) {
    let font = if mono() {
        FontId::monospace(size)
    } else {
        FontId::proportional(size)
    };
    p.text(pos, align, s.to_string(), font, color);
}

pub fn fmt_time(secs: u32) -> String {
    format!("{}:{:02}", secs / 60, secs % 60)
}

/// "2026-11-22" -> "Nov 22, 2026", the way the client writes dates. Anything
/// that is not a plain date comes back untouched, and an empty one becomes a
/// dash rather than an empty cell.
pub fn pretty_date(iso: &str) -> String {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let s = iso.trim();
    if s.is_empty() {
        return "-".to_string();
    }
    let b = s.as_bytes();
    if b.len() < 10 || (b[4] != b'-' && b[4] != b'/') || (b[7] != b'-' && b[7] != b'/') {
        return s.to_string();
    }
    let (Ok(year), Ok(month), Ok(day)) = (
        s[..4].parse::<u32>(),
        s[5..7].parse::<usize>(),
        s[8..10].parse::<u32>(),
    ) else {
        return s.to_string();
    };
    match MONTHS.get(month.wrapping_sub(1)) {
        Some(name) => format!("{name} {day}, {year}"),
        None => s.to_string(),
    }
}

/// Fake album cover: flat base colour plus a couple of shapes in the accent colour.
pub fn art(p: &Painter, rect: Rect, c1: Color32, c2: Color32) {
    let r = (rect.width() * 0.05).max(3.0).min(8.0) as u8;
    rr(p, rect, r, c1);
    let p = p.with_clip_rect(rect);
    let w = rect.width();
    p.circle_filled(rect.max - Vec2::splat(w * 0.24), w * 0.40, c2.gamma_multiply(0.80));
    p.circle_filled(
        rect.min + Vec2::splat(w * 0.28),
        w * 0.15,
        c2.gamma_multiply(0.45),
    );
    p.circle_stroke(
        rect.min + Vec2::splat(w * 0.28),
        w * 0.24,
        Stroke::new((w * 0.012).max(1.0), c2.gamma_multiply(0.35)),
    );
}

/// Vertical two-colour gradient (square corners, used for page headers).
pub fn gradient(p: &Painter, rect: Rect, top: Color32, bottom: Color32) {
    let mut mesh = egui::Mesh::default();
    mesh.colored_vertex(rect.left_top(), top);
    mesh.colored_vertex(rect.right_top(), top);
    mesh.colored_vertex(rect.right_bottom(), bottom);
    mesh.colored_vertex(rect.left_bottom(), bottom);
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(0, 2, 3);
    p.add(Shape::mesh(mesh));
}

pub fn chevron(p: &Painter, c: Pos2, size: f32, dir: f32, color: Color32) {
    let pts = vec![
        c + Vec2::new(-2.5 * dir, -size * 0.5),
        c + Vec2::new(2.5 * dir, 0.0),
        c + Vec2::new(-2.5 * dir, size * 0.5),
    ];
    p.add(Shape::line(pts, Stroke::new(2.0_f32, color)));
}

pub fn play_icon(p: &Painter, c: Pos2, size: f32, color: Color32) {
    let h = size * 0.5;
    let pts = vec![
        c + Vec2::new(-h * 0.65, -h),
        c + Vec2::new(-h * 0.65, h),
        c + Vec2::new(h, 0.0),
    ];
    p.add(Shape::convex_polygon(pts, color, Stroke::NONE));
}

pub fn pause_icon(p: &Painter, c: Pos2, size: f32, color: Color32) {
    let h = size * 0.5;
    let w = size * 0.22;
    rr(
        p,
        Rect::from_center_size(c + Vec2::new(-size * 0.18, 0.0), Vec2::new(w, h * 2.0)),
        0,
        color,
    );
    rr(
        p,
        Rect::from_center_size(c + Vec2::new(size * 0.18, 0.0), Vec2::new(w, h * 2.0)),
        0,
        color,
    );
}

pub fn prev_icon(p: &Painter, c: Pos2, size: f32, color: Color32) {
    let h = size * 0.5;
    rr(
        p,
        Rect::from_center_size(c + Vec2::new(-h * 0.85, 0.0), Vec2::new(2.0, h * 2.0)),
        0,
        color,
    );
    let pts = vec![
        c + Vec2::new(h * 0.7, -h),
        c + Vec2::new(h * 0.7, h),
        c + Vec2::new(-h * 0.6, 0.0),
    ];
    p.add(Shape::convex_polygon(pts, color, Stroke::NONE));
}

pub fn next_icon(p: &Painter, c: Pos2, size: f32, color: Color32) {
    let h = size * 0.5;
    rr(
        p,
        Rect::from_center_size(c + Vec2::new(h * 0.85, 0.0), Vec2::new(2.0, h * 2.0)),
        0,
        color,
    );
    let pts = vec![
        c + Vec2::new(-h * 0.7, -h),
        c + Vec2::new(-h * 0.7, h),
        c + Vec2::new(h * 0.6, 0.0),
    ];
    p.add(Shape::convex_polygon(pts, color, Stroke::NONE));
}

pub fn shuffle_icon(p: &Painter, rect: Rect, color: Color32) {
    let c = rect.center();
    let s = 8.0;
    let st = Stroke::new(1.6_f32, color);
    p.add(Shape::line(
        vec![
            c + Vec2::new(-s, -s * 0.55),
            c + Vec2::new(-s * 0.3, -s * 0.55),
            c + Vec2::new(s * 0.3, s * 0.55),
            c + Vec2::new(s, s * 0.55),
        ],
        st,
    ));
    p.add(Shape::line(
        vec![
            c + Vec2::new(-s, s * 0.55),
            c + Vec2::new(-s * 0.35, s * 0.55),
        ],
        st,
    ));
    p.add(Shape::line(
        vec![c + Vec2::new(s * 0.45, s * 0.55), c + Vec2::new(s, s * 0.55)],
        st,
    ));
    p.add(Shape::line(
        vec![
            c + Vec2::new(s * 0.6, s * 0.55 - 3.0),
            c + Vec2::new(s, s * 0.55),
            c + Vec2::new(s * 0.6, s * 0.55 + 3.0),
        ],
        st,
    ));
}

/// Pencil glyph for the player bar's "Edit album" button.
pub fn edit_icon(p: &Painter, rect: Rect, color: Color32) {
    let c = rect.center();
    let st = Stroke::new(1.5_f32, color);
    p.add(Shape::line(
        vec![c + Vec2::new(-5.0, 5.0), c + Vec2::new(4.5, -4.5)],
        st,
    ));
    p.add(Shape::line(
        vec![
            c + Vec2::new(-6.5, 6.5),
            c + Vec2::new(-5.0, 5.0),
            c + Vec2::new(-6.5, 4.5),
        ],
        st,
    ));
    p.add(Shape::line(
        vec![c + Vec2::new(3.0, -5.5), c + Vec2::new(5.5, -3.0)],
        st,
    ));
}

/// A small cross, used to take one row out of a playlist.
pub fn close_icon(p: &Painter, c: Pos2, size: f32, color: Color32) {
    let st = Stroke::new(1.6_f32, color);
    p.add(Shape::line_segment(
        [c + Vec2::new(-size, -size), c + Vec2::new(size, size)],
        st,
    ));
    p.add(Shape::line_segment(
        [c + Vec2::new(size, -size), c + Vec2::new(-size, size)],
        st,
    ));
}

/// Lines plus a plus: "add this song to a playlist".
pub fn playlist_add_icon(p: &Painter, rect: Rect, color: Color32) {
    let c = rect.center();
    let st = Stroke::new(1.5_f32, color);
    for (dy, len) in [(-5.0_f32, 7.0_f32), (0.0, 9.0), (5.0, 6.0)] {
        p.add(Shape::line_segment(
            [c + Vec2::new(-9.0, dy), c + Vec2::new(-9.0 + len, dy)],
            st,
        ));
    }
    p.add(Shape::line_segment(
        [c + Vec2::new(5.0, -5.0), c + Vec2::new(5.0, 5.0)],
        st,
    ));
    p.add(Shape::line_segment(
        [c, c + Vec2::new(10.0, 0.0)],
        st,
    ));
}

pub fn repeat_icon(p: &Painter, rect: Rect, color: Color32) {
    let c = rect.center();
    let w = 8.0;
    let h = 5.0;
    let st = Stroke::new(1.6_f32, color);
    p.add(Shape::line(
        vec![
            c + Vec2::new(-w, c.y - h),
            c + Vec2::new(w, c.y - h),
            c + Vec2::new(w, c.y + h * 0.4),
        ],
        st,
    ));
    p.add(Shape::line(
        vec![
            c + Vec2::new(w, c.y + h),
            c + Vec2::new(-w, c.y + h),
            c + Vec2::new(-w, c.y - h * 0.4),
        ],
        st,
    ));
    p.add(Shape::line(
        vec![
            c + Vec2::new(-w + 3.0, c.y - h * 0.4 - 3.0),
            c + Vec2::new(-w, c.y - h * 0.4),
            c + Vec2::new(-w + 3.0, c.y - h * 0.4 + 3.0),
        ],
        st,
    ));
}

pub fn volume_icon(p: &Painter, rect: Rect, color: Color32) {
    let c = rect.center();
    let pts = vec![
        c + Vec2::new(-7.0, -3.0),
        c + Vec2::new(-3.0, -3.0),
        c + Vec2::new(1.0, -7.0),
        c + Vec2::new(1.0, 7.0),
        c + Vec2::new(-3.0, 3.0),
        c + Vec2::new(-7.0, 3.0),
    ];
    p.add(Shape::convex_polygon(pts, color, Stroke::NONE));
    p.add(Shape::line(
        vec![
            c + Vec2::new(4.0, -4.0),
            c + Vec2::new(7.0, 0.0),
            c + Vec2::new(4.0, 4.0),
        ],
        Stroke::new(1.6_f32, color),
    ));
}

/// Screen-on-a-stand glyph for the device switcher.
pub fn device_icon(p: &Painter, rect: Rect, color: Color32) {
    let c = rect.center();
    let st = Stroke::new(1.6_f32, color);
    let screen = Rect::from_center_size(c + Vec2::new(0.0, -1.5), Vec2::new(14.0, 9.0));
    p.rect_stroke(screen, CornerRadius::same(2), st, egui::StrokeKind::Inside);
    p.add(Shape::line_segment(
        [
            Pos2::new(c.x - 3.5, c.y + 6.5),
            Pos2::new(c.x + 3.5, c.y + 6.5),
        ],
        st,
    ));
    p.add(Shape::line_segment(
        [Pos2::new(c.x, c.y + 3.0), Pos2::new(c.x, c.y + 6.5)],
        st,
    ));
}

pub fn search_icon(p: &Painter, c: Pos2, r: f32, color: Color32) {
    p.circle_stroke(c, r, Stroke::new(1.8_f32, color));
    p.add(Shape::line(
        vec![c + Vec2::new(r * 0.7, r * 0.7), c + Vec2::new(r * 1.6, r * 1.6)],
        Stroke::new(1.8_f32, color),
    ));
}

pub fn home_icon(p: &Painter, c: Pos2, color: Color32) {
    let st = Stroke::new(1.7_f32, color);
    p.add(Shape::line(
        vec![
            c + Vec2::new(-8.0, 0.0),
            c + Vec2::new(0.0, -7.0),
            c + Vec2::new(8.0, 0.0),
        ],
        st,
    ));
    p.add(Shape::line(
        vec![
            c + Vec2::new(-5.5, -1.5),
            c + Vec2::new(-5.5, 7.0),
            c + Vec2::new(5.5, 7.0),
            c + Vec2::new(5.5, -1.5),
        ],
        st,
    ));
}

pub fn import_icon(p: &Painter, c: Pos2, color: Color32) {
    let st = Stroke::new(1.7_f32, color);
    p.add(Shape::line(
        vec![
            c + Vec2::new(-8.0, 1.0),
            c + Vec2::new(-8.0, 7.0),
            c + Vec2::new(8.0, 7.0),
            c + Vec2::new(8.0, 1.0),
        ],
        st,
    ));
    p.add(Shape::line_segment(
        [c + Vec2::new(0.0, -8.0), c + Vec2::new(0.0, 2.0)],
        st,
    ));
    p.add(Shape::line(
        vec![
            c + Vec2::new(-4.0, -2.0),
            c + Vec2::new(0.0, 2.0),
            c + Vec2::new(4.0, -2.0),
        ],
        st,
    ));
}

pub fn gear_icon(p: &Painter, c: Pos2, color: Color32) {
    p.circle_stroke(c, 6.0, Stroke::new(1.6_f32, color));
    p.circle_filled(c, 2.0, color);
    for i in 0..8 {
        let a = i as f32 * std::f32::consts::TAU / 8.0;
        let d = Vec2::new(a.cos(), a.sin());
        p.add(Shape::line_segment(
            [c + d * 7.0, c + d * 9.5],
            Stroke::new(1.6_f32, color),
        ));
    }
}

pub fn queue_icon(p: &Painter, c: Pos2, color: Color32) {
    let st = Stroke::new(1.7_f32, color);
    for (i, w) in [8.0_f32, 8.0, 5.5].iter().enumerate() {
        let y = c.y - 5.0 + i as f32 * 5.0;
        p.add(Shape::line_segment(
            [Pos2::new(c.x - 8.0, y), Pos2::new(c.x - 8.0 + w, y)],
            st,
        ));
    }
    let pts = vec![
        c + Vec2::new(4.0, -6.5),
        c + Vec2::new(4.0, 0.5),
        c + Vec2::new(9.0, -3.0),
    ];
    p.add(Shape::convex_polygon(pts, color, Stroke::NONE));
}

/// Three little bars used to mark the row that is playing.
pub fn equalizer(p: &Painter, c: Pos2, color: Color32) {
    for (i, h) in [7.0, 11.0, 5.0].iter().enumerate() {
        let x = c.x - 5.0 + i as f32 * 5.0;
        rr(
            p,
            Rect::from_center_size(Pos2::new(x, c.y), Vec2::new(2.5, *h)),
            1,
            color,
        );
    }
}

fn heart_points(c: Pos2, size: f32) -> Vec<Pos2> {
    let s = size / 17.0;
    (0..44)
        .map(|i| {
            let t = i as f32 * std::f32::consts::TAU / 44.0;
            let x = 16.0 * t.sin().powi(3);
            let y = -(13.0 * t.cos() - 5.0 * (2.0 * t).cos() - 2.0 * (3.0 * t).cos() - (4.0 * t).cos());
            c + Vec2::new(x, y) * s
        })
        .collect()
}

/// A small map pin, used to mark a collection held at the top of the library.
pub fn pin(p: &Painter, c: Pos2, color: Color32) {
    let head = c + Vec2::new(0.0, -3.0);
    p.circle_stroke(head, 3.4, Stroke::new(1.5_f32, color));
    p.circle_filled(head, 1.1, color);
    p.add(Shape::line_segment(
        [c + Vec2::new(0.0, 0.4), c + Vec2::new(0.0, 5.4)],
        Stroke::new(1.5_f32, color),
    ));
}

pub fn heart(p: &Painter, c: Pos2, size: f32, filled: bool, color: Color32) {
    let s = size * 0.5;
    if filled {
        p.circle_filled(c + Vec2::new(-s * 0.42, -s * 0.34), s * 0.56, color);
        p.circle_filled(c + Vec2::new(s * 0.42, -s * 0.34), s * 0.56, color);
        p.add(Shape::convex_polygon(
            vec![
                c + Vec2::new(-s * 0.96, -s * 0.16),
                c + Vec2::new(s * 0.96, -s * 0.16),
                c + Vec2::new(0.0, s * 0.98),
            ],
            color,
            Stroke::NONE,
        ));
    } else {
        p.add(Shape::closed_line(
            heart_points(c + Vec2::new(0.0, size * 0.06), size),
            Stroke::new(1.5_f32, color),
        ));
    }
}

/// Simple proportional bar used in the taste profile.
pub fn stat_bar(p: &Painter, rect: Rect, frac: f32, bg: Color32, fg: Color32) {
    rr(p, rect, 3, bg);
    let w = (rect.width() * frac.clamp(0.0, 1.0)).max(2.0);
    rr(
        p,
        Rect::from_min_size(rect.min, Vec2::new(w, rect.height())),
        3,
        fg,
    );
}
