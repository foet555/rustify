// Persistent listening profile: play counts, genre affinity, likes, history
// and UI preferences. Stored as a small tab separated file under %APPDATA%.
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use crate::theme::Custom;

/// How a catalogue click chooses its audio source. `Auto` keeps the app's
/// original behaviour, `Full` refuses previews, and `Preview` never hands a
/// click to Spotify even when the in-app player is ready.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum StreamMode {
    #[default]
    Auto,
    Full,
    Preview,
}

impl StreamMode {
    pub fn key(self) -> &'static str {
        match self {
            StreamMode::Auto => "auto",
            StreamMode::Full => "full",
            StreamMode::Preview => "preview",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            StreamMode::Auto => "Auto",
            StreamMode::Full => "Full songs only",
            StreamMode::Preview => "Previews only",
        }
    }

    pub fn from_key(key: &str) -> Self {
        match key.trim().to_ascii_lowercase().as_str() {
            "full" => StreamMode::Full,
            "preview" => StreamMode::Preview,
            _ => StreamMode::Auto,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum EqPreset {
    #[default]
    Flat,
    Bass,
    Vocal,
    Treble,
    Loudness,
    Podcast,
}

impl EqPreset {
    pub const ALL: [EqPreset; 6] = [
        EqPreset::Flat,
        EqPreset::Bass,
        EqPreset::Vocal,
        EqPreset::Treble,
        EqPreset::Loudness,
        EqPreset::Podcast,
    ];

    pub fn label(self) -> &'static str {
        match self {
            EqPreset::Flat => "Flat",
            EqPreset::Bass => "Bass boost",
            EqPreset::Vocal => "Vocal",
            EqPreset::Treble => "Treble boost",
            EqPreset::Loudness => "Loudness",
            EqPreset::Podcast => "Podcast",
        }
    }

    /// The five band gains in dB, low to high.
    pub fn gains(self) -> [f32; 5] {
        match self {
            EqPreset::Flat => [0.0, 0.0, 0.0, 0.0, 0.0],
            EqPreset::Bass => [6.0, 4.0, 0.0, 0.0, 0.0],
            EqPreset::Vocal => [-2.0, 0.0, 3.0, 4.0, 1.0],
            EqPreset::Treble => [0.0, 0.0, 0.0, 3.0, 6.0],
            EqPreset::Loudness => [5.0, 2.0, -1.0, 2.0, 4.0],
            EqPreset::Podcast => [-4.0, 0.0, 4.0, 3.0, -2.0],
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            EqPreset::Flat => "flat",
            EqPreset::Bass => "bass",
            EqPreset::Vocal => "vocal",
            EqPreset::Treble => "treble",
            EqPreset::Loudness => "loudness",
            EqPreset::Podcast => "podcast",
        }
    }

    pub fn from_key(key: &str) -> Self {
        match key.trim().to_ascii_lowercase().as_str() {
            "bass" => EqPreset::Bass,
            "vocal" => EqPreset::Vocal,
            "treble" => EqPreset::Treble,
            "loudness" => EqPreset::Loudness,
            "podcast" => EqPreset::Podcast,
            _ => EqPreset::Flat,
        }
    }
}

/// Spacing scale for the custom density option. Cozy is the original layout.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Density {
    Compact,
    #[default]
    Cozy,
    Spacious,
}

impl Density {
    pub const ALL: [Density; 3] = [Density::Compact, Density::Cozy, Density::Spacious];

    pub fn label(self) -> &'static str {
        match self {
            Density::Compact => "Compact",
            Density::Cozy => "Cozy",
            Density::Spacious => "Spacious",
        }
    }

    pub fn factor(self) -> f32 {
        match self {
            Density::Compact => 0.88,
            Density::Cozy => 1.0,
            Density::Spacious => 1.12,
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            Density::Compact => "compact",
            Density::Cozy => "cozy",
            Density::Spacious => "spacious",
        }
    }

    pub fn from_key(key: &str) -> Self {
        match key.trim().to_ascii_lowercase().as_str() {
            "compact" => Density::Compact,
            "spacious" => Density::Spacious,
            _ => Density::Cozy,
        }
    }
}

