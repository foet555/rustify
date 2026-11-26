// Real sound for files the owner already has on this PC. Nothing is
// downloaded or streamed: the index is a scan of one folder, and playback goes
// through rodio, whose Symphonia decoder covers MP3, WAV, FLAC, OGG/Vorbis and
// AAC/M4A - the formats Windows' own MCI could not play (no OGG at all).
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use rodio::mixer::Mixer;
use rodio::{Decoder, DeviceSinkBuilder, MixerDeviceSink, Player as Sink, Source};

/// A five-band graphic equalizer. The bands sit where the ear notices them
/// most, and each one is a peaking biquad at Q 1.0.
pub const EQ_BANDS: [f32; 5] = [60.0, 230.0, 910.0, 3600.0, 14000.0];

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct EqConfig {
    pub bypass: bool,
    pub gains_db: [f32; 5],
}

impl Default for EqConfig {
    fn default() -> Self {
        Self {
            bypass: true,
            gains_db: [0.0; 5],
        }
    }
}

/// Shared with every playing source. Changing a band only bumps a generation
/// counter, so the audio thread picks it up without rebuilding the sink or
/// losing the play position.
struct EqShared {
    bypass: AtomicBool,
    gains: [AtomicU32; 5],
    generation: AtomicU64,
}

impl EqShared {
    fn new(config: EqConfig) -> Self {
        let mut gains = std::array::from_fn(|_| AtomicU32::new(0));
        for (slot, gain) in gains.iter_mut().zip(config.gains_db) {
            slot.store(gain.to_bits(), Ordering::Relaxed);
        }
        Self {
            bypass: AtomicBool::new(config.bypass),
            gains,
            generation: AtomicU64::new(1),
        }
    }

    fn set(&self, config: EqConfig) {
        self.bypass.store(config.bypass, Ordering::Relaxed);
        for (slot, gain) in self.gains.iter().zip(config.gains_db) {
            slot.store(gain.to_bits(), Ordering::Relaxed);
        }
        self.generation.fetch_add(1, Ordering::Relaxed);
    }

    fn snapshot(&self) -> (EqConfig, u64) {
        let mut gains = [0.0; 5];
        for (slot, value) in gains.iter_mut().zip(&self.gains) {
            *slot = f32::from_bits(value.load(Ordering::Relaxed));
        }
        (
            EqConfig {
                bypass: self.bypass.load(Ordering::Relaxed),
                gains_db: gains,
            },
            self.generation.load(Ordering::Relaxed),
        )
    }
}

/// One peaking biquad, transposed direct form II. A zero gain collapses to a
/// pass-through, so a bypassed equalizer adds no colour at all.
#[derive(Clone, Copy, Default)]
struct Biquad {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
    z1: f32,
    z2: f32,
}

impl Biquad {
    fn peaking(freq: f32, gain_db: f32, sample_rate: u32) -> Self {
        if gain_db.abs() < 0.01 {
            return Self {
                b0: 1.0,
                ..Self::default()
            };
        }
        let nyquist = sample_rate as f32 * 0.5;
        let freq = freq.clamp(10.0, nyquist * 0.95);
        let a = 10f32.powf(gain_db / 40.0);
        let w0 = std::f32::consts::TAU * freq / sample_rate as f32;
        let alpha = w0.sin() / 2.0;
        let cos = w0.cos();
        let b0 = 1.0 + alpha * a;
        let b1 = -2.0 * cos;
        let b2 = 1.0 - alpha * a;
        let a0 = 1.0 + alpha / a;
        let a1 = -2.0 * cos;
        let a2 = 1.0 - alpha / a;
        Self {
            b0: b0 / a0,
            b1: b1 / a0,
            b2: b2 / a0,
            a1: a1 / a0,
            a2: a2 / a0,
            z1: 0.0,
            z2: 0.0,
        }
    }

    fn reset(&mut self) {
        self.z1 = 0.0;
        self.z2 = 0.0;
    }

    fn tick(&mut self, x: f32) -> f32 {
        let y = self.b0 * x + self.z1;
        self.z1 = self.b1 * x - self.a1 * y + self.z2;
        self.z2 = self.b2 * x - self.a2 * y;
        y
    }
}

struct EqKernel {
    bands: Vec<[Biquad; 5]>,
    preamp: f32,
}

