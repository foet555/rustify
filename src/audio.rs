// Real sound for files the owner already has on this PC. Nothing is
// downloaded or streamed: the index is a scan of one folder, and playback goes
// through rodio, whose Symphonia decoder covers MP3, WAV, FLAC, OGG/Vorbis and
// AAC/M4A - the formats Windows' own MCI could not play (no OGG at all).
use std::path::{Path, PathBuf};
use std::time::Duration;

use rodio::{Decoder, DeviceSinkBuilder, MixerDeviceSink, Player as Sink, Source};

/// Extensions rodio decodes out of the box.
const PLAYABLE: [&str; 7] = ["mp3", "wav", "m4a", "aac", "ogg", "oga", "flac"];

/// Lowercase words only, so "Song - Remastered (Official Audio).mp3" and
/// "Song" can be compared without punctuation noise.
pub fn normalize(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut space = true;
    for ch in s.chars().flat_map(char::to_lowercase) {
        if ch.is_alphanumeric() {
            out.push(ch);
            space = false;
        } else if !space {
            out.push(' ');
            space = true;
        }
    }
    out.trim().to_string()
}

pub struct Index {
    files: Vec<(String, PathBuf)>,
}

impl Default for Index {
    fn default() -> Self {
        Self { files: Vec::new() }
    }
}

impl Index {
    /// Walk `dir` (a few levels deep) once and remember every playable file.
    pub fn scan(dir: &str) -> Self {
        let mut files = Vec::new();
        let root = Path::new(dir);
        if root.is_dir() {
            walk(root, 0, &mut files);
        }
        Self { files }
    }

    /// Folders scanned when no folder was chosen. The usual homes of a
    /// person's own audio, so playback works without visiting Settings first.
    pub fn default_roots() -> Vec<PathBuf> {
        let mut roots = Vec::new();
        if let Ok(home) = std::env::var("USERPROFILE") {
            let home = Path::new(&home);
            for rel in [
                "Music",
                "Downloads",
                "Desktop",
                "Documents",
                "OneDrive\\Music",
            ] {
                let dir = home.join(rel);
                if dir.is_dir() {
                    roots.push(dir);
                }
            }
        }
        roots
    }

    /// One chosen folder, or every default root when the choice is empty.
    pub fn scan_roots(chosen: &str) -> Self {
        let chosen = chosen.trim();
        let roots = if chosen.is_empty() {
            Self::default_roots()
        } else {
            vec![PathBuf::from(chosen)]
        };
        let mut files = Vec::new();
        for root in roots {
            if root.is_dir() {
                walk(&root, 0, &mut files);
            }
        }
        // Overlapping folders must not list the same file twice.
        files.sort_by(|a, b| a.1.cmp(&b.1));
        files.dedup_by(|a, b| a.1 == b.1);
        Self { files }
    }

    pub fn len(&self) -> usize {
        self.files.len()
    }

    /// Every indexed file, for the "Local files" view.
    pub fn paths(&self) -> Vec<PathBuf> {
        self.files.iter().map(|(_, p)| p.clone()).collect()
    }

