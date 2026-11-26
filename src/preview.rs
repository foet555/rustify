//! Free 30-second previews for catalogue rows that are not on this PC.
//!
//! Spotify's Web API can only drive a Spotify client that is already
//! streaming, so a row with no file used to need the Spotify app (and, for
//! playback control, Premium). These clips come from the public search APIs
//! Deezer and iTunes publish for previews: no account, no app, nothing ripped
//! or re-hosted. A clip is the official 30-second preview, not the full song.
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{channel, Receiver, Sender};

use serde_json::Value;

use crate::audio;

/// Deezer and iTunes both publish 30-second clips; the label says so.
pub const CLIP_SECS: u32 = 30;

/// The one thing the resolver needs from the network, so tests can inject a
/// canned catalogue instead of calling out.
pub trait Http: Send + Sync {
    fn get(&self, url: &str) -> Result<String, String>;
    fn bytes(&self, url: &str) -> Result<Vec<u8>, String>;
}

pub struct Curl;

impl Http for Curl {
    fn get(&self, url: &str) -> Result<String, String> {
        let config = format!(
            "url = \"{}\"\nheader = \"Accept: application/json\"\nheader = \"User-Agent: Mozilla/5.0 (Windows NT 10.0; Win64; x64) Rustify\"\n",
            crate::spotify::curl_escape(url)
        );
        let response = crate::spotify::run_curl(&config)?;
        if !response.ok() {
            return Err(format!("preview search returned HTTP {}", response.status));
        }
        Ok(response.body)
    }

    fn bytes(&self, url: &str) -> Result<Vec<u8>, String> {
        crate::spotify::fetch_bytes(url)
    }
}

/// A downloaded preview for one track. `path` is inside the cache, so playing
/// the same track again does not touch the network.
#[derive(Clone)]
pub struct Clip {
    pub path: PathBuf,
    pub title: String,
}

/// Where clips are kept between plays.
pub fn cache_dir() -> PathBuf {
    std::env::var("APPDATA")
        .map(|app| PathBuf::from(app).join("spotify-rust").join("previews"))
        .unwrap_or_else(|_| std::env::temp_dir().join("rustify-previews"))
}

/// Resolve a track name to an official preview, downloading it once. Deezer's
/// catalogue is searched first because its previews are plain MP3; iTunes is
/// the fallback when Deezer has no match or no preview URL.
pub fn resolve_with(
    http: &dyn Http,
    dir: &Path,
    artist: &str,
    title: &str,
) -> Result<Clip, String> {
    let stem = cache_stem(artist, title);
    if let Some(path) = cached(dir, &stem) {
        return Ok(Clip {
            path,
            title: title.to_string(),
        });
    }
    std::fs::create_dir_all(dir)
        .map_err(|e| format!("the preview cache could not be created: {e}"))?;

    let mut reason = String::new();
    for (source, url) in [
        ("Deezer", deezer_url(artist, title)),
        ("iTunes", itunes_url(artist, title)),
    ] {
        let body = match http.get(&url) {
            Ok(body) => body,
            Err(e) => {
                reason = format!("{source}: {e}");
                continue;
            }
        };
        let found = if source == "Deezer" {
            parse_deezer(&body, artist, title)
        } else {
            parse_itunes(&body, artist, title)
        };
        let Some(found) = found else {
            reason = format!("{source} had no match");
            continue;
        };
        let bytes = match http.bytes(&found.url) {
            Ok(bytes) if !bytes.is_empty() => bytes,
            Ok(_) => {
                reason = format!("{source} sent an empty clip");
                continue;
            }
            Err(e) => {
                reason = format!("{source}: {e}");
                continue;
            }
        };
        let path = dir.join(format!("{stem}.{}", clip_ext(&found.url)));
        let part = dir.join(format!("{stem}.part"));
        std::fs::write(&part, &bytes)
            .map_err(|e| format!("the preview could not be saved: {e}"))?;
        // Windows rename refuses to replace an existing file; a zero-length
        // leftover from an interrupted download must not block the clip.
        let _ = std::fs::remove_file(&path);
        std::fs::rename(&part, &path).map_err(|e| {
            let _ = std::fs::remove_file(&part);
            format!("the preview could not be saved: {e}")
        })?;
        return Ok(Clip {
            path,
            title: found.title,
        });
    }
    Err(format!("no preview found on Deezer or iTunes ({reason})"))
}

