// Persistent listening profile: play counts, genre affinity, likes, history
// and UI preferences. Stored as a small tab separated file under %APPDATA%.
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use crate::theme::Custom;

#[derive(Clone, Default, PartialEq, Debug)]
pub struct Profile {
    pub artist_plays: BTreeMap<String, u32>,
    pub track_plays: BTreeMap<String, u32>,
    pub genre_plays: BTreeMap<String, u32>,
    pub liked: BTreeSet<String>,
    pub recent: Vec<String>,
    /// Collections (albums and playlists) pinned to the top of the sidebar.
    pub pinned: Vec<String>,
    pub listened_secs: f64,
    pub plays: u32,
    pub theme: usize,
    pub volume: f32,
    /// Spotify app Client ID used by the playlist importer (public, not a secret).
    pub client_id: String,
    /// Folder of the owner's own audio files that the player can actually play.
    pub audio_dir: String,
    /// Discord application ID for rich presence (public, not a secret).
    pub discord_client_id: String,
    pub discord_enabled: bool,
    /// The owner's own palette: gradient, photo, blur and dim.
    pub custom: Custom,
}

impl Profile {
    pub fn default_path() -> PathBuf {
        let base = std::env::var("APPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("."));
        base.join("spotify-rust").join("taste.txt")
    }

    pub fn load_from(path: &Path) -> Self {
        let mut p = Profile {
            volume: 0.75,
            ..Default::default()
        };
        if let Ok(text) = std::fs::read_to_string(path) {
            p.parse(&text);
        }
        if p.volume <= 0.0 {
            p.volume = 0.75;
        }
        p
    }

    pub fn save_to(&self, path: &Path) {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let mut out = String::new();
        out.push_str(&format!(
            "theme={}\nvolume={}\nlistened={:.2}\nplays={}\n",
            self.theme, self.volume, self.listened_secs, self.plays
        ));
        out.push_str(&format!("client_id={}\n", self.client_id));
        out.push_str(&format!("audio_dir={}\n", self.audio_dir));
        out.push_str(&format!("discord_client_id={}\n", self.discord_client_id));
        out.push_str(&format!(
            "discord_enabled={}\n",
            u8::from(self.discord_enabled)
        ));
        out.push_str(&format!(
            "custom_top={}\ncustom_bottom={}\ncustom_accent={}\ncustom_text={}\ncustom_image={}\ncustom_blur={:.3}\ncustom_dim={:.3}\n",
            self.custom.top,
            self.custom.bottom,
            self.custom.accent,
            self.custom.text,
            self.custom.image,
            self.custom.blur,
            self.custom.dim
        ));
        for (k, v) in &self.artist_plays {
            out.push_str(&format!("artist\t{k}\t{v}\n"));
        }
        for (k, v) in &self.track_plays {
            out.push_str(&format!("track\t{k}\t{v}\n"));
        }
        for (k, v) in &self.genre_plays {
            out.push_str(&format!("genre\t{k}\t{v}\n"));
        }
        for k in &self.liked {
            out.push_str(&format!("liked\t{k}\n"));
        }
        for k in &self.recent {
            out.push_str(&format!("recent\t{k}\n"));
        }
        for k in &self.pinned {
            out.push_str(&format!("pin\t{k}\n"));
        }
        let _ = std::fs::write(path, out);
    }

    fn parse(&mut self, text: &str) {
        for line in text.lines() {
            let mut parts = line.split('\t');
            match parts.next().unwrap_or("") {
                "artist" => {
                    if let (Some(k), Some(v)) = (parts.next(), parts.next()) {
                        self.artist_plays
                            .insert(k.to_string(), v.parse().unwrap_or(0));
                    }
                }
                "track" => {
                    if let (Some(k), Some(v)) = (parts.next(), parts.next()) {
                        self.track_plays
                            .insert(k.to_string(), v.parse().unwrap_or(0));
                    }
                }
                "genre" => {
                    if let (Some(k), Some(v)) = (parts.next(), parts.next()) {
                        self.genre_plays
                            .insert(k.to_string(), v.parse().unwrap_or(0));
                    }
                }
                "liked" => {
                    if let Some(k) = parts.next() {
                        self.liked.insert(k.to_string());
                    }
                }
                "recent" => {
                    if let Some(k) = parts.next() {
                        if !self.recent.iter().any(|r| r.as_str() == k) {
                            self.recent.push(k.to_string());
                        }
                    }
                }
                "pin" => {
                    if let Some(k) = parts.next() {
                        if !self.pinned.iter().any(|p| p.as_str() == k) {
                            self.pinned.push(k.to_string());
                        }
                    }
                }
                _ => {
                    if let Some((k, v)) = line.split_once('=') {
                        match k {
                            "theme" => self.theme = v.parse().unwrap_or(0),
                            "volume" => self.volume = v.parse().unwrap_or(0.75),
                            "listened" => self.listened_secs = v.parse().unwrap_or(0.0),
                            "plays" => self.plays = v.parse().unwrap_or(0),
                            "client_id" => self.client_id = v.to_string(),
                            "audio_dir" => self.audio_dir = v.to_string(),
                            "discord_client_id" => self.discord_client_id = v.to_string(),
                            "discord_enabled" => {
                                self.discord_enabled = v == "1" || v.eq_ignore_ascii_case("true")
                            }
                            "custom_top" => self.custom.top = v.parse().unwrap_or(self.custom.top),
                            "custom_bottom" => {
                                self.custom.bottom = v.parse().unwrap_or(self.custom.bottom)
                            }
                            "custom_accent" => {
                                self.custom.accent = v.parse().unwrap_or(self.custom.accent)
                            }
                            "custom_text" => self.custom.text = v.parse().unwrap_or(self.custom.text),
                            "custom_image" => self.custom.image = v.to_string(),
                            "custom_blur" => {
                                self.custom.blur = v.parse().unwrap_or(self.custom.blur)
                            }
                            "custom_dim" => self.custom.dim = v.parse().unwrap_or(self.custom.dim),
                            _ => {}
                        }
                    }
                }
            }
        }
        self.recent.truncate(12);
    }

    /// Record one play in memory. Call `save` to persist.
    pub fn record_play(&mut self, artist: &str, track_key: &str, genres: &[String]) {
        *self.artist_plays.entry(artist.to_string()).or_insert(0) += 1;
        *self.track_plays.entry(track_key.to_string()).or_insert(0) += 1;
        // The first tag is the collection's primary genre, so it counts double:
        // playing a synthwave playlist should rank synthwave above the secondary
        // "electronic" tag the same playlist also carries.
        for (i, g) in genres.iter().enumerate() {
            let weight = if i == 0 { 2 } else { 1 };
            *self.genre_plays.entry(g.clone()).or_insert(0) += weight;
        }
        self.plays += 1;
        self.recent.retain(|k| k.as_str() != track_key);
        self.recent.insert(0, track_key.to_string());
        self.recent.truncate(12);
    }

    /// Flip a like in memory. Callers persist it with an explicit path.
    pub fn flip_like(&mut self, key: &str) {
        if !self.liked.remove(key) {
            self.liked.insert(key.to_string());
        }
    }

    pub fn is_liked(&self, key: &str) -> bool {
        self.liked.contains(key)
    }

    pub fn is_pinned(&self, key: &str) -> bool {
        self.pinned.iter().any(|p| p == key)
    }

    /// Pin or unpin one collection. The caller saves the profile.
    pub fn flip_pin(&mut self, key: &str) {
        match self.pinned.iter().position(|p| p == key) {
            Some(i) => {
                self.pinned.remove(i);
            }
            None => self.pinned.push(key.to_string()),
        }
    }

    /// Clear listening statistics in memory. Likes and preferences are kept.
    pub fn clear_stats(&mut self) {
        self.artist_plays.clear();
        self.track_plays.clear();
        self.genre_plays.clear();
        self.recent.clear();
        self.listened_secs = 0.0;
        self.plays = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pins_survive_a_save_and_load() {
        let mut p = Profile::default();
        p.flip_pin("album|After Hours");
        p.flip_pin("playlist|Deltabangers");
        assert!(p.is_pinned("album|After Hours"));
        p.flip_pin("album|After Hours");
        assert!(!p.is_pinned("album|After Hours"), "flipping twice unpins");

        let dir = std::env::temp_dir().join(format!("rustify-pin-test-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("taste.txt");
        p.save_to(&path);
        let back = Profile::load_from(&path);
        assert_eq!(back.pinned, vec!["playlist|Deltabangers".to_string()]);
        assert!(!back.is_pinned("album|After Hours"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