    /// Best file for a track. An exact hit wins: the file is named after the
    /// title, or after "artist - title". Only then does a looser "the name
    /// mentions the title" match apply, and only when the file also mentions
    /// the artist - without that, a same-named song by someone else would
    /// play in place of the one that was clicked.
    pub fn find(&self, title: &str, artist: &str) -> Option<PathBuf> {
        let want = normalize(title);
        if want.is_empty() {
            return None;
        }
        let artist = normalize(artist);
        // "Local file" is the label rows get when their name has no artist in
        // it; it is not a word a real file name would contain.
        let artist = if artist == "local file" {
            String::new()
        } else {
            artist
        };
        let first_artist_word = artist
            .split(' ')
            .next()
            .filter(|w| w.len() >= 4)
            .unwrap_or("")
            .to_string();
        let joined = format!("{artist} {want}");
        let reversed = format!("{want} {artist}");
        let mut bare: Option<&PathBuf> = None;
        let mut loose: Option<(usize, &PathBuf)> = None;
        for (name, path) in &self.files {
            // "Artist - Title" and "Title - Artist" are the usual file-naming
            // conventions, so a file named that way is an exact hit.
            if !artist.is_empty() && (name == &joined || name == &reversed) {
                return Some(path.clone());
            }
            if name == &want {
                if bare.is_none() {
                    bare = Some(path);
                }
                continue;
            }
            // Short titles ("#1", "Intro") must not match half the folder by
            // accident, so a loose match needs a title long enough to mean
            // something, and the artist name has to be on the file too.
            let names_artist = artist.is_empty()
                || name.contains(&artist)
                || (!first_artist_word.is_empty() && name.contains(&first_artist_word));
            let hit = names_artist
                && ((want.len() >= 5 && name.contains(&want))
                    || (want.len() >= 6 && want.contains(name.as_str())));
            if hit {
                let score = name.len();
                if loose.map(|(s, _)| score < s).unwrap_or(true) {
                    loose = Some((score, path));
                }
            }
        }
        bare.cloned().or_else(|| loose.map(|(_, p)| p.clone()))
    }
}

fn walk(dir: &Path, depth: usize, out: &mut Vec<(String, PathBuf)>) {
    if depth > 3 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk(&path, depth + 1, out);
            continue;
        }
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_ascii_lowercase())
            .unwrap_or_default();
        if !PLAYABLE.contains(&ext.as_str()) {
            continue;
        }
        if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
            let name = normalize(stem);
            if !name.is_empty() {
                out.push((name, path));
            }
        }
    }
}

/// One output device, reused for every track: playing a new file drops the
/// previous sink, which is what stops the old sound.
pub struct Player {
    device: Option<MixerDeviceSink>,
    sink: Option<Sink>,
    volume: f32,
    open: Option<PathBuf>,
    length: Option<Duration>,
    problem: Option<String>,
}

impl Default for Player {
    fn default() -> Self {
        Self::new()
    }
}

impl Player {
    pub fn new() -> Self {
        // A machine with no output device is not fatal: the rest of the app
        // still runs, and `problem` explains the silence in the player bar.
        let (device, problem) = match DeviceSinkBuilder::open_default_sink() {
            Ok(mut d) => {
                // The app stops playback on purpose; rodio's drop notice would
                // only be noise on the console.
                d.log_on_drop(false);
                (Some(d), None)
            }
            Err(e) => (None, Some(format!("no audio output device ({e})"))),
        };
        Self {
            device,
            sink: None,
            volume: 0.75,
            open: None,
            length: None,
            problem,
        }
    }

    pub fn current(&self) -> Option<&Path> {
        self.open.as_deref()
    }

    /// Why the last attempt produced no sound, if it did not.
    pub fn problem(&self) -> Option<&str> {
        self.problem.as_deref()
    }

    /// Whether an output device was found at all.
    pub fn ready(&self) -> bool {
        self.device.is_some()
    }

    pub fn play(&mut self, path: &Path, from_secs: f32, volume: f32) {
        self.stop();
        self.volume = volume.clamp(0.0, 1.0);
        let Some(device) = &self.device else {
            return;
        };
        let file = match std::fs::File::open(path) {
            Ok(f) => f,
            Err(e) => {
                self.problem = Some(format!("cannot open {}: {e}", path.display()));
                return;
            }
        };
        let decoder = match Decoder::try_from(file) {
            Ok(d) => d,
            Err(e) => {
                self.problem = Some(format!("{} could not be decoded ({e})", path.display()));
                return;
            }
        };
        self.length = decoder.total_duration();
        let sink = Sink::connect_new(device.mixer());
        sink.set_volume(self.volume);
        sink.append(decoder);
        if from_secs > 0.0 {
            let _ = sink.try_seek(Duration::from_secs_f32(from_secs));
        }
        sink.play();
        self.sink = Some(sink);
        self.open = Some(path.to_path_buf());
        self.problem = None;
    }

    pub fn pause(&mut self) {
        if let Some(sink) = &self.sink {
            sink.pause();
        }
    }

    pub fn resume(&mut self) {
        if let Some(sink) = &self.sink {
            sink.play();
        }
    }