struct Found {
    url: String,
    title: String,
}

fn parse_deezer(body: &str, artist: &str, title: &str) -> Option<Found> {
    let value: Value = serde_json::from_str(body).ok()?;
    let list = value.get("data")?.as_array()?;
    let mut best: Option<(u8, Found)> = None;
    for item in list {
        let url = item.get("preview").and_then(Value::as_str).unwrap_or("");
        if url.trim().is_empty() {
            continue;
        }
        let got_title = item.get("title").and_then(Value::as_str).unwrap_or("");
        let got_artist = item
            .pointer("/artist/name")
            .and_then(Value::as_str)
            .unwrap_or("");
        let Some(score) = likeness(artist, title, got_artist, got_title) else {
            continue;
        };
        if best.as_ref().map(|(s, _)| score > *s).unwrap_or(true) {
            best = Some((
                score,
                Found {
                    url: url.to_string(),
                    title: got_title.to_string(),
                },
            ));
        }
    }
    best.map(|(_, found)| found)
}

fn parse_itunes(body: &str, artist: &str, title: &str) -> Option<Found> {
    let value: Value = serde_json::from_str(body).ok()?;
    let list = value.get("results")?.as_array()?;
    let mut best: Option<(u8, Found)> = None;
    for item in list {
        let url = item.get("previewUrl").and_then(Value::as_str).unwrap_or("");
        if url.trim().is_empty() {
            continue;
        }
        let got_title = item.get("trackName").and_then(Value::as_str).unwrap_or("");
        let got_artist = item.get("artistName").and_then(Value::as_str).unwrap_or("");
        let Some(score) = likeness(artist, title, got_artist, got_title) else {
            continue;
        };
        if best.as_ref().map(|(s, _)| score > *s).unwrap_or(true) {
            best = Some((
                score,
                Found {
                    url: url.to_string(),
                    title: got_title.to_string(),
                },
            ));
        }
    }
    best.map(|(_, found)| found)
}

/// How well a catalogue result answers the click. `None` means "a different
/// song": a preview must never be played just because one word overlaps.
fn likeness(want_artist: &str, want_title: &str, got_artist: &str, got_title: &str) -> Option<u8> {
    let want = core_title(want_title);
    let got = core_title(got_title);
    if want.is_empty() || got.is_empty() {
        return None;
    }
    let title_points = if want == got {
        2
    } else if want.len() >= 5 && (got.contains(&want) || want.contains(&got)) {
        1
    } else {
        return None;
    };
    let want_artist = audio::normalize(want_artist);
    let got_artist = audio::normalize(got_artist);
    let artist_points = if want_artist.is_empty() {
        0
    } else if want_artist == got_artist
        || got_artist.contains(&want_artist)
        || want_artist.contains(&got_artist)
    {
        1
    } else {
        // One distinctive word is enough: "Deltarune OST" still matches
        // "Deltarune", while "Some Covers Band" does not.
        let shared = want_artist
            .split(' ')
            .filter(|word| word.len() >= 4)
            .any(|word| got_artist.split(' ').any(|part| part == word));
        if !shared {
            return None;
        }
        0
    };
    Some(title_points * 2 + artist_points)
}

/// Lowercase title without bracketed asides or trailing edition labels, so
/// "Sanctuary (from Deltarune) - Remastered" and "Sanctuary" compare equal.
fn core_title(s: &str) -> String {
    let mut kept = String::new();
    let mut depth = 0u32;
    for ch in s.chars() {
        match ch {
            '(' | '[' => depth += 1,
            ')' | ']' => depth = depth.saturating_sub(1),
            _ if depth == 0 => kept.push(ch),
            _ => {}
        }
    }
    let mut out = audio::normalize(&kept);
    for tail in [
        " remastered",
        " remaster",
        " radio edit",
        " album version",
        " single version",
    ] {
        if let Some(short) = out.strip_suffix(tail) {
            out = short.trim_end().to_string();
        }
    }
    out
}

