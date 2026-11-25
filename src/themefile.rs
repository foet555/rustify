//! Themes that live in a text file you can edit while the app is running.
//!
//! The built-in picker is fast, but a file is the only honest way to offer
//! "make it exactly mine": every colour, the corner radius and the font are
//! plain keys, and saving the file repaints the window on the next frame.
//! The parser is a small TOML subset (`key = value`, `#` hex colours, numbers
//! and strings) so the app keeps its single dependency-free reading path.
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::theme::{self, rgb, Theme};

/// Written on first run. The comments double as the documentation, because a
/// theme file nobody understands is just a config file.
pub const EXAMPLE: &str = r##"# Rustify theme file - save and the window repaints instantly.
# Colours accept "#1db954" or 0x1db954.

name = "Studio"
bg = "#121212"
sidebar = "#000000"
card = "#181818"
card_hover = "#2a2a2a"
surface = "#262626"
player = "#0b0b0b"
accent = "#1db954"
text = "#ffffff"
muted = "#b3b3b3"
faint = "#7c7c7c"
rail = "#484848"

# Optional vertical gradient painted behind every panel. Uncomment both.
# gradient_top = "#1a0a06"
# gradient_bottom = "#4a1608"

# Corner radius of panels, cards and buttons: 0 square, 4 Spotify, 12 soft.
radius = 4
# Whole-interface scale. 0.85 to 1.25, 1.0 is normal.
zoom = 1.0
# "sans" or "mono" for every label Rustify paints itself.
font = "sans"
"##;

#[derive(Clone, PartialEq, Debug)]
pub struct File {
    pub name: String,
    pub bg: u32,
    pub sidebar: u32,
    pub card: u32,
    pub card_hover: u32,
    pub surface: u32,
    pub player: u32,
    pub accent: u32,
    pub text: u32,
    pub muted: u32,
    pub faint: u32,
    pub rail: u32,
    pub gradient: Option<(u32, u32)>,
    pub radius: u8,
    pub zoom: f32,
    pub mono: bool,
}

impl Default for File {
    fn default() -> Self {
        Self {
            name: "Studio".to_string(),
            bg: 0x121212,
            sidebar: 0x000000,
            card: 0x181818,
            card_hover: 0x2a2a2a,
            surface: 0x262626,
            player: 0x0b0b0b,
            accent: 0x1db954,
            text: 0xffffff,
            muted: 0xb3b3b3,
            faint: 0x7c7c7c,
            rail: 0x484848,
            gradient: None,
            radius: 4,
            zoom: 1.0,
            mono: false,
        }
    }
}

fn color(v: &str) -> Option<u32> {
    let v = v.trim().trim_matches('"').trim_start_matches('#');
    let v = v.strip_prefix("0x").or_else(|| v.strip_prefix("0X")).unwrap_or(v);
    // Accept rrggbb and rrggbbaa: the trailing alpha byte is ignored rather
    // than rejected, so a palette copied from CSS still loads.
    match v.len() {
        6 => u32::from_str_radix(v, 16).ok(),
        8 => u32::from_str_radix(&v[..6], 16).ok(),
        _ => None,
    }
}

impl File {
    /// Parses the file. Returns the theme plus one line per key that could not
    /// be used, so the settings panel can say what it ignored instead of
    /// silently dropping a typo.
    pub fn parse(text: &str) -> (Self, Vec<String>) {
        let mut f = File::default();
        let mut warnings = Vec::new();
        for (n, raw) in text.lines().enumerate() {
            let line = raw.trim();
            if line.is_empty() || line.starts_with('#') || line.starts_with('[') {
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                warnings.push(format!("line {}: expected key = value", n + 1));
                continue;
            };
            let (key, value) = (key.trim(), value.trim());
            if value.starts_with('#') {
                // Not a colour: a trailing comment after a bare value.
                warnings.push(format!("line {}: missing value for {key}", n + 1));
                continue;
            }
            let bare = value.split_whitespace().next().unwrap_or(value);
            let mut bad = false;
            match key {
                "name" => f.name = value.trim_matches('"').to_string(),
                "bg" => set_colour(&mut f.bg, bare, &mut bad),
                "sidebar" => set_colour(&mut f.sidebar, bare, &mut bad),
                "card" => set_colour(&mut f.card, bare, &mut bad),
                "card_hover" => set_colour(&mut f.card_hover, bare, &mut bad),
                "surface" => set_colour(&mut f.surface, bare, &mut bad),
                "player" => set_colour(&mut f.player, bare, &mut bad),
                "accent" => set_colour(&mut f.accent, bare, &mut bad),
                "text" => set_colour(&mut f.text, bare, &mut bad),
                "muted" => set_colour(&mut f.muted, bare, &mut bad),
                "faint" => set_colour(&mut f.faint, bare, &mut bad),
                "rail" => set_colour(&mut f.rail, bare, &mut bad),
                "gradient_top" | "gradient_bottom" => {
                    let mut c = f.bg;
                    set_colour(&mut c, bare, &mut bad);
                    let (top, bottom) = f.gradient.unwrap_or((f.bg, f.bg));
                    f.gradient = Some(if key == "gradient_top" {
                        (c, bottom)
                    } else {
                        (top, c)
                    });
                }
                "radius" => match value.parse::<f32>() {
                    Ok(v) => f.radius = v.clamp(0.0, 24.0) as u8,
                    Err(_) => bad = true,
                },
                "zoom" => match value.parse::<f32>() {
                    Ok(v) => f.zoom = v.clamp(0.8, 1.3),
                    Err(_) => bad = true,
                },
                "font" => match value.trim_matches('"').to_ascii_lowercase().as_str() {
                    "mono" | "monospace" => f.mono = true,
                    "sans" | "proportional" => f.mono = false,
                    _ => bad = true,
                },
                _ => warnings.push(format!("line {}: unknown key {key}", n + 1)),
            }
            if bad {
                warnings.push(format!("line {}: {key} = {value} is not a valid value", n + 1));
            }
        }
        if f.gradient.is_some() && f.gradient.map(|g| g.0 == g.1).unwrap_or(false) {
            // A gradient with identical ends is just a flat fill; doing the
            // mesh work for it would only cost frames.
            f.gradient = None;
        }
        (f, warnings)
    }