impl EqKernel {
    fn new(config: EqConfig, sample_rate: u32, channels: usize) -> Self {
        let gains = if config.bypass {
            [0.0; 5]
        } else {
            config.gains_db
        };
        let max_boost = gains.iter().copied().fold(0.0, f32::max).max(0.0);
        let bands = (0..channels.max(1))
            .map(|_| {
                let mut row = [Biquad::default(); 5];
                for (band, slot) in row.iter_mut().enumerate() {
                    *slot = Biquad::peaking(EQ_BANDS[band], gains[band], sample_rate);
                }
                row
            })
            .collect();
        Self {
            bands,
            // Half of the maximum boost is compensated, leaving the character
            // of the preset audible while keeping peaks out of hard clipping.
            preamp: 10f32.powf(-max_boost / 40.0),
        }
    }

    fn reset(&mut self) {
        for row in &mut self.bands {
            for band in row {
                band.reset();
            }
        }
    }

    fn process(&mut self, sample: f32, channel: usize) -> f32 {
        let Some(row) = self.bands.get_mut(channel) else {
            return sample;
        };
        let mut out = sample;
        for band in row {
            out = band.tick(out);
        }
        out * self.preamp
    }
}

/// Wraps a decoded source with the live equalizer. The shared state is checked
/// every 64 samples; a preset change reaches sound in a couple of milliseconds.
pub struct EqSource<S> {
    inner: S,
    shared: Arc<EqShared>,
    kernel: EqKernel,
    generation: u64,
    counter: u64,
    frame: usize,
    channels: usize,
    rate: u32,
}

impl<S: Source<Item = f32>> EqSource<S> {
    fn new(inner: S, shared: Arc<EqShared>) -> Self {
        let (config, generation) = shared.snapshot();
        let channels = inner.channels().get() as usize;
        let rate = inner.sample_rate().get();
        let kernel = EqKernel::new(config, rate, channels);
        Self {
            inner,
            shared,
            kernel,
            generation,
            counter: 0,
            frame: 0,
            channels,
            rate,
        }
    }
}

impl<S: Source<Item = f32>> Iterator for EqSource<S> {
    type Item = f32;

    fn next(&mut self) -> Option<f32> {
        let sample = self.inner.next()?;
        self.counter += 1;
        if self.counter % 64 == 0 {
            let generation = self.shared.generation.load(Ordering::Relaxed);
            if generation != self.generation {
                let (config, generation) = self.shared.snapshot();
                self.kernel = EqKernel::new(config, self.rate, self.channels);
                self.generation = generation;
            }
        }
        let channel = self.frame % self.channels.max(1);
        self.frame = self.frame.wrapping_add(1);
        Some(self.kernel.process(sample, channel))
    }
}

impl<S: Source<Item = f32>> Source for EqSource<S> {
    fn current_span_len(&self) -> Option<usize> {
        self.inner.current_span_len()
    }

    fn channels(&self) -> rodio::ChannelCount {
        self.inner.channels()
    }

    fn sample_rate(&self) -> rodio::SampleRate {
        self.inner.sample_rate()
    }

    fn total_duration(&self) -> Option<Duration> {
        self.inner.total_duration()
    }