fn deezer_url(artist: &str, title: &str) -> String {
    let query = format!("track:\"{}\" artist:\"{}\"", title.trim(), artist.trim());
    format!(
        "https://api.deezer.com/search?q={}&limit=10",
        crate::catalog::encode_query(&query)
    )
}

fn itunes_url(artist: &str, title: &str) -> String {
    let query = format!("{} {}", title.trim(), artist.trim());
    format!(
        "https://itunes.apple.com/search?term={}&media=music&entity=song&limit=10",
        crate::catalog::encode_query(&query)
    )
}

/// Stable, filesystem-safe name for one track's clip. A plain hash keeps names
/// short while preventing two different songs from sharing a file.
fn cache_stem(artist: &str, title: &str) -> String {
    let label = audio::normalize(&format!("{artist} {title}"));
    let mut words = String::new();
    for word in label.split(' ').take(8) {
        if !words.is_empty() {
            words.push('-');
        }
        words.push_str(word);
    }
    if words.len() > 48 {
        words.truncate(48);
        words = words.trim_end_matches('-').to_string();
    }
    format!("{words}-{:016x}", stamp(artist, title))
}

/// FNV-1a, so the cache name does not depend on the standard library's
/// hasher keys and survives a rebuild.
fn stamp(artist: &str, title: &str) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in artist
        .bytes()
        .chain(std::iter::once(0u8))
        .chain(title.bytes())
    {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

fn cached(dir: &Path, stem: &str) -> Option<PathBuf> {
    for ext in ["mp3", "m4a", "aac", "ogg"] {
        let path = dir.join(format!("{stem}.{ext}"));
        if path.metadata().map(|m| m.len() > 0).unwrap_or(false) {
            return Some(path);
        }
    }
    None
}

fn clip_ext(url: &str) -> &'static str {
    let path = url.split(['?', '#']).next().unwrap_or(url);
    match path.rsplit('.').next().unwrap_or("") {
        ext if ext.eq_ignore_ascii_case("m4a") => "m4a",
        ext if ext.eq_ignore_ascii_case("aac") => "aac",
        ext if ext.eq_ignore_ascii_case("ogg") => "ogg",
        _ => "mp3",
    }
}

/// One preview download at a time is enough: clicks are serial anyway, and a
/// newer click simply replaces the key the result is checked against.
pub struct Queue {
    http: Arc<dyn Http>,
    dir: PathBuf,
    tx: Sender<(String, Result<Clip, String>)>,
    rx: Receiver<(String, Result<Clip, String>)>,
    pending: Option<String>,
    enabled: bool,
}

impl Queue {
    pub fn new() -> Self {
        Self::with(Arc::new(Curl), cache_dir(), true)
    }

    /// A queue with an explicit cache directory. The mobile build keeps its
    /// clips in the app's private files so Android can clean them up with the
    /// rest of the app data.
    pub fn with_cache(dir: PathBuf) -> Self {
        Self::with(Arc::new(Curl), dir, true)
    }

    /// Tests drive clicks without touching the network.
    #[cfg(test)]
    pub fn disabled() -> Self {
        Self::with(Arc::new(Curl), cache_dir(), false)
    }

    fn with(http: Arc<dyn Http>, dir: PathBuf, enabled: bool) -> Self {
        let (tx, rx) = channel();
        Self {
            http,
            dir,
            tx,
            rx,
            pending: None,
            enabled,
        }
    }

    /// Start resolving this track unless it is already in flight. Returns true
    /// when a download was actually started.
    pub fn request(&mut self, key: &str, artist: &str, title: &str) -> bool {
        if !self.enabled || self.pending.as_deref() == Some(key) {
            return false;
        }
        self.pending = Some(key.to_string());
        let tx = self.tx.clone();
        let http = self.http.clone();
        let dir = self.dir.clone();
        let key = key.to_string();
        let artist = artist.to_string();
        let title = title.to_string();
        std::thread::spawn(move || {
            let result = resolve_with(&*http, &dir, &artist, &title);
            let _ = tx.send((key, result));
        });
        true
    }

    pub fn pump(&mut self) -> Option<(String, Result<Clip, String>)> {
        let (key, result) = self.rx.try_recv().ok()?;
        if self.pending.as_deref() == Some(key.as_str()) {
            self.pending = None;
        }
        Some((key, result))
    }