#[derive(Clone, PartialEq, Debug)]
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
    /// Display names the owner gave collections, keyed by `kind|original name`.
    pub names: BTreeMap<String, String>,
    /// Blend the end of one track into the start of the next. Off by default.
    pub crossfade: bool,
    /// How many seconds the blend lasts, when it is on.
    pub crossfade_secs: f32,
    /// Rise from silence at the start of a track. On by default.
    pub fade_in: bool,
    /// Keep playing when a song ends. On by default: the next song in the
    /// open list plays in order, then the taste mixes carry on.
    pub autoplay: bool,
    /// Play the logo animation on launch. On by default.
    pub intro: bool,
    /// Put the current song in the window title. On by default.
    pub title_track: bool,
    /// Stop recording plays and listening time. Off by default.
    pub private_session: bool,
    /// Show the extra transport buttons next to Shuffle. On by default.
    pub player_buttons: bool,
    /// Which stream path a catalogue click takes: Auto, full songs or previews.
    pub stream_mode: StreamMode,
    /// Apply a fixed gain so quiet files and loud previews sit closer together.
    pub normalize: bool,
    /// Target gain in dB, applied when `normalize` is on.
    pub normalize_db: f32,
    /// Hide the right-hand Now Playing panel. Custom, not a Spotify option.
    pub hide_now_playing: bool,
    /// Pre-decode the next local file and append it to the same sink, so the
    /// join has no silence between tracks.
    pub gapless: bool,
    /// Bypass keeps the equalizer out of the signal path while the chosen
    /// preset is remembered for later.
    pub eq_bypass: bool,
    pub eq_preset: EqPreset,
    /// A colour that overrides the active theme's accent. None follows the
    /// theme.
    pub accent: Option<u32>,
    /// Rustify-only spacing scale: Compact, Cozy or Spacious.
    pub density: Density,
    /// Put the last track back, paused at its old position, on launch.
    pub restore_last: bool,
    /// The custom sleep timer: fade to silence before pausing.
    pub sleep_fade: bool,
    /// Where playback stopped, used by `restore_last`.
    pub last_collection: String,
    pub last_track: String,
    pub last_pos: f32,
}