    fn try_seek(&mut self, pos: Duration) -> Result<(), rodio::source::SeekError> {
        self.inner.try_seek(pos)?;
        self.kernel.reset();
        self.frame = 0;
        Ok(())
    }
}

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
        Self::scan_paths(&roots)
    }

    /// Scan several roots in one pass. The Android build uses this to look in
    /// the app's external music folder and its internal fallback without
    /// listing the same file twice.
    pub fn scan_paths(roots: &[PathBuf]) -> Self {
        let mut files = Vec::new();
        for root in roots {
            if root.is_dir() {
                walk(root, 0, &mut files);
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

/// One fade in progress: the current sink rises while an optional outgoing
/// sink falls, both sharing the player's volume as their ceiling.
struct Fade {
    started: Instant,
    secs: f32,
    outgoing: Option<Sink>,
}

/// One output device, reused for every track: playing a new file drops the
/// previous sink, which is what stops the old sound.
pub struct Player {
    device: Option<MixerDeviceSink>,
    /// The mixer feeding the output device. Kept separately so a headless
    /// capture can pull the same mix without opening speakers.
    mixer: Option<Mixer>,
    sink: Option<Sink>,
    fade: Option<Fade>,
    volume: f32,
    open: Option<PathBuf>,
    length: Option<Duration>,
    problem: Option<String>,
    eq: Arc<EqShared>,
    /// A decoded next track already appended to the same sink. When it has
    /// played through, `promote_queued` makes it the current track.
    queued: Option<(PathBuf, Option<Duration>)>,
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
        let mixer = device.as_ref().map(|d| d.mixer().clone());
        Self {
            device,
            mixer,
            sink: None,
            fade: None,
            volume: 0.75,
            open: None,
            length: None,
            problem,
            eq: Arc::new(EqShared::new(EqConfig::default())),
            queued: None,
        }
    }

    /// Builds a player whose output is a mixer the caller owns. The real
    /// decoder, EQ, sink, fade and gapless paths are the same; only the
    /// hardware stream is replaced by a pulled `MixerSource`.
    pub fn with_mixer(mixer: Mixer) -> Self {
        Self {
            device: None,
            mixer: Some(mixer),
            sink: None,
            fade: None,
            volume: 0.75,
            open: None,
            length: None,
            problem: None,
            eq: Arc::new(EqShared::new(EqConfig::default())),
            queued: None,
        }
    }

    /// Replaces the live equalizer. The playing source picks the new bands up
    /// on its next block; nothing is restarted.
    pub fn set_eq(&mut self, config: EqConfig) {
        // Called every frame so a preset switch is immediate; only a real
        // change bumps the generation and reaches the audio thread.
        if self.eq.snapshot().0 != config {
            self.eq.set(config);
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
        self.play_with_fade(path, from_secs, volume, 0.0);
    }

    /// Starts a file, optionally rising from silence over `fade_secs`. A short
    /// fade-in is what makes the first track and manual skips feel smooth.
    pub fn play_with_fade(&mut self, path: &Path, from_secs: f32, volume: f32, fade_secs: f32) {
        self.stop();
        self.volume = volume.clamp(0.0, 1.0);
        let Some(mixer) = &self.mixer else {
            return;
        };
        let start = if fade_secs > 0.0 { 0.0 } else { self.volume };
        match make_sink(mixer, path, from_secs, start, &self.eq) {
            Ok((sink, length)) => {
                self.sink = Some(sink);
                self.length = length;
                self.open = Some(path.to_path_buf());
                self.problem = None;
                if fade_secs > 0.0 {
                    self.fade = Some(Fade {
                        started: Instant::now(),
                        secs: fade_secs,
                        outgoing: None,
                    });
                }
            }
            Err(e) => self.problem = Some(e),
        }
    }

    /// Blends the next file into the one already playing. The old sink keeps
    /// running while the new one rises, so the two really overlap instead of
    /// cutting at the track boundary.
    pub fn crossfade_to(&mut self, path: &Path, volume: f32, secs: f32) {
        let Some(old) = self.sink.take() else {
            self.play_with_fade(path, 0.0, volume, secs);
            return;
        };
        self.volume = volume.clamp(0.0, 1.0);
        self.queued = None;
        let Some(mixer) = &self.mixer else {
            self.sink = Some(old);
            return;
        };
        match make_sink(mixer, path, 0.0, 0.0, &self.eq) {
            Ok((sink, length)) => {
                self.sink = Some(sink);
                self.length = length;
                self.open = Some(path.to_path_buf());
                self.problem = None;
                self.fade = Some(Fade {
                    started: Instant::now(),
                    secs: secs.max(0.1),
                    outgoing: Some(old),
                });
            }
            Err(e) => {
                self.sink = Some(old);
                self.problem = Some(e);
            }
        }
    }

    /// True while a crossfade or fade-in is still moving.
    pub fn crossfading(&self) -> bool {
        self.fade.is_some()
    }

    /// Advances the fade by the frame clock. Cheap when nothing is fading.
    pub fn tick(&mut self) {
        let Some((t, has_outgoing)) = self.fade.as_ref().map(|f| {
            (
                (f.started.elapsed().as_secs_f32() / f.secs.max(0.01)).clamp(0.0, 1.0),
                f.outgoing.is_some(),
            )
        }) else {
            return;
        };
        let base = self.volume;
        if let Some(sink) = &self.sink {
            sink.set_volume(base * t);
        }
        if has_outgoing {
            if let Some(fade) = &self.fade {
                if let Some(old) = &fade.outgoing {
                    old.set_volume(base * (1.0 - t));
                }
            }
        }
        if t >= 1.0 {
            if let Some(sink) = &self.sink {
                sink.set_volume(base);
            }
            self.fade = None;
        }
    }

    pub fn pause(&mut self) {
        // A pause ends any blend cleanly; the current track resumes at full
        // volume rather than finishing a fade that was interrupted.
        self.fade = None;
        if let Some(sink) = &self.sink {
            sink.set_volume(self.volume);
            sink.pause();
        }
    }

    pub fn resume(&mut self) {
        if let Some(sink) = &self.sink {
            sink.play();
        }
    }

    pub fn seek(&mut self, secs: f32) {
        // Seeking drops a blend: the old track must not keep playing under a
        // scrubber that has moved somewhere else.
        self.fade = None;
        if let Some(sink) = &self.sink {
            sink.set_volume(self.volume);
            let _ = sink.try_seek(Duration::from_secs_f32(secs.max(0.0)));
        }
    }

    pub fn set_volume(&mut self, v: f32) {
        self.volume = v.clamp(0.0, 1.0);
        let t = self
            .fade
            .as_ref()
            .map(|f| {
                (f.started.elapsed().as_secs_f32() / f.secs.max(0.01)).clamp(0.0, 1.0)
            })
            .unwrap_or(1.0);
        if let Some(sink) = &self.sink {
            sink.set_volume(self.volume * t);
        }
        if let Some(fade) = &self.fade {
            if let Some(old) = &fade.outgoing {
                old.set_volume(self.volume * (1.0 - t));
            }
        }
    }

    pub fn stop(&mut self) {
        // Dropping the sinks removes them from the mixer, which stops the
        // sound; dropping the fade also drops the outgoing sink and any
        // gapless track waiting behind it.
        self.sink = None;
        self.fade = None;
        self.queued = None;
        self.open = None;
        self.length = None;
    }

    /// Decodes `path` and appends it to the current sink. rodio's queue is
    /// sample-contiguous, so the next track starts without a gap. False when
    /// there is no sink or the file cannot be decoded; the caller then falls
    /// back to the normal boundary.
    pub fn queue_next(&mut self, path: &Path) -> bool {
        if self.queued.is_some() {
            return false;
        }
        let Some(sink) = &self.sink else {
            return false;
        };
        match make_source(path, 0.0, &self.eq) {
            Ok((source, length)) => {
                sink.append(source);
                self.queued = Some((path.to_path_buf(), length));
                true
            }
            Err(e) => {
                self.problem = Some(e);
                false
            }
        }
    }

    pub fn has_queued(&self) -> bool {
        self.queued.is_some()
    }

    /// The queued source has reached the top of the sink. Move the public
    /// "current file" pointers to it and drop the old length.
    pub fn promote_queued(&mut self) -> Option<PathBuf> {
        let (path, length) = self.queued.take()?;
        self.open = Some(path.clone());
        self.length = length;
        Some(path)
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

    /// True once the queued source has played to its end. Decoders that do
    /// not report a duration (minimp3 on Android) rely on this instead of a
    /// clock to know when the track is over.
    pub fn finished(&self) -> bool {
        self.sink.as_ref().is_some_and(|sink| sink.empty())
    }
}

/// Builds one playing sink for a file. Kept free of `self` so `play` and
/// `crossfade_to` can share it while the device is borrowed.
fn make_sink(
    mixer: &Mixer,
    path: &Path,
    from_secs: f32,
    volume: f32,
    eq: &Arc<EqShared>,
) -> Result<(Sink, Option<Duration>), String> {
    let (source, length) = make_source(path, from_secs, eq)?;
    let sink = Sink::connect_new(mixer);
    sink.set_volume(volume);
    sink.append(source);
    sink.play();
    Ok((sink, length))
}

fn make_source(
    path: &Path,
    from_secs: f32,
    eq: &Arc<EqShared>,
) -> Result<
    (
        EqSource<Decoder<std::io::BufReader<std::fs::File>>>,
        Option<Duration>,
    ),
    String,
> {
    let file = std::fs::File::open(path)
        .map_err(|e| format!("cannot open {}: {e}", path.display()))?;
    let decoder = Decoder::try_from(file)
        .map_err(|e| format!("{} could not be decoded ({e})", path.display()))?;
    let length = decoder.total_duration();
    let mut source = EqSource::new(decoder, eq.clone());
    if from_secs > 0.0 {
        let _ = source.try_seek(Duration::from_secs_f32(from_secs));
    }
    Ok((source, length))
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

    struct Tone {
        freq: f32,
        rate: u32,
        i: usize,
        len: usize,
    }

    impl Tone {
        fn new(freq: f32) -> Self {
            Self {
                freq,
                rate: 48_000,
                i: 0,
                len: 48_000,
            }
        }
    }

    impl Iterator for Tone {
        type Item = f32;

        fn next(&mut self) -> Option<f32> {
            if self.i >= self.len {
                return None;
            }
            let t = self.i as f32 / self.rate as f32;
            self.i += 1;
            Some((std::f32::consts::TAU * self.freq * t).sin() * 0.5)
        }
    }

    impl Source for Tone {
        fn current_span_len(&self) -> Option<usize> {
            Some(self.len - self.i)
        }

        fn channels(&self) -> rodio::ChannelCount {
            std::num::NonZero::new(1).unwrap()
        }

        fn sample_rate(&self) -> rodio::SampleRate {
            std::num::NonZero::new(self.rate).unwrap()
        }

        fn total_duration(&self) -> Option<Duration> {
            Some(Duration::from_secs(1))
        }
    }

    fn rms(freq: f32, config: EqConfig) -> f32 {
        let shared = Arc::new(EqShared::new(config));
        let mut source = EqSource::new(Tone::new(freq), shared);
        let mut sum = 0.0f64;
        let mut n = 0u32;
        for sample in source.by_ref() {
            sum += (sample as f64) * (sample as f64);
            n += 1;
        }
        (sum / n as f64).sqrt() as f32
    }

    const BYPASS: EqConfig = EqConfig {
        bypass: true,
        gains_db: [0.0; 5],
    };

    const BASS: EqConfig = EqConfig {
        bypass: false,
        gains_db: [6.0, 4.0, 0.0, 0.0, 0.0],
    };

    #[test]
    fn the_equalizer_boosts_bass_and_leaves_treble_alone() {
        let clean_bass = rms(60.0, BYPASS);
        let shaped_bass = rms(60.0, BASS);
        println!(
            "EQ measurement: 60 Hz bypass RMS {clean_bass:.4} -> bass preset {shaped_bass:.4}"
        );
        assert!(
            shaped_bass > clean_bass * 1.2,
            "bass should rise: {clean_bass} -> {shaped_bass}"
        );

        let clean_treble = rms(10_000.0, BYPASS);
        let shaped_treble = rms(10_000.0, BASS);
        println!(
            "EQ measurement: 10 kHz bypass RMS {clean_treble:.4} -> bass preset {shaped_treble:.4}"
        );
        assert!(
            shaped_treble < clean_treble * 0.95,
            "the same preset must not lift treble: {clean_treble} -> {shaped_treble}"
        );
    }

    #[test]
    fn bypass_is_bit_exact_and_a_live_change_reaches_the_source() {
        let shared = Arc::new(EqShared::new(BYPASS));
        let mut source = EqSource::new(Tone::new(1_000.0), shared.clone());
        let expected: Vec<f32> = Tone::new(1_000.0).take(512).collect();
        let got: Vec<f32> = source.by_ref().take(512).collect();
        assert_eq!(got, expected, "a bypassed EQ must not colour the samples");

        let before: Vec<f32> = source.by_ref().take(512).collect();
        let before_rms = (before.iter().map(|s| s * s).sum::<f32>() / 512.0).sqrt();
        shared.set(BASS);
        let after: Vec<f32> = source.by_ref().take(512).collect();
        let after_rms = (after.iter().map(|s| s * s).sum::<f32>() / 512.0).sqrt();
        assert!(
            (after_rms - before_rms).abs() > 0.05,
            "a preset change must reach the playing source: {before_rms} -> {after_rms}"
        );
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