    pub fn pending(&self) -> Option<&str> {
        self.pending.as_deref()
    }

    pub fn cancel(&mut self) {
        self.pending = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    struct Fake {
        deezer: String,
        itunes: String,
        calls: Mutex<Vec<String>>,
    }

    impl Fake {
        fn new(deezer: &str, itunes: &str) -> Self {
            Self {
                deezer: deezer.to_string(),
                itunes: itunes.to_string(),
                calls: Mutex::new(Vec::new()),
            }
        }

        fn calls(&self) -> Vec<String> {
            self.calls.lock().unwrap().clone()
        }
    }

    impl Http for Fake {
        fn get(&self, url: &str) -> Result<String, String> {
            self.calls.lock().unwrap().push(url.to_string());
            if url.contains("deezer") {
                Ok(self.deezer.clone())
            } else {
                Ok(self.itunes.clone())
            }
        }

        fn bytes(&self, url: &str) -> Result<Vec<u8>, String> {
            self.calls.lock().unwrap().push(url.to_string());
            Ok(b"clip-bytes".to_vec())
        }
    }

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "rustify-preview-{tag}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn deezer_picks_the_right_artist_and_skips_a_cover() {
        let fake = Fake::new(
            r#"{"data":[
                {"title":"Sanctuary","artist":{"name":"Some Covers Band"},"preview":"https://cdn.example/wrong.mp3"},
                {"title":"Sanctuary (from Deltarune)","artist":{"name":"Deltarune"},"preview":"https://cdn.example/right.mp3"}
            ]}"#,
            "{}",
        );
        let dir = temp_dir("deezer");
        let clip = resolve_with(&fake, &dir, "Deltarune", "Sanctuary").expect("a clip");
        assert_eq!(std::fs::read(&clip.path).unwrap(), b"clip-bytes");
        let calls = fake.calls();
        assert!(calls.iter().any(|u| u == "https://cdn.example/right.mp3"));
        assert!(!calls.iter().any(|u| u == "https://cdn.example/wrong.mp3"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn itunes_is_the_fallback_when_deezer_has_no_preview() {
        let fake = Fake::new(
            r#"{"data":[]}"#,
            r#"{"results":[{"trackName":"Sanctuary","artistName":"Deltarune","previewUrl":"https://audio.example/clip.m4a"}]}"#,
        );
        let dir = temp_dir("itunes");
        let clip = resolve_with(&fake, &dir, "Deltarune", "Sanctuary").expect("fallback clip");
        assert_eq!(clip.path.extension().and_then(|e| e.to_str()), Some("m4a"));
        assert!(clip.path.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_second_play_reuses_the_cached_clip_without_the_network() {
        let dir = temp_dir("cache");
        let first = Fake::new(
            r#"{"data":[{"title":"Sanctuary","artist":{"name":"Deltarune"},"preview":"https://cdn.example/one.mp3"}]}"#,
            "{}",
        );
        let one = resolve_with(&first, &dir, "Deltarune", "Sanctuary").expect("first");
        let second = Fake::new("{}", "{}");
        let two = resolve_with(&second, &dir, "Deltarune", "Sanctuary").expect("cached");
        assert_eq!(one.path, two.path);
        assert!(
            second.calls().is_empty(),
            "the cache must answer without HTTP"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn no_match_is_an_error_not_a_wrong_song() {
        let fake = Fake::new(
            r#"{"data":[{"title":"Sanctuary","artist":{"name":"Some Covers Band"},"preview":"https://cdn.example/wrong.mp3"}]}"#,
            r#"{"results":[{"trackName":"Sanctuary","artistName":"Some Covers Band","previewUrl":"https://audio.example/wrong.m4a"}]}"#,
        );
        let dir = temp_dir("nomatch");
        assert!(resolve_with(&fake, &dir, "Deltarune", "Sanctuary").is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn editions_do_not_hide_a_match() {
        assert_eq!(
            core_title("Sanctuary (from Deltarune) - Remastered"),
            "sanctuary"
        );
        assert!(likeness("Deltarune OST", "Sanctuary", "Deltarune", "Sanctuary").is_some());
        assert!(likeness("Deltarune", "Sanctuary", "Some Covers Band", "Sanctuary").is_none());
    }
}