    pub fn theme(&self) -> Theme {
        let name: &'static str = Box::leak(self.name.clone().into_boxed_str());
        Theme {
            name,
            bg: theme::opaque(rgb(self.bg)),
            sidebar: theme::opaque(rgb(self.sidebar)),
            card: theme::opaque(rgb(self.card)),
            card_hover: theme::opaque(rgb(self.card_hover)),
            surface: theme::opaque(rgb(self.surface)),
            player: theme::opaque(rgb(self.player)),
            accent: rgb(self.accent),
            on_accent: if theme::lum(rgb(self.accent)) > 0.55 {
                theme::rgb(0x000000)
            } else {
                theme::rgb(0xffffff)
            },
            text: rgb(self.text),
            muted: rgb(self.muted),
            faint: rgb(self.faint),
            rail: rgb(self.rail),
            gradient: self.gradient.map(|(a, b)| (rgb(a), rgb(b))),
        }
    }

    /// Radius and zoom are global, so they are applied to the UI as a whole
    /// rather than stored per theme.
    pub fn apply_globals(&self) {
        theme::set_radius_scale(self.radius as f32 / 4.0);
        theme::set_mono(self.mono);
    }

    pub fn save_to(&self, path: &Path) {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let mut out = String::new();
        out.push_str(&format!("name = \"{}\"\n", self.name));
        for (k, v) in [
            ("bg", self.bg),
            ("sidebar", self.sidebar),
            ("card", self.card),
            ("card_hover", self.card_hover),
            ("surface", self.surface),
            ("player", self.player),
            ("accent", self.accent),
            ("text", self.text),
            ("muted", self.muted),
            ("faint", self.faint),
            ("rail", self.rail),
        ] {
            out.push_str(&format!("{k} = \"#{v:06x}\"\n"));
        }
        if let Some((a, b)) = self.gradient {
            out.push_str(&format!("gradient_top = \"#{a:06x}\"\n"));
            out.push_str(&format!("gradient_bottom = \"#{b:06x}\"\n"));
        }
        out.push_str(&format!("radius = {}\nzoom = {:.2}\n", self.radius, self.zoom));
        out.push_str(&format!(
            "font = \"{}\"\n",
            if self.mono { "mono" } else { "sans" }
        ));
        let _ = std::fs::write(path, out);
    }
}

/// A bad colour keeps whatever the key already had and raises the warning
/// flag, so one typo never repaints the whole window black.
fn set_colour(slot: &mut u32, value: &str, bad: &mut bool) {
    match color(value) {
        Some(c) => *slot = c,
        None => *bad = true,
    }
}

pub fn default_path() -> PathBuf {
    crate::profile::Profile::default_path().with_file_name("theme.toml")
}

/// The file plus the small amount of bookkeeping needed to notice an edit.
pub struct Watch {
    pub path: PathBuf,
    stamp: Option<SystemTime>,
    pub file: File,
    pub warnings: Vec<String>,
    pub error: Option<String>,
}

impl Watch {
    pub fn at(path: PathBuf) -> Self {
        let mut w = Self {
            path,
            stamp: None,
            file: File::default(),
            warnings: Vec::new(),
            error: None,
        };
        // Always leave a file behind, comments and all: an empty folder
        // teaches nobody the keys.
        if !w.path.exists() {
            if let Some(dir) = w.path.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            let _ = std::fs::write(&w.path, EXAMPLE);
        }
        w.reload();
        w
    }