impl Default for Profile {
    fn default() -> Self {
        Self {
            artist_plays: BTreeMap::new(),
            track_plays: BTreeMap::new(),
            genre_plays: BTreeMap::new(),
            liked: BTreeSet::new(),
            recent: Vec::new(),
            pinned: Vec::new(),
            listened_secs: 0.0,
            plays: 0,
            theme: 0,
            volume: 0.75,
            client_id: String::new(),
            audio_dir: String::new(),
            discord_client_id: String::new(),
            discord_enabled: false,
            custom: Custom::default(),
            names: BTreeMap::new(),
            crossfade: false,
            crossfade_secs: 4.0,
            fade_in: true,
            autoplay: true,
            intro: true,
            title_track: true,
            private_session: false,
            player_buttons: true,
            stream_mode: StreamMode::Auto,
            normalize: false,
            normalize_db: -6.0,
            hide_now_playing: false,
            gapless: false,
            eq_bypass: true,
            eq_preset: EqPreset::Flat,
            accent: None,
            density: Density::Cozy,
            restore_last: true,
            sleep_fade: false,
            last_collection: String::new(),
            last_track: String::new(),
            last_pos: 0.0,
        }
    }
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
        if p.crossfade_secs <= 0.0 {
            p.crossfade_secs = 4.0;
        }
        p.crossfade_secs = p.crossfade_secs.clamp(1.0, 10.0);
        // A profile written before normalization existed keeps the -6 dB
        // default; a corrupt value is clamped instead of silencing playback.
        p.normalize_db = p.normalize_db.clamp(-12.0, 0.0);
        p.last_pos = p.last_pos.max(0.0);
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
            "custom_top={}\ncustom_bottom={}\ncustom_accent={}\ncustom_text={}\ncustom_image={}\ncustom_blur={:.3}\ncustom_dim={:.3}\ncustom_wash={:.3}\n",
            self.custom.top,
            self.custom.bottom,
            self.custom.accent,
            self.custom.text,
            self.custom.image,
            self.custom.blur,
            self.custom.dim,
            self.custom.wash
        ));
        out.push_str(&format!(
            "crossfade={}\ncrossfade_secs={:.2}\nfade_in={}\nautoplay={}\nautoplay_migrated=1\nintro={}\ntitle_track={}\nprivate_session={}\nplayer_buttons={}\nstream_mode={}\nnormalize={}\nnormalize_db={:.2}\nhide_now_playing={}\n",
            u8::from(self.crossfade),
            self.crossfade_secs,
            u8::from(self.fade_in),
            u8::from(self.autoplay),
            u8::from(self.intro),
            u8::from(self.title_track),
            u8::from(self.private_session),
            u8::from(self.player_buttons),
            self.stream_mode.key(),
            u8::from(self.normalize),
            self.normalize_db,
            u8::from(self.hide_now_playing)
        ));
        out.push_str(&format!(
            "gapless={}\neq_bypass={}\neq_preset={}\naccent={}\ndensity={}\nrestore_last={}\nsleep_fade={}\nlast_collection={}\nlast_track={}\nlast_pos={:.2}\n",
            u8::from(self.gapless),
            u8::from(self.eq_bypass),
            self.eq_preset.key(),
            self.accent
                .map(|c| format!("{c:06x}"))
                .unwrap_or_else(|| "default".to_string()),
            self.density.key(),
            u8::from(self.restore_last),
            u8::from(self.sleep_fade),
            self.last_collection,
            self.last_track,
            self.last_pos
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
        for (k, v) in &self.names {
            out.push_str(&format!("name\t{k}\t{v}\n"));
        }
        let _ = std::fs::write(path, out);
    }

    fn parse(&mut self, text: &str) {
        // Profiles written before ordered autoplay existed always said
        // `autoplay=0`, because that was the old default. Turning it on once
        // for those files gives the new behaviour without ignoring a later
        // deliberate switch-off: the first save writes the marker below.
        let mut autoplay_migrated = false;
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
                "name" => {
                    if let (Some(k), Some(v)) = (parts.next(), parts.next()) {
                        self.names.insert(k.to_string(), v.to_string());
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
                            "custom_wash" => {
                                self.custom.wash = v.parse().unwrap_or(self.custom.wash)
                            }
                            "crossfade" => {
                                self.crossfade = v == "1" || v.eq_ignore_ascii_case("true")
                            }
                            "crossfade_secs" => {
                                self.crossfade_secs = v.parse().unwrap_or(self.crossfade_secs)
                            }
                            "fade_in" => {
                                self.fade_in = v == "1" || v.eq_ignore_ascii_case("true")
                            }
                            "autoplay" => {
                                self.autoplay = v == "1" || v.eq_ignore_ascii_case("true")
                            }
                            "autoplay_migrated" => autoplay_migrated = true,
                            "intro" => self.intro = v == "1" || v.eq_ignore_ascii_case("true"),
                            "title_track" => {
                                self.title_track = v == "1" || v.eq_ignore_ascii_case("true")
                            }
                            "private_session" => {
                                self.private_session = v == "1" || v.eq_ignore_ascii_case("true")
                            }
                            "player_buttons" => {
                                self.player_buttons = v == "1" || v.eq_ignore_ascii_case("true")
                            }
                            "stream_mode" => self.stream_mode = StreamMode::from_key(v),
                            "normalize" => {
                                self.normalize = v == "1" || v.eq_ignore_ascii_case("true")
                            }
                            "normalize_db" => {
                                self.normalize_db = v.parse().unwrap_or(self.normalize_db)
                            }
                            "hide_now_playing" => {
                                self.hide_now_playing =
                                    v == "1" || v.eq_ignore_ascii_case("true")
                            }
                            "gapless" => {
                                self.gapless = v == "1" || v.eq_ignore_ascii_case("true")
                            }
                            "eq_bypass" => {
                                self.eq_bypass = v == "1" || v.eq_ignore_ascii_case("true")
                            }
                            "eq_preset" => self.eq_preset = EqPreset::from_key(v),
                            "accent" => {
                                let v = v.trim().trim_start_matches('#');
                                self.accent = if v.eq_ignore_ascii_case("default")
                                    || v.is_empty()
                                {
                                    None
                                } else {
                                    u32::from_str_radix(v, 16).ok()
                                }
                            }
                            "density" => self.density = Density::from_key(v),
                            "restore_last" => {
                                self.restore_last = v == "1" || v.eq_ignore_ascii_case("true")
                            }
                            "sleep_fade" => {
                                self.sleep_fade = v == "1" || v.eq_ignore_ascii_case("true")
                            }
                            "last_collection" => self.last_collection = v.to_string(),
                            "last_track" => self.last_track = v.to_string(),
                            "last_pos" => self.last_pos = v.parse().unwrap_or(0.0),
                            _ => {}
                        }
                    }
                }
            }
        }
        self.recent.truncate(12);
        if !autoplay_migrated {
            self.autoplay = true;
        }
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

    /// Linear gain applied to local files and previews. A fixed target, not a
    /// per-track loudness analysis: the app has no ReplayGain metadata to read.
    pub fn normalization_gain(&self) -> f32 {
        if self.normalize {
            10f32.powf(self.normalize_db / 20.0)
        } else {
            1.0
        }
    }

    /// The equalizer preset's five gains, already zeroed while bypassed.
    pub fn eq_gains(&self) -> [f32; 5] {
        if self.eq_bypass {
            [0.0; 5]
        } else {
            self.eq_preset.gains()
        }
    }

    /// Drop pins and display names that point at collections this build no
    /// longer ships. Anything still present - the seeded playlist, imported
    /// playlists the owner edited - keeps its saved state. Returns true when
    /// something was removed, so the caller can persist the pruned profile.
    pub fn prune_missing_collections(&mut self, present: &[String]) -> bool {
        let before_pins = self.pinned.len();
        let before_names = self.names.len();
        self.pinned.retain(|key| present.iter().any(|p| p == key));
        self.names.retain(|key, _| present.iter().any(|p| p == key));
        before_pins != self.pinned.len() || before_names != self.names.len()
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
    fn crossfade_is_off_by_default_and_survives_a_save() {
        let fresh = Profile::load_from(std::path::Path::new("this-file-does-not-exist"));
        assert!(!fresh.crossfade, "crossfade must start switched off");
        assert_eq!(fresh.crossfade_secs, 4.0);

        let mut p = Profile::default();
        p.crossfade = true;
        p.crossfade_secs = 6.5;
        p.custom.wash = 0.8;

        let dir = std::env::temp_dir().join(format!("rustify-fade-test-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("taste.txt");
        p.save_to(&path);
        let back = Profile::load_from(&path);
        assert!(back.crossfade);
        assert_eq!(back.crossfade_secs, 6.5);
        assert!((back.custom.wash - 0.8).abs() < 0.001);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn playback_appearance_and_privacy_flags_survive_a_save() {
        let fresh = Profile::default();
        assert!(fresh.fade_in, "tracks should fade in by default");
        assert!(fresh.autoplay, "autoplay starts on so music keeps going");
        assert!(fresh.intro, "the animated intro is on by default");
        assert!(fresh.title_track, "the window title follows the song by default");
        assert!(!fresh.private_session, "history is recorded by default");
        assert!(fresh.player_buttons, "the extra player buttons start visible");

        let mut p = Profile::default();
        p.fade_in = false;
        p.player_buttons = false;
        p.autoplay = true;
        p.intro = false;
        p.title_track = false;
        p.private_session = true;

        let dir = std::env::temp_dir().join(format!("rustify-flags-test-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("taste.txt");
        p.save_to(&path);
        let back = Profile::load_from(&path);
        assert!(!back.fade_in);
        assert!(back.autoplay);
        assert!(!back.intro);
        assert!(!back.title_track);
        assert!(back.private_session);
        assert!(!back.player_buttons);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn removed_builtins_lose_their_pins_but_user_playlists_keep_theirs() {
        let mut p = Profile::default();
        p.flip_pin("playlist|Pop Right Now");
        p.flip_pin("playlist|deltsbangers");
        p.flip_pin("playlist|My Own Mix");
        p.names
            .insert("playlist|Pop Right Now".to_string(), "Old Pop".to_string());
        p.names
            .insert("playlist|deltsbangers".to_string(), "Delts".to_string());
        p.names
            .insert("playlist|My Own Mix".to_string(), "Mine".to_string());

        let present = vec![
            "playlist|deltsbangers".to_string(),
            "playlist|My Own Mix".to_string(),
        ];
        assert!(p.prune_missing_collections(&present));
        assert_eq!(
            p.pinned,
            vec!["playlist|deltsbangers".to_string(), "playlist|My Own Mix".to_string()]
        );
        assert_eq!(
            p.names.get("playlist|deltsbangers").map(String::as_str),
            Some("Delts")
        );
        assert_eq!(
            p.names.get("playlist|My Own Mix").map(String::as_str),
            Some("Mine")
        );
        assert!(!p.names.contains_key("playlist|Pop Right Now"));
        assert!(!p.is_pinned("playlist|Pop Right Now"));
        assert!(!p.prune_missing_collections(&present), "a second pass changes nothing");
    }

    #[test]
    fn the_equalizer_accents_density_and_startup_state_round_trip() {
        let mut p = Profile::default();
        assert!(p.eq_bypass, "the equalizer starts bypassed");
        assert_eq!(p.eq_preset, EqPreset::Flat);
        assert_eq!(p.accent, None);
        assert_eq!(p.density, Density::Cozy);
        assert!(p.restore_last);
        assert!(!p.sleep_fade);
        assert!(!p.gapless);

        p.gapless = true;
        p.eq_bypass = false;
        p.eq_preset = EqPreset::Bass;
        p.accent = Some(0x5aa0ff);
        p.density = Density::Compact;
        p.restore_last = false;
        p.sleep_fade = true;
        p.last_collection = "playlist|deltsbangers".to_string();
        p.last_track = "Deltarune|Sanctuary".to_string();
        p.last_pos = 42.5;

        let dir = std::env::temp_dir().join(format!("rustify-new-settings-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("taste.txt");
        p.save_to(&path);
        let back = Profile::load_from(&path);
        assert!(back.gapless);
        assert!(!back.eq_bypass);
        assert_eq!(back.eq_preset, EqPreset::Bass);
        assert_eq!(back.accent, Some(0x5aa0ff));
        assert_eq!(back.density, Density::Compact);
        assert!(!back.restore_last);
        assert!(back.sleep_fade);
        assert_eq!(back.last_collection, "playlist|deltsbangers");
        assert_eq!(back.last_track, "Deltarune|Sanctuary");
        assert!((back.last_pos - 42.5).abs() < 0.001);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn equalizer_presets_change_the_right_bands_and_bypass_is_silent() {
        assert_ne!(EqPreset::Flat.gains(), EqPreset::Bass.gains());
        assert_ne!(EqPreset::Bass.gains(), EqPreset::Treble.gains());
        assert_eq!(EqPreset::Bass.gains()[0], 6.0);
        assert_eq!(EqPreset::Treble.gains()[4], 6.0);
        let mut p = Profile::default();
        p.eq_preset = EqPreset::Bass;
        assert_eq!(p.eq_gains(), [0.0; 5], "bypass zeroes the signal path");
        p.eq_bypass = false;
        assert_eq!(p.eq_gains(), EqPreset::Bass.gains());
        assert!(Density::Compact.factor() < Density::Cozy.factor());
        assert!(Density::Spacious.factor() > Density::Cozy.factor());
    }

    #[test]
    fn streaming_normalization_and_layout_preferences_survive_a_save() {
        let fresh = Profile::load_from(std::path::Path::new("this-file-does-not-exist"));
        assert_eq!(fresh.stream_mode, StreamMode::Auto);
        assert!(!fresh.normalize, "normalization starts off");
        assert_eq!(fresh.normalize_db, -6.0);
        assert!(!fresh.hide_now_playing);

        let mut p = Profile::default();
        p.stream_mode = StreamMode::Preview;
        p.normalize = true;
        p.normalize_db = -4.5;
        p.hide_now_playing = true;

        let dir = std::env::temp_dir().join(format!("rustify-stream-test-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("taste.txt");
        p.save_to(&path);
        let back = Profile::load_from(&path);
        assert_eq!(back.stream_mode, StreamMode::Preview);
        assert!(back.normalize);
        assert_eq!(back.normalize_db, -4.5);
        assert!(back.hide_now_playing);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_old_profile_without_the_new_keys_gets_sane_defaults() {
        let dir = std::env::temp_dir().join(format!("rustify-old-profile-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("taste.txt");
        std::fs::write(&path, "theme=1\nvolume=0.6\nplays=3\n").unwrap();

        let p = Profile::load_from(&path);
        assert_eq!(p.theme, 1);
        assert!((p.volume - 0.6).abs() < 0.001, "old settings still load");
        assert_eq!(p.stream_mode, StreamMode::Auto);
        assert!(!p.normalize);
        assert_eq!(p.normalize_db, -6.0);
        assert!(!p.hide_now_playing);
        assert!(!p.gapless);
        assert!(p.eq_bypass);
        assert_eq!(p.eq_preset, EqPreset::Flat);
        assert_eq!(p.accent, None);
        assert_eq!(p.density, Density::Cozy);
        assert!(p.restore_last);
        assert!(!p.sleep_fade);
        assert!(p.last_collection.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn custom_collection_names_survive_a_save_and_load() {
        let mut p = Profile::default();
        p.names
            .insert("album|After Hours".to_string(), "Late Night".to_string());
        p.names
            .insert("playlist|Drive".to_string(), "Night Drive".to_string());

        let dir = std::env::temp_dir().join(format!("rustify-name-test-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("taste.txt");
        p.save_to(&path);
        let back = Profile::load_from(&path);
        assert_eq!(
            back.names.get("album|After Hours").map(String::as_str),
            Some("Late Night")
        );
        assert_eq!(
            back.names.get("playlist|Drive").map(String::as_str),
            Some("Night Drive")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

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