    pub fn seek(&mut self, secs: f32) {
        if let Some(sink) = &self.sink {
            let _ = sink.try_seek(Duration::from_secs_f32(secs.max(0.0)));
        }
    }

    pub fn set_volume(&mut self, v: f32) {
        self.volume = v.clamp(0.0, 1.0);
        if let Some(sink) = &self.sink {
            sink.set_volume(self.volume);
        }
    }

    pub fn stop(&mut self) {
        // Dropping the sink removes it from the mixer, which stops the sound.
        self.sink = None;
        self.open = None;
        self.length = None;
    }

    /// Position of the sound being played, used by the `--audio-check` probe.
    pub fn position_ms(&self) -> u64 {
        self.sink
            .as_ref()
            .map(|s| s.get_pos().as_millis() as u64)
            .unwrap_or(0)
    }

    pub fn length_ms(&self) -> u64 {
        self.length.map(|d| d.as_millis() as u64).unwrap_or(0)
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn titles_and_file_names_are_normalised_for_matching() {
        assert_eq!(normalize("Ruder Buster (from Deltarune) - Metal Version"),
                   "ruder buster from deltarune metal version");
        assert_eq!(normalize("  Song__Name!!  "), "song name");
        assert_eq!(normalize("***"), "");
    }

    #[test]
    fn ogg_and_flac_are_indexed_alongside_mp3() {
        assert!(PLAYABLE.contains(&"ogg"));
        assert!(PLAYABLE.contains(&"flac"));
        assert!(PLAYABLE.contains(&"mp3"));
    }

    #[test]
    fn index_finds_the_right_file_for_a_track() {
        let dir = std::env::temp_dir().join(format!("rustify-audio-test-{}", std::process::id()));
        let _ = std::fs::create_dir_all(dir.join("sub"));
        let want = dir.join("Ruder Buster (from Deltarune) - Metal Version.mp3");
        std::fs::write(&want, b"not really audio").unwrap();
        std::fs::write(dir.join("other song.mp3"), b"x").unwrap();
        std::fs::write(dir.join("voice.ogg"), b"x").unwrap();
        std::fs::write(dir.join("readme.txt"), b"x").unwrap();

        let index = Index::scan(dir.to_str().unwrap());
        assert_eq!(index.len(), 3, "only playable files are indexed");
        let found = index
            .find("Ruder Buster (from Deltarune) - Metal Version", "Celestial Fury")
            .expect("exact title file");
        assert_eq!(found.file_name(), want.file_name());
        assert!(index.find("Song That Is Not Here", "Nobody").is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_artist_decides_between_two_files_with_the_same_title() {
        let dir = std::env::temp_dir().join(format!("rustify-artist-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let right = dir.join("Deltarune - Sanctuary.ogg");
        std::fs::write(&right, b"x").unwrap();
        std::fs::write(dir.join("4rd_sanctuary.ogg"), b"x").unwrap();
        std::fs::write(dir.join("Other Artist - Sanctuary.ogg"), b"x").unwrap();

        let index = Index::scan(dir.to_str().unwrap());
        let found = index
            .find("Sanctuary", "Deltarune")
            .expect("the file named after artist and title");
        assert_eq!(found.file_name(), right.file_name());
        // A file that merely mentions "Sanctuary" is not this other artist's song.
        assert!(index.find("Sanctuary", "Nobody Known").is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_empty_choice_scans_the_default_folders_without_duplicates() {
        let dir = std::env::temp_dir().join(format!("rustify-roots-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("one.mp3"), b"x").unwrap();

        let chosen = Index::scan_roots(dir.to_str().unwrap());
        assert_eq!(chosen.len(), 1, "a chosen folder is scanned");

        // The default roots always exist on this OS, but whether they hold
        // audio is the owner's business; scanning them must just be safe and
        // must never list one path twice.
        let defaults = Index::scan_roots("");
        let mut paths = defaults.paths();
        let total = paths.len();
        paths.sort();
        paths.dedup();
        assert_eq!(paths.len(), total, "no duplicated file");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