    /// Re-reads the file. Returns true when the theme actually changed.
    pub fn reload(&mut self) -> bool {
        let stamp = std::fs::metadata(&self.path)
            .and_then(|m| m.modified())
            .ok();
        let text = match std::fs::read_to_string(&self.path) {
            Ok(t) => t,
            Err(e) => {
                self.error = Some(format!("{}: {e}", self.path.display()));
                return false;
            }
        };
        let (file, warnings) = File::parse(&text);
        let changed = file != self.file;
        self.stamp = stamp;
        self.warnings = warnings;
        self.error = None;
        if changed {
            self.file = file;
        }
        changed
    }

    /// Cheap enough to run every frame: one stat call, not a read.
    pub fn poll(&mut self) -> bool {
        let stamp = std::fs::metadata(&self.path)
            .and_then(|m| m.modified())
            .ok();
        if stamp.is_some() && stamp != self.stamp {
            return self.reload();
        }
        false
    }

    pub fn theme(&self) -> Theme {
        self.file.theme()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_full_palette() {
        let text = r##"
name = "Test"
bg = "#101010"
sidebar = "#000000"
card = "0x181818"
card_hover = "#2a2a2a"
surface = "#262626"
player = "#0b0b0b"
accent = "#1db954"
text = "#ffffff"
muted = "#b3b3b3"
faint = "#7c7c7c"
rail = "#484848"
radius = 12
zoom = 1.2
font = "mono"
"##;
        let (f, warnings) = File::parse(text);
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(f.name, "Test");
        assert_eq!(f.bg, 0x101010);
        assert_eq!(f.card, 0x181818);
        assert_eq!(f.accent, 0x1db954);
        assert_eq!(f.radius, 12);
        assert!((f.zoom - 1.2).abs() < 1e-6);
        assert!(f.mono);
        let t = f.theme();
        assert_eq!(t.name, "Test");
        assert_eq!(t.accent, rgb(0x1db954));
        assert!(t.gradient.is_none());
    }

    #[test]
    fn gradient_needs_two_colours_and_drops_a_flat_one() {
        let (two, _) = File::parse("gradient_top = \"#100000\"\ngradient_bottom = \"#200000\"\n");
        assert_eq!(two.gradient, Some((0x100000, 0x200000)));
        let (flat, _) = File::parse("gradient_top = \"#100000\"\ngradient_bottom = \"#100000\"\n");
        assert_eq!(flat.gradient, None);
    }

    #[test]
    fn bad_values_are_reported_not_silently_dropped() {
        let (f, warnings) = File::parse("bg = \"nope\"\nradius = wide\nfont = \"cursive\"\nshade = 4\n");
        assert_eq!(f.bg, File::default().bg);
        assert_eq!(warnings.len(), 4, "{warnings:?}");
        assert!(warnings.iter().any(|w| w.contains("shade")));
    }

    #[test]
    fn trailing_comments_and_sections_are_ignored() {
        let (f, warnings) = File::parse("[colors]\n# accent = \"#ff0000\"\naccent = \"#ff0000\" # orange\n");
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(f.accent, 0xff0000);
    }

    #[test]
    fn zoom_is_clamped_to_a_usable_range() {
        assert!((File::parse("zoom = 9\n").0.zoom - 1.3).abs() < 1e-6);
        assert!((File::parse("zoom = 0.1\n").0.zoom - 0.8).abs() < 1e-6);
    }

    #[test]
    fn saved_file_reloads_identically() {
        let dir = std::env::temp_dir().join(format!("rustify-theme-{}", std::process::id()));
        let path = dir.join("theme.toml");
        let mut f = File::default();
        f.name = "Round trip".into();
        f.accent = 0xff7a00;
        f.gradient = Some((0x111111, 0x222222));
        f.radius = 10;
        f.zoom = 1.1;
        f.mono = true;
        f.save_to(&path);
        let text = std::fs::read_to_string(&path).unwrap();
        let (back, warnings) = File::parse(&text);
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(back, f);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn watch_picks_up_a_saved_edit() {
        let dir = std::env::temp_dir().join(format!("rustify-watch-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("theme.toml");
        let mut w = Watch::at(path.clone());
        assert_eq!(w.file.accent, 0x1db954);
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(&path, "accent = \"#ff7a00\"\nradius = 8\n").unwrap();
        assert!(w.poll());
        assert_eq!(w.file.accent, 0xff7a00);
        assert_eq!(w.file.radius, 8);
        // A second poll with no edit must not report a change.
        assert!(!w.poll());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
