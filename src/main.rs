#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod audio;
mod connect;
mod discord;
mod art;
mod catalog;
mod gate;
mod library;
mod preview;
mod profile;
mod spotify;
#[cfg(test)]
mod fake_spotify;
mod taste;
mod theme;
mod themefile;
mod webplayer;

use eframe::egui::{self, Align, Align2, Color32, Layout, Rect, RichText, Sense, Stroke, Vec2};
use art as artwork;
use library::{Collection, Kind, Track, GENRES};
use profile::Profile;
use std::path::PathBuf;
use theme as th;
use theme::{rgb, Theme};

const APP_TITLE: &str = "Rustify";
const LOCKED_TITLE: &str = "Spotify required - Rustify gate";

/// The owner's real store must always surface these playlists. The headless
/// `--library-report` check fails loudly if one disappears or loses its tracks.
const REQUIRED_PLAYLISTS: [&str; 8] = [
    "Scarlet Forest",
    "Lost better",
    "Grill a",
    "Discord songs",
    "Earape",
    "Imagine LOL",
    "Tycoons be like",
    "Fnaf ambiance",
];

#[cfg(windows)]
#[link(name = "user32")]
unsafe extern "system" {
    fn OpenClipboard(hwnd: *mut core::ffi::c_void) -> i32;
    fn CloseClipboard() -> i32;
    fn GetClipboardData(format: u32) -> *mut core::ffi::c_void;
    fn IsClipboardFormatAvailable(format: u32) -> i32;
}

#[cfg(windows)]
#[link(name = "kernel32")]
unsafe extern "system" {
    fn GlobalLock(handle: *mut core::ffi::c_void) -> *mut core::ffi::c_void;
    fn GlobalUnlock(handle: *mut core::ffi::c_void) -> i32;
}

/// Reads Unicode text from the Windows clipboard for right-click paste.
/// Deliberately no extra crate or helper process: this is a single cheap call.
#[cfg(windows)]
fn clipboard_text() -> Option<String> {
    use std::os::windows::ffi::OsStringExt;

    const CF_UNICODETEXT: u32 = 13;
    unsafe {
        if IsClipboardFormatAvailable(CF_UNICODETEXT) == 0 || OpenClipboard(std::ptr::null_mut()) == 0 {
            return None;
        }
        let handle = GetClipboardData(CF_UNICODETEXT);
        let mut out = None;
        if !handle.is_null() {
            let ptr = GlobalLock(handle) as *const u16;
            if !ptr.is_null() {
                let mut len = 0usize;
                while *ptr.add(len) != 0 && len < 1_000_000 {
                    len += 1;
                }
                let wide = std::slice::from_raw_parts(ptr, len);
                out = Some(std::ffi::OsString::from_wide(wide).to_string_lossy().to_string());
                GlobalUnlock(handle);
            }
        }
        CloseClipboard();
        out
    }
}

#[cfg(not(windows))]
fn clipboard_text() -> Option<String> {
    None
}

fn clean_paste(clip: &str) -> Option<String> {
    let clean: String = clip
        .chars()
        .map(|c| match c {
            '\r' | '\n' | '\t' => ' ',
            _ => c,
        })
        .collect();
    let clean = clean.trim();
    (!clean.is_empty()).then(|| clean.to_string())
}

/// Appends pasted text to a single-line field, flattening any line breaks.
fn append_pasted(text: &mut String, clip: &str) -> bool {
    match clean_paste(clip) {
        Some(clean) => {
            text.push_str(&clean);
            true
        }
        None => false,
    }
}

/// Right-click a field to paste. An unfocused field is replaced, a focused one
/// keeps its text and appends at the end, like a normal paste target.
fn paste_on_right_click(resp: &egui::Response, text: &mut String) -> bool {
    if !resp.secondary_clicked() {
        return false;
    }
    let Some(clip) = clipboard_text() else {
        return false;
    };
    if clean_paste(&clip).is_none() {
        return false;
    }
    if !resp.has_focus() {
        text.clear();
    }
    if append_pasted(text, &clip) {
        resp.request_focus();
        if let Some(mut state) = egui::text_edit::TextEditState::load(&resp.ctx, resp.id) {
            let end = egui::text::CCursor::new(text.chars().count());
            state
                .cursor
                .set_char_range(Some(egui::text::CCursorRange::one(end)));
            state.store(&resp.ctx, resp.id);
        }
        true
    } else {
        false
    }
}

fn arg_value<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .map(String::as_str)
}

fn main() -> eframe::Result {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--spotify-check") {
        print!("{}", gate::report(&gate::check()));
        return Ok(());
    }
    if args.iter().any(|a| a == "--profile-report") {
        let lib = library::sample();
        let p = Profile::load_from(&Profile::default_path());
        print!("{}", selftest_report_text(&p, &lib));
        return Ok(());
    }
    if args.iter().any(|a| a == "--selftest-write") {
        selftest_write();
        return Ok(());
    }
    if args.iter().any(|a| a == "--selftest-report") {
        selftest_report();
        return Ok(());
    }
    if args.iter().any(|a| a == "--taste-report") {
        let app = App::launch_state(gate::check());
        print!("{}", app.taste_report());
        return Ok(());
    }
    if args.iter().any(|a| a == "--audio-report") {
        let mut app = App::launch_state(gate::check());
        app.attach_local_files();
        print!("{}", app.audio_report());
        if args.iter().any(|a| a == "--play-first") {
            app.audio_selftest();
        }
        return Ok(());
    }
    if args.iter().any(|a| a == "--library-report") {
        let app = match arg_value(&args, "--store") {
            Some(path) => App::launch_state_at(gate::check(), PathBuf::from(path)),
            None => App::launch_state(gate::check()),
        };
        print!("{}", app.library_report());
        match app.verify_library() {
            Ok(summary) => print!("{summary}"),
            Err(errors) => {
                for e in &errors {
                    eprintln!("library check failed: {e}");
                }
                std::process::exit(1);
            }
        }
        return Ok(());
    }

    if let Some(path) = arg_value(&args, "--audio-check") {
        let file = PathBuf::from(path);
        let dir = file
            .parent()
            .map(|p| p.display().to_string())
            .unwrap_or_default();
        let stem = file
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or_default()
            .to_string();
        let index = audio::Index::scan(&dir);
        let Some(found) = index.find(&stem, "") else {
            eprintln!("audio check failed: no indexed file matches {stem}");
            std::process::exit(1);
        };
        let mut player = audio::Player::new();
        player.play(&found, 0.0, 0.4);
        std::thread::sleep(std::time::Duration::from_millis(1500));
        let pos = player.position_ms();
        let len = player.length_ms();
        player.stop();
        println!(
            "indexed={} file={}\nlength_ms={len} position_ms={pos}",
            index.len(),
            found.display()
        );
        if len == 0 || pos == 0 {
            eprintln!("audio check failed: MCI did not play the file");
            std::process::exit(1);
        }
        return Ok(());
    }

    let presence = gate::check_local_player();
    let title = if presence.found {
        APP_TITLE
    } else {
        LOCKED_TITLE
    };

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1380.0, 880.0])
            .with_min_inner_size([1040.0, 660.0])
            .with_icon(th::icon(128))
            .with_title(title),
        ..Default::default()
    };
    eframe::run_native(
        title,
        options,
        Box::new(move |cc| Ok(Box::new(App::with_gate(cc, presence)))),
    )
}

// ------------------------------------------------------------------ types

#[derive(Clone, Copy, PartialEq, Eq)]
enum View {
    Home,
    Search,
    Taste,
    Import,
    Collection(usize),
    Liked,
    Mix(usize),
    Genre(&'static str),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum LibFilter {
    All,
    Playlists,
    Albums,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Icon {
    Play,
    Pause,
    Prev,
    Next,
    Shuffle,
    Repeat,
}

#[derive(Clone, Copy)]
enum NavIcon {
    Home,
    Search,
    Taste,
    Import,
}

struct App {
    lib: Vec<Collection>,
    profile: Profile,
    themes: Vec<Theme>,
    theme_idx: usize,
    gate: gate::Presence,
    view: View,
    query: String,
    queue: Vec<Track>,
    queue_genres: Vec<String>,
    now: usize,
    pos: f32,
    playing: bool,
    shuffle: bool,
    repeat: bool,
    volume: f32,
    show_queue: bool,
    show_themes: bool,
    lib_filter: LibFilter,
    rng: u64,
    save_acc: f32,
    profile_path: PathBuf,
    imported: Vec<spotify::ImportedPlaylist>,
    import_rx: Option<std::sync::mpsc::Receiver<spotify::Msg>>,
    import_busy: bool,
    import_log: String,
    import_path: String,
    link_input: String,
    imports_path: PathBuf,
    import_error: Option<String>,
    client_id: String,
    audio: audio::Player,
    audio_lib: audio::Index,
    audio_dir: String,
    locked_frames: u32,
    player_frames: u32,
    connect: Option<connect::Session>,
    connect_playback: Option<connect::Playback>,
    connect_error: Option<String>,
    connect_busy: bool,
    connect_log: String,
    connect_login_rx: Option<std::sync::mpsc::Receiver<conn_worker::Login>>,
    /// Flipped to abandon a sign-in that is waiting on the browser.
    connect_cancel: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    conn_tx: Option<std::sync::mpsc::Sender<conn_worker::Request>>,
    conn_rx: Option<std::sync::mpsc::Receiver<conn_worker::Out>>,
    connect_stamp: Option<std::time::Instant>,
    spotify_mode: bool,
    /// Set when the owner picks a file that is on this PC: that file owns the
    /// transport even while a Spotify session is attached, so a Local files
    /// row never turns into a Spotify name search.
    local_override: bool,
    /// A Spotify play waiting for a Spotify client to appear as a device.
    pending_play_uri: Option<PendingPlay>,
    device_asked: Option<std::time::Instant>,
    /// A play press that found no Spotify client. The tick opens the app once
    /// and polls the device list until it answers.
    wake: Option<Wake>,
    bg_tex: Option<egui::TextureHandle>,
    bg_key: String,
    bg_error: Option<String>,
    discord: Option<discord::Handle>,
    discord_sent: Option<discord::Now>,
    discord_status: String,
    /// What the owner typed into the library filter, kept apart from the
    /// global catalogue search.
    lib_query: String,
    /// Cover of the track handed to Spotify Connect, so the player bar and the
    /// Now Playing panel can show the real artwork instead of a gradient.
    now_cover: Option<String>,
    /// Catalogue search: the query actually sent, results, and the debounce.
    search_q: String,
    search_pending: Option<(String, std::time::Instant)>,
    search_busy: bool,
    search_error: Option<String>,
    search_hits: Vec<catalog::Hit>,
    /// The Colours-in-a-text-file theme, watched for edits every frame.
    theme_file: themefile::Watch,
    devices: Vec<connect::Device>,
    show_devices: bool,
    /// Spotify's own player inside a hidden webview: full songs without the
    /// desktop app. None until the window exists and the session is attached.
    web: Option<webplayer::Player>,
    /// One start attempt per run; a failure falls back to device playback.
    web_tried: bool,
    web_error: Option<String>,
    /// Device id the SDK reported. Preferred for every play command.
    web_device: Option<String>,
    /// A device the owner picked by hand. It outranks the in-app player until
    /// it disappears from the list.
    picked_device: Option<String>,
    /// One cover backfill per launch, so a missing image is not retried on
    /// every repaint or every session poll.
    covers_asked: bool,
    /// When a token was last asked for / handed to the player.
    web_token_asked: Option<std::time::Instant>,
    web_token_at: Option<std::time::Instant>,
    /// One-line reason an action could not make sound, shown in the player
    /// bar for a few seconds so a click never ends in unexplained silence.
    notice: Option<(String, std::time::Instant)>,
    /// Official 30-second previews for rows that are not on this PC and
    /// have no Spotify app behind them. Public catalogue clips, no login.
    preview: preview::Queue,
    /// True while the sound is a downloaded preview clip instead of a file.
    previewing: bool,
    /// Clip already downloaded for the track named by the key.
    preview_file: Option<(String, PathBuf)>,
}

/// A Spotify play command waiting for a device to receive it, because Spotify
/// may still be starting up.
#[derive(Clone)]
struct PendingPlay {
    uri: String,
    track: bool,
}

/// A play press with no Spotify client attached. `opened` keeps the app from
/// being launched twice, and `resume` remembers that the press was a plain
/// play rather than a specific track.
struct Wake {
    since: std::time::Instant,
    opened: bool,
    resume: bool,
}

/// Name of the generated collection that lists the owner's own files.
const LOCAL_FILES: &str = "Local files";

/// Build the "Local files" collection from whatever the folder scan found.
/// Titles are the file stems, so the player's exact-match lookup always lands
/// on the same file the row was made from.
fn local_files_collection(index: &audio::Index) -> Option<library::Collection> {
    let mut tracks: Vec<library::Track> = index
        .paths()
        .into_iter()
        .filter_map(|path| {
            let stem = path.file_stem().and_then(|s| s.to_str())?.to_string();
            if stem.trim().is_empty() {
                return None;
            }
            let (artist, title) = match stem.split_once(" - ") {
                Some((a, t)) => (a.trim().to_string(), t.trim().to_string()),
                None => ("Local file".to_string(), stem),
            };
            let (c1, c2) = palette_for(&title);
            Some(library::Track {
                title,
                artist,
                album: String::new(),
                secs: 0,
                c1,
                c2,
                added: String::new(),
                art: library::Art::default(),
                path: Some(path),
            })
        })
        .collect();
    if tracks.is_empty() {
        return None;
    }
    tracks.sort_by(|a, b| a.title.to_lowercase().cmp(&b.title.to_lowercase()));
    let count = tracks.len();
    Some(library::Collection::new(
        LOCAL_FILES,
        library::Kind::Playlist,
        format!("{count} files on this PC"),
        rgb(0x1f3a2e),
        rgb(0x1db954),
        vec!["local"],
        tracks,
    ))
}

/// Spotify says Premium is required for Web API playback on this account.
fn premium_error(message: &str) -> bool {
    message.to_ascii_lowercase().contains("premium")
}

impl App {
    fn with_gate(cc: &eframe::CreationContext<'_>, gate: gate::Presence) -> Self {
        let mut app = Self::launch_state(gate);
        app.attach_local_files();
        // A saved session is only useful once its worker is running again.
        if let Some(session) = app.connect.clone() {
            app.attach_session(session);
        }
        app.with_style(cc)
    }

    /// Rebuild the "Local files" collection in place. Never duplicates it, and
    /// keeps an open collection view pointing at the same playlist.
    fn attach_local_files(&mut self) {
        let Some(coll) = local_files_collection(&self.audio_lib) else {
            return;
        };
        let had = self.lib.iter().any(|c| c.name == LOCAL_FILES);
        self.lib.retain(|c| c.name != LOCAL_FILES);
        self.lib.insert(0, coll);
        if had {
            if let View::Collection(i) = self.view {
                self.view = View::Collection(i + 1);
            }
        }
    }

    /// Everything the launched window starts with: profile, catalogue and the
    /// playlists saved on disk. Kept free of egui setup so the real launch
    /// state can be inspected headlessly with `--library-report`.
    fn launch_state(gate: gate::Presence) -> Self {
        Self::launch_state_at(gate, spotify::default_path())
    }

    /// Same launch state, reading playlists from an explicit store file. The
    /// window always uses the default path; the override exists so the check
    /// can be proven against a copy without touching the real store.
    fn launch_state_at(gate: gate::Presence, imports_path: PathBuf) -> Self {
        let profile_path = Profile::default_path();
        let profile = Profile::load_from(&profile_path);
        let theme_file = themefile::Watch::at(themefile::default_path());
        let mut themes = th::with_custom(&profile.custom);
        // The file theme is always last, so editing it never shifts the other
        // slots and a saved index keeps pointing at the same palette.
        themes.push(theme_file.theme());
        let theme_idx = profile.theme.min(themes.len() - 1);
        let discord = if profile.discord_enabled && !profile.discord_client_id.trim().is_empty() {
            Some(discord::spawn(profile.discord_client_id.clone()))
        } else {
            None
        };
        let lib = library::sample();
        // Left empty, the usual folders (Music, Downloads, Desktop, Documents)
        // are scanned, so playback works before anyone visits Settings.
        let audio_dir = profile.audio_dir.clone();
        let audio_lib = audio::Index::scan_roots(&audio_dir);
        // A saved session means playback control is already set up. The tokens
        // are DPAPI-sealed and refreshed by the worker, never written plainly.
        let connect = connect::load(&connect::default_path(), &profile.client_id);
        let spotify_mode = connect.is_some();

        let queue = lib[0].tracks.clone();
        let queue_genres = lib[0].genres.iter().map(|g| g.to_string()).collect();

        Self {
            lib,
            volume: profile.volume,
            profile,
            themes,
            theme_idx,
            gate,
            view: View::Home,
            query: String::new(),
            queue,
            queue_genres,
            now: 0,
            pos: 0.0,
            playing: false,
            shuffle: false,
            repeat: false,
            show_queue: false,
            show_themes: false,
            lib_filter: LibFilter::All,
            rng: 0x2545f4914f6cdd1d,
            save_acc: 0.0,
            imported: Vec::new(),
            import_rx: None,
            import_busy: false,
            import_log: String::new(),
            import_path: String::new(),
            link_input: String::new(),
            imports_path: profile_path.with_extension("imports.txt"),
            import_error: None,
            client_id: String::new(),
            audio: audio::Player::new(),
            audio_lib,
            audio_dir,
            profile_path,
            locked_frames: 0,
            player_frames: 0,
            connect,
            connect_playback: None,
            connect_error: None,
            connect_busy: false,
            connect_log: String::new(),
            connect_login_rx: None,
            connect_cancel: None,
            conn_tx: None,
            conn_rx: None,
            connect_stamp: None,
            spotify_mode,
            local_override: false,
            pending_play_uri: None,
            device_asked: None,
            wake: None,
            bg_tex: None,
            bg_key: String::new(),
            bg_error: None,
            discord,
            discord_sent: None,
            discord_status: String::new(),
            lib_query: String::new(),
            now_cover: None,
            search_q: String::new(),
            search_pending: None,
            search_busy: false,
            search_error: None,
            search_hits: Vec::new(),
            theme_file,
            devices: Vec::new(),
            show_devices: false,
            web: None,
            web_tried: false,
            web_error: None,
            web_device: None,
            picked_device: None,
            web_token_asked: None,
            web_token_at: None,
            covers_asked: false,
            notice: None,
            preview: preview::Queue::new(),
            previewing: false,
            preview_file: None,
        }
        .with_imports_at(imports_path)
    }

    #[cfg(test)]
    fn for_test(gate: gate::Presence, profile_path: PathBuf) -> Self {
        let profile = Profile::load_from(&profile_path);
        let theme_file = themefile::Watch::at(profile_path.with_file_name("theme.toml"));
        let mut themes = th::with_custom(&profile.custom);
        themes.push(theme_file.theme());
        let theme_idx = profile.theme.min(themes.len() - 1);
        let lib = library::sample();
        let audio_dir = profile.audio_dir.clone();
        let audio_lib = audio::Index::scan_roots(&audio_dir);
        let queue = lib[0].tracks.clone();
        let queue_genres = lib[0].genres.iter().map(|g| g.to_string()).collect();

        Self {
            lib,
            volume: profile.volume,
            profile,
            themes,
            theme_idx,
            gate,
            view: View::Home,
            query: String::new(),
            queue,
            queue_genres,
            now: 0,
            pos: 0.0,
            playing: false,
            shuffle: false,
            repeat: false,
            show_queue: false,
            show_themes: false,
            lib_filter: LibFilter::All,
            rng: 0x2545f4914f6cdd1d,
            save_acc: 0.0,
            imported: Vec::new(),
            import_rx: None,
            import_busy: false,
            import_log: String::new(),
            import_path: String::new(),
            link_input: String::new(),
            imports_path: profile_path.with_extension("imports.txt"),
            import_error: None,
            client_id: String::new(),
            audio: audio::Player::new(),
            audio_lib,
            audio_dir,
            profile_path,
            locked_frames: 0,
            player_frames: 0,
            connect: None,
            connect_playback: None,
            connect_error: None,
            connect_busy: false,
            connect_log: String::new(),
            connect_login_rx: None,
            connect_cancel: None,
            conn_tx: None,
            conn_rx: None,
            connect_stamp: None,
            spotify_mode: false,
            local_override: false,
            pending_play_uri: None,
            device_asked: None,
            wake: None,
            bg_tex: None,
            bg_key: String::new(),
            bg_error: None,
            // No worker threads are started from a test.
            discord: None,
            discord_sent: None,
            discord_status: String::new(),
            lib_query: String::new(),
            now_cover: None,
            search_q: String::new(),
            search_pending: None,
            search_busy: false,
            search_error: None,
            search_hits: Vec::new(),
            theme_file,
            devices: Vec::new(),
            show_devices: false,
            web: None,
            web_tried: false,
            web_error: None,
            web_device: None,
            picked_device: None,
            web_token_asked: None,
            web_token_at: None,
            covers_asked: false,
            notice: None,
            preview: preview::Queue::disabled(),
            previewing: false,
            preview_file: None,
        }
    }

    fn with_style(self, cc: &eframe::CreationContext<'_>) -> Self {
        self.apply_style(&cc.egui_ctx);
        self
    }

    fn theme(&self) -> Theme {
        self.themes[self.theme_idx].clone()
    }

    /// Radius, zoom and font family come from the theme file, so they are
    /// pushed into the shared knobs before anything is painted.
    fn sync_theme_globals(&self) {
        if self.theme_idx + 1 == self.themes.len() {
            self.theme_file.file.apply_globals();
            th::set_zoom(self.theme_file.file.zoom);
        } else {
            th::set_radius_scale(1.0);
            th::set_mono(false);
            th::set_zoom(1.0);
        }
    }

    fn apply_style(&self, ctx: &egui::Context) {
        self.sync_theme_globals();
        let t = self.theme();
        let sum = t.text.r() as u32 + t.text.g() as u32 + t.text.b() as u32;
        let light = sum < 320;
        let mut style = (*ctx.style()).clone();
        let mut v = if light {
            egui::Visuals::light()
        } else {
            egui::Visuals::dark()
        };
        v.panel_fill = t.bg;
        // Windows and text fields stay opaque even in a glass theme, or their
        // contents become unreadable over the background photo.
        v.window_fill = th::opaque(t.card);
        v.extreme_bg_color = th::opaque(t.surface);
        v.faint_bg_color = t.card;
        v.override_text_color = Some(t.text);
        v.window_stroke = Stroke::new(1.0_f32, t.rail);
        v.selection.bg_fill = t.accent.gamma_multiply(0.45);
        v.widgets.inactive.bg_fill = t.surface;
        v.widgets.inactive.weak_bg_fill = t.surface;
        v.widgets.hovered.bg_fill = t.card_hover;
        v.widgets.hovered.weak_bg_fill = t.card_hover;
        v.widgets.active.bg_fill = t.card_hover;
        v.widgets.active.weak_bg_fill = t.card_hover;
        style.visuals = v;
        style.spacing.item_spacing = Vec2::new(8.0, 8.0);
        style.spacing.button_padding = Vec2::new(10.0, 5.0);
        // The file's font choice reaches egui's own widgets too, so a label and
        // a painted title never disagree about the typeface.
        if th::mono() {
            for (_, font) in style.text_styles.iter_mut() {
                font.family = egui::FontFamily::Monospace;
            }
        }
        ctx.set_style(style);
        ctx.set_zoom_factor(th::zoom());
    }

    /// What the player would do for every track in the real library: which
    /// file the title resolves to, and how many resolve to nothing.
    fn audio_report(&self) -> String {
        let mut out = format!(
            "folder={}\nindexed={}\nvolume={:.2}\nspotify_mode={}\n",
            if self.audio_dir.trim().is_empty() {
                audio::Index::default_roots()
                    .iter()
                    .map(|p| p.display().to_string())
                    .collect::<Vec<_>>()
                    .join(";")
            } else {
                self.audio_dir.clone()
            },
            self.audio_lib.len(),
            self.volume,
            self.spotify_mode
        );
        let mut hit = 0usize;
        let mut miss = 0usize;
        for c in &self.lib {
            for t in &c.tracks {
                let found = t
                    .path
                    .clone()
                    .or_else(|| self.audio_lib.find(&t.title, &t.artist));
                match found {
                    Some(p) => {
                        hit += 1;
                        if hit <= 12 {
                            out.push_str(&format!(
                                "HIT  {}\t{}\t{}\n",
                                c.name,
                                t.title,
                                p.display()
                            ));
                        }
                    }
                    None => {
                        miss += 1;
                        if miss <= 12 {
                            out.push_str(&format!(
                                "MISS {}\t{}\t{}\n",
                                c.name,
                                t.title,
                                t.artist
                            ));
                        }
                    }
                }
            }
        }
        out.push_str(&format!("hit={hit} miss={miss}\n"));
        out
    }

    /// Opens and plays the first real library track that has a file, through
    /// the same Player the UI uses, and reports whether sound advanced.
    fn audio_selftest(&self) {
        for c in &self.lib {
            for t in &c.tracks {
                if let Some(path) = self.audio_lib.find(&t.title, &t.artist) {
                    let mut player = audio::Player::new();
                    println!("selftest: {} -> {}", t.title, path.display());
                    player.play(&path, 0.0, self.volume);
                    std::thread::sleep(std::time::Duration::from_millis(1500));
                    println!(
                        "selftest: length_ms={} position_ms={}",
                        player.length_ms(),
                        player.position_ms()
                    );
                    return;
                }
            }
        }
        println!("selftest: no library track resolved to a file");
    }

    fn save_profile(&mut self) {
        self.profile.volume = self.volume;
        self.profile.theme = self.theme_idx;
        self.profile.save_to(&self.profile_path);
    }

    fn toggle_like(&mut self, key: &str) {
        self.profile.flip_like(key);
        self.save_profile();
    }

    // ------------------------------------------------------------- playback

    fn cur(&self) -> Option<&Track> {
        self.queue.get(self.now)
    }

    fn dur(&self) -> f32 {
        if self.spotify_playing() {
            if let Some(p) = &self.connect_playback {
                return p.duration_ms as f32 / 1000.0;
            }
        }
        // A file knows its own length even when the row came from a folder
        // scan with no metadata, so the scrubber and auto-advance still work.
        if self.audio.current().is_some() {
            let ms = self.audio.length_ms();
            if ms > 0 {
                return ms as f32 / 1000.0;
            }
        }
        self.cur().map(|t| t.secs as f32).unwrap_or(0.0)
    }

    fn log_current(&mut self) {
        let Some(track) = self.queue.get(self.now).cloned() else {
            return;
        };
        let mut genres = taste::genres_for(&self.lib, &track);
        if genres.is_empty() {
            genres = self.queue_genres.clone();
        }
        self.profile
            .record_play(&track.artist, &track.key(), &genres);
        self.save_profile();
    }

    fn play_from(&mut self, coll: &Collection, start: usize) {
        if coll.tracks.is_empty() {
            return;
        }
        self.queue = coll.tracks.clone();
        self.queue_genres = coll.genres.iter().map(|g| g.to_string()).collect();
        self.now = start.min(self.queue.len() - 1);
        self.pos = 0.0;
        self.playing = true;
        self.log_current();
        self.play_local(0.0);
    }

    /// Play the owner's own file for the current track. A track with no file
    /// falls back to its official 30-second preview, so a catalogue click
    /// makes sound without Spotify installed. Missing audio is not an error:
    /// the timer still runs so the rest of the app behaves normally.
    fn play_local(&mut self, from: f32) {
        // In Spotify mode the audio belongs to Spotify; starting rodio as
        // well would play two things at once.
        if self.spotify_playing() {
            return;
        }
        let Some(track) = self.cur().cloned() else {
            self.audio.stop();
            return;
        };
        let found = track
            .path
            .clone()
            .or_else(|| self.audio_lib.find(&track.title, &track.artist));
        match found {
            Some(path) => {
                self.previewing = false;
                self.preview_file = None;
                self.preview.cancel();
                self.audio.play(&path, from, self.volume);
            }
            None => {
                self.audio.stop();
                self.start_preview(&track);
            }
        }
    }

    /// Ask the catalogue APIs for this track's official preview. The download
    /// runs on its own thread; `poll_preview` starts the clip when it lands.
    fn start_preview(&mut self, track: &Track) {
        if self.spotify_playing() {
            return;
        }
        self.previewing = false;
        self.preview
            .request(&track.key(), &track.artist, &track.title);
    }

    fn poll_preview(&mut self, ctx: &egui::Context) {
        while let Some((key, result)) = self.preview.pump() {
            let Some(track) = self.cur().cloned() else {
                continue;
            };
            if track.key() != key || self.spotify_playing() {
                continue;
            }
            match result {
                Ok(clip) => {
                    self.preview_file = Some((key, clip.path.clone()));
                    // A paused window only stores the clip; play resumes it.
                    if self.playing && self.audio.current().is_none() {
                        self.audio.play(&clip.path, 0.0, self.volume);
                        self.pos = 0.0;
                        self.previewing = true;
                        self.notify(format!(
                            "Playing a {}-second preview of {} - no Spotify needed",
                            preview::CLIP_SECS,
                            clip.title
                        ));
                    }
                }
                Err(e) => self.notify(format!("No preview for this track ({e})")),
            }
        }
        if self.preview.pending().is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(300));
        }
    }

    fn set_playing(&mut self, on: bool) {
        if self.spotify_playing() {
            self.pending_play_uri = None;
            if !on {
                self.wake = None;
            }
            if on && self.connect_playback.is_none() {
                // Nothing is playing yet, so there may be no client to send
                // the command to. Look for one, opening Spotify if needed.
                self.begin_wake(true);
            } else {
                if on {
                    self.ensure_spotify_device();
                }
                self.connect_command(if on {
                    connect::Command::Play
                } else {
                    connect::Command::Pause
                });
            }
            self.playing = on;
            if let Some(p) = &mut self.connect_playback {
                p.playing = on;
            }
            self.connect_stamp = Some(std::time::Instant::now());
            return;
        }
        self.playing = on;
        if !on {
            self.audio.pause();
            return;
        }
        let found = self.cur().cloned().and_then(|t| {
            t.path
                .or_else(|| self.audio_lib.find(&t.title, &t.artist))
        });
        let already_open = match (&found, self.audio.current()) {
            (Some(path), Some(open)) => open == path.as_path(),
            _ => false,
        };
        if already_open {
            self.audio.resume();
            return;
        }
        // A preview that landed while paused is this track's sound; resume
        // it instead of asking the catalogue again.
        let ready = self
            .preview_file
            .as_ref()
            .filter(|(key, _)| self.cur().is_some_and(|t| t.key() == *key))
            .map(|(_, path)| path.clone());
        match ready {
            Some(path) => {
                self.audio.play(&path, 0.0, self.volume);
                self.previewing = true;
            }
            None => self.play_local(self.pos),
        }
    }

    fn rescan_audio(&mut self) {
        self.audio_lib = audio::Index::scan_roots(&self.audio_dir);
        self.attach_local_files();
        self.profile.audio_dir = self.audio_dir.clone();
        self.save_profile();
        if self.playing {
            self.play_local(self.pos);
        }
    }

    fn play_coll(&mut self, ci: usize, ti: usize) {
        let coll = self.lib[ci].clone();
        self.play_row(&coll, ti);
    }

    /// Plays one row from a collection view. A row that carries its own file is
    /// always the file on this PC, even in Spotify mode; the rest follow the
    /// mode: Spotify gets a name search, or the local index gets a last look.
    fn play_row(&mut self, coll: &Collection, ti: usize) {
        let track = coll.tracks.get(ti).cloned();
        if track.as_ref().is_some_and(|t| t.path.is_some()) {
            if self.spotify_connected() {
                // The file is the audio. Pause Spotify first so its stream and
                // the file never play over each other.
                self.connect_command(connect::Command::Pause);
                self.pending_play_uri = None;
                self.device_asked = None;
                self.wake = None;
                self.local_override = true;
            }
            self.play_from(coll, ti);
            return;
        }
        if self.spotify_connected() {
            // Spotify owns playback in this mode, and imported rows only carry
            // names, so hand the clicked track to Spotify by name.
            self.local_override = false;
            self.audio.stop();
            self.play_from(coll, ti);
            if let Some(track) = self.cur().cloned() {
                self.play_search_on_spotify(&track);
            }
            return;
        }
        // A click on a catalogued track still has a legal source without
        // Spotify: the official 30-second preview. Say what is happening
        // while the clip downloads instead of leaving the click silent.
        let missing = track
            .as_ref()
            .map(|t| self.audio_lib.find(&t.title, &t.artist).is_none())
            .unwrap_or(false);
        if missing {
            self.notify(format!(
                "No file on this PC - fetching a {}-second preview (connect Spotify for the full song)",
                preview::CLIP_SECS
            ));
        }
        self.play_from(coll, ti);
    }

    fn notify(&mut self, msg: impl Into<String>) {
        self.notice = Some((msg.into(), std::time::Instant::now()));
    }

    /// Play one file as a single-track queue, carrying the catalogue metadata
    /// so the player bar and Now Playing show what was picked.
    fn play_file(&mut self, path: &std::path::Path, hit: &catalog::Hit) {
        self.previewing = false;
        self.preview_file = None;
        self.preview.cancel();
        let (c1, c2) = palette_for(&hit.artist);
        self.queue = vec![Track {
            title: hit.title.clone(),
            artist: hit.artist.clone(),
            album: hit.album.clone(),
            secs: 0,
            c1,
            c2,
            added: String::new(),
            art: hit.art.clone(),
            path: Some(path.to_path_buf()),
        }];
        self.queue_genres.clear();
        self.now = 0;
        self.pos = 0.0;
        self.playing = true;
        self.audio.play(path, 0.0, self.volume);
        self.log_current();
    }

    fn rand(&mut self) -> u64 {
        self.rng = self
            .rng
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        self.rng >> 33
    }

    fn skip(&mut self, dir: isize) {
        if self.spotify_playing() {
            self.connect_command(if dir < 0 {
                connect::Command::Previous
            } else {
                connect::Command::Next
            });
            self.connect_stamp = Some(std::time::Instant::now());
            return;
        }
        if self.queue.is_empty() {
            return;
        }
        let len = self.queue.len() as isize;
        let mut i = self.now as isize + dir;
        if self.shuffle && dir > 0 {
            i = (self.rand() % self.queue.len() as u64) as isize;
        }
        self.now = (((i % len) + len) % len) as usize;
        self.pos = 0.0;
        self.playing = true;
        self.log_current();
        self.play_local(0.0);
    }

    fn advance(&mut self, ctx: &egui::Context) {
        let dt = ctx.input(|i| i.stable_dt).min(0.25);
        if let Some((_, at)) = &self.notice {
            if at.elapsed() > std::time::Duration::from_secs(7) {
                self.notice = None;
            }
        }
        if self.spotify_playing() {
            // Spotify is the clock: the polled position plus the time since it
            // arrived, so the bar moves smoothly between requests.
            let (progress, playing) = match &self.connect_playback {
                Some(p) => (p.progress_ms as f32 / 1000.0, p.playing),
                None => (0.0, false),
            };
            let drift = if playing {
                self.connect_stamp
                    .map(|t| t.elapsed().as_secs_f32())
                    .unwrap_or(0.0)
            } else {
                0.0
            };
            self.pos = progress + drift;
            ctx.request_repaint_after(std::time::Duration::from_millis(if playing {
                120
            } else {
                500
            }));
            return;
        }
        if self.playing {
            ctx.request_repaint_after(std::time::Duration::from_millis(80));
            let dur = self.dur();
            if dur > 0.0 {
                self.pos += dt;
                self.profile.listened_secs += dt as f64;
                if self.pos >= dur {
                    if self.repeat {
                        self.pos = 0.0;
                        self.play_local(0.0);
                    } else if self.now + 1 < self.queue.len() {
                        self.now += 1;
                        self.pos = 0.0;
                        self.log_current();
                        self.play_local(0.0);
                    } else {
                        self.pos = 0.0;
                        self.playing = false;
                        self.audio.stop();
                    }
                }
            }
            self.save_acc += dt;
            if self.save_acc > 20.0 {
                self.save_acc = 0.0;
                self.save_profile();
            }
        }
    }

    fn resolve(&self) -> Option<Collection> {
        match self.view {
            View::Collection(i) => self.lib.get(i).cloned(),
            View::Liked => Some(taste::liked_collection(&self.profile, &self.lib)),
            View::Mix(n) => Some(taste::mix(
                &taste::TasteSnapshot::build(&self.profile, &self.lib),
                &self.lib,
                n,
            )),
            View::Genre(g) => Some(taste::genre_collection(&self.lib, g)),
            _ => None,
        }
    }

    // ---------------------------------------------------------------- sidebar

    fn sidebar(&mut self, ui: &mut egui::Ui) {
        let t = self.theme();

        let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 34.0), Sense::hover());
        {
            let p = ui.painter();
            let c = rect.left_center() + Vec2::new(13.0, 0.0);
            th::logo(p, c, 11.0, th::brand(), Color32::WHITE);
            th::text(
                p,
                rect.left_center() + Vec2::new(32.0, 0.0),
                Align2::LEFT_CENTER,
                "Rustify",
                21.0,
                t.text,
            );
        }
        ui.add_space(16.0);

        let mut nav = None;
        if nav_row(ui, &t, "Home", NavIcon::Home, self.view == View::Home).clicked() {
            nav = Some(View::Home);
        }
        if nav_row(ui, &t, "Search", NavIcon::Search, self.view == View::Search).clicked() {
            nav = Some(View::Search);
        }
        if nav_row(ui, &t, "Import from Spotify", NavIcon::Import, self.view == View::Import).clicked() {
            nav = Some(View::Import);
        }
        if nav_row(ui, &t, "Taste profile", NavIcon::Taste, self.view == View::Taste).clicked() {
            nav = Some(View::Taste);
        }
        ui.add_space(14.0);

        ui.horizontal(|ui| {
            ui.label(RichText::new("Your Library").size(15.0).color(t.muted));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let like_key = self.cur().map(|c| c.key()).unwrap_or_default();
                let liked = self.profile.is_liked(&like_key);
                if mini_heart(ui, &t, liked).clicked() && !like_key.is_empty() {
                    self.toggle_like(&like_key);
                }
            });
        });
        ui.add_space(6.0);

        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            if chip(ui, &t, "All", self.lib_filter == LibFilter::All).clicked() {
                self.lib_filter = LibFilter::All;
            }
            if chip(ui, &t, "Playlists", self.lib_filter == LibFilter::Playlists).clicked() {
                self.lib_filter = LibFilter::Playlists;
            }
            if chip(ui, &t, "Albums", self.lib_filter == LibFilter::Albums).clicked() {
                self.lib_filter = LibFilter::Albums;
            }
        });
        ui.add_space(6.0);

        // Search in library: filters the list below, never the whole catalogue,
        // the way the filter field in Spotify's own sidebar works.
        let field = egui::TextEdit::singleline(&mut self.lib_query)
            .hint_text("Search in Your Library")
            .desired_width(ui.available_width())
            .text_color(t.text)
            .margin(egui::Margin::symmetric(10, 6));
        let resp = ui.add(field).on_hover_text("Right-click to paste");
        paste_on_right_click(&resp, &mut self.lib_query);
        ui.add_space(8.0);

        let mut open = None;
        let list_h = ui.available_height();
        egui::ScrollArea::vertical()
            .max_height(list_h)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                // A pinned collection is drawn above every unpinned row, in
                // every library state: the pills and the library search narrow
                // the rest of the list, never the pins.
                let pinned_rows: Vec<usize> = (0..self.lib.len())
                    .filter(|&i| self.profile.is_pinned(&self.lib[i].key()))
                    .collect();
                let mut pin_toggle: Option<String> = None;
                for i in pinned_rows {
                    let (clicked, toggle) = self.lib_row(ui, i);
                    if toggle.is_some() {
                        pin_toggle = toggle;
                    }
                    if clicked {
                        open = Some(View::Collection(i));
                    }
                }

                let liked_total = self.profile.liked.len();
                let (row, resp) =
                    ui.allocate_exact_size(Vec2::new(ui.available_width(), 52.0), Sense::click());
                let p = ui.painter();
                if resp.hovered() || self.view == View::Liked {
                    th::rr(p, row.shrink2(Vec2::new(0.0, 2.0)), 5, t.card_hover);
                }
                let art = Rect::from_min_size(row.min + Vec2::new(6.0, 6.0), Vec2::splat(40.0));
                artwork::paint(p, art, None, artwork::THUMB, rgb(0x5038a0), rgb(0xcbb6ff));
                th::heart(p, art.center(), 16.0, true, rgb(0xffffff));
                let tx = art.right() + 10.0;
                let clip = p.with_clip_rect(Rect::from_min_max(
                    egui::pos2(tx, row.top()),
                    egui::pos2(row.right() - 4.0, row.bottom()),
                ));
                th::text(
                    &clip,
                    egui::pos2(tx, row.center().y - 8.0),
                    Align2::LEFT_CENTER,
                    "Liked Songs",
                    13.5,
                    if self.view == View::Liked { t.accent } else { t.text },
                );
                th::text(
                    &clip,
                    egui::pos2(tx, row.center().y + 9.0),
                    Align2::LEFT_CENTER,
                    format!("Playlist \u{b7} {liked_total} songs"),
                    11.0,
                    t.muted,
                );
                if resp.clicked() {
                    open = Some(View::Liked);
                }

                let needle = self.lib_query.trim().to_lowercase();
                // Everything unpinned still obeys the pill and the library
                // search; pinned rows were drawn above and never dropped.
                let rest: Vec<usize> = (0..self.lib.len())
                    .filter(|&i| {
                        let coll = &self.lib[i];
                        !self.profile.is_pinned(&coll.key())
                            && (needle.is_empty() || coll.name.to_lowercase().contains(&needle))
                            && match self.lib_filter {
                                LibFilter::All => true,
                                LibFilter::Playlists => coll.kind != Kind::Album,
                                LibFilter::Albums => coll.kind == Kind::Album,
                            }
                    })
                    .collect();
                for i in rest {
                    let (clicked, toggle) = self.lib_row(ui, i);
                    if toggle.is_some() {
                        pin_toggle = toggle;
                    }
                    if clicked {
                        open = Some(View::Collection(i));
                    }
                }
                if let Some(key) = pin_toggle {
                    self.profile.flip_pin(&key);
                    self.save_profile();
                }
            });

        if let Some(v) = nav.or(open) {
            self.view = v;
        }
    }

    /// One collection row in the sidebar. Returns whether the row was clicked
    /// and the key of a collection whose pin the context menu toggled.
    fn lib_row(&self, ui: &mut egui::Ui, i: usize) -> (bool, Option<String>) {
        let t = self.theme();
        let coll = &self.lib[i];
        let key = coll.key();
        let pinned = self.profile.is_pinned(&key);
        let active = self.view == View::Collection(i);
        let (row, resp) =
            ui.allocate_exact_size(Vec2::new(ui.available_width(), 52.0), Sense::click());
        let mut pin_toggle = None;
        resp.context_menu(|ui| {
            let label = if pinned { "Unpin from top" } else { "Pin to top" };
            if ui.button(label).clicked() {
                pin_toggle = Some(key.clone());
                ui.close_menu();
            }
        });
        let p = ui.painter();
        if resp.hovered() || active {
            th::rr(p, row.shrink2(Vec2::new(0.0, 2.0)), 5, t.card_hover);
        }
        let art = Rect::from_min_size(row.min + Vec2::new(6.0, 6.0), Vec2::splat(40.0));
        artwork::paint(
            p,
            art,
            coll.art.pick(artwork::THUMB),
            artwork::THUMB,
            coll.c1,
            coll.c2,
        );
        let tx = art.right() + 10.0;
        let text_right = if pinned { row.right() - 24.0 } else { row.right() - 4.0 };
        let clip = p.with_clip_rect(Rect::from_min_max(
            egui::pos2(tx, row.top()),
            egui::pos2(text_right, row.bottom()),
        ));
        th::text(
            &clip,
            egui::pos2(tx, row.center().y - 8.0),
            Align2::LEFT_CENTER,
            &coll.name,
            13.5,
            if active { t.accent } else { t.text },
        );
        th::text(
            &clip,
            egui::pos2(tx, row.center().y + 9.0),
            Align2::LEFT_CENTER,
            format!(
                "{} \u{b7} {}",
                coll.kind.label().to_lowercase(),
                coll.subtitle
            ),
            11.0,
            t.muted,
        );
        if pinned {
            th::pin(p, egui::pos2(row.right() - 13.0, row.center().y), t.muted);
        }
        (resp.clicked(), pin_toggle)
    }

    // ------------------------------------------------------------------ body

    fn body(&mut self, ui: &mut egui::Ui) {
        self.header(ui);
        ui.add_space(4.0);
        if self.connect.is_none() && self.view != View::Import {
            self.connect_banner(ui);
        }

        let view = self.view;
        let resolved = match view {
            View::Home | View::Search | View::Taste | View::Import => None,
            _ => self.resolve(),
        };

        // `show_viewport` reports which part of the list is on screen, so the
        // track table below can build only the rows the owner can actually
        // see. A big imported playlist no longer lays out - or fetches art
        // for - hundreds of off-screen rows on every frame.
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show_viewport(ui, |ui, viewport| match view {
                View::Home => self.home(ui),
                View::Search => self.search(ui),
                View::Taste => self.taste(ui),
                View::Import => self.import_page(ui),
                _ => {
                    if let Some(coll) = resolved {
                        self.collection_view(ui, &coll, viewport);
                    }
                }
            });
    }

    fn header(&mut self, ui: &mut egui::Ui) {
        let t = self.theme();
        ui.horizontal(|ui| {
            let (r, resp) = ui.allocate_exact_size(Vec2::splat(30.0), Sense::click());
            {
                let p = ui.painter();
                th::rr(p, r, 15, if resp.hovered() { t.card_hover } else { t.card });
                th::chevron(
                    p,
                    r.center(),
                    9.0,
                    -1.0,
                    if self.view == View::Home { t.faint } else { t.text },
                );
            }
            if resp.clicked() {
                self.view = View::Home;
            }

            let (r2, _) = ui.allocate_exact_size(Vec2::splat(30.0), Sense::hover());
            {
                let p = ui.painter();
                th::rr(p, r2, 15, t.card);
                th::chevron(p, r2.center(), 9.0, 1.0, t.faint);
            }

            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 10.0;

                // profile chip -> taste profile
                let (r, chip_resp) = ui.allocate_exact_size(Vec2::new(112.0, 30.0), Sense::click());
                {
                    let p = ui.painter();
                    th::rr(p, r, 15, if chip_resp.hovered() { t.card_hover } else { t.card });
                    p.circle_filled(r.left_center() + Vec2::new(16.0, 0.0), 11.0, t.accent);
                    th::text(
                        p,
                        r.left_center() + Vec2::new(16.0, 0.0),
                        Align2::CENTER_CENTER,
                        "Y",
                        12.0,
                        t.on_accent,
                    );
                    th::text(
                        p,
                        r.left_center() + Vec2::new(33.0, 0.0),
                        Align2::LEFT_CENTER,
                        "You",
                        12.5,
                        t.text,
                    );
                }
                if chip_resp.clicked() {
                    self.view = View::Taste;
                }

                // queue toggle
                let (r, q_resp) = ui.allocate_exact_size(Vec2::splat(30.0), Sense::click());
                {
                    let p = ui.painter();
                    th::rr(p, r, 15, if q_resp.hovered() { t.card_hover } else { t.card });
                    th::queue_icon(p, r.center(), if self.show_queue { t.accent } else { t.text });
                }
                if q_resp.clicked() {
                    self.show_queue = !self.show_queue;
                }

                // theme picker
                let (r, g_resp) = ui.allocate_exact_size(Vec2::splat(30.0), Sense::click());
                {
                    let p = ui.painter();
                    th::rr(p, r, 15, if g_resp.hovered() { t.card_hover } else { t.card });
                    th::gear_icon(p, r.center(), t.text);
                }
                if g_resp.clicked() {
                    self.show_themes = !self.show_themes;
                }

                // search field: shrink before it can reach the nav buttons
                ui.add_space(4.0);
                let field_w = (ui.available_width() - 10.0).clamp(90.0, 300.0);
                // egui paints hint text at the widget's text origin, which the
                // margin only partly accounts for, so the field draws its own
                // placeholder clear of the magnifier instead.
                let field = egui::TextEdit::singleline(&mut self.query)
                    .desired_width(field_w)
                    .text_color(t.text)
                    .margin(egui::Margin::symmetric(34, 8));
                let resp = ui.add(field).on_hover_text("Right-click to paste");
                paste_on_right_click(&resp, &mut self.query);
                th::search_icon(
                    ui.painter(),
                    resp.rect.left_center() + Vec2::new(17.0, 0.0),
                    6.5,
                    t.muted,
                );
                if self.query.is_empty() {
                    let clip = ui.painter().with_clip_rect(resp.rect);
                    th::text(
                        &clip,
                        resp.rect.left_center() + Vec2::new(34.0, 0.0),
                        Align2::LEFT_CENTER,
                        "What do you want to play?",
                        13.5,
                        t.muted,
                    );
                }
                if resp.has_focus() && self.view != View::Search {
                    self.view = View::Search;
                }
            });
        });
    }

    /// One click into the Spotify sign-in. Without a session every row can
    /// only play a preview; this login is what makes Spotify the full-song
    /// player, so the way in should not be buried on the Import page.
    fn connect_banner(&mut self, ui: &mut egui::Ui) {
        let t = self.theme();
        egui::Frame::new()
            .fill(t.card)
            .corner_radius(egui::CornerRadius::same(8))
            .inner_margin(egui::Margin::symmetric(12, 8))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if self.connect_busy {
                            let cancel = egui::Button::new(
                                RichText::new("Cancel sign-in").color(t.text),
                            )
                            .fill(t.card_hover);
                            if ui.add(cancel).clicked() {
                                self.cancel_connect();
                            }
                        } else {
                            let button = egui::Button::new(
                                RichText::new("Connect Spotify").color(t.on_accent),
                            )
                            .fill(t.accent);
                            if ui.add(button).clicked() {
                                self.view = View::Import;
                                self.start_connect();
                            }
                        }
                        ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                            ui.label(
                                RichText::new(
                                    "Spotify is not connected - rows can only play a 30-second preview. Connect once for full songs (Premium).",
                                )
                                .size(12.0)
                                .color(t.muted),
                            );
                        });
                    });
                });
            });
        ui.add_space(8.0);
    }

    fn home(&mut self, ui: &mut egui::Ui) {
        let t = self.theme();
        ui.spacing_mut().item_spacing.x = 10.0;

        ui.label(RichText::new(greeting()).size(30.0).color(t.text));
        ui.add_space(2.0);
        let taste = taste::TasteSnapshot::build(&self.profile, &self.lib);
        let line = match taste.top_genre() {
            Some(g) => format!("Because you have been into {g} lately"),
            None => "Play something and this page will rearrange itself around you".to_string(),
        };
        ui.label(RichText::new(line).size(12.5).color(t.muted));
        ui.add_space(14.0);

        let mut pins: Vec<(String, Option<String>, Color32, Color32, View)> = vec![
            (
                "Liked Songs".to_string(),
                None,
                rgb(0x5038a0),
                rgb(0xcbb6ff),
                View::Liked,
            ),
            (
                "Daily Mix 1".to_string(),
                None,
                rgb(0x1e5b8f),
                rgb(0x7fd8e8),
                View::Mix(0),
            ),
            (
                "Daily Mix 2".to_string(),
                None,
                rgb(0x5a1f6e),
                rgb(0xff7ad9),
                View::Mix(1),
            ),
            (
                "Daily Mix 3".to_string(),
                None,
                rgb(0x2f4f3f),
                rgb(0xa8d8b9),
                View::Mix(2),
            ),
        ];
        for i in taste::recommended(&taste, &self.lib).into_iter().take(2) {
            let c = &self.lib[i];
            pins.push((
                c.name.clone(),
                c.art.pick(artwork::TILE).map(str::to_string),
                c.c1,
                c.c2,
                View::Collection(i),
            ));
        }

        let cols = 3usize;
        let tile_w = (ui.available_width() - 10.0 * (cols as f32 - 1.0)) / cols as f32;
        let mut open = None;
        for row in 0..2 {
            ui.horizontal(|ui| {
                for k in 0..cols {
                    let i = row * cols + k;
                    if i >= pins.len() {
                        break;
                    }
                    let (name, cover, c1, c2, view) = pins[i].clone();
                    if pin_tile(ui, &t, &name, cover.as_deref(), c1, c2, tile_w).clicked() {
                        open = Some(view);
                    }
                }
            });
            ui.add_space(10.0);
        }

        ui.add_space(10.0);
        ui.label(RichText::new("Made for you").size(21.0).color(t.text));
        ui.add_space(8.0);
        // The playlists imported from Spotify come first: this is the owner's
        // own music, so it should lead the grid instead of sitting behind the
        // built-in demo catalogue.
        let mut order: Vec<usize> = (0..self.lib.len()).collect();
        order.sort_by_key(|&i| !self.lib[i].imported());
        ui.horizontal_wrapped(|ui| {
            for i in order {
                let c = &self.lib[i];
                if card(
                    ui,
                    &t,
                    &c.name,
                    &c.subtitle,
                    c.art.pick(artwork::TILE),
                    c.c1,
                    c.c2,
                    166.0,
                )
                .clicked()
                {
                    open = Some(View::Collection(i));
                }
            }
        });

        section(ui, &t, "Recommended for you");
        ui.horizontal_wrapped(|ui| {
            for i in taste::recommended(&taste, &self.lib)
                .into_iter()
                .take(5)
            {
                let c = &self.lib[i];
                if card(
                    ui,
                    &t,
                    &c.name,
                    &c.subtitle,
                    c.art.pick(artwork::TILE),
                    c.c1,
                    c.c2,
                    166.0,
                )
                .clicked()
                {
                    open = Some(View::Collection(i));
                }
            }
        });

        let artists: Vec<(String, u32)> = taste.played_artists.iter().take(5).cloned().collect();
        if !artists.is_empty() {
            section(ui, &t, "Your top artists");
            let mut picked: Option<String> = None;
            ui.horizontal_wrapped(|ui| {
                for (name, plays) in &artists {
                    if artist_card(ui, &t, name, *plays, 150.0).clicked() {
                        picked = Some(name.clone());
                    }
                }
            });
            if let Some(name) = picked {
                self.query = name;
                self.view = View::Search;
            }
        }

        let recent = taste::recent_collections(&self.profile, &self.lib, 5);
        if !recent.is_empty() {
            section(ui, &t, "Jump back in");
            ui.horizontal_wrapped(|ui| {
                for i in recent {
                    let c = &self.lib[i];
                    if card(
                    ui,
                    &t,
                    &c.name,
                    &c.subtitle,
                    c.art.pick(artwork::TILE),
                    c.c1,
                    c.c2,
                    166.0,
                )
                .clicked()
                {
                        open = Some(View::Collection(i));
                    }
                }
            });
        }

        ui.add_space(26.0);
        if let Some(v) = open {
            self.view = v;
        }
    }

    fn search(&mut self, ui: &mut egui::Ui) {
        let t = self.theme();
        let q = self.query.trim().to_lowercase();
        let mut open = None;

        if q.is_empty() {
            ui.label(RichText::new("Browse all").size(24.0).color(t.text));
            ui.add_space(12.0);
            let cols = 3usize;
            let gap = 10.0;
            let w = (ui.available_width() - gap * (cols as f32 - 1.0)) / cols as f32;

            let mut seen: Vec<&'static str> = Vec::new();
            for g in GENRES {
                if !seen.contains(&g) {
                    seen.push(g);
                }
            }
            for row in 0..3 {
                ui.horizontal(|ui| {
                    for k in 0..cols {
                        let i = row * cols + k;
                        if i >= seen.len() {
                            break;
                        }
                        let g = seen[i];
                        if genre_tile(ui, &t, g, w).clicked() {
                            open = Some(View::Genre(g));
                        }
                    }
                });
                ui.add_space(gap);
            }
            ui.add_space(20.0);
            if let Some(v) = open {
                self.view = v;
            }
            return;
        }

        // Without a session the page can only offer library matches; say that
        // up front instead of letting "Spotify" rows look playable.
        if !self.spotify_mode {
            let (r, resp) = ui.allocate_exact_size(
                Vec2::new(ui.available_width(), 40.0),
                Sense::click(),
            );
            let p = ui.painter();
            th::rr(p, r, 6, t.card);
            th::text(
                &p,
                r.left_center() + Vec2::new(12.0, 0.0),
                Align2::LEFT_CENTER,
                "Spotify catalogue and playback are off - click to connect your account",
                12.0,
                t.muted,
            );
            if resp.clicked() {
                self.view = View::Import;
            }
            ui.add_space(12.0);
        }

        let mut hits: Vec<(usize, usize)> = Vec::new();
        for (ci, coll) in self.lib.iter().enumerate() {
            for (ti, t2) in coll.tracks.iter().enumerate() {
                if t2.title.to_lowercase().contains(&q)
                    || t2.artist.to_lowercase().contains(&q)
                    || t2.album.to_lowercase().contains(&q)
                {
                    hits.push((ci, ti));
                }
            }
        }

        // Real Spotify results first, when a session exists. Everything below
        // this block is the library fallback, so an error or a missing session
        // still fills the page with matches.
        if self.spotify_mode {
            let mut play: Option<catalog::Hit> = None;
            let mut queue: Option<catalog::Hit> = None;
            if !self.search_hits.is_empty() {
                ui.label(RichText::new("From Spotify").size(24.0).color(t.text));
                ui.add_space(4.0);
                let hits = self.search_hits.clone();
                for hit in &hits {
                    let out = catalog_row(ui, &t, hit);
                    if out.play {
                        play = Some(hit.clone());
                    }
                    if out.queue {
                        queue = Some(hit.clone());
                    }
                }
                ui.add_space(20.0);
            } else if self.search_busy {
                ui.label(
                    RichText::new("Searching Spotify...")
                        .size(13.0)
                        .color(t.muted),
                );
                ui.add_space(14.0);
            } else if let Some(e) = self.search_error.clone() {
                ui.label(
                    RichText::new(format!("Spotify search unavailable: {e}"))
                        .size(12.0)
                        .color(t.muted),
                );
                ui.label(
                    RichText::new("Showing what your library has instead.")
                        .size(12.0)
                        .color(t.faint),
                );
                ui.add_space(14.0);
            }
            if let Some(hit) = play {
                self.play_hit(&hit);
            }
            if let Some(hit) = queue {
                self.queue_hit(&hit);
            }
            ui.label(RichText::new("In your library").size(20.0).color(t.text));
            ui.add_space(4.0);
        } else {
            ui.label(RichText::new("Songs").size(24.0).color(t.text));
            ui.label(
                RichText::new("Connect Spotify playback in Settings to search the whole catalogue.")
                    .size(11.5)
                    .color(t.faint),
            );
            ui.add_space(4.0);
        }
        ui.add_space(6.0);

        if hits.is_empty() {
            ui.label(
                RichText::new(format!("No results for \"{}\"", self.query.trim()))
                    .size(14.0)
                    .color(t.muted),
            );
            return;
        }

        ui.label(
            RichText::new(format!("{} results", hits.len()))
                .size(12.0)
                .color(t.faint),
        );
        ui.add_space(6.0);

        let mut play: Option<(usize, usize)> = None;
        for (n, (ci, ti)) in hits.iter().take(40).enumerate() {
            let track = self.lib[*ci].tracks[*ti].clone();
            let out = track_row(
                ui,
                &t,
                &track,
                n + 1,
                false,
                false,
                true,
                false,
                self.profile.is_liked(&track.key()),
            );
            if out.heart {
                self.toggle_like(&track.key());
            } else if out.row.clicked() {
                play = Some((*ci, *ti));
            }
        }
        ui.add_space(20.0);

        if let Some((ci, ti)) = play {
            self.play_coll(ci, ti);
        }
    }

    fn taste(&mut self, ui: &mut egui::Ui) {
        let t = self.theme();
        let snap = taste::TasteSnapshot::build(&self.profile, &self.lib);
        let artists = snap.played_artists.clone();
        let genres = snap.played_genres.clone();
        let plays = snap.plays;
        let minutes = (self.profile.listened_secs / 60.0).round() as u32;
        let likes = self.profile.liked.len();
        let artist_count = snap.played_artists.len();
        let personality = snap.personality();

        ui.label(RichText::new("Your taste profile").size(30.0).color(t.text));
        ui.add_space(4.0);
        ui.label(RichText::new(personality).size(15.0).color(t.accent));
        ui.add_space(2.0);
        ui.label(
            RichText::new(format!(
                "Learned from {plays} plays across {artist_count} artists, saved on this PC"
            ))
            .size(12.0)
            .color(t.muted),
        );
        if snap.imported_tracks > 0 {
            ui.label(
                RichText::new(format!(
                    "+ {} imported tracks in {} playlists feed the genres below",
                    snap.imported_tracks, snap.imported_playlists
                ))
                .size(12.0)
                .color(t.muted),
            );
        }
        ui.add_space(16.0);

        let box_w = (ui.available_width() - 30.0) / 4.0;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 10.0;
            stat_box(ui, &t, &format!("{minutes}"), "minutes listened", box_w);
            stat_box(ui, &t, &format!("{plays}"), "songs played", box_w);
            stat_box(ui, &t, &format!("{likes}"), "liked songs", box_w);
            let top = snap.top_genre().unwrap_or("-").to_string();
            stat_box(ui, &t, &top, "top genre", box_w);
        });

        section(ui, &t, "What you listen to most");
        if genres.is_empty() {
            ui.label(
                RichText::new("No plays yet - the imported genres below set the tone.")
                    .size(13.0)
                    .color(t.muted),
            );
        } else {
            genre_bars(ui, &t, &genres, "plays");
        }

        if !snap.imported_genres.is_empty() {
            section(ui, &t, "From your imported playlists");
            ui.label(
                RichText::new(format!(
                    "{} playlists · {} tracks",
                    snap.imported_playlists, snap.imported_tracks
                ))
                .size(11.5)
                .color(t.muted),
            );
            ui.add_space(6.0);
            genre_bars(ui, &t, &snap.imported_genres, "tracks");
        }

        section(ui, &t, "Your artists");
        if artists.is_empty() {
            ui.label(
                RichText::new("No favourite artists yet.")
                    .size(13.0)
                    .color(t.muted),
            );
        } else {
            let max = artists.first().map(|(_, c)| *c).unwrap_or(1).max(1);
            let mut picked: Option<String> = None;
            for (name, count) in &artists {
                let (rect, resp) =
                    ui.allocate_exact_size(Vec2::new(ui.available_width(), 34.0), Sense::click());
                let p = ui.painter();
                if resp.hovered() {
                    th::rr(p, rect, 4, t.card);
                }
                let (c1, c2) = palette_for(name);
                artwork::paint(
                    p,
                    Rect::from_min_size(rect.min + Vec2::new(4.0, 4.0), Vec2::splat(26.0)),
                    None,
                    artwork::THUMB,
                    c1,
                    c2,
                );
                th::text(
                    p,
                    rect.left_center() + Vec2::new(40.0, 0.0),
                    Align2::LEFT_CENTER,
                    name,
                    13.5,
                    t.text,
                );
                th::stat_bar(
                    p,
                    Rect::from_min_size(
                        egui::pos2(rect.left() + 220.0, rect.center().y - 4.0),
                        Vec2::new((rect.width() - 320.0).max(40.0), 8.0),
                    ),
                    *count as f32 / max as f32,
                    t.card_hover,
                    c2,
                );
                th::text(
                    p,
                    rect.right_center() - Vec2::new(4.0, 0.0),
                    Align2::RIGHT_CENTER,
                    format!("{count} plays"),
                    11.5,
                    t.muted,
                );
                if resp.clicked() {
                    picked = Some(name.clone());
                }
            }
            if let Some(name) = picked {
                self.query = name;
                self.view = View::Search;
            }
        }

        if !snap.imported_artists.is_empty() {
            section(ui, &t, "Artists in your imports");
            let max = snap
                .imported_artists
                .first()
                .map(|(_, c)| *c)
                .unwrap_or(1)
                .max(1);
            for (name, count) in snap.imported_artists.iter().take(8) {
                let (rect, resp) =
                    ui.allocate_exact_size(Vec2::new(ui.available_width(), 34.0), Sense::hover());
                let p = ui.painter();
                if resp.hovered() {
                    th::rr(p, rect, 4, t.card);
                }
                let (c1, c2) = palette_for(name);
                artwork::paint(
                    p,
                    Rect::from_min_size(rect.min + Vec2::new(4.0, 4.0), Vec2::splat(26.0)),
                    None,
                    artwork::THUMB,
                    c1,
                    c2,
                );
                th::text(
                    p,
                    rect.left_center() + Vec2::new(40.0, 0.0),
                    Align2::LEFT_CENTER,
                    name,
                    13.5,
                    t.text,
                );
                th::stat_bar(
                    p,
                    Rect::from_min_size(
                        egui::pos2(rect.left() + 220.0, rect.center().y - 4.0),
                        Vec2::new((rect.width() - 320.0).max(40.0), 8.0),
                    ),
                    *count as f32 / max as f32,
                    t.card_hover,
                    c2,
                );
                th::text(
                    p,
                    rect.right_center() - Vec2::new(4.0, 0.0),
                    Align2::RIGHT_CENTER,
                    format!("{count} tracks"),
                    11.5,
                    t.muted,
                );
            }
        }

        section(ui, &t, "On repeat");
        let mut top_tracks: Vec<(String, u32)> = self
            .profile
            .track_plays
            .iter()
            .map(|(k, c)| (k.clone(), *c))
            .collect();
        top_tracks.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        top_tracks.truncate(6);

        let mut list: Vec<Track> = Vec::new();
        for (k, _) in &top_tracks {
            if let Some(t2) = taste::track_by_key(&self.lib, k) {
                list.push(t2.clone());
            }
        }

        if list.is_empty() {
            ui.label(
                RichText::new("Play a few songs and your repeats show up here.")
                    .size(13.0)
                    .color(t.muted),
            );
        } else {
            let coll = Collection::new(
                "On repeat",
                Kind::Playlist,
                format!("{} tracks", list.len()),
                rgb(0x3a2352),
                rgb(0x8f7bd6),
                vec!["mix"],
                list.clone(),
            );
            let mut play = None;
            for (i, track) in list.iter().enumerate() {
                let out = track_row(
                    ui,
                    &t,
                    track,
                    i + 1,
                    false,
                    false,
                    true,
                    false,
                    self.profile.is_liked(&track.key()),
                );
                if out.heart {
                    self.toggle_like(&track.key());
                } else if out.row.clicked() {
                    play = Some(i);
                }
            }
            if let Some(i) = play {
                self.play_row(&coll, i);
            }
        }

        ui.add_space(22.0);
        if ui.button("Reset taste profile").clicked() {
            self.profile.clear_stats();
            self.save_profile();
        }
        ui.add_space(24.0);
    }

    // --------------------------------------------------------- spotify import

    fn with_imports_at(mut self, imports_path: PathBuf) -> Self {
        self.client_id = self.profile.client_id.clone();
        self.imports_path = imports_path;
        self.imported = spotify::load(&self.imports_path);
        self.rebuild_lib();
        self.queue = self.lib[0].tracks.clone();
        self.queue_genres = self.lib[0].genres.iter().map(|g| g.to_string()).collect();
        self
    }

    fn rebuild_lib(&mut self) {
        let mut lib = library::sample();
        lib.extend(spotify::to_collections(&self.imported));
        taste::assign_imported_genres(&mut lib);
        self.lib = lib;
        if self.now >= self.queue.len() {
            self.now = 0;
        }
    }

    /// Machine-readable dump of the state the window opens with, produced by
    /// the same loader the launch path uses. Used to verify the real store.
    /// The one personalization probe: real plays plus the inferred genres and
    /// artists of imported playlists. Every surface and check reads this.
    fn taste_snapshot(&self) -> taste::TasteSnapshot {
        taste::TasteSnapshot::build(&self.profile, &self.lib)
    }

    fn library_report(&self) -> String {
        let catalogue = self.lib.len().saturating_sub(self.imported.len());
        let imported_tracks: usize = self.imported.iter().map(|p| p.tracks.len()).sum();
        let mut out = String::new();
        out.push_str(&format!("profile={}\n", self.profile_path.display()));
        out.push_str(&format!("store={}\n", self.imports_path.display()));
        out.push_str(&format!("catalogue={catalogue}\n"));
        out.push_str(&format!("imported_playlists={}\n", self.imported.len()));
        out.push_str(&format!("imported_tracks={imported_tracks}\n"));
        out.push_str(&format!("library={}\n", self.lib.len()));
        for c in &self.lib {
            out.push_str(&format!(
                "collection={}|{}|{}|{}\n",
                c.kind.label(),
                c.name,
                c.tracks.len(),
                c.total_secs()
            ));
        }

        // Proof that an imported track feeds personalization through the same
        // probe the taste page reads (kept in memory so the real profile is
        // never touched by a report).
        let snap = self.taste_snapshot();
        if let Some(p) = &snap.sample {
            out.push_str(&format!("probe_playlist={}\n", p.playlist));
            out.push_str(&format!("probe_track={}|{}\n", p.title, p.artist));
            if let Some((name, plays)) = snap.artists.first() {
                out.push_str(&format!("taste_artist={name}|{plays}\n"));
            }
            out.push_str(&format!("taste_plays={}\n", snap.plays));
        }

        // Proof that a real-artist catalogue track still maps to genres.
        if let Some(c) = self.lib.iter().find(|c| {
            c.kind == Kind::Album && !c.genres.is_empty() && !c.imported()
        }) {
            let track = &c.tracks[0];
            out.push_str(&format!(
                "catalogue_probe={}|{}|{}\n",
                c.name,
                track.artist,
                taste::genres_for(&self.lib, track).join("+")
            ));
        }
        out
    }

    /// Self-asserting half of `--library-report`: every required playlist must
    /// be in the launch library with tracks, the catalogue must be present, and
    /// playing an imported track must reach both the artist and genre profile.
    fn verify_library(&self) -> Result<String, Vec<String>> {
        let mut errors: Vec<String> = Vec::new();
        let mut out = String::new();
        for name in REQUIRED_PLAYLISTS {
            match self.lib.iter().find(|c| c.name == name) {
                None => errors.push(format!("playlist missing from the launch library: {name}")),
                Some(c) if c.tracks.is_empty() => {
                    errors.push(format!("playlist has no tracks: {name}"))
                }
                Some(c) => out.push_str(&format!(
                    "ok playlist {name}: {} tracks\n",
                    c.tracks.len()
                )),
            }
        }

        let catalogue: Vec<&Collection> =
            self.lib.iter().filter(|c| !c.imported()).collect();
        let catalogue_tracks: usize = catalogue.iter().map(|c| c.tracks.len()).sum();
        if catalogue.is_empty() || catalogue_tracks == 0 {
            errors.push("catalogue is empty".to_string());
        } else {
            out.push_str(&format!(
                "ok catalogue: {} collections, {catalogue_tracks} tracks\n",
                catalogue.len()
            ));
        }

        let snap = self.taste_snapshot();
        match &snap.sample {
            None => errors.push("no imported playlist with tracks to probe".to_string()),
            Some(sample) => {
                if sample.genres.is_empty() {
                    errors.push(format!("no genre mapping for imported track: {}", sample.title));
                } else {
                    out.push_str(&format!(
                        "ok taste: {} -> {}\n",
                        sample.artist,
                        sample.genres.join("+")
                    ));
                }
                if snap.artist_weight(&sample.artist) == 0 {
                    errors.push(format!("taste profile did not record artist: {}", sample.artist));
                }
                if sample.genre_plays == 0 {
                    errors.push(format!("taste profile did not record a genre for: {}", sample.title));
                }
            }
        }

        if errors.is_empty() {
            Ok(out)
        } else {
            Err(errors)
        }
    }

    /// Exactly the numbers the taste page draws, on the same probe.
    fn taste_report(&self) -> String {
        let snap = self.taste_snapshot();
        let mut out = String::new();
        out.push_str(&format!("personality={}\n", snap.personality()));
        out.push_str(&format!("plays={}\n", snap.plays));
        out.push_str(&format!(
            "imported_playlists={}\nimported_tracks={}\n",
            snap.imported_playlists, snap.imported_tracks
        ));
        match snap.top_genre() {
            Some(g) => out.push_str(&format!("top_genre={g}|{}\n", snap.genre_weight(g))),
            None => out.push_str("top_genre=-\n"),
        }
        for (label, rows) in [
            ("played_genre", &snap.played_genres),
            ("imported_genre", &snap.imported_genres),
        ] {
            for (name, count) in rows.iter().take(8) {
                out.push_str(&format!("{label}={name}|{count}\n"));
            }
        }
        for (label, rows) in [
            ("played_artist", &snap.played_artists),
            ("imported_artist", &snap.imported_artists),
            ("artist", &snap.artists),
        ] {
            for (name, count) in rows.iter().take(8) {
                out.push_str(&format!("{label}={name}|{count}\n"));
            }
        }
        if let Some(sample) = &snap.sample {
            out.push_str(&format!("sample_playlist={}\n", sample.playlist));
            out.push_str(&format!("sample_track={}|{}\n", sample.title, sample.artist));
            out.push_str(&format!("sample_genres={}\n", sample.genres.join("+")));
            out.push_str(&format!("sample_artist_plays={}\n", sample.artist_plays));
            out.push_str(&format!("sample_genre_plays={}\n", sample.genre_plays));
        }
        for n in 0..3 {
            let m = taste::mix(&snap, &self.lib, n);
            let head: Vec<String> = m.tracks.iter().take(3).map(|t| t.key()).collect();
            out.push_str(&format!(
                "mix{}={}|{}|{}\n",
                n + 1,
                m.name,
                m.subtitle,
                head.join(",")
            ));
        }
        out
    }

    fn start_import(&mut self) {
        self.start_import_with(spotify::Options::default());
    }

    fn start_import_with(&mut self, options: spotify::Options) {
        if self.import_busy {
            return;
        }
        let client_id = self.client_id.trim().to_string();
        if client_id.is_empty() {
            self.import_log
                .push_str("Paste your Spotify app Client ID first.\n");
            return;
        }
        self.profile.client_id = client_id.clone();
        self.save_profile();
        self.import_busy = true;
        self.import_log.clear();
        self.import_error = None;
        self.import_log.push_str("Starting...\n");

        let (tx, rx) = std::sync::mpsc::channel();
        self.import_rx = Some(rx);
        std::thread::spawn(move || {
            let logger = tx.clone();
            let mut log = move |m: String| {
                let _ = logger.send(spotify::Msg::Log(m));
            };
            let result = spotify::run_import_with_options(&client_id, &options, &mut log);
            let _ = tx.send(spotify::Msg::Done(result));
        });
    }

    // -------------------------------------------------- background & presence

    /// Loads the custom theme's photo, blurred once, and only when the path or
    /// the blur amount actually changes.
    fn sync_background(&mut self, ctx: &egui::Context) {
        let on_custom = self.theme_idx == th::custom_index();
        let path = self.profile.custom.image.trim().to_string();
        let key = if on_custom && !path.is_empty() {
            format!("{}|{:.2}", path, self.profile.custom.blur)
        } else {
            String::new()
        };
        if key == self.bg_key {
            return;
        }
        self.bg_key = key.clone();
        self.bg_tex = None;
        self.bg_error = None;
        if key.is_empty() {
            return;
        }
        let decoded = std::fs::read(&path)
            .map_err(|e| format!("cannot read {path}: {e}"))
            .and_then(|bytes| {
                image::load_from_memory(&bytes).map_err(|e| format!("not a readable image: {e}"))
            });
        match decoded {
            Ok(img) => {
                let rgba = img.to_rgba8();
                let (w, h) = rgba.dimensions();
                let (pixels, bw, bh) =
                    blur_rgba(rgba.into_raw(), w, h, self.profile.custom.blur);
                let color = egui::ColorImage::from_rgba_unmultiplied(
                    [bw as usize, bh as usize],
                    &pixels,
                );
                self.bg_tex = Some(ctx.load_texture(
                    "rustify-background",
                    color,
                    egui::TextureOptions::LINEAR,
                ));
            }
            Err(e) => self.bg_error = Some(e),
        }
    }

    /// Gradient, then photo, then a dim layer. Painted into the background
    /// layer so every translucent panel composites over it.
    fn paint_background(&mut self, ctx: &egui::Context) {
        let t = self.theme();
        let rect = ctx.screen_rect();
        if rect.width() <= 0.0 {
            return;
        }
        let p = ctx.layer_painter(egui::LayerId::background());
        p.rect_filled(rect, 0, t.base());
        if let Some((top, bottom)) = t.gradient {
            th::gradient(&p, rect, top, bottom);
        }
        if let Some(tex) = &self.bg_tex {
            let size = tex.size_vec2();
            if size.x > 0.0 && size.y > 0.0 {
                // Cover, not stretch: fill the window and crop the overflow.
                let scale = (rect.width() / size.x).max(rect.height() / size.y);
                let fitted = Rect::from_center_size(rect.center(), size * scale);
                p.image(
                    tex.id(),
                    fitted,
                    Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                    Color32::WHITE,
                );
            }
            let dim = self.profile.custom.dim.clamp(0.0, 1.0);
            if dim > 0.0 {
                p.rect_filled(rect, 0, Color32::from_black_alpha((dim * 235.0) as u8));
            }
        }
    }

    /// Feeds the Discord worker whatever is playing, and only when it changes.
    fn pump_discord(&mut self, ctx: &egui::Context) {
        let Some(handle) = &self.discord else {
            return;
        };
        while let Ok(line) = handle.status.try_recv() {
            self.discord_status = line;
        }
        let tx = handle.tx.clone();
        let track = if self.spotify_playing() {
            self.connect_playback.as_ref().map(|p| {
                (p.track.clone(), p.artist.clone(), p.playing, p.progress_ms)
            })
        } else {
            self.cur().map(|tr| {
                (
                    tr.title.clone(),
                    tr.artist.clone(),
                    self.playing,
                    (self.pos * 1000.0).max(0.0) as u64,
                )
            })
        };
        let now = track.map(|(track, artist, playing, elapsed_ms)| discord::Now {
            track,
            artist,
            playing,
            elapsed_ms,
        });
        if now != self.discord_sent {
            self.discord_sent = now.clone();
            let _ = tx.send(now);
        }
        ctx.request_repaint_after(std::time::Duration::from_millis(900));
    }

    fn start_discord(&mut self) {
        let id = self.profile.discord_client_id.trim().to_string();
        if id.is_empty() {
            self.discord_status =
                "Paste your Discord application ID first".to_string();
            return;
        }
        self.profile.discord_client_id = id.clone();
        self.discord = Some(discord::spawn(id));
        self.discord_sent = None;
        self.discord_status = "Starting...".to_string();
        self.save_profile();
    }

    /// Stops the worker. The card is cleared first so it does not linger on the
    /// profile after presence is switched off.
    fn stop_discord(&mut self, clear_card: bool) {
        if clear_card {
            if let Some(handle) = &self.discord {
                let _ = handle.tx.send(None);
                std::thread::sleep(std::time::Duration::from_millis(120));
            }
        }
        self.discord = None;
        self.discord_sent = None;
        self.discord_status.clear();
    }

    /// Rebuilds the custom palette after an edit and switches to it.
    fn apply_custom(&mut self) {
        if let Some(slot) = self.themes.get_mut(th::custom_index()) {
            *slot = self.profile.custom.theme();
        }
        self.theme_idx = th::custom_index();
        self.profile.theme = self.theme_idx;
        self.save_profile();
    }

    // ------------------------------------------------------- spotify control

    /// True when the transport controls drive the owner's Spotify app instead
    /// of the local files. Spotify still plays the audio itself.
    fn spotify_connected(&self) -> bool {
        self.spotify_mode && self.conn_tx.is_some()
    }

    /// True when Spotify is the one making sound. A file picked from "Local
    /// files" takes the transport back without dropping the session.
    fn spotify_playing(&self) -> bool {
        self.spotify_connected() && !self.local_override
    }

    /// One short line explaining silence, shown under the player-bar title.
    /// Without it a press of play just moves the clock and nobody knows why.
    fn audio_status(&self) -> Option<&'static str> {
        if self.spotify_playing() {
            if self.wake.is_some() {
                return Some("waiting for Spotify...");
            }
            // Only explain the silence when sound was actually asked for. An
            // idle session with nothing playing is not an error.
            return (self.playing && self.connect_playback.is_none())
                .then_some("no active Spotify device");
        }
        if !self.playing {
            return None;
        }
        if self.audio.current().is_none() {
            // A preview download is not silence to explain; it is sound on
            // the way.
            if self.preview.pending().is_some() {
                return Some("loading a 30-second preview");
            }
            if self.audio_lib.len() == 0 {
                return Some("no playable files found - Settings");
            }
            return Some(if !self.audio.ready() {
                "no audio output device"
            } else if self.audio.problem().is_some() {
                "this file will not play"
            } else {
                "no file for this track"
            });
        }
        if self.previewing {
            return Some("30-second preview - full song on Spotify");
        }
        None
    }

    /// Hand playback to a Spotify client before asking it to play: the Web API
    /// rejects play commands while none is active.
    fn ensure_spotify_device(&mut self) -> bool {
        if self.connect_playback.is_some() {
            return true;
        }
        if self.devices.is_empty() {
            self.request_devices();
            return false;
        }
        let target = self
            .devices
            .iter()
            .find(|d| d.active)
            .or_else(|| self.devices.first())
            .map(|d| d.id.clone());
        match target {
            Some(id) => {
                self.connect_command(connect::Command::Transfer(id));
                true
            }
            None => false,
        }
    }

    /// Device a play command should go to, preferring the one already playing.
    fn spotify_device_id(&self) -> Option<String> {
        // A device the owner picked by hand wins; otherwise the SDK player is
        // the point of this build, so every play goes there without the
        // desktop app having to be awake.
        if let Some(id) = &self.picked_device {
            if self.devices.iter().any(|d| &d.id == id) {
                return Some(id.clone());
            }
        }
        if let Some(id) = &self.web_device {
            return Some(id.clone());
        }
        self.devices
            .iter()
            .find(|d| d.active)
            .or_else(|| self.devices.first())
            .map(|d| d.id.clone())
    }

    /// Hands a track or context to Spotify. With no Spotify client running
    /// there is no device to receive it, so the installed app is opened and the
    /// command retried for a few seconds instead of failing on the first press.
    fn spotify_play(&mut self, hit: &catalog::Hit) {
        self.local_override = false;
        self.audio.stop();
        self.previewing = false;
        self.preview_file = None;
        self.preview.cancel();
        self.pending_play_uri = Some(PendingPlay {
            uri: hit.uri.clone(),
            track: hit.kind == catalog::HitKind::Track,
        });
        self.playing = true;
        self.tick_pending_play();
    }

    fn tick_pending_play(&mut self) {
        let Some(pending) = self.pending_play_uri.clone() else {
            return;
        };
        if let Some(device) = self.spotify_device_id() {
            self.pending_play_uri = None;
            self.device_asked = None;
            self.wake = None;
            self.connect_command(if pending.track {
                connect::Command::PlayUri {
                    uri: pending.uri,
                    device: Some(device),
                }
            } else {
                connect::Command::PlayContext {
                    uri: pending.uri,
                    device: Some(device),
                }
            });
            self.playing = true;
            self.connect_stamp = Some(std::time::Instant::now());
            return;
        }
        // No client yet: the wake tick opens Spotify once and keeps asking
        // until a device shows up.
        self.begin_wake(false);
    }

    /// Starts the "no client yet" wait: asks for devices straight away, opens
    /// Spotify once after a moment, and gives up after twenty seconds.
    fn begin_wake(&mut self, resume: bool) {
        if let Some(w) = &mut self.wake {
            w.resume |= resume;
            return;
        }
        self.wake = Some(Wake {
            since: std::time::Instant::now(),
            opened: false,
            resume,
        });
        self.device_asked = None;
        self.request_devices();
    }

    fn tick_wake(&mut self) {
        let Some((since, opened, resume)) = self
            .wake
            .as_ref()
            .map(|w| (w.since, w.opened, w.resume))
        else {
            return;
        };
        if self.connect_playback.is_some() {
            self.wake = None;
            return;
        }
        if !self.devices.is_empty() {
            self.wake = None;
            if resume {
                self.playing = true;
                self.ensure_spotify_device();
            }
            return;
        }
        if since.elapsed() > std::time::Duration::from_secs(20) {
            self.wake = None;
            self.pending_play_uri = None;
            self.playing = false;
            self.notify("Spotify did not start - open the Spotify app and press play");
            return;
        }
        if !opened && since.elapsed() >= std::time::Duration::from_millis(1200) {
            if let Some(w) = &mut self.wake {
                w.opened = true;
            }
            gate::launch_spotify();
            self.notify("Opening Spotify for playback...");
        }
        let ask = self
            .device_asked
            .map(|at| at.elapsed() >= std::time::Duration::from_millis(700))
            .unwrap_or(true);
        if ask {
            self.device_asked = Some(std::time::Instant::now());
            self.request_devices();
        }
    }

    /// Imported rows do not carry Spotify URIs, so ask Spotify for the best
    /// match by name and play that.
    fn play_search_on_spotify(&mut self, track: &Track) {
        let query = format!("{} {}", track.title, track.artist);
        if let Some(tx) = &self.conn_tx {
            let _ = tx.send(conn_worker::Request::PlaySearch(query));
        }
        self.notify("Looking for this track on Spotify...");
    }

    fn start_connect(&mut self) {
        if self.connect_busy {
            return;
        }
        let client_id = self.client_id.trim().to_string();
        if client_id.is_empty() {
            self.connect_error = Some("Paste your Spotify app Client ID first.".to_string());
            return;
        }
        self.profile.client_id = client_id.clone();
        self.save_profile();
        self.connect_busy = true;
        self.connect_error = None;
        self.connect_log.clear();
        self.connect_log
            .push_str("Starting Spotify sign-in for playback control...\n");
        let (tx, rx) = std::sync::mpsc::channel();
        self.connect_login_rx = Some(rx);
        let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        self.connect_cancel = Some(cancel.clone());
        let options = spotify::Options {
            cancel: Some(cancel),
            ..spotify::Options::default()
        };
        conn_worker::login(client_id, options, tx);
    }

    /// Abandon a sign-in that is waiting for the browser. The worker notices
    /// the flag, drops the callback listener and reports "cancelled", so the
    /// port is free for an immediate retry.
    fn cancel_connect(&mut self) {
        if let Some(flag) = &self.connect_cancel {
            flag.store(true, std::sync::atomic::Ordering::SeqCst);
        }
    }

    fn attach_session(&mut self, session: connect::Session) {
        let link = conn_worker::spawn(session.clone(), connect::default_path());
        self.connect = Some(session);
        self.web_tried = false;
        self.picked_device = None;
        self.conn_tx = Some(link.cmds);
        self.conn_rx = Some(link.out);
        self.spotify_mode = true;
        self.local_override = false;
        // Signing in is the moment a preview can become the full song: stop
        // the clip and hand the current track to Spotify instead of leaving
        // the 30 seconds playing.
        self.preview.cancel();
        if self.previewing {
            self.audio.stop();
            self.previewing = false;
            self.preview_file = None;
            if let Some(track) = self.cur().cloned() {
                self.play_search_on_spotify(&track);
            }
        }
    }

    /// Stops the worker and drops every piece of playback state it produced.
    fn detach_spotify(&mut self, why: &str) {
        self.conn_tx = None;
        self.conn_rx = None;
        self.connect = None;
        self.connect_playback = None;
        self.connect_stamp = None;
        self.pending_play_uri = None;
        self.device_asked = None;
        self.wake = None;
        self.local_override = false;
        self.spotify_mode = false;
        if !why.is_empty() {
            self.connect_error = Some(why.to_string());
        }
    }

    fn disconnect_spotify(&mut self) {
        connect::forget(&connect::default_path());
        self.detach_spotify("");
        self.connect_log
            .push_str("Disconnected and deleted the saved Spotify session.\n");
    }

    /// Starts the hidden WebView2 player once a session is attached, answers
    /// its token requests, and remembers the device Spotify gives it.
    fn tick_web_player(&mut self, ctx: &egui::Context) {
        if self.connect.is_none() {
            self.web = None;
            self.web_device = None;
            return;
        }
        if !self.web_tried {
            self.web_tried = true;
            match webplayer::Player::start() {
                Ok(web) => {
                    if cfg!(debug_assertions) {
                        eprintln!("webplayer: started");
                    }
                    self.web = Some(web);
                    self.web_error = None;
                }
                Err(e) => {
                    if cfg!(debug_assertions) {
                        eprintln!("webplayer: start failed: {e}");
                    }
                    self.web_error = Some(e);
                }
            }
        }
        let Some(web) = &mut self.web else {
            return;
        };
        for event in web.poll() {
            if cfg!(debug_assertions) {
                let line = match &event {
                    webplayer::Event::Ready(id) => format!("ready {id}"),
                    webplayer::Event::NotReady => "not ready".to_string(),
                    webplayer::Event::Auth(m) => format!("auth error: {m}"),
                    webplayer::Event::Account(m) => format!("account error: {m}"),
                    webplayer::Event::Error(m) => format!("error: {m}"),
                };
                eprintln!("webplayer: {line}");
            }
            match event {
                webplayer::Event::Ready(id) => {
                    self.web_device = Some(id);
                    self.web_error = None;
                }
                webplayer::Event::NotReady => self.web_device = None,
                webplayer::Event::Auth(msg) => {
                    self.web_error = Some(format!(
                        "Spotify rejected the in-app player ({msg}). Reconnect to allow playback."
                    ));
                }
                webplayer::Event::Account(msg) => {
                    self.web_error = Some(format!(
                        "This account cannot use the in-app player ({msg}). Premium is required."
                    ));
                }
                webplayer::Event::Error(msg) => {
                    self.web_error = Some(format!("In-app player: {msg}"));
                }
            }
        }
        // The page fetches its token from the bridge and picks up replacements,
        // so hand it one as soon as it is needed and refresh well before the
        // hour Spotify gives each token.
        if self
            .web_token_at
            .is_some_and(|t| t.elapsed() > std::time::Duration::from_secs(45 * 60))
        {
            if let Some(web) = &self.web {
                web.clear_token();
            }
            self.web_token_at = None;
        }
        let needs = self.web.as_ref().is_some_and(|w| w.needs_token());
        if needs
            && self
                .web_token_asked
                .is_none_or(|t| t.elapsed() > std::time::Duration::from_secs(5))
        {
            if let Some(tx) = &self.conn_tx {
                let _ = tx.send(conn_worker::Request::Token);
                self.web_token_asked = Some(std::time::Instant::now());
            }
        }
        ctx.request_repaint_after(std::time::Duration::from_millis(250));
    }

    /// Fills album covers for imported rows that predate cover storage. One
    /// Spotify search per missing row, once per launch, off the UI thread.
    fn request_missing_covers(&mut self) {
        if self.covers_asked || self.connect.is_none() {
            return;
        }
        let mut jobs = Vec::new();
        for (pi, p) in self.imported.iter().enumerate() {
            for (ti, t) in p.tracks.iter().enumerate() {
                if t.art.is_empty() && !t.title.trim().is_empty() {
                    jobs.push((
                        pi,
                        ti,
                        format!("{} {}", t.title.trim(), t.artist.trim()),
                    ));
                }
            }
        }
        if jobs.is_empty() {
            return;
        }
        self.covers_asked = true;
        if let Some(tx) = &self.conn_tx {
            let _ = tx.send(conn_worker::Request::Covers(jobs));
        }
    }

    fn connect_command(&mut self, command: connect::Command) {
        if let Some(tx) = &self.conn_tx {
            let _ = tx.send(conn_worker::Request::Api(command));
        }
    }

    /// Ask Spotify which clients are signed in, for the device switcher.
    fn request_devices(&mut self) {
        if let Some(tx) = &self.conn_tx {
            let _ = tx.send(conn_worker::Request::Devices);
        }
    }

    /// Sends a catalogue search once typing pauses, so a six-letter word is one
    /// request instead of six. No session means no request at all: the UI falls
    /// back to the library and nothing is sent.
    fn tick_search(&mut self, ctx: &egui::Context) {
        let q = self.query.trim().to_string();
        if !self.spotify_mode || q.is_empty() {
            if !self.search_hits.is_empty() {
                self.search_hits.clear();
            }
            self.search_q.clear();
            self.search_pending = None;
            self.search_busy = false;
            self.search_error = None;
            return;
        }
        if q == self.search_q {
            return;
        }
        match self.search_pending.clone() {
            Some((pending, at)) if pending == q => {
                if at.elapsed() >= std::time::Duration::from_millis(300) {
                    self.search_q = q.clone();
                    self.search_busy = true;
                    self.search_error = None;
                    self.search_pending = None;
                    if let Some(tx) = &self.conn_tx {
                        let _ = tx.send(conn_worker::Request::Search(q));
                    }
                } else {
                    ctx.request_repaint_after(std::time::Duration::from_millis(120));
                }
            }
            _ => self.search_pending = Some((q, std::time::Instant::now())),
        }
    }

    /// Sends a search result to the Spotify client that is already playing.
    /// The audio stays Spotify's; this only names what to play.
    fn play_hit(&mut self, hit: &catalog::Hit) {
        self.now_cover = hit.art.pick(artwork::TILE).map(str::to_string);
        if self.spotify_connected() {
            self.spotify_play(hit);
            return;
        }
        // No Spotify session: the only sound this machine can make is the
        // owner's own file, so use it when the title matches one.
        if hit.kind == catalog::HitKind::Track {
            if let Some(path) = self.audio_lib.find(&hit.title, &hit.artist) {
                let name = path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("local file")
                    .to_string();
                self.play_file(&path, hit);
                self.notify(format!("Playing your local file: {name}"));
                return;
            }
        }
        // A catalogue hit without Spotify still has an official 30-second
        // preview; play that instead of a dead end.
        if hit.kind == catalog::HitKind::Track {
            let (c1, c2) = palette_for(&hit.artist);
            self.queue = vec![Track {
                title: hit.title.clone(),
                artist: hit.artist.clone(),
                album: hit.album.clone(),
                secs: 0,
                c1,
                c2,
                added: String::new(),
                art: hit.art.clone(),
                path: None,
            }];
            self.queue_genres.clear();
            self.now = 0;
            self.pos = 0.0;
            self.playing = true;
            self.previewing = false;
            self.preview_file = None;
            self.log_current();
            if let Some(track) = self.cur().cloned() {
                self.start_preview(&track);
                self.notify(format!(
                    "Fetching a {}-second preview - no Spotify needed",
                    preview::CLIP_SECS
                ));
            }
            return;
        }
        self.notify(
            "Not connected - open Import from Spotify in the sidebar and press Connect playback control",
        );
    }

    fn queue_hit(&mut self, hit: &catalog::Hit) {
        if hit.kind == catalog::HitKind::Track {
            self.connect_command(connect::Command::QueueUri(hit.uri.clone()));
        }
    }

    /// Drains the sign-in and playback channels. Both run on worker threads so
    /// a slow request can never stall a frame.
    fn poll_connect(&mut self, ctx: &egui::Context) {
        if let Some(rx) = self.connect_login_rx.take() {
            let mut keep = true;
            loop {
                match rx.try_recv() {
                    Ok(conn_worker::Login::Log(line)) => {
                        self.connect_log.push_str(&line);
                        self.connect_log.push('\n');
                    }
                    Ok(conn_worker::Login::Done(result)) => {
                        self.connect_busy = false;
                        self.connect_cancel = None;
                        keep = false;
                        match result {
                            Ok(tokens) => {
                                let session = connect::Session {
                                    client_id: self.profile.client_id.clone(),
                                    endpoints: spotify::Endpoints::default(),
                                    tokens,
                                };
                                connect::save(&connect::default_path(), &session);
                                self.attach_session(session);
                                self.connect_error = None;
                                self.connect_log.push_str(
                                    "Playback control connected. Spotify plays the audio.\n",
                                );
                            }
                            Err(e) if e.contains(spotify::CANCELLED) => {
                                self.connect_error = None;
                                self.connect_log.push_str("Sign-in cancelled.\n");
                            }
                            Err(e) => {
                                self.connect_error = Some(e.clone());
                                self.connect_log.push_str(&format!("Connect failed: {e}\n"));
                            }
                        }
                        break;
                    }
                    Err(std::sync::mpsc::TryRecvError::Empty) => break,
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                        self.connect_busy = false;
                        self.connect_cancel = None;
                        keep = false;
                        self.connect_error =
                            Some("The Spotify sign-in stopped before it finished.".to_string());
                        break;
                    }
                }
            }
            if keep {
                self.connect_login_rx = Some(rx);
            }
            ctx.request_repaint_after(std::time::Duration::from_millis(200));
        }

        if let Some(rx) = self.conn_rx.take() {
            let mut keep = true;
            loop {
                match rx.try_recv() {
                    Ok(conn_worker::Out::State(Ok(state))) => {
                        self.connect_error = None;
                        // A local file owns the transport: Spotify's paused
                        // state must not stop the local clock or replace the
                        // track shown in the player bar.
                        if !self.local_override {
                            self.playing = state.as_ref().map(|p| p.playing).unwrap_or(false);
                            self.connect_playback = state;
                            self.connect_stamp = Some(std::time::Instant::now());
                        }
                    }
                    Ok(conn_worker::Out::State(Err(e))) | Ok(conn_worker::Out::Cmd(Err(e))) => {
                        self.connect_error = Some(e.clone());
                        // Without Premium the Web API rejects playback. Rather
                        // than leave the click silent, switch this track to the
                        // free preview, which Spotify does not gate.
                        if !self.local_override && premium_error(&e) {
                            self.local_override = true;
                            self.pending_play_uri = None;
                            self.wake = None;
                            if let Some(track) = self.cur().cloned() {
                                self.start_preview(&track);
                                self.notify(
                                    "Spotify Premium is needed to play here - trying a 30-second preview",
                                );
                            }
                        }
                    }
                    Ok(conn_worker::Out::Cmd(Ok(()))) => self.connect_error = None,
                    Ok(conn_worker::Out::Token(Ok(token))) => {
                        if let Some(web) = &self.web {
                            web.set_token(&token);
                        }
                        self.web_token_at = Some(std::time::Instant::now());
                        self.web_token_asked = None;
                    }
                    Ok(conn_worker::Out::Token(Err(e))) => self.web_error = Some(e),
                    Ok(conn_worker::Out::Cover(pi, ti, art)) => {
                        let mut name = String::new();
                        let mut title = String::new();
                        let mut artist = String::new();
                        if let Some(p) = self.imported.get_mut(pi) {
                            name = p.name.clone();
                            if let Some(t) = p.tracks.get_mut(ti) {
                                t.art = art.clone();
                                title = t.title.clone();
                                artist = t.artist.clone();
                            }
                            if p.art.is_empty() {
                                if let Some(first) = p.tracks.iter().find(|t| !t.art.is_empty()) {
                                    p.art = first.art.clone();
                                }
                            }
                        }
                        // The visible library is a separate tree, so the row
                        // is matched by what it shows.
                        if let Some(coll) = self.lib.iter_mut().find(|c| c.name == name) {
                            if let Some(t) = coll
                                .tracks
                                .iter_mut()
                                .find(|t| t.title == title && t.artist == artist)
                            {
                                t.art = art.clone();
                            }
                            if coll.art.is_empty() {
                                coll.art = art;
                            }
                        }
                    }
                    Ok(conn_worker::Out::CoversDone) => {
                        crate::spotify::save(&self.imports_path, &self.imported);
                    }
                    Ok(conn_worker::Out::Devices(Ok(list))) => {
                        if self
                            .picked_device
                            .as_ref()
                            .is_some_and(|id| !list.iter().any(|d| &d.id == id))
                        {
                            self.picked_device = None;
                        }
                        self.devices = list;
                    }
                    Ok(conn_worker::Out::PlaySearch(Ok(hits))) => {
                        match hits
                            .into_iter()
                            .find(|h| h.kind == catalog::HitKind::Track)
                        {
                            Some(hit) => {
                                self.now_cover =
                                    hit.art.pick(artwork::TILE).map(str::to_string);
                                self.spotify_play(&hit);
                            }
                            None => self.notify("Spotify has no match for this track"),
                        }
                    }
                    Ok(conn_worker::Out::PlaySearch(Err(e))) => self.connect_error = Some(e),
                    Ok(conn_worker::Out::Search(Ok(hits))) => {
                        self.search_hits = hits;
                        self.search_busy = false;
                        self.search_error = None;
                    }
                    Ok(conn_worker::Out::Search(Err(e))) => {
                        // Falling back is the contract: the UI keeps the local
                        // matches and says why the catalogue is missing.
                        self.search_hits.clear();
                        self.search_busy = false;
                        self.search_error = Some(e);
                    }
                    Ok(conn_worker::Out::Devices(Err(e))) => self.connect_error = Some(e),
                    Ok(conn_worker::Out::Dead(why)) => {
                        // The refresh token itself is gone, so the saved session
                        // can never work again; drop it instead of retrying.
                        connect::forget(&connect::default_path());
                        self.detach_spotify(&why);
                        keep = false;
                        break;
                    }
                    Err(std::sync::mpsc::TryRecvError::Empty) => break,
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                        self.detach_spotify("");
                        keep = false;
                        break;
                    }
                }
            }
            if keep {
                self.conn_rx = Some(rx);
            } else {
                self.conn_tx = None;
            }
            ctx.request_repaint_after(std::time::Duration::from_millis(300));
        }

        self.tick_wake();
        self.tick_pending_play();
        if self.pending_play_uri.is_some() || self.wake.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(200));
        }
    }

    fn poll_import(&mut self, ctx: &egui::Context) {
        let Some(rx) = self.import_rx.take() else {
            return;
        };
        loop {
            match rx.try_recv() {
                Ok(spotify::Msg::Log(line)) => {
                    self.import_log.push_str(&line);
                    self.import_log.push('\n');
                }
                Ok(spotify::Msg::Done(result)) => {
                    self.finish_import(result);
                    return;
                }
                Ok(spotify::Msg::One(result)) => {
                    self.finish_link(result);
                    return;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => break,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    self.finish_import(Err("The import stopped before it finished.".to_string()));
                    return;
                }
            }
        }
        self.import_rx = Some(rx);
        ctx.request_repaint_after(std::time::Duration::from_millis(200));
    }

    fn finish_import(&mut self, result: Result<Vec<spotify::ImportedPlaylist>, String>) {
        // Stamp anything the source did not date, so the Date Added column is
        // the day this playlist landed in the library rather than a blank.
        let mut result = result;
        if let Ok(list) = &mut result {
            let today = spotify::today_iso();
            for p in list.iter_mut() {
                for t in p.tracks.iter_mut() {
                    if t.added.is_empty() {
                        t.added = today.clone();
                    }
                }
            }
        }
        self.import_busy = false;
        self.import_rx = None;
        if let Err(e) = &result {
            self.import_error = Some(e.clone());
        }
        match result {
            Ok(list) => {
                self.import_error = None;
                let playlists = list.len();
                let tracks: usize = list.iter().map(|p| p.tracks.len()).sum();
                spotify::save(&self.imports_path, &list);
                self.imported = list;
                self.rebuild_lib();
                self.view = View::Import;
                self.import_log.push_str(&format!(
                    "Done. {playlists} playlists and {tracks} tracks are now in your library.\n"
                ));
            }
            Err(e) => self
                .import_log
                .push_str(&format!("Import failed: {e}\n")),
        }
    }

    fn start_link_import(&mut self) {
        if self.import_busy {
            return;
        }
        let link = self.link_input.trim().to_string();
        if link.is_empty() {
            self.import_error = Some("Paste a Spotify playlist link first.".to_string());
            return;
        }
        self.import_error = None;
        self.import_busy = true;
        let (tx, rx) = std::sync::mpsc::channel();
        self.import_rx = Some(rx);
        std::thread::spawn(move || {
            let logger = tx.clone();
            let mut log = move |m: String| {
                let _ = logger.send(spotify::Msg::Log(m));
            };
            let result = spotify::import_public_playlist(&link, &mut log);
            let _ = tx.send(spotify::Msg::One(result));
        });
    }

    fn finish_link(&mut self, result: Result<spotify::ImportedPlaylist, String>) {
        self.import_busy = false;
        self.import_rx = None;
        match result {
            Ok(playlist) => {
                self.import_error = None;
                let name = playlist.name.clone();
                let count = playlist.tracks.len();
                if let Some(existing) = self.imported.iter_mut().find(|p| p.name == name) {
                    *existing = playlist;
                    self.import_log
                        .push_str(&format!("Updated {name} ({count} tracks).\n"));
                } else {
                    self.imported.push(playlist);
                    self.import_log
                        .push_str(&format!("Added {name} ({count} tracks).\n"));
                }
                spotify::save(&self.imports_path, &self.imported);
                self.rebuild_lib();
                self.view = View::Import;
            }
            Err(e) => {
                self.import_error = Some(e.clone());
                self.import_log.push_str(&format!("Import failed: {e}\n"));
            }
        }
    }

    fn import_file(&mut self, path: &std::path::Path) {
        let name = path
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "Imported playlist".to_string());
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(e) => {
                self.import_log
                    .push_str(&format!("Could not read {}: {e}\n", path.display()));
                return;
            }
        };
        match spotify::import_csv(&text, &name) {
            Ok(playlist) => {
                let count = playlist.tracks.len();
                self.imported.push(playlist);
                spotify::save(&self.imports_path, &self.imported);
                self.rebuild_lib();
                self.view = View::Import;
                self.import_log
                    .push_str(&format!("Imported {name} ({count} tracks).\n"));
            }
            Err(e) => self
                .import_log
                .push_str(&format!("CSV import failed: {e}\n")),
        }
    }

    fn clear_imported(&mut self) {
        self.imported.clear();
        self.import_error = None;
        spotify::save(&self.imports_path, &self.imported);
        self.rebuild_lib();
        self.view = View::Import;
        self.import_log
            .push_str("Removed the imported playlists from your library.\n");
    }

    fn import_page(&mut self, ui: &mut egui::Ui) {
        let t = self.theme();
        let redirect = spotify::redirect_uri(spotify::DEFAULT_PORT);

        ui.add_space(6.0);
        ui.label(RichText::new("Import from Spotify").size(30.0).color(t.text));
        ui.add_space(2.0);
        ui.label(
            RichText::new(
                "Copy the playlists you own or collaborate on into Rustify. They are saved locally, so they are still here next time you open the app.",
            )
            .size(12.5)
            .color(t.muted),
        );
        ui.add_space(14.0);

        // Playback control: Spotify plays the audio, Rustify only sends the
        // same commands the Spotify app itself would.
        egui::Frame::new()
            .fill(t.card)
            .corner_radius(egui::CornerRadius::same(8))
            .inner_margin(egui::Margin::same(14))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new("Play through your Spotify app")
                            .size(17.0)
                            .color(t.text),
                    );
                    if self.connect.is_some() {
                        ui.label(RichText::new("connected").size(11.0).color(t.accent));
                    }
                });
                ui.add_space(4.0);
                ui.label(
                    RichText::new(
                        "Rustify can drive the Spotify app you are signed into: play, pause, skip, seek and volume. Spotify plays the audio itself, so nothing is copied or downloaded from it. Needs Spotify Premium, and the Client ID above.",
                    )
                    .size(12.0)
                    .color(t.muted),
                );
                ui.add_space(8.0);
                if self.connect.is_some() {
                    ui.horizontal(|ui| {
                        ui.checkbox(&mut self.spotify_mode, "Use Spotify playback");
                        if ui.button("Disconnect").clicked() {
                            self.disconnect_spotify();
                        }
                    });
                    ui.add_space(4.0);
                    let (web_text, web_color) = match (&self.web_error, &self.web_device) {
                        (Some(err), _) => (err.clone(), Color32::from_rgb(0xff, 0x6b, 0x6b)),
                        (None, Some(_)) => (
                            "In-app player ready: full songs play here, no desktop app needed."
                                .to_string(),
                            t.accent,
                        ),
                        (None, None) => ("Starting the in-app player...".to_string(), t.muted),
                    };
                    ui.label(RichText::new(web_text).size(11.5).color(web_color));
                    ui.add_space(4.0);
                    let status = match (&self.connect_error, &self.connect_playback) {
                        (Some(err), _) => err.clone(),
                        (None, Some(p)) => format!(
                            "{} on {} - {} ({})",
                            if p.playing { "Playing" } else { "Paused" },
                            p.device,
                            p.track,
                            p.artist
                        ),
                        (None, None) => "Open Spotify on this PC and start a song.".to_string(),
                    };
                    let bad = self.connect_error.is_some();
                    ui.label(
                        RichText::new(status).size(11.5).color(if bad {
                            Color32::from_rgb(0xff, 0x6b, 0x6b)
                        } else {
                            t.muted
                        }),
                    );
                } else {
                    ui.horizontal(|ui| {
                        let button = egui::Button::new(
                            RichText::new("Connect playback control").color(t.on_accent),
                        )
                        .fill(t.accent);
                        if ui.add_enabled(!self.connect_busy, button).clicked() {
                            self.start_connect();
                        }
                        if self.connect_busy {
                            ui.label(
                                RichText::new("Waiting for Spotify...")
                                    .size(12.0)
                                    .color(t.accent),
                            );
                            if ui.small_button("Cancel").clicked() {
                                self.cancel_connect();
                            }
                        }
                    });
                    if let Some(err) = &self.connect_error {
                        ui.add_space(6.0);
                        ui.label(
                            RichText::new(err.as_str())
                                .size(12.0)
                                .color(Color32::from_rgb(0xff, 0x6b, 0x6b)),
                        );
                    }
                }
                if !self.connect_log.is_empty() {
                    ui.add_space(6.0);
                    ui.add(egui::Label::new(
                        RichText::new(&self.connect_log)
                            .monospace()
                            .size(10.5)
                            .color(t.muted),
                    ));
                }
            });

        ui.add_space(10.0);
        egui::Frame::new()
            .fill(t.card)
            .corner_radius(egui::CornerRadius::same(8))
            .inner_margin(egui::Margin::same(14))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(
                    RichText::new("1. Connect your Spotify account")
                        .size(17.0)
                        .color(t.text),
                );
                ui.add_space(4.0);
                ui.label(
                    RichText::new(
                        "Create an app at developer.spotify.com/dashboard and add the redirect URI below to it. Development Mode apps need the app owner to have an active Spotify Premium subscription.",
                    )
                    .size(12.0)
                    .color(t.muted),
                );
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button("Open developer dashboard").clicked() {
                        gate::open_url("https://developer.spotify.com/dashboard");
                    }
                    let app_url = if self.client_id.trim().is_empty() {
                        "https://developer.spotify.com/dashboard".to_string()
                    } else {
                        format!(
                            "https://developer.spotify.com/dashboard/{}",
                            self.client_id.trim()
                        )
                    };
                    if ui.button("Open this app").clicked() {
                        gate::open_url(&app_url);
                    }
                    if ui.button("Copy redirect URI").clicked() {
                        ui.ctx().copy_text(redirect.clone());
                    }
                    ui.monospace(&redirect);
                });
                ui.label(
                    RichText::new(
                        "Spotify's \"redirect_uri: Not matching configuration\" means this exact URI is missing from that app's Redirect URIs. Add it, Save, then Connect again.",
                    )
                    .size(11.0)
                    .color(t.faint),
                );
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Client ID").size(12.5).color(t.muted));
                    let resp = ui
                        .add(
                            egui::TextEdit::singleline(&mut self.client_id)
                                .desired_width(330.0)
                                .hint_text("paste your app's Client ID"),
                        )
                        .on_hover_text("Right-click to paste");
                    paste_on_right_click(&resp, &mut self.client_id);
                });
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Music folder").size(12.5).color(t.muted));
                    let resp = ui
                        .add(
                            egui::TextEdit::singleline(&mut self.audio_dir)
                                .desired_width(330.0)
                                .hint_text("empty: scan Music, Downloads, Desktop"),
                        )
                        .on_hover_text("Right-click to paste");
                    paste_on_right_click(&resp, &mut self.audio_dir);
                    if resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                        self.rescan_audio();
                    }
                    if ui.button("Rescan").clicked() {
                        self.rescan_audio();
                    }
                });
                ui.label(
                    RichText::new(if self.audio_dir.trim().is_empty() {
                        "Scanning Music, Downloads, Desktop and Documents. Pick one folder to narrow it (MP3, WAV, M4A, OGG, FLAC). Spotify streams cannot be played here.".to_string()
                    } else {
                        format!(
                            "{} playable file(s) indexed (MP3, WAV, M4A, OGG, FLAC). Sound comes only from your own files; Spotify streams are not playable here.",
                            self.audio_lib.len()
                        )
                    })
                    .size(11.5)
                    .color(t.faint),
                );
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    let button = egui::Button::new(
                        RichText::new("Connect and copy playlists").color(t.on_accent),
                    )
                    .fill(t.accent);
                    if ui.add_enabled(!self.import_busy, button).clicked() {
                        self.start_import();
                    }
                    if self.import_busy {
                        ui.label(
                            RichText::new("Waiting for Spotify...")
                                .size(12.0)
                                .color(t.accent),
                        );
                    }
                });
                if let Some(err) = &self.import_error {
                    ui.add_space(6.0);
                    ui.label(
                        RichText::new(err.as_str())
                            .size(12.0)
                            .color(Color32::from_rgb(0xff, 0x6b, 0x6b)),
                    );
                }
                if !self.import_log.is_empty() {
                    ui.add_space(8.0);
                    egui::Frame::new()
                        .fill(t.surface)
                        .corner_radius(egui::CornerRadius::same(6))
                        .inner_margin(egui::Margin::same(8))
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            egui::ScrollArea::vertical()
                                .max_height(150.0)
                                .stick_to_bottom(true)
                                .show(ui, |ui| {
                                    ui.add(egui::Label::new(
                                        RichText::new(&self.import_log)
                                            .monospace()
                                            .size(11.0)
                                            .color(t.muted),
                                    ));
                                });
                        });
                }
            });

        ui.add_space(10.0);
        egui::Frame::new()
            .fill(t.card)
            .corner_radius(egui::CornerRadius::same(8))
            .inner_margin(egui::Margin::same(14))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(
                    RichText::new("2. Add a public playlist by link")
                        .size(17.0)
                        .color(t.text),
                );
                ui.add_space(4.0);
                ui.label(
                    RichText::new(
                        "No Spotify app or login needed. In Spotify open the playlist, choose Share > Copy link, then paste it here.",
                    )
                    .size(12.0)
                    .color(t.muted),
                );
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    let resp = ui
                        .add(
                            egui::TextEdit::singleline(&mut self.link_input)
                                .desired_width(430.0)
                                .hint_text("https://open.spotify.com/playlist/..."),
                        )
                        .on_hover_text("Right-click to paste");
                    paste_on_right_click(&resp, &mut self.link_input);
                    let button = egui::Button::new(
                        RichText::new("Add playlist").color(t.on_accent),
                    )
                    .fill(t.accent);
                    if ui.add_enabled(!self.import_busy, button).clicked() {
                        self.start_link_import();
                    }
                });
            });

        ui.add_space(10.0);
        egui::Frame::new()
            .fill(t.card)
            .corner_radius(egui::CornerRadius::same(8))
            .inner_margin(egui::Margin::same(14))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(
                    RichText::new("3. Or import a CSV export")
                        .size(17.0)
                        .color(t.text),
                );
                ui.add_space(4.0);
                ui.label(
                    RichText::new(
                        "No Spotify app needed. Export a playlist with Exportify, then drag the CSV onto this window or paste its path here.",
                    )
                    .size(12.0)
                    .color(t.muted),
                );
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    let resp = ui
                        .add(
                            egui::TextEdit::singleline(&mut self.import_path)
                                .desired_width(430.0)
                                .hint_text("C:\\Music\\playlist.csv"),
                        )
                        .on_hover_text("Right-click to paste");
                    paste_on_right_click(&resp, &mut self.import_path);
                    if ui.button("Import CSV").clicked() {
                        let path = self.import_path.trim().to_string();
                        if !path.is_empty() {
                            self.import_file(std::path::Path::new(&path));
                        }
                    }
                    if ui.button("Open Exportify").clicked() {
                        gate::open_url("https://exportify.net");
                    }
                });
            });

        ui.add_space(10.0);
        egui::Frame::new()
            .fill(t.card)
            .corner_radius(egui::CornerRadius::same(8))
            .inner_margin(egui::Margin::same(14))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(
                    RichText::new(format!("Imported playlists ({})", self.imported.len()))
                        .size(17.0)
                        .color(t.text),
                );
                if self.imported.is_empty() {
                    ui.add_space(4.0);
                    ui.label(
                        RichText::new("Nothing copied yet.")
                            .size(12.0)
                            .color(t.muted),
                    );
                } else {
                    ui.add_space(6.0);
                    for p in &self.imported {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(&p.name).size(13.0).color(t.text));
                            ui.label(
                                RichText::new(format!(
                                    "{} tracks - {}",
                                    p.tracks.len(),
                                    th::fmt_time(p.total_secs())
                                ))
                                .size(11.5)
                                .color(t.muted),
                            );
                        });
                    }
                    ui.add_space(8.0);
                    if ui.button("Remove imported playlists").clicked() {
                        self.clear_imported();
                    }
                }
            });
        ui.add_space(18.0);
    }

    fn collection_view(
        &mut self,
        ui: &mut egui::Ui,
        coll: &Collection,
        viewport: egui::Rect,
    ) {
        let t = self.theme();
        let width = ui.available_width();
        let cur_key = self.cur().map(|c| c.key());
        let playing = self.playing;

        let (banner, _) = ui.allocate_exact_size(Vec2::new(width, 206.0), Sense::hover());
        {
            let p = ui.painter();
            // The header takes the artwork's own dominant colour once the
            // image is decoded, so the page looks like the cover instead of a
            // hash of the playlist's name.
            let cover = coll.art.pick(artwork::TILE);
            let base = artwork::dominant(ui.ctx(), cover, artwork::TILE).unwrap_or(coll.c1);
            th::gradient(p, banner, base.gamma_multiply(0.85), t.bg);
            let p = p.with_clip_rect(banner);
            let art = Rect::from_min_size(banner.min + Vec2::new(24.0, 28.0), Vec2::splat(150.0));
            artwork::paint(&p, art, cover, artwork::TILE, coll.c1, coll.c2);
            let tx = art.right() + 24.0;
            th::text(
                &p,
                egui::pos2(tx, art.top() + 8.0),
                Align2::LEFT_TOP,
                coll.kind.label(),
                11.5,
                t.text,
            );
            th::text(
                &p,
                egui::pos2(tx, art.top() + 26.0),
                Align2::LEFT_TOP,
                &coll.name,
                40.0,
                t.text,
            );
            // Running time is unknown for folder-scanned files; a trailing
            // separator with nothing after it would look broken.
            let runtime = coll.runtime();
            let line = if runtime.is_empty() {
                coll.subtitle.clone()
            } else {
                format!("{}  \u{b7}  {}", coll.subtitle, runtime)
            };
            th::text(
                &p,
                egui::pos2(tx, art.bottom() - 20.0),
                Align2::LEFT_CENTER,
                line,
                12.5,
                t.muted,
            );
        }

        ui.add_space(14.0);

        let active_here = cur_key
            .as_deref()
            .map(|k| coll.tracks.iter().any(|tr| tr.key() == k))
            .unwrap_or(false);
        let mut start_at: Option<usize> = None;
        let mut toggle = false;

        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 16.0;
            let (r, resp) = ui.allocate_exact_size(Vec2::splat(54.0), Sense::click());
            {
                let p = ui.painter();
                let bg = if resp.hovered() {
                    t.accent.gamma_multiply(1.15)
                } else {
                    t.accent
                };
                p.circle_filled(r.center(), 27.0, bg);
                if active_here && playing {
                    th::pause_icon(p, r.center(), 14.0, t.on_accent);
                } else {
                    th::play_icon(p, r.center() + Vec2::new(1.5, 0.0), 16.0, t.on_accent);
                }
            }
            if resp.clicked() {
                if active_here {
                    toggle = true;
                } else {
                    start_at = Some(0);
                }
            }

            if icon_btn(ui, &t, Icon::Shuffle, self.shuffle).clicked() {
                self.shuffle = !self.shuffle;
            }
            if icon_btn(ui, &t, Icon::Repeat, self.repeat).clicked() {
                self.repeat = !self.repeat;
            }

            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.label(
                    RichText::new(format!("{} tracks", coll.tracks.len()))
                        .size(12.0)
                        .color(t.faint),
                );
            });
        });

        if toggle {
            self.set_playing(!self.playing);
        }
        if let Some(s) = start_at {
            self.play_row(coll, s);
        }

        ui.add_space(10.0);
        if coll.tracks.is_empty() {
            ui.label(
                RichText::new("Nothing in here yet - hit the heart on songs you like.")
                    .size(13.5)
                    .color(t.muted),
            );
            ui.add_space(20.0);
            return;
        }

        // Only show the date column when the collection actually knows its
        // dates; a column of dashes is worse than no column.
        let has_dates = coll.tracks.iter().any(|tr| !tr.added.is_empty());
        let (head, _) = ui.allocate_exact_size(Vec2::new(width, 26.0), Sense::hover());
        {
            let p = ui.painter();
            let w = head.width();
            th::text(
                p,
                egui::pos2(head.left() + 26.0, head.center().y),
                Align2::CENTER_CENTER,
                "#",
                11.0,
                t.faint,
            );
            th::text(
                p,
                egui::pos2(head.left() + 104.0, head.center().y),
                Align2::LEFT_CENTER,
                "TITLE",
                11.0,
                t.faint,
            );
            th::text(
                p,
                egui::pos2(head.left() + w * if has_dates { 0.42 } else { 0.62 }, head.center().y),
                Align2::LEFT_CENTER,
                "ALBUM",
                11.0,
                t.faint,
            );
            if has_dates {
                th::text(
                    p,
                    egui::pos2(head.left() + w * 0.64, head.center().y),
                    Align2::LEFT_CENTER,
                    "DATE ADDED",
                    11.0,
                    t.faint,
                );
            }
            th::text(
                p,
                egui::pos2(head.right() - 12.0, head.center().y),
                Align2::RIGHT_CENTER,
                "TIME",
                11.0,
                t.faint,
            );
            p.add(egui::Shape::line_segment(
                [head.left_bottom(), head.right_bottom()],
                Stroke::new(1.0_f32, t.rail.gamma_multiply(0.6)),
            ));
        }

        let mut go = None;
        let mut relike = None;
        // Rows are a fixed 54 px, and zero spacing makes the scroll maths
        // exact. Spacers stand in for the rows above and below the viewport,
        // so the scrollbar still covers the whole playlist while only the
        // visible rows are built.
        const ROW_H: f32 = 54.0;
        ui.spacing_mut().item_spacing.y = 0.0;
        // The viewport is relative to the content: `min_rect().height()` is
        // how much header sits above the list, in those same units. Screen
        // coordinates would include the page offset and blank the list.
        let top = ui.min_rect().height();
        let n = coll.tracks.len();
        let first = (((viewport.min.y - top) / ROW_H).floor().max(0.0) as usize).min(n);
        let last = ((((viewport.max.y - top) / ROW_H).ceil().max(0.0) as usize) + 1).min(n);
        if first > 0 {
            ui.allocate_space(Vec2::new(ui.available_width(), ROW_H * first as f32));
        }
        for i in first..last {
            let track = &coll.tracks[i];
            let is_cur = cur_key.as_deref() == Some(track.key().as_str());
            let out = track_row(
                ui,
                &t,
                track,
                i + 1,
                is_cur && playing,
                is_cur,
                true,
                has_dates,
                self.profile.is_liked(&track.key()),
            );
            if out.heart {
                relike = Some(track.key());
            } else if out.row.clicked() {
                go = Some(i);
            }
        }
        if last < n {
            ui.allocate_space(Vec2::new(ui.available_width(), ROW_H * (n - last) as f32));
        }
        ui.add_space(24.0);

        if let Some(k) = relike {
            self.toggle_like(&k);
        }
        if let Some(i) = go {
            self.play_row(coll, i);
        }
    }

    // ------------------------------------------------------------ player bar

    fn player(&mut self, ui: &mut egui::Ui) {
        let t = self.theme();
        let h = ui.available_height();
        let total = ui.available_width();
        // Fixed columns keep the three groups from sliding into each other when
        // the window is narrow, but each one is clamped so the middle never
        // collapses to nothing.
        let right_w = (total * 0.27).clamp(140.0, 300.0);
        let left_w = (total * 0.31)
            .clamp(180.0, 400.0)
            .min((total - right_w - 150.0).max(170.0));
        let mid_w = (total - left_w - right_w).max(140.0);

        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;

            ui.allocate_ui_with_layout(
                Vec2::new(left_w, h),
                Layout::left_to_right(Align::Center),
                |ui| {
                    ui.set_min_width(left_w);
                    let spotify = if self.spotify_playing() {
                        self.connect_playback.clone()
                    } else {
                        None
                    };
                    let track = if spotify.is_some() {
                        None
                    } else {
                        self.cur().cloned()
                    };
                    if let Some(sp) = spotify {
                        let (c1, c2) = palette_for(if sp.artist.is_empty() {
                            sp.track.as_str()
                        } else {
                            sp.artist.as_str()
                        });
                        let (r, _) = ui.allocate_exact_size(Vec2::splat(54.0), Sense::hover());
                        artwork::paint(
                            ui.painter(),
                            r,
                            self.now_cover.as_deref(),
                            artwork::THUMB,
                            c1,
                            c2,
                        );
                        let box_w = (left_w - 54.0 - 26.0 - 16.0).max(40.0);
                        let (slot, _) =
                            ui.allocate_exact_size(Vec2::new(box_w + 10.0, 54.0), Sense::hover());
                        let box_r = Rect::from_min_max(slot.min + Vec2::new(10.0, 0.0), slot.max);
                        let paint = ui.painter().with_clip_rect(box_r);
                        th::text(
                            &paint,
                            egui::pos2(box_r.left(), box_r.center().y - 16.0),
                            Align2::LEFT_CENTER,
                            &sp.track,
                            13.5,
                            t.text,
                        );
                        th::text(
                            &paint,
                            egui::pos2(box_r.left(), box_r.center().y + 1.0),
                            Align2::LEFT_CENTER,
                            &sp.artist,
                            11.5,
                            t.muted,
                        );
                        th::text(
                            &paint,
                            egui::pos2(box_r.left(), box_r.center().y + 17.0),
                            Align2::LEFT_CENTER,
                            format!("Spotify - {}", sp.device),
                            10.0,
                            t.accent,
                        );

                        let key = format!("{}|{}", sp.artist, sp.track);
                        let liked = self.profile.is_liked(&key);
                        let (hr, hresp) = ui.allocate_exact_size(Vec2::splat(26.0), Sense::click());
                        th::heart(
                            ui.painter(),
                            hr.center(),
                            16.0,
                            liked,
                            if liked { t.accent } else { t.muted },
                        );
                        if hresp.clicked() {
                            self.toggle_like(&key);
                        }
                    } else if let Some(tr) = track {
                        let (r, _) = ui.allocate_exact_size(Vec2::splat(54.0), Sense::hover());
                        artwork::paint(
                            ui.painter(),
                            r,
                            tr.art.pick(artwork::THUMB),
                            artwork::THUMB,
                            tr.c1,
                            tr.c2,
                        );
                        let box_w = (left_w - 54.0 - 26.0 - 16.0).max(40.0);
                        let (slot, _) = ui
                            .allocate_exact_size(Vec2::new(box_w + 10.0, 54.0), Sense::hover());
                        let box_r = Rect::from_min_max(
                            slot.min + Vec2::new(10.0, 0.0),
                            slot.max,
                        );
                        let p = ui.painter().with_clip_rect(box_r);
                        th::text(
                            &p,
                            egui::pos2(box_r.left(), box_r.center().y - 9.0),
                            Align2::LEFT_CENTER,
                            &tr.title,
                            13.5,
                            t.text,
                        );
                        th::text(
                            &p,
                            egui::pos2(box_r.left(), box_r.center().y + 9.0),
                            Align2::LEFT_CENTER,
                            &tr.artist,
                            11.5,
                            t.muted,
                        );
                        // A notice explains the last failed click; otherwise
                        // the standing reason for silence is enough.
                        let line = match (&self.notice, self.audio_status()) {
                            (Some((msg, _)), _) => Some(msg.as_str()),
                            (None, status) => status,
                        };
                        if let Some(why) = line {
                            th::text(
                                &p,
                                egui::pos2(box_r.left(), box_r.center().y + 23.0),
                                Align2::LEFT_CENTER,
                                why,
                                10.0,
                                if self.notice.is_some() { t.accent } else { t.faint },
                            );
                        }

                        let key = tr.key();
                        let liked = self.profile.is_liked(&key);
                        let (hr, hresp) = ui
                            .allocate_exact_size(Vec2::splat(26.0), Sense::click());
                        th::heart(
                            ui.painter(),
                            hr.center(),
                            16.0,
                            liked,
                            if liked { t.accent } else { t.muted },
                        );
                        if hresp.clicked() {
                            self.toggle_like(&key);
                        }
                    } else {
                        ui.label(RichText::new("Nothing playing").size(13.0).color(t.faint));
                    }
                },
            );

            ui.allocate_ui_with_layout(
                Vec2::new(mid_w, h),
                Layout::top_down(Align::Center),
                |ui| {
                    ui.set_min_width(mid_w);
                    ui.add_space(2.0);
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 12.0;
                        if icon_btn(ui, &t, Icon::Shuffle, self.shuffle).clicked() {
                            self.shuffle = !self.shuffle;
                        }
                        if icon_btn(ui, &t, Icon::Prev, false).clicked() {
                            self.skip(-1);
                        }
                        let toggle =
                            icon_btn(ui, &t, if self.playing { Icon::Pause } else { Icon::Play }, true);
                        if toggle.clicked() {
                            self.set_playing(!self.playing);
                        }
                        if icon_btn(ui, &t, Icon::Next, false).clicked() {
                            self.skip(1);
                        }
                        if icon_btn(ui, &t, Icon::Repeat, self.repeat).clicked() {
                            self.repeat = !self.repeat;
                        }
                    });

                    let dur = self.dur().max(1.0);
                    let frac = (self.pos / dur).clamp(0.0, 1.0);
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 10.0;
                        ui.label(
                            RichText::new(th::fmt_time(self.pos as u32))
                                .size(10.5)
                                .color(t.faint),
                        );
                        let bar_w = (ui.available_width() - 58.0).max(60.0);
                        if let Some(f) = bar(ui, &t, frac, bar_w) {
                            self.pos = f * dur;
                            if self.spotify_playing() {
                                self.connect_command(connect::Command::Seek(
                                    (self.pos * 1000.0).max(0.0) as u64,
                                ));
                                self.connect_stamp = Some(std::time::Instant::now());
                            } else {
                                self.audio.seek(self.pos);
                            }
                        }
                        ui.label(
                            RichText::new(th::fmt_time(self.dur() as u32))
                                .size(10.5)
                                .color(t.faint),
                        );
                    });
                },
            );

            ui.allocate_ui_with_layout(
                Vec2::new(right_w, h),
                Layout::right_to_left(Align::Center),
                |ui| {
                    ui.set_min_width(right_w);
                    ui.spacing_mut().item_spacing.x = 8.0;
                    if self.spotify_mode {
                        let (dr, dresp) = ui.allocate_exact_size(Vec2::splat(26.0), Sense::click());
                        th::device_icon(
                            ui.painter(),
                            dr,
                            if dresp.hovered() { t.text } else { t.muted },
                        );
                        if dresp.clicked() {
                            self.show_devices = !self.show_devices;
                            self.request_devices();
                        }
                    }
                    let (r, _) = ui.allocate_exact_size(Vec2::splat(26.0), Sense::hover());
                    th::volume_icon(ui.painter(), r, t.muted);
                    let (picked, released) = bar_drag(ui, &t, self.volume, 96.0);
                    if let Some(f) = picked {
                        self.volume = f;
                        self.profile.volume = f;
                        // The local sink is what previews and files play
                        // through, so it always follows the slider. Spotify
                        // gets its own copy while it owns the transport.
                        self.audio.set_volume(f);
                        if self.spotify_playing() {
                            self.connect_command(connect::Command::Volume(
                                (f * 100.0).round().clamp(0.0, 100.0) as u8,
                            ));
                        }
                    }
                    if released {
                        self.save_profile();
                    }
                    ui.label(
                        RichText::new(format!("{}%", (self.volume * 100.0).round() as i32))
                            .size(10.5)
                            .color(t.faint),
                    );
                },
            );
        });
    }

    // ---------------------------------------------------------- queue panel

    fn now_playing(&mut self, ui: &mut egui::Ui) {
        let t = self.theme();
        // Two tabs, like the panel in the real client: the track itself, and
        // what is coming next.
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            if tab(ui, &t, "Now playing", !self.show_queue).clicked() {
                self.show_queue = false;
            }
            if tab(ui, &t, "Queue", self.show_queue).clicked() {
                self.show_queue = true;
            }
        });
        ui.add_space(12.0);
        if self.show_queue {
            self.queue_view(ui);
            return;
        }

        if let Some(track) = self.cur().cloned() {
            // Square to the panel's inner width: the real client's art has no
            // dead strip down the right of the sidebar.
            let side = ui.available_width().min(272.0);
            let (rect, _) = ui.allocate_exact_size(Vec2::splat(side), Sense::hover());
            let cover = track
                .art
                .pick(artwork::TILE)
                .map(str::to_string)
                .or_else(|| self.now_cover.clone());
            artwork::paint(
                ui.painter(),
                rect,
                cover.as_deref(),
                artwork::TILE,
                track.c1,
                track.c2,
            );
            ui.add_space(14.0);
            ui.label(RichText::new(&track.title).size(18.0).color(t.text));
            ui.label(RichText::new(&track.artist).size(13.0).color(t.muted));
            ui.label(RichText::new(&track.album).size(11.5).color(t.faint));
            ui.add_space(8.0);

            let key = track.key();
            let liked = self.profile.is_liked(&key);
            // Wide enough that the heart never touches the word: Spotify's
            // own pill is this shape, and the collision was visible.
            let (r, resp) = ui.allocate_exact_size(Vec2::new(86.0, 28.0), Sense::click());
            {
                let p = ui.painter();
                th::rr(p, r, 14, if resp.hovered() { t.card_hover } else { t.surface });
                th::heart(
                    p,
                    r.left_center() + Vec2::new(18.0, 0.0),
                    14.0,
                    liked,
                    if liked { t.accent } else { t.muted },
                );
                th::text(
                    p,
                    r.left_center() + Vec2::new(34.0, 0.0),
                    Align2::LEFT_CENTER,
                    if liked { "Liked" } else { "Like" },
                    11.5,
                    t.text,
                );
            }
            if resp.clicked() {
                self.toggle_like(&key);
            }
            credits(ui, &t, &self.profile, &track);
        } else {
            ui.label(RichText::new("Nothing playing yet").size(13.0).color(t.muted));
        }
    }

    /// The queue half of the right sidebar.
    fn queue_view(&mut self, ui: &mut egui::Ui) {
        let t = self.theme();
        ui.label(RichText::new("Next up").size(14.0).color(t.text));
        ui.add_space(6.0);

        let start = self.now + 1;
        let n = self.queue.len();
        let mut jump = None;
        if start >= n {
            ui.label(RichText::new("End of queue").size(12.0).color(t.faint));
        } else {
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show_rows(ui, 46.0, n - start, |ui, rows| {
                for k in rows {
                    let i = start + k;
                    let tr = self.queue[i].clone();
                    let (rect, resp) =
                        ui.allocate_exact_size(Vec2::new(ui.available_width(), 46.0), Sense::click());
                    let p = ui.painter();
                    if resp.hovered() {
                        th::rr(p, rect, 4, t.card_hover);
                    }
                    let art = Rect::from_min_size(rect.min + Vec2::new(2.0, 5.0), Vec2::splat(36.0));
                    artwork::paint(
                        p,
                        art,
                        tr.art.pick(artwork::THUMB),
                        artwork::THUMB,
                        tr.c1,
                        tr.c2,
                    );
                    let clip = p.with_clip_rect(Rect::from_min_max(
                        egui::pos2(art.right() + 8.0, rect.top()),
                        egui::pos2(rect.right() - 4.0, rect.bottom()),
                    ));
                    th::text(
                        &clip,
                        egui::pos2(art.right() + 8.0, rect.center().y - 7.0),
                        Align2::LEFT_CENTER,
                        &tr.title,
                        12.5,
                        t.text,
                    );
                    th::text(
                        &clip,
                        egui::pos2(art.right() + 8.0, rect.center().y + 8.0),
                        Align2::LEFT_CENTER,
                        &tr.artist,
                        11.0,
                        t.muted,
                    );
                    if resp.clicked() {
                        jump = Some(i);
                    }
                }
            });
        }

        if let Some(i) = jump {
            self.now = i;
            self.pos = 0.0;
            self.playing = true;
            self.log_current();
        }
    }

    // ------------------------------------------------------------------ gate

    /// Device switcher: every Spotify client the account is signed in on, so
    /// playback can hop to the phone without leaving Rustify.
    fn devices_window(&mut self, ctx: &egui::Context) {
        if !self.show_devices {
            return;
        }
        let t = self.theme();
        let devices = self.devices.clone();
        let mut open = true;
        let mut picked: Option<String> = None;
        let mut refresh = false;
        egui::Window::new("Play on")
            .open(&mut open)
            .anchor(Align2::RIGHT_BOTTOM, [-20.0, -108.0])
            .resizable(false)
            .collapsible(false)
            .default_width(280.0)
            .show(ctx, |ui| {
                ui.set_width(280.0);
                if devices.is_empty() {
                    ui.label(
                        RichText::new(
                            "No Spotify client is awake. Open Spotify somewhere, then refresh.",
                        )
                        .size(12.0)
                        .color(t.muted),
                    );
                }
                for d in &devices {
                    let (rect, resp) =
                        ui.allocate_exact_size(Vec2::new(ui.available_width(), 42.0), Sense::click());
                    let p = ui.painter();
                    if resp.hovered() && !d.active {
                        th::rr(p, rect, 4, t.card_hover);
                    }
                    let icon = Rect::from_center_size(
                        egui::pos2(rect.left() + 18.0, rect.center().y),
                        Vec2::splat(26.0),
                    );
                    th::device_icon(p, icon, if d.active { t.accent } else { t.muted });
                    let clip = p.with_clip_rect(Rect::from_min_max(
                        egui::pos2(rect.left() + 38.0, rect.top()),
                        rect.max,
                    ));
                    th::text(
                        &clip,
                        egui::pos2(rect.left() + 38.0, rect.center().y - 7.0),
                        Align2::LEFT_CENTER,
                        &d.name,
                        13.0,
                        t.text,
                    );
                    let sub = if d.active {
                        "Currently playing".to_string()
                    } else if d.kind.is_empty() {
                        "Click to move playback".to_string()
                    } else {
                        format!("{} - click to move playback", d.kind)
                    };
                    th::text(
                        &clip,
                        egui::pos2(rect.left() + 38.0, rect.center().y + 8.0),
                        Align2::LEFT_CENTER,
                        sub,
                        10.5,
                        if d.active { t.accent } else { t.faint },
                    );
                    if resp.clicked() && !d.active {
                        picked = Some(d.id.clone());
                    }
                }
                ui.add_space(6.0);
                if ui.button("Refresh list").clicked() {
                    refresh = true;
                }
            });
        if let Some(id) = picked {
            // A Spotify device is now the audio output; nothing should keep
            // playing from this PC.
            self.local_override = false;
            self.audio.stop();
            self.picked_device = Some(id.clone());
            self.connect_command(connect::Command::Transfer(id));
            self.show_devices = false;
        }
        if refresh {
            self.request_devices();
        }
        if !open {
            self.show_devices = false;
        }
    }

    fn gate_screen(&mut self, ctx: &egui::Context) {
        let t = self.theme();
        let mut recheck = false;
        let mut open_page = false;
        let passed = self.gate.evidence.iter().filter(|e| e.ok).count();
        let total = self.gate.evidence.len();

        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(t.bg)
                    .inner_margin(egui::Margin::symmetric(46, 26)),
            )
            .show(ctx, |ui| {
                ui.add_space(16.0);
                ui.vertical_centered(|ui| {
                    let (rect, _) = ui.allocate_exact_size(Vec2::splat(70.0), Sense::hover());
                    let p = ui.painter();
                    th::logo(p, rect.center(), 35.0, th::brand(), Color32::WHITE);
                    ui.add_space(12.0);
                    ui.label(RichText::new("Spotify is required").size(32.0).color(t.text));
                    ui.add_space(6.0);
                    ui.label(
                        RichText::new(
                            "Rustify is a Rust re-imagining of the Spotify player, and it only unlocks on a PC that already has Spotify installed. Install the real app, then check again.",
                        )
                        .size(13.5)
                        .color(t.muted),
                    );
                    ui.add_space(4.0);
                    ui.label(
                        RichText::new(format!("{passed} of {total} installation checks passed"))
                            .size(12.0)
                            .color(if passed > 0 { t.accent } else { t.faint }),
                    );
                    ui.add_space(14.0);
                });

                let list_h = (ui.available_height() - 96.0).max(80.0);
                egui::ScrollArea::vertical()
                    .max_height(list_h)
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        for e in &self.gate.evidence {
                            let (rect, _) = ui.allocate_exact_size(
                                Vec2::new(ui.available_width(), 26.0),
                                Sense::hover(),
                            );
                            let p = ui.painter();
                            th::rr(p, rect, 4, t.card);
                            p.circle_filled(
                                rect.left_center() + Vec2::new(14.0, 0.0),
                                4.0,
                                if e.ok { t.accent } else { t.rail },
                            );
                            th::text(
                                p,
                                rect.left_center() + Vec2::new(28.0, 0.0),
                                Align2::LEFT_CENTER,
                                &e.label,
                                12.0,
                                if e.ok { t.text } else { t.muted },
                            );
                            th::text(
                                p,
                                rect.right_center() - Vec2::new(12.0, 0.0),
                                Align2::RIGHT_CENTER,
                                if e.ok { "found" } else { "missing" },
                                11.0,
                                if e.ok { t.accent } else { t.faint },
                            );
                            ui.add_space(4.0);
                        }
                    });

                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 10.0;
                    let button =
                        egui::Button::new(RichText::new("Check again").size(13.0).color(t.on_accent))
                            .fill(t.accent)
                            .min_size(Vec2::new(140.0, 34.0));
                    if ui.add(button).clicked() {
                        recheck = true;
                    }
                    if ui
                        .add(
                            egui::Button::new(RichText::new("Get Spotify for Windows").size(13.0))
                                .min_size(Vec2::new(190.0, 34.0)),
                        )
                        .clicked()
                    {
                        open_page = true;
                    }
                });
            });

        if recheck {
            self.gate = gate::check();
        }
        if open_page {
            gate::open_download_page();
        }
    }

    fn theme_window(&mut self, ctx: &egui::Context) {
        if !self.show_themes {
            return;
        }
        let current = self.theme();
        let themes = self.themes.clone();
        let selected = self.theme_idx;
        let mut open = true;
        let mut pick = None;
        // Edits are collected and applied after the window closes, so the
        // palette rebuild never fights the borrow checker mid-frame.
        let mut custom = self.profile.custom.clone();
        let mut custom_changed = false;
        let mut discord_id = self.profile.discord_client_id.clone();
        let mut discord_on = self.profile.discord_enabled;
        let mut toggle_discord = false;
        let status = self.discord_status.clone();
        let bg_error = self.bg_error.clone();
        let theme_path = self.theme_file.path.display().to_string();
        let theme_warnings = self.theme_file.warnings.clone();
        let theme_error = self.theme_file.error.clone();
        let mut reload_file = false;
        let mut open_folder = false;
        let mut save_file = false;

        egui::Window::new("Settings")
            .open(&mut open)
            .resizable(false)
            .collapsible(false)
            .default_width(240.0)
            .anchor(Align2::RIGHT_TOP, Vec2::new(-18.0, 70.0))
            .show(ctx, |ui| {
                ui.label(RichText::new("Themes").size(13.0).color(current.text));
                ui.label(
                    RichText::new("Remembered next launch")
                        .size(10.5)
                        .color(current.faint),
                );
                ui.add_space(6.0);
                egui::ScrollArea::vertical()
                    .max_height(200.0)
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        for (i, thm) in themes.iter().enumerate() {
                            let (rect, resp) = ui
                                .allocate_exact_size(Vec2::new(214.0, 34.0), Sense::click());
                            let p = ui.painter();
                            if resp.hovered() {
                                th::rr(p, rect, 5, current.card_hover);
                            }
                            let swatch = Rect::from_min_size(
                                rect.min + Vec2::new(6.0, 7.0),
                                Vec2::splat(20.0),
                            );
                            // Gradients get a real gradient chip, so the list
                            // shows what the theme actually looks like.
                            match thm.gradient {
                                Some((top, bottom)) => {
                                    let clip = p.with_clip_rect(swatch);
                                    th::gradient(&clip, swatch, top, bottom);
                                }
                                None => th::rr(p, swatch, 4, thm.bg),
                            }
                            th::rr(
                                p,
                                Rect::from_min_size(
                                    rect.min + Vec2::new(15.0, 16.0),
                                    Vec2::splat(11.0),
                                ),
                                3,
                                thm.accent,
                            );
                            th::text(
                                p,
                                rect.left_center() + Vec2::new(36.0, 0.0),
                                Align2::LEFT_CENTER,
                                thm.name,
                                13.0,
                                current.text,
                            );
                            if i == selected {
                                th::text(
                                    p,
                                    rect.right_center() - Vec2::new(10.0, 0.0),
                                    Align2::RIGHT_CENTER,
                                    "On",
                                    11.0,
                                    current.accent,
                                );
                            }
                            if resp.clicked() {
                                pick = Some(i);
                            }
                        }
                    });

                ui.add_space(10.0);
                ui.separator();
                ui.add_space(6.0);
                ui.label(RichText::new("Theme file").size(13.0).color(current.text));
                ui.label(
                    RichText::new("Edit it and save: the window repaints on its own.")
                        .size(10.5)
                        .color(current.faint),
                );
                ui.label(
                    RichText::new(theme_path.as_str())
                        .size(9.5)
                        .color(current.muted),
                );
                if let Some(e) = &theme_error {
                    ui.label(
                        RichText::new(e.as_str())
                            .size(10.0)
                            .color(Color32::from_rgb(0xff, 0x6b, 0x6b)),
                    );
                }
                for w in &theme_warnings {
                    ui.label(
                        RichText::new(w.as_str())
                            .size(10.0)
                            .color(Color32::from_rgb(0xff, 0xc0, 0x6b)),
                    );
                }
                ui.horizontal(|ui| {
                    if ui.button("Reload now").clicked() {
                        reload_file = true;
                    }
                    if ui.button("Open folder").clicked() {
                        open_folder = true;
                    }
                });
                if ui
                    .button("Copy current theme into the file")
                    .on_hover_text("Writes the palette you are looking at into theme.toml, ready to tweak")
                    .clicked()
                {
                    save_file = true;
                }

                ui.add_space(10.0);
                ui.separator();
                ui.add_space(6.0);
                ui.label(RichText::new("Custom theme").size(13.0).color(current.text));
                ui.label(
                    RichText::new(
                        "Two colours, an optional photo, blur and dim. Any edit switches you to Custom.",
                    )
                    .size(10.5)
                    .color(current.faint),
                );
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Top").size(11.5).color(current.muted));
                    if color_button(ui, &mut custom.top) {
                        custom_changed = true;
                    }
                    ui.label(RichText::new("Bottom").size(11.5).color(current.muted));
                    if color_button(ui, &mut custom.bottom) {
                        custom_changed = true;
                    }
                    ui.label(RichText::new("Accent").size(11.5).color(current.muted));
                    if color_button(ui, &mut custom.accent) {
                        custom_changed = true;
                    }
                    ui.label(RichText::new("Text").size(11.5).color(current.muted));
                    if color_button(ui, &mut custom.text) {
                        custom_changed = true;
                    }
                });
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Photo").size(11.5).color(current.muted));
                    let resp = ui.add(
                        egui::TextEdit::singleline(&mut custom.image)
                            .desired_width(132.0)
                            .hint_text("C:\\Pictures\\bg.jpg"),
                    );
                    paste_on_right_click(&resp, &mut custom.image);
                    if resp.changed() {
                        custom_changed = true;
                    }
                    if ui.button("Clear").clicked() {
                        custom.image.clear();
                        custom_changed = true;
                    }
                });
                if ui
                    .add(egui::Slider::new(&mut custom.blur, 0.0..=1.0).text("Blur"))
                    .changed()
                {
                    custom_changed = true;
                }
                if ui
                    .add(egui::Slider::new(&mut custom.dim, 0.0..=1.0).text("Dim"))
                    .changed()
                {
                    custom_changed = true;
                }
                if let Some(e) = &bg_error {
                    ui.label(
                        RichText::new(e.as_str())
                            .size(10.5)
                            .color(Color32::from_rgb(0xff, 0x6b, 0x6b)),
                    );
                }
                if ui.button("Use custom theme").clicked() {
                    custom_changed = true;
                }

                ui.add_space(10.0);
                ui.separator();
                ui.add_space(6.0);
                ui.label(RichText::new("Discord presence").size(13.0).color(current.text));
                ui.label(
                    RichText::new(
                        "Shows the current song on your Discord profile. Make an app at discord.com/developers, name it Rustify, and paste its Application ID here.",
                    )
                    .size(10.5)
                    .color(current.faint),
                );
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.label(RichText::new("App ID").size(11.5).color(current.muted));
                    let resp = ui.add(
                        egui::TextEdit::singleline(&mut discord_id)
                            .desired_width(150.0)
                            .hint_text("123456789012345678"),
                    );
                    paste_on_right_click(&resp, &mut discord_id);
                });
                if ui
                    .checkbox(&mut discord_on, "Share what is playing")
                    .changed()
                {
                    toggle_discord = true;
                }
                if !status.is_empty() {
                    ui.label(RichText::new(status.as_str()).size(10.5).color(current.muted));
                }
            });

        self.show_themes = open;
        if save_file {
            // Takes whatever theme is on screen and makes it editable: the
            // quickest route from "nearly right" to "exactly mine".
            let t = self.theme();
            let mut f = self.theme_file.file.clone();
            f.name = t.name.to_string();
            for (slot, c) in [
                (&mut f.bg, t.bg),
                (&mut f.sidebar, t.sidebar),
                (&mut f.card, t.card),
                (&mut f.card_hover, t.card_hover),
                (&mut f.surface, t.surface),
                (&mut f.player, t.player),
            ] {
                *slot = hex_of(c);
            }
            f.accent = hex_of(t.accent);
            f.text = hex_of(t.text);
            f.muted = hex_of(t.muted);
            f.faint = hex_of(t.faint);
            f.rail = hex_of(t.rail);
            f.gradient = t.gradient.map(|(a, b)| (hex_of(a), hex_of(b)));
            f.save_to(&self.theme_file.path);
            self.theme_file.reload();
            self.themes = th::with_custom(&self.profile.custom);
            self.themes.push(self.theme_file.theme());
            self.theme_idx = self.theme_idx.min(self.themes.len() - 1);
        }
        if reload_file {
            self.theme_file.reload();
            self.themes = th::with_custom(&self.profile.custom);
            self.themes.push(self.theme_file.theme());
            self.theme_idx = self.theme_idx.min(self.themes.len() - 1);
        }
        if open_folder {
            if let Some(dir) = self.theme_file.path.parent() {
                let _ = std::process::Command::new("explorer").arg(dir).spawn();
            }
        }
        if custom_changed {
            self.profile.custom = custom;
            self.apply_custom();
        }
        if let Some(i) = pick {
            self.theme_idx = i;
            self.profile.theme = i;
            self.save_profile();
        }
        if discord_id != self.profile.discord_client_id {
            self.profile.discord_client_id = discord_id;
            self.save_profile();
            self.discord_sent = None;
            if self.profile.discord_enabled {
                self.start_discord();
            }
        }
        if toggle_discord || discord_on != self.profile.discord_enabled {
            self.profile.discord_enabled = discord_on;
            self.save_profile();
            if discord_on {
                self.start_discord();
            } else {
                self.stop_discord(true);
            }
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Frame {
    Locked,
    Player,
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let _ = self.run_frame(ctx);
    }
}

impl App {
    /// One frame of work. While the gate is locked this draws the gate screen
    /// and returns: no clock, no playback, no panels and no profile writes.
    fn run_frame(&mut self, ctx: &egui::Context) -> Frame {
        self.apply_style(ctx);
        // Theme file edits land on the next frame, without a restart.
        if self.theme_file.poll() {
            self.themes = th::with_custom(&self.profile.custom);
            self.themes.push(self.theme_file.theme());
            self.theme_idx = self.theme_idx.min(self.themes.len() - 1);
            self.apply_style(ctx);
        }

        if !self.gate.found {
            self.gate_screen(ctx);
            self.locked_frames += 1;
            return Frame::Locked;
        }

        self.player_frames += 1;
        self.poll_import(ctx);
        self.poll_connect(ctx);
        self.tick_web_player(ctx);
        self.request_missing_covers();
        self.poll_preview(ctx);
        self.tick_search(ctx);
        self.sync_background(ctx);
        self.pump_discord(ctx);

        let dropped: Vec<PathBuf> = ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .filter_map(|f| f.path.clone())
                .collect()
        });
        if !dropped.is_empty() {
            for path in dropped {
                self.import_file(&path);
            }
            self.view = View::Import;
        }
        self.advance(ctx);

        if !ctx.wants_keyboard_input() && ctx.input(|i| i.key_pressed(egui::Key::Space)) {
            self.set_playing(!self.playing);
        }

        self.paint_background(ctx);
        let t = self.theme();

        egui::TopBottomPanel::bottom("player_bar")
            .exact_height(96.0)
            .frame(
                egui::Frame::new()
                    .fill(t.player)
                    .inner_margin(egui::Margin::symmetric(16, 10)),
            )
            .show(ctx, |ui| self.player(ui));

        egui::SidePanel::left("sidebar")
            .exact_width(250.0)
            .resizable(false)
            .frame(
                egui::Frame::new()
                    .fill(t.sidebar)
                    .inner_margin(egui::Margin::symmetric(12, 12)),
            )
            .show(ctx, |ui| self.sidebar(ui));

        // The right sidebar is always there: cover art, credits and the queue
        // share it, and the queue button in the header swaps between them.
        egui::SidePanel::right("now_playing")
            .exact_width(300.0)
            .resizable(false)
            .frame(
                egui::Frame::new()
                    .fill(t.card)
                    .inner_margin(egui::Margin::symmetric(14, 12)),
            )
            .show(ctx, |ui| self.now_playing(ui));

        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(t.bg)
                    .inner_margin(egui::Margin::symmetric(22, 12)),
            )
            .show(ctx, |ui| self.body(ui));

        self.devices_window(ctx);
        self.theme_window(ctx);

        // An idle window draws no frames on its own, which would mean a saved
        // theme file went unnoticed until the mouse moved. A slow tick keeps
        // the watcher live without costing anything worth measuring.
        ctx.request_repaint_after(std::time::Duration::from_millis(600));

        Frame::Player
    }
}

// ------------------------------------------------------------------ widgets

struct RowOut {
    row: egui::Response,
    heart: bool,
}

/// Shrinks a photo before it ever reaches the GPU, which is both the blur and
/// the reason a 4K wallpaper does not eat a 48 MB texture.
pub fn blur_rgba(pixels: Vec<u8>, w: u32, h: u32, blur: f32) -> (Vec<u8>, u32, u32) {
    let softness = 1 + (blur.clamp(0.0, 1.0) * 23.0).round() as u32;
    let size_cap = (w / 2200).max(h / 2200).max(1);
    let factor = softness.max(size_cap);
    if factor <= 1 || w < factor * 2 || h < factor * 2 {
        return (pixels, w, h);
    }
    let (ow, oh) = (w / factor, h / factor);
    let mut out = vec![0u8; (ow * oh * 4) as usize];
    // One box average per output pixel; scaling it back up with linear
    // filtering finishes the job for free.
    for y in 0..oh {
        for x in 0..ow {
            let (mut r, mut g, mut b, mut a) = (0u32, 0u32, 0u32, 0u32);
            for sy in 0..factor {
                for sx in 0..factor {
                    let px = (((y * factor + sy) * w + (x * factor + sx)) * 4) as usize;
                    r += pixels[px] as u32;
                    g += pixels[px + 1] as u32;
                    b += pixels[px + 2] as u32;
                    a += pixels[px + 3] as u32;
                }
            }
            let n = factor * factor;
            let o = ((y * ow + x) * 4) as usize;
            out[o] = (r / n) as u8;
            out[o + 1] = (g / n) as u8;
            out[o + 2] = (b / n) as u8;
            out[o + 3] = (a / n) as u8;
        }
    }
    (out, ow, oh)
}

/// Hex in, hex out, through egui's colour picker.
fn color_button(ui: &mut egui::Ui, hex: &mut u32) -> bool {
    let mut srgb = [(*hex >> 16) as u8, (*hex >> 8) as u8, *hex as u8];
    let changed = ui.color_edit_button_srgb(&mut srgb).changed();
    if changed {
        *hex = ((srgb[0] as u32) << 16) | ((srgb[1] as u32) << 8) | srgb[2] as u32;
    }
    changed
}

/// Colour to 0xrrggbb, for writing a palette back into the theme file.
fn hex_of(c: Color32) -> u32 {
    ((c.r() as u32) << 16) | ((c.g() as u32) << 8) | c.b() as u32
}

fn palette_for(name: &str) -> (Color32, Color32) {
    let h = name
        .bytes()
        .fold(2166136261u32, |a, b| (a ^ b as u32).wrapping_mul(16777619));
    let options = [
        (0x5038a0, 0xcbb6ff),
        (0x1e5b8f, 0x7fd8e8),
        (0x5a1f6e, 0xff7ad9),
        (0x2f4f3f, 0xa8d8b9),
        (0x4a3c2a, 0xd9b98a),
        (0x3a2352, 0x8f7bd6),
    ];
    let (a, b) = options[(h as usize) % options.len()];
    (rgb(a), rgb(b))
}

fn nav_row(ui: &mut egui::Ui, t: &Theme, label: &str, icon: NavIcon, active: bool) -> egui::Response {
    let (row, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 34.0), Sense::click());
    let p = ui.painter();
    let fg = if active || resp.hovered() {
        t.text
    } else {
        t.muted
    };
    let c = row.left_center() + Vec2::new(12.0, 0.0);
    match icon {
        NavIcon::Home => th::home_icon(p, c, fg),
        NavIcon::Search => th::search_icon(p, c, 7.0, fg),
        NavIcon::Taste => th::equalizer(p, c, if active { t.accent } else { fg }),
        NavIcon::Import => th::import_icon(p, c, fg),
    }
    th::text(
        p,
        row.left_center() + Vec2::new(30.0, 0.0),
        Align2::LEFT_CENTER,
        label,
        14.5,
        if active { t.accent } else { fg },
    );
    resp
}

fn chip(ui: &mut egui::Ui, t: &Theme, label: &str, active: bool) -> egui::Response {
    let w = 22.0 + label.chars().count() as f32 * 6.6;
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, 24.0), Sense::click());
    let p = ui.painter();
    let bg = if active {
        t.text
    } else if resp.hovered() {
        t.card_hover
    } else {
        t.card
    };
    th::rr(p, rect, 12, bg);
    th::text(
        p,
        rect.center(),
        Align2::CENTER_CENTER,
        label,
        11.0,
        if active { t.bg } else { t.text },
    );
    resp
}

/// One of the two tabs at the top of the right sidebar.
fn tab(ui: &mut egui::Ui, t: &Theme, label: &str, active: bool) -> egui::Response {
    let w = 24.0 + label.chars().count() as f32 * 6.6;
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, 28.0), Sense::click());
    let p = ui.painter();
    if active {
        th::rr(p, rect, 14, t.surface);
    } else if resp.hovered() {
        th::rr(p, rect, 14, t.card_hover);
    }
    th::text(
        p,
        rect.center(),
        Align2::CENTER_CENTER,
        label,
        12.5,
        if active { t.text } else { t.muted },
    );
    resp
}

/// One label/value row in the credits block.
fn credit_line(ui: &mut egui::Ui, t: &Theme, label: &str, value: &str) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 32.0), Sense::hover());
    let p = ui.painter();
    th::text(
        p,
        egui::pos2(rect.left(), rect.center().y - 7.0),
        Align2::LEFT_CENTER,
        label,
        10.5,
        t.faint,
    );
    let clip = p.with_clip_rect(rect);
    th::text(
        &clip,
        egui::pos2(rect.left(), rect.center().y + 7.0),
        Align2::LEFT_CENTER,
        value,
        12.5,
        t.text,
    );
}

/// Credits for the playing track. Every line is data the app already holds;
/// nothing here is invented, so no songwriter is named that Spotify did not
/// send us.
fn credits(ui: &mut egui::Ui, t: &Theme, p: &Profile, track: &Track) {
    let key = track.key();
    let plays = p.track_plays.get(&key).copied().unwrap_or(0);
    ui.add_space(18.0);
    ui.label(RichText::new("Credits").size(15.0).color(t.text));
    ui.add_space(6.0);
    credit_line(ui, t, "Performed by", &track.artist);
    credit_line(
        ui,
        t,
        "Album",
        if track.album.is_empty() { "Single" } else { &track.album },
    );
    // A folder-scanned file has no duration until it is decoded, so say
    // nothing rather than "0:00".
    let length = if track.secs > 0 {
        th::fmt_time(track.secs)
    } else {
        "-".to_string()
    };
    credit_line(ui, t, "Length", &length);
    credit_line(ui, t, "Your plays", &plays.to_string());
    credit_line(
        ui,
        t,
        "In your library",
        if p.is_liked(&key) { "Liked" } else { "Not saved" },
    );
}

fn mini_heart(ui: &mut egui::Ui, t: &Theme, liked: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(24.0), Sense::click());
    th::heart(
        ui.painter(),
        rect.center(),
        14.0,
        liked,
        if liked { t.accent } else { t.muted },
    );
    resp
}

fn section(ui: &mut egui::Ui, t: &Theme, title: &str) {
    ui.add_space(20.0);
    ui.label(RichText::new(title).size(21.0).color(t.text));
    ui.add_space(8.0);
}

fn genre_bars(ui: &mut egui::Ui, t: &Theme, rows: &[(String, u32)], suffix: &str) {
    let max = rows.first().map(|(_, c)| *c).unwrap_or(1).max(1);
    for (g, count) in rows {
        let pct = (*count as f32 / max as f32).clamp(0.04, 1.0);
        let (rect, _) =
            ui.allocate_exact_size(Vec2::new(ui.available_width(), 26.0), Sense::hover());
        let p = ui.painter();
        th::text(
            p,
            rect.left_center() + Vec2::new(2.0, 0.0),
            Align2::LEFT_CENTER,
            g,
            13.0,
            t.text,
        );
        th::stat_bar(
            p,
            Rect::from_min_size(
                egui::pos2(rect.left() + 150.0, rect.center().y - 4.0),
                Vec2::new((rect.width() - 240.0).max(40.0), 8.0),
            ),
            pct,
            t.card_hover,
            t.accent,
        );
        th::text(
            p,
            rect.right_center() - Vec2::new(4.0, 0.0),
            Align2::RIGHT_CENTER,
            format!("{count} {suffix}"),
            11.5,
            t.muted,
        );
    }
}

fn stat_box(ui: &mut egui::Ui, t: &Theme, value: &str, label: &str, w: f32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(w, 74.0), Sense::hover());
    let p = ui.painter();
    th::rr(p, rect, 8, t.card);
    th::text(
        p,
        rect.left_top() + Vec2::new(14.0, 14.0),
        Align2::LEFT_TOP,
        value,
        24.0,
        t.text,
    );
    th::text(
        p,
        rect.left_top() + Vec2::new(14.0, 46.0),
        Align2::LEFT_TOP,
        label,
        11.5,
        t.muted,
    );
}

fn pin_tile(
    ui: &mut egui::Ui,
    t: &Theme,
    name: &str,
    cover: Option<&str>,
    c1: Color32,
    c2: Color32,
    width: f32,
) -> egui::Response {
    let h = 62.0;
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(width, h), Sense::click());
    let p = ui.painter();
    th::rr(p, rect, 4, if resp.hovered() { t.card_hover } else { t.card });
    let art = Rect::from_min_size(rect.min, Vec2::splat(h));
    artwork::paint(p, art, cover, artwork::TILE, c1, c2);
    let clip = p.with_clip_rect(Rect::from_min_max(
        egui::pos2(art.right(), rect.top()),
        egui::pos2(rect.right() - 40.0, rect.bottom()),
    ));
    th::text(
        &clip,
        egui::pos2(art.right() + 14.0, rect.center().y),
        Align2::LEFT_CENTER,
        name,
        14.0,
        t.text,
    );
    if resp.hovered() {
        let c = egui::pos2(rect.right() - 28.0, rect.center().y);
        p.circle_filled(c, 18.0, t.accent);
        th::play_icon(p, c + Vec2::new(1.0, 0.0), 13.0, t.on_accent);
    }
    resp
}

fn card(
    ui: &mut egui::Ui,
    t: &Theme,
    name: &str,
    subtitle: &str,
    cover: Option<&str>,
    c1: Color32,
    c2: Color32,
    width: f32,
) -> egui::Response {
    let height = width + 66.0;
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(width, height), Sense::click());
    let p = ui.painter();
    if resp.hovered() {
        th::rr(p, rect, 8, t.card_hover);
    }
    let art = Rect::from_min_size(rect.min + Vec2::splat(12.0), Vec2::splat(width - 24.0));
    artwork::paint(p, art, cover, artwork::TILE, c1, c2);
    if resp.hovered() {
        let c = art.right_bottom() + Vec2::new(-22.0, -4.0);
        p.circle_filled(c, 20.0, t.accent);
        th::play_icon(p, c + Vec2::new(1.5, 0.0), 14.0, t.on_accent);
    }
    let clip = p.with_clip_rect(rect);
    th::text(
        &clip,
        egui::pos2(art.left(), art.bottom() + 14.0),
        Align2::LEFT_TOP,
        name,
        14.5,
        t.text,
    );
    th::text(
        &clip,
        egui::pos2(art.left(), art.bottom() + 34.0),
        Align2::LEFT_TOP,
        subtitle,
        11.5,
        t.muted,
    );
    resp
}

fn artist_card(ui: &mut egui::Ui, t: &Theme, name: &str, plays: u32, width: f32) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(width, width + 52.0), Sense::click());
    let p = ui.painter();
    if resp.hovered() {
        th::rr(p, rect, 8, t.card_hover);
    }
    let (c1, c2) = palette_for(name);
    let center = rect.center_top() + Vec2::new(width * 0.5, 12.0 + (width - 24.0) * 0.5);
    let radius = (width - 24.0) * 0.5;
    p.circle_filled(center, radius, c1);
    p.circle_filled(
        center + Vec2::new(radius * 0.35, radius * 0.25),
        radius * 0.55,
        c2.gamma_multiply(0.75),
    );
    th::text(
        p,
        center,
        Align2::CENTER_CENTER,
        name.chars().next().unwrap_or('?').to_string(),
        26.0,
        t.text,
    );
    let clip = p.with_clip_rect(rect);
    th::text(
        &clip,
        egui::pos2(rect.left() + 6.0, center.y + radius + 12.0),
        Align2::LEFT_TOP,
        name,
        13.5,
        t.text,
    );
    th::text(
        &clip,
        egui::pos2(rect.left() + 6.0, center.y + radius + 30.0),
        Align2::LEFT_TOP,
        format!("{plays} plays"),
        11.0,
        t.muted,
    );
    resp
}

fn genre_tile(ui: &mut egui::Ui, t: &Theme, genre: &'static str, width: f32) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(width, 96.0), Sense::click());
    let p = ui.painter();
    let (c1, c2) = library::genre_colors(genre);
    th::rr(p, rect, 6, c1);
    let p = p.with_clip_rect(rect);
    p.circle_filled(rect.max - Vec2::splat(26.0), 44.0, c2.gamma_multiply(0.7));
    if resp.hovered() {
        th::rr(&p, rect, 6, c2.gamma_multiply(0.18));
    }
    th::text(
        &p,
        rect.left_top() + Vec2::new(14.0, 14.0),
        Align2::LEFT_TOP,
        genre,
        16.0,
        t.text,
    );
    th::text(
        &p,
        rect.left_bottom() + Vec2::new(14.0, -14.0),
        Align2::LEFT_BOTTOM,
        "tap to open",
        10.5,
        t.text.gamma_multiply(0.75),
    );
    resp
}

fn track_row(
    ui: &mut egui::Ui,
    t: &Theme,
    track: &Track,
    n: usize,
    show_eq: bool,
    highlight: bool,
    show_album: bool,
    show_date: bool,
    liked: bool,
) -> RowOut {
    let width = ui.available_width();
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(width, 54.0), Sense::click());
    let p = ui.painter();
    if resp.hovered() {
        th::rr(p, rect, 4, t.card_hover);
    }
    let cy = rect.center().y;

    if resp.hovered() {
        th::play_icon(p, egui::pos2(rect.left() + 26.0, cy), 11.0, t.text);
    } else if show_eq {
        th::equalizer(p, egui::pos2(rect.left() + 26.0, cy), t.accent);
    } else {
        th::text(
            p,
            egui::pos2(rect.left() + 26.0, cy),
            Align2::CENTER_CENTER,
            n,
            13.0,
            t.faint,
        );
    }

    let art = Rect::from_center_size(egui::pos2(rect.left() + 78.0, cy), Vec2::splat(38.0));
    artwork::paint(
        p,
        art,
        track.art.pick(artwork::THUMB),
        artwork::THUMB,
        track.c1,
        track.c2,
    );

    // Column edges in the same proportions the header row paints.
    let text_right = if show_album {
        rect.left() + width * 0.40
    } else {
        rect.right() - 84.0
    };
    let clip = p.with_clip_rect(Rect::from_min_max(
        egui::pos2(rect.left() + 104.0, rect.top()),
        egui::pos2(text_right, rect.bottom()),
    ));
    th::text(
        &clip,
        egui::pos2(rect.left() + 104.0, cy - 8.0),
        Align2::LEFT_CENTER,
        &track.title,
        14.0,
        if highlight { t.accent } else { t.text },
    );
    th::text(
        &clip,
        egui::pos2(rect.left() + 104.0, cy + 9.0),
        Align2::LEFT_CENTER,
        &track.artist,
        11.5,
        t.muted,
    );

    if show_album {
        let album_left = rect.left() + width * 0.42;
        let album_right = if show_date {
            rect.left() + width * 0.62
        } else {
            rect.right() - 92.0
        };
        let album_clip = p.with_clip_rect(Rect::from_min_max(
            egui::pos2(album_left, rect.top()),
            egui::pos2(album_right, rect.bottom()),
        ));
        th::text(
            &album_clip,
            egui::pos2(album_left, cy),
            Align2::LEFT_CENTER,
            &track.album,
            12.5,
            t.muted,
        );
    }

    if show_date {
        let date_left = rect.left() + width * 0.64;
        let date_clip = p.with_clip_rect(Rect::from_min_max(
            egui::pos2(date_left, rect.top()),
            egui::pos2(rect.right() - 92.0, rect.bottom()),
        ));
        th::text(
            &date_clip,
            egui::pos2(date_left, cy),
            Align2::LEFT_CENTER,
            th::pretty_date(&track.added),
            12.0,
            t.muted,
        );
    }

    let heart_rect = Rect::from_center_size(
        egui::pos2(rect.right() - 66.0, cy),
        Vec2::splat(26.0),
    );
    let heart_resp = ui.interact(
        heart_rect,
        ui.id()
            .with(("heart", n, track.title.as_str(), track.artist.as_str())),
        Sense::click(),
    );
    th::heart(
        p,
        heart_rect.center(),
        15.0,
        liked,
        if heart_resp.hovered() {
            t.text
        } else if liked {
            t.accent
        } else {
            t.muted
        },
    );

    if track.secs > 0 {
        th::text(
            p,
            egui::pos2(rect.right() - 12.0, cy),
            Align2::RIGHT_CENTER,
            th::fmt_time(track.secs),
            12.5,
            t.muted,
        );
    }

    RowOut {
        row: resp,
        heart: heart_resp.clicked(),
    }
}

struct HitOut {
    play: bool,
    queue: bool,
}

/// A search result row: real cover, title, artist, album, length and a queue
/// button. Tracks play on click; albums and artists play their context.
fn catalog_row(ui: &mut egui::Ui, t: &Theme, hit: &catalog::Hit) -> HitOut {
    let width = ui.available_width();
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(width, 54.0), Sense::click());
    let p = ui.painter();
    if resp.hovered() {
        th::rr(p, rect, 4, t.card_hover);
    }
    let cy = rect.center().y;
    let art = Rect::from_center_size(egui::pos2(rect.left() + 28.0, cy), Vec2::splat(42.0));
    artwork::paint(
        p,
        art,
        hit.art.pick(artwork::THUMB),
        artwork::THUMB,
        t.rail,
        t.muted,
    );

    let title_right = rect.left() + width * 0.44;
    let clip = p.with_clip_rect(Rect::from_min_max(
        egui::pos2(rect.left() + 58.0, rect.top()),
        egui::pos2(title_right, rect.bottom()),
    ));
    th::text(
        &clip,
        egui::pos2(rect.left() + 58.0, cy - 8.0),
        Align2::LEFT_CENTER,
        &hit.title,
        14.0,
        t.text,
    );
            let sub = match hit.kind {
        catalog::HitKind::Track => hit.artist.clone(),
        kind => format!("{} \u{b7} {}", kind.label(), hit.artist),
    };
    th::text(
        &clip,
        egui::pos2(rect.left() + 58.0, cy + 9.0),
        Align2::LEFT_CENTER,
        sub,
        11.5,
        t.muted,
    );

    if !hit.album.is_empty() {
        let album_left = rect.left() + width * 0.46;
        let album_clip = p.with_clip_rect(Rect::from_min_max(
            egui::pos2(album_left, rect.top()),
            egui::pos2(rect.right() - 120.0, rect.bottom()),
        ));
        th::text(
            &album_clip,
            egui::pos2(album_left, cy),
            Align2::LEFT_CENTER,
            &hit.album,
            12.5,
            t.muted,
        );
    }

    let mut queued = false;
    if hit.kind == catalog::HitKind::Track {
        let qr = Rect::from_center_size(egui::pos2(rect.right() - 66.0, cy), Vec2::splat(26.0));
        let qresp = ui.interact(qr, ui.id().with(("queue", hit.id.as_str())), Sense::click());
        th::queue_icon(
            p,
            qr.center(),
            if qresp.hovered() { t.text } else { t.faint },
        );
        queued = qresp.clicked();
        th::text(
            p,
            egui::pos2(rect.right() - 12.0, cy),
            Align2::RIGHT_CENTER,
            th::fmt_time(hit.secs),
            12.5,
            t.muted,
        );
    } else {
        th::text(
            p,
            egui::pos2(rect.right() - 14.0, cy),
            Align2::RIGHT_CENTER,
            "Play",
            11.5,
            t.accent,
        );
    }

    HitOut {
        play: resp.clicked(),
        queue: queued,
    }
}

fn icon_btn(ui: &mut egui::Ui, t: &Theme, kind: Icon, active: bool) -> egui::Response {
    let big = matches!(kind, Icon::Play | Icon::Pause);
    let side = if big { 38.0 } else { 24.0 };
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(side), Sense::click());
    let p = ui.painter();
    if big {
        let bg = if resp.hovered() {
            t.accent.gamma_multiply(1.15)
        } else {
            t.accent
        };
        p.circle_filled(rect.center(), side * 0.5, bg);
        if kind == Icon::Play {
            th::play_icon(p, rect.center() + Vec2::new(1.5, 0.0), 13.0, t.on_accent);
        } else {
            th::pause_icon(p, rect.center(), 12.0, t.on_accent);
        }
    } else {
        let col = if active {
            t.accent
        } else if resp.hovered() {
            t.text
        } else {
            t.muted
        };
        match kind {
            Icon::Prev => th::prev_icon(p, rect.center(), 10.0, col),
            Icon::Next => th::next_icon(p, rect.center(), 10.0, col),
            Icon::Shuffle => th::shuffle_icon(p, rect, col),
            Icon::Repeat => th::repeat_icon(p, rect, col),
            _ => {}
        }
    }
    resp
}

/// Click/drag bar. Returns the new fraction (0..1) when the user seeks.
fn bar(ui: &mut egui::Ui, t: &Theme, frac: f32, width: f32) -> Option<f32> {
    bar_drag(ui, t, frac, width).0
}

/// The same bar, plus whether the pointer was released this frame, so a caller
/// can write the value once at the end of a drag instead of every frame.
fn bar_drag(ui: &mut egui::Ui, t: &Theme, frac: f32, width: f32) -> (Option<f32>, bool) {
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(width, 16.0), Sense::click_and_drag());
    let p = ui.painter();
    let cy = rect.center().y;
    let rail = Rect::from_min_max(
        egui::pos2(rect.left(), cy - 2.0),
        egui::pos2(rect.right(), cy + 2.0),
    );
    th::rr(p, rail, 2, t.rail);
    let f = frac.clamp(0.0, 1.0);
    let filled = Rect::from_min_max(rail.min, egui::pos2(rail.left() + rail.width() * f, rail.max.y));
    th::rr(p, filled, 2, t.text);
    let active = resp.hovered() || resp.dragged();
    if active {
        p.circle_filled(egui::pos2(rail.left() + rail.width() * f, cy), 6.0, t.text);
    }
    let mut value = None;
    if let Some(pos) = resp.interact_pointer_pos() {
        if resp.clicked() || resp.dragged() {
            value = Some(((pos.x - rect.left()) / rect.width()).clamp(0.0, 1.0));
        }
    }
    (value, resp.drag_stopped())
}

/// Background playback control. One worker owns the Spotify session, so a token
/// refresh happens in exactly one place and each request runs off the UI thread
/// (every call shells out to curl.exe, which is far too slow for a frame).
mod conn_worker {
    use super::connect;
    use super::spotify;
    use std::sync::mpsc::{channel, Receiver, Sender, TryRecvError};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    pub enum Login {
        Log(String),
        Done(Result<connect::Tokens, String>),
    }

    pub enum Out {
        State(Result<Option<connect::Playback>, String>),
        Cmd(Result<(), String>),
        Devices(Result<Vec<connect::Device>, String>),
        Search(Result<Vec<crate::catalog::Hit>, String>),
        PlaySearch(Result<Vec<crate::catalog::Hit>, String>),
        Dead(String),
        Token(Result<String, String>),
        /// One album cover found for an imported row.
        Cover(usize, usize, crate::library::Art),
        CoversDone,
    }

    /// What the window can ask the worker for: a playback command, the list of
    /// Spotify clients the account is signed in on, or a catalogue search.
    pub enum Request {
        Api(connect::Command),
        Devices,
        Search(String),
        /// Search for a library row that has no stored Spotify URI, then play
        /// the best track match.
        PlaySearch(String),
        /// An access token for the in-app player page.
        Token,
        /// Imported rows that still need a cover, as (playlist, track, query).
        Covers(Vec<(usize, usize, String)>),
    }

    pub struct Link {
        pub cmds: Sender<Request>,
        pub out: Receiver<Out>,
    }

    pub fn login(client_id: String, options: spotify::Options, tx: Sender<Login>) {
        std::thread::spawn(move || {
            let lines = tx.clone();
            let mut log = move |m: String| {
                let _ = lines.send(Login::Log(m));
            };
            let result = connect::login(
                &spotify::Curl,
                &client_id,
                &options,
                &mut log,
            );
            let _ = tx.send(Login::Done(result));
        });
    }

    /// Refresh if the access token is about to expire (and persist the new one),
    /// then run one request against the session.
    fn step<F, R>(
        session: &Arc<Mutex<connect::Session>>,
        store: &std::path::Path,
        run: F,
    ) -> Result<R, String>
    where
        F: FnOnce(&connect::Session) -> Result<R, String>,
    {
        let mut guard = session
            .lock()
            .map_err(|_| "the playback session was lost".to_string())?;
        if connect::ensure_fresh(&spotify::Curl, &mut guard)? {
            connect::save(store, &guard);
        }
        run(&guard)
    }

    pub fn spawn(session: connect::Session, store: std::path::PathBuf) -> Link {
        let (cmd_tx, cmd_rx) = channel::<Request>();
        let (out_tx, out_rx) = channel::<Out>();
        let session = Arc::new(Mutex::new(session));
        std::thread::spawn(move || {
            let http = spotify::Curl;
            let mut last_poll = Instant::now() - Duration::from_secs(5);
            loop {
                let mut stop = false;
                loop {
                    match cmd_rx.try_recv() {
                        Ok(Request::Search(query)) => {
                            let result = step(&session, &store, |s| {
                                crate::catalog::search(
                                    &http,
                                    &s.endpoints.api,
                                    &s.tokens.access,
                                    &query,
                                )
                            });
                            match result {
                                Ok(hits) => {
                                    if out_tx.send(Out::Search(Ok(hits))).is_err() {
                                        return;
                                    }
                                }
                                Err(e) if connect::fatal(&e) => {
                                    let _ = out_tx.send(Out::Dead(e));
                                    return;
                                }
                                Err(e) => {
                                    if out_tx.send(Out::Search(Err(e))).is_err() {
                                        return;
                                    }
                                }
                            }
                        }
                        Ok(Request::PlaySearch(query)) => {
                            let result = step(&session, &store, |s| {
                                crate::catalog::search(
                                    &http,
                                    &s.endpoints.api,
                                    &s.tokens.access,
                                    &query,
                                )
                            });
                            match result {
                                Ok(hits) => {
                                    if out_tx.send(Out::PlaySearch(Ok(hits))).is_err() {
                                        return;
                                    }
                                }
                                Err(e) if connect::fatal(&e) => {
                                    let _ = out_tx.send(Out::Dead(e));
                                    return;
                                }
                                Err(e) => {
                                    if out_tx.send(Out::PlaySearch(Err(e))).is_err() {
                                        return;
                                    }
                                }
                            }
                        }
                        Ok(Request::Token) => {
                            let result = step(&session, &store, |s| Ok(s.tokens.access.clone()));
                            match result {
                                Ok(token) => {
                                    if out_tx.send(Out::Token(Ok(token))).is_err() {
                                        return;
                                    }
                                }
                                Err(e) => {
                                    if out_tx.send(Out::Token(Err(e))).is_err() {
                                        return;
                                    }
                                }
                            }
                        }
                        Ok(Request::Covers(jobs)) => {
                            // Covers are looked up on their own thread so a
                            // long backfill never delays a play command.
                            let session = session.clone();
                            let store = store.clone();
                            let out = out_tx.clone();
                            std::thread::spawn(move || {
                                let http = spotify::Curl;
                                for (pi, ti, query) in jobs {
                                    let result = step(&session, &store, |s| {
                                        crate::catalog::search(
                                            &http,
                                            &s.endpoints.api,
                                            &s.tokens.access,
                                            &query,
                                        )
                                    });
                                    match result {
                                        Ok(hits) => {
                                            let found = hits.into_iter().find(|h| {
                                                h.kind == crate::catalog::HitKind::Track
                                                    && !h.art.is_empty()
                                            });
                                            if let Some(hit) = found {
                                                if out.send(Out::Cover(pi, ti, hit.art)).is_err() {
                                                    return;
                                                }
                                            }
                                        }
                                        Err(e) if connect::fatal(&e) => {
                                            let _ = out.send(Out::Dead(e));
                                            return;
                                        }
                                        Err(_) => {}
                                    }
                                }
                                let _ = out.send(Out::CoversDone);
                            });
                        }
                        Ok(Request::Devices) => {
                            let result = step(&session, &store, |s| {
                                connect::devices(&http, &s.endpoints.api, &s.tokens.access)
                            });
                            match result {
                                Ok(list) => {
                                    if out_tx.send(Out::Devices(Ok(list))).is_err() {
                                        return;
                                    }
                                }
                                Err(e) if connect::fatal(&e) => {
                                    let _ = out_tx.send(Out::Dead(e));
                                    return;
                                }
                                Err(e) => {
                                    if out_tx.send(Out::Devices(Err(e))).is_err() {
                                        return;
                                    }
                                }
                            }
                        }
                        Ok(Request::Api(cmd)) => {
                            // Volume is best effort. Several Spotify devices
                            // ignore it, and that is not a connection failure
                            // worth painting the window red for.
                            let quiet = matches!(&cmd, connect::Command::Volume(_));
                            let result = step(&session, &store, |s| {
                                connect::command(&http, &s.endpoints.api, &s.tokens.access, cmd)
                            });
                            match result {
                                Ok(()) => {
                                    if out_tx.send(Out::Cmd(Ok(()))).is_err() {
                                        return;
                                    }
                                }
                                Err(e) if connect::fatal(&e) => {
                                    let _ = out_tx.send(Out::Dead(e));
                                    return;
                                }
                                Err(_) if quiet => {
                                    if out_tx.send(Out::Cmd(Ok(()))).is_err() {
                                        return;
                                    }
                                }
                                Err(e) => {
                                    if out_tx.send(Out::Cmd(Err(e))).is_err() {
                                        return;
                                    }
                                }
                            }
                        }
                        Err(TryRecvError::Empty) => break,
                        Err(TryRecvError::Disconnected) => {
                            stop = true;
                            break;
                        }
                    }
                }
                if stop {
                    return;
                }
                if last_poll.elapsed() >= Duration::from_millis(1200) {
                    last_poll = Instant::now();
                    let result = step(&session, &store, |s| {
                        connect::state(&http, &s.endpoints.api, &s.tokens.access)
                    });
                    match result {
                        Ok(state) => {
                            if out_tx.send(Out::State(Ok(state))).is_err() {
                                return;
                            }
                        }
                        Err(e) if connect::fatal(&e) => {
                            let _ = out_tx.send(Out::Dead(e));
                            return;
                        }
                        Err(e) => {
                            if out_tx.send(Out::State(Err(e))).is_err() {
                                return;
                            }
                        }
                    }
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        });
        Link {
            cmds: cmd_tx,
            out: out_rx,
        }
    }
}

#[cfg(test)]
mod frame_tests {
    use super::*;
    use std::io::{Read, Write};

    // ------------------------------------------- headless Spotify stand-in

    fn pump(app: &mut App, ctx: &egui::Context) {
        let mut input = egui::RawInput::default();
        input.screen_rect = Some(egui::Rect::from_min_size(
            egui::pos2(0.0, 0.0),
            egui::vec2(1280.0, 800.0),
        ));
        let _ = ctx.run(input, |ctx| {
            let _ = app.run_frame(ctx);
        });
    }

    fn pump_until<F: Fn(&App) -> bool>(
        app: &mut App,
        ctx: &egui::Context,
        timeout: std::time::Duration,
        done: F,
    ) -> bool {
        let start = std::time::Instant::now();
        while start.elapsed() < timeout {
            pump(app, ctx);
            if done(app) {
                return true;
            }
            std::thread::sleep(std::time::Duration::from_millis(15));
        }
        false
    }

    fn logged_line<'a>(app: &'a App, prefix: &str) -> Option<&'a str> {
        app.import_log.lines().find_map(|l| l.strip_prefix(prefix))
    }

    fn query_value(url: &str, key: &str) -> Option<String> {
        let query = url.split_once('?')?.1;
        for pair in query.split('&') {
            let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
            if k == key {
                return Some(v.to_string());
            }
        }
        None
    }

    fn start_test_import(app: &mut App, fake: &crate::fake_spotify::FakeSpotify) {
        app.client_id = "test-client-id".to_string();
        app.start_import_with(spotify::Options {
            endpoints: spotify::Endpoints {
                authorize: format!("{}/authorize", fake.base),
                token: format!("{}/api/token", fake.base),
                api: format!("{}/v1", fake.base),
            },
            port: 0,
            open_browser: false,
            timeout: std::time::Duration::from_secs(5),
            cancel: None,
        });
    }

    /// Waits until the flow has bound its callback listener and logged the URL
    /// it would have opened, then returns (authorize url, redirect uri, state).
    fn await_authorize(app: &mut App, ctx: &egui::Context) -> (String, String, String) {
        let appeared = pump_until(app, ctx, std::time::Duration::from_secs(10), |a| {
            logged_line(a, "If no browser opens, paste this URL: ").is_some()
        });
        assert!(
            appeared,
            "authorize URL never appeared; log: {}",
            app.import_log
        );
        let url = logged_line(app, "If no browser opens, paste this URL: ")
            .unwrap()
            .to_string();
        let redirect = logged_line(app, "Redirect URI must be exactly ")
            .unwrap()
            .to_string();
        let state = query_value(&url, "state").expect("state in authorize URL");
        (url, redirect, state)
    }

    /// Stands in for the user's browser: opens the redirect URI the app is
    /// listening on and delivers the query string Spotify would have sent.
    fn simulate_browser(redirect: &str, query: &str) {
        let host = redirect
            .trim_start_matches("http://")
            .split('/')
            .next()
            .unwrap();
        let mut stream = std::net::TcpStream::connect(host).expect("connect callback listener");
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(3)))
            .unwrap();
        let request =
            format!("GET /callback?{query} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n");
        stream.write_all(request.as_bytes()).unwrap();
        let mut response = String::new();
        let _ = stream.read_to_string(&mut response);
        assert!(
            response.starts_with("HTTP/1.1 200"),
            "callback listener did not answer: {response}"
        );
    }

    #[test]
    fn full_spotify_import_is_headless_and_feeds_the_taste_profile() {
        use crate::fake_spotify::{FakePlan, FakeSpotify};

        let fake = FakeSpotify::start(FakePlan::ok());
        let path = temp_profile("import-chain");
        let open = gate::verdict(vec![gate::Evidence {
            label: "test probe".to_string(),
            ok: true,
        }]);
        let mut app = App::for_test(open, path.clone());
        let before = app.lib.len();
        let ctx = egui::Context::default();

        app.view = View::Import;
        start_test_import(&mut app, &fake);
        let (url, redirect, state) = await_authorize(&mut app, &ctx);

        // The app built an authorize URL at the stand-in with PKCE + scopes and
        // the real loopback redirect it bound.
        assert!(url.starts_with(&format!("{}/authorize?", fake.base)));
        assert!(url.contains("response_type=code"));
        assert!(url.contains("client_id=test-client-id"));
        assert!(url.contains("code_challenge_method=S256"));
        assert!(url.contains("playlist-read-private%20playlist-read-collaborative"));
        let port = redirect
            .trim_start_matches("http://127.0.0.1:")
            .split('/')
            .next()
            .unwrap();
        assert!(url.contains(&format!(
            "redirect_uri=http%3A%2F%2F127.0.0.1%3A{port}%2Fcallback"
        )));

        simulate_browser(&redirect, &format!("code=test-code&state={state}"));

        let finished = pump_until(&mut app, &ctx, std::time::Duration::from_secs(15), |a| {
            !a.import_busy
        });
        assert!(finished, "import never finished; log: {}", app.import_log);
        assert!(
            app.import_error.is_none(),
            "unexpected error: {:?}",
            app.import_error
        );
        assert!(app.import_log.contains("Done."), "log: {}", app.import_log);

        let names: Vec<&str> = app.imported.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, vec!["Night Drive", "Podcast Junk", "Empty Shelf"]);
        assert_eq!(app.imported[0].tracks.len(), 2);
        assert_eq!(app.imported[0].tracks[1].title, "Afterglow");
        assert!(
            app.imported[1].tracks.is_empty(),
            "episode-only playlist should import with no tracks"
        );
        assert_eq!(app.imported[2].tracks.len(), 0);

        // They are real library collections now, not just import records.
        assert_eq!(app.lib.len(), before + 3);
        assert!(app
            .lib
            .iter()
            .any(|c| c.name == "Night Drive" && c.tracks.len() == 2));

        // Saved locally and reloadable.
        let saved = spotify::load(&app.imports_path);
        assert_eq!(saved, app.imported, "import must survive save -> load");

        // Playing an imported track feeds the personalization profile by artist.
        let index = app
            .lib
            .iter()
            .position(|c| c.name == "Night Drive")
            .unwrap();
        let coll = app.lib[index].clone();
        app.play_from(&coll, 0);
        assert_eq!(app.profile.artist_plays.get("Nova Kite"), Some(&1));
        assert_eq!(app.taste_snapshot().artists[0].0, "Nova Kite");

        // All requests went to the stand-in, including both paging hops.
        let seen = fake.seen.lock().unwrap().clone();
        assert!(seen.iter().any(|r| r.starts_with("POST /api/token")));
        assert!(seen.iter().any(|r| r.contains("/v1/me/playlists")));
        assert!(seen.iter().any(|r| r.contains("offset=2")));
        assert!(seen.iter().any(|r| r.contains("/v1/playlists/p1/items")));
        assert!(seen.iter().any(|r| r.contains("/v1/playlists/p2/items")));

        // The populated import page still renders.
        assert_eq!(drive(&mut app, &ctx, 1), vec![Frame::Player]);

        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(&app.imports_path);
    }

    fn run_failing_import(
        plan: crate::fake_spotify::FakePlan,
        tag: &str,
        callback: &dyn Fn(&str) -> String,
    ) -> (App, PathBuf) {
        use crate::fake_spotify::FakeSpotify;

        let fake = FakeSpotify::start(plan);
        let path = temp_profile(tag);
        let open = gate::verdict(vec![gate::Evidence {
            label: "test probe".to_string(),
            ok: true,
        }]);
        let mut app = App::for_test(open, path.clone());
        let before = app.lib.len();
        let ctx = egui::Context::default();

        app.view = View::Import;
        start_test_import(&mut app, &fake);
        let (_url, redirect, state) = await_authorize(&mut app, &ctx);
        simulate_browser(&redirect, &callback(&state));

        let finished = pump_until(&mut app, &ctx, std::time::Duration::from_secs(10), |a| {
            !a.import_busy
        });
        assert!(finished, "import never finished; log: {}", app.import_log);

        // Failure must surface as an error and must not clear or extend the library.
        assert!(
            app.import_error.is_some(),
            "failure was silent; log: {}",
            app.import_log
        );
        assert!(app.import_log.contains("Import failed"), "log: {}", app.import_log);
        assert!(app.imported.is_empty(), "failed import added playlists");
        assert_eq!(app.lib.len(), before, "failed import changed the library");

        app.view = View::Import;
        assert_eq!(
            drive(&mut app, &ctx, 1),
            vec![Frame::Player],
            "import page must render after a failure"
        );

        let _ = std::fs::remove_file(&path);
        (app, path)
    }

    #[test]
    fn denied_authorization_shows_a_visible_import_error() {
        let (app, _path) = run_failing_import(
            crate::fake_spotify::FakePlan::ok(),
            "import-denied",
            &|state| format!("error=access_denied&state={state}"),
        );
        let error = app.import_error.unwrap();
        assert!(error.contains("access_denied"), "error: {error}");
    }

    #[test]
    fn token_exchange_error_shows_a_visible_import_error() {
        let plan = crate::fake_spotify::FakePlan {
            token_status: 400,
            token_body: r#"{"error":"invalid_grant","error_description":"Invalid authorization code"}"#
                .to_string(),
            playlists_expired: false,
        };
        let (app, _path) = run_failing_import(plan, "import-token", &|state| {
            format!("code=bad-code&state={state}")
        });
        let error = app.import_error.unwrap();
        assert!(
            error.contains("Invalid authorization code"),
            "error: {error}"
        );
    }

    #[test]
    fn expired_access_token_shows_a_visible_import_error() {
        let plan = crate::fake_spotify::FakePlan {
            playlists_expired: true,
            ..crate::fake_spotify::FakePlan::ok()
        };
        let (app, _path) = run_failing_import(plan, "import-expired", &|state| {
            format!("code=test-code&state={state}")
        });
        let error = app.import_error.unwrap();
        assert!(error.contains("access token expired"), "error: {error}");
    }

    #[test]
    fn launch_state_loads_a_saved_store_and_feeds_taste() {
        let path = temp_profile("launch-store");
        let imports = path.with_extension("imports.txt");
        let saved = vec![spotify::ImportedPlaylist {
            art: library::Art::default(),
            name: "Night Drive".to_string(),
            tracks: vec![
                spotify::ImportedTrack {
                    art: library::Art::default(),
                    title: "Nightcall".to_string(),
                    artist: "Kavinsky".to_string(),
                    album: "OutRun".to_string(),
                    secs: 258,
                    added: "2026-11-22".to_string(),
                },
                spotify::ImportedTrack {
                    art: library::Art::default(),
                    title: "A Real Hero".to_string(),
                    artist: "College".to_string(),
                    album: "Drive".to_string(),
                    secs: 267,
                    added: "2026-11-22".to_string(),
                },
            ],
        }];
        spotify::save(&imports, &saved);
        let open = gate::verdict(vec![gate::Evidence {
            label: "test probe".to_string(),
            ok: true,
        }]);

        // Same loader the launched window runs, reading the store that
        // `spotify::save` just wrote to disk.
        let mut app = App::launch_state_at(open, imports.clone());
        // Isolate the taste assertions from the developer's real history and
        // keep playback writes in the temp file.
        app.profile = Profile::default();
        app.profile_path = path.clone();
        let coll = app
            .lib
            .iter()
            .find(|c| c.name == "Night Drive")
            .expect("saved playlist must reach the launch library")
            .clone();
        assert_eq!(coll.tracks.len(), 2);
        assert_eq!(coll.tracks[0].title, "Nightcall");
        assert!(
            !coll.genres.is_empty(),
            "imported playlist needs a genre tag for taste mapping"
        );
        assert_eq!(app.imported.len(), 1);
        assert_eq!(app.queue.len(), app.lib[0].tracks.len());
        assert!(app
            .lib
            .iter()
            .any(|c| c.name == "Random Access Memories"
                && c.genres.contains(&"electronic")));

        // Playback from a loaded playlist records the artist for the taste profile.
        app.play_from(&coll, 0);
        assert_eq!(app.profile.artist_plays.get("Kavinsky"), Some(&1));
        assert_eq!(app.taste_snapshot().artists[0].0, "Kavinsky");
        assert!(app.profile.genre_plays.values().copied().sum::<u32>() > 0);

        let report = app.library_report();
        assert!(report.contains("collection=PLAYLIST|Night Drive|2|525"));
        assert!(report.contains("probe_playlist=Night Drive"));
        // The playback above already counted once, the report probe replays it.
        assert!(report.contains("taste_artist=Kavinsky|2"));

        // The taste page and both reports read the same snapshot.
        let snap = app.taste_snapshot();
        assert!(snap
            .imported_genres
            .iter()
            .any(|(g, n)| g == "synthwave" && *n == 2));
        assert!(snap.imported_genres.iter().all(|(g, _)| g != "mix"));
        assert_eq!(snap.top_genre(), Some("synthwave"));
        let taste_report = app.taste_report();
        assert!(taste_report.contains("top_genre=synthwave|4"));
        assert!(taste_report.contains("imported_genre=synthwave|2"));
        assert!(taste_report.contains("played_genre=synthwave|2"));
        assert!(taste_report.contains("personality=Neon dreamer"));
        assert!(taste_report.contains("sample_track=Nightcall|Kavinsky"));
        assert!(taste_report.contains("sample_genres=synthwave"));
        assert!(!taste_report.contains("sample_genre_plays=0"));
        assert!(taste_report.contains("mix1=Daily Mix 1|Because you like Kavinsky"));

        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(&imports);
    }

    #[test]
    fn real_store_launches_with_every_playlist_and_taste_mapping() {
        let store = spotify::default_path();
        let saved = spotify::load(&store);
        if saved.is_empty() {
            eprintln!(
                "skipping real-store launch check: no playlists saved at {}",
                store.display()
            );
            return;
        }

        // Exactly the loader the window runs, reading the persisted store.
        let open = gate::verdict(vec![gate::Evidence {
            label: "test probe".to_string(),
            ok: true,
        }]);
        let app = App::launch_state(open);

        for name in REQUIRED_PLAYLISTS {
            let expected = saved
                .iter()
                .find(|p| p.name == name)
                .unwrap_or_else(|| panic!("store no longer contains {name}"));
            let got = app
                .lib
                .iter()
                .find(|c| c.name == name)
                .unwrap_or_else(|| panic!("launch library is missing {name}"));
            assert_eq!(got.tracks.len(), expected.tracks.len(), "track count for {name}");
            assert!(!got.tracks.is_empty(), "{name} has no tracks");
            assert!(!got.genres.is_empty(), "{name} has no genre mapping");
            assert!(
                got.genres.iter().any(|g| *g != "mix"),
                "{name} still only has the mix fallback"
            );
        }
        for p in &saved {
            let got = app
                .lib
                .iter()
                .find(|c| c.name == p.name)
                .unwrap_or_else(|| panic!("launch library is missing {}", p.name));
            assert_eq!(got.tracks.len(), p.tracks.len(), "track count for {}", p.name);
        }
        assert!(app
            .lib
            .iter()
            .any(|c| c.name == "Random Access Memories" && !c.tracks.is_empty()));

        let check = app.verify_library();
        assert!(check.is_ok(), "launch check failed: {:?}", check.err());
        assert!(check.unwrap().contains("ok taste:"));

        // The personalization surface is fed by the same snapshot as the check.
        let snap = app.taste_snapshot();
        assert_eq!(snap.imported_playlists, saved.len());
        assert!(!snap.imported_genres.is_empty());
        assert!(snap.imported_genres.iter().all(|(g, _)| g != "mix"));
        // The claimed genre may only come from recorded plays: with no plays it
        // must stay unclaimed even though the imported playlists have genres.
        match (snap.played_genres.first(), snap.top_genre()) {
            (Some((played, _)), Some(top)) => {
                assert_eq!(top, played.as_str());
                assert_ne!(top, "mix");
            }
            (None, None) => {}
            (_, claimed) => panic!("top genre must come from plays, got {claimed:?}"),
        }
        let taste = app.taste_report();
        assert!(!taste.contains("imported_genre=mix"));
        assert!(taste.contains("top_genre="));
        let sample_genres = snap
            .sample
            .as_ref()
            .map(|s| s.genres.join("+"))
            .unwrap_or_default();
        assert!(!sample_genres.is_empty(), "snapshot has no sample genres");
        assert!(
            taste.contains(&format!("sample_genres={sample_genres}")),
            "taste report disagrees with the snapshot sample"
        );
    }

    #[test]
    fn real_store_taste_view_renders_the_shared_snapshot() {
        let store = spotify::default_path();
        if spotify::load(&store).is_empty() {
            eprintln!(
                "skipping rendered taste check: no playlists saved at {}",
                store.display()
            );
            return;
        }

        let open = gate::verdict(vec![gate::Evidence {
            label: "test probe".to_string(),
            ok: true,
        }]);
        let mut app = App::launch_state(open);
        let snap = app.taste_snapshot();
        app.view = View::Taste;
        let text = frame_text(&mut app, &egui::Context::default());

        assert!(text.contains("Your taste profile"), "rendered frame:\n{text}");
        assert!(
            text.contains("From your imported playlists"),
            "rendered frame:\n{text}"
        );
        assert!(text.contains(&format!(
            "{} playlists · {} tracks",
            snap.imported_playlists, snap.imported_tracks
        )));
        assert!(
            text.contains(snap.personality().as_str()),
            "rendered frame:\n{text}"
        );

        assert!(text.contains("top genre"), "rendered frame:\n{text}");
        if let Some(top) = snap.top_genre() {
            assert_ne!(top, "mix", "top genre fell back to mix");
            assert!(text.contains(top), "top genre {top} not rendered:\n{text}");
        } else {
            // Nothing played yet: the page must not invent a genre.
            assert!(
                text.contains("Open ears"),
                "no-play taste page fell back to an invented profile:\n{text}"
            );
        }

        let lines: Vec<&str> = text.lines().map(str::trim).collect();

        // Every real imported genre and its snapshot count is on screen, as an
        // adjacent genre/count pair, so a diverging count cannot hide behind a
        // duplicate number in another row.
        for (genre, count) in &snap.imported_genres {
            let want = format!("{count} tracks");
            assert!(
                lines
                    .windows(2)
                    .any(|w| w[0] == genre.as_str() && w[1] == want.as_str()),
                "missing rendered row {genre} / {want}:\n{text}"
            );
        }
        for genre in ["pop", "electronic", "indie", "lofi", "chill", "dance", "rock"] {
            assert!(
                snap.imported_genres.iter().any(|(g, _)| g == genre),
                "snapshot lost imported genre {genre}"
            );
            assert!(text.contains(genre), "{genre} not rendered:\n{text}");
        }

        // Imported artists and their counts are rendered from the same probe.
        for (artist, count) in snap.imported_artists.iter().take(4) {
            let want = format!("{count} tracks");
            assert!(
                lines
                    .windows(2)
                    .any(|w| w[0] == artist.as_str() && w[1] == want.as_str()),
                "missing rendered row {artist} / {want}:\n{text}"
            );
        }

        // The stat box reads the snapshot's own value: a played genre, or the
        // dash when nothing has been played yet. Never an imported-inferred one.
        let stat = snap.top_genre().unwrap_or("-");
        assert!(
            lines
                .windows(2)
                .any(|w| w[0] == stat && w[1] == "top genre"),
            "top genre stat does not show {stat}:\n{text}"
        );

        // "mix" may only appear where nothing real is known; nothing here is.
        assert!(
            !lines.iter().any(|l| *l == "mix"),
            "render still shows the mix fallback:\n{text}"
        );
        assert!(!text.contains("Open ears - a bit of everything"));
    }

    fn temp_profile(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!("rustify-app-{}-{}.txt", tag, std::process::id()))
    }

    /// Renders one real frame through the app's own paint path and collects
    /// every text shape it actually drew.
    fn frame_text(app: &mut App, ctx: &egui::Context) -> String {
        let mut input = egui::RawInput::default();
        input.screen_rect = Some(egui::Rect::from_min_size(
            egui::pos2(0.0, 0.0),
            egui::vec2(1440.0, 2400.0),
        ));
        let output = ctx.run(input, |ctx| {
            app.run_frame(ctx);
        });
        let mut lines = Vec::new();
        for clipped in &output.shapes {
            collect_text(&clipped.shape, &mut lines);
        }
        lines.join("\n")
    }

    fn collect_text(shape: &egui::Shape, out: &mut Vec<String>) {
        match shape {
            egui::Shape::Text(t) => out.push(t.galley.text().to_string()),
            egui::Shape::Vec(v) => {
                for s in v {
                    collect_text(s, out);
                }
            }
            _ => {}
        }
    }

    fn drive(app: &mut App, ctx: &egui::Context, frames: usize) -> Vec<Frame> {
        let mut input = egui::RawInput::default();
        input.screen_rect = Some(egui::Rect::from_min_size(
            egui::pos2(0.0, 0.0),
            egui::vec2(1280.0, 800.0),
        ));
        let mut seen = Vec::new();
        for _ in 0..frames {
            let next = input.clone();
            let _ = ctx.run(next, |ctx| seen.push(app.run_frame(ctx)));
        }
        seen
    }

    #[test]
    fn right_click_paste_flattens_clipboard_text() {
        let mut text = String::new();
        assert!(append_pasted(&mut text, "abc123\r\n"));
        assert_eq!(text, "abc123");
        assert!(append_pasted(&mut text, "\tdef"));
        assert_eq!(text, "abc123def");
        assert!(!append_pasted(&mut text, " \n\t "));
        assert_eq!(text, "abc123def");
    }

    #[test]
    fn locked_frame_blocks_playback_and_profile_writes() {
        let path = temp_profile("locked");
        let mut seeded = Profile::default();
        seeded.record_play(
            "Nova Kite",
            "Nova Kite|Neon Horizon",
            &["synthwave".to_string()],
        );
        seeded.save_to(&path);
        let before = std::fs::read(&path).unwrap();

        let mut app = App::for_test(gate::check_with(true), path.clone());
        assert!(!app.gate.found, "forced probes must leave the gate locked");

        app.playing = true;
        app.pos = 42.0;
        let now_before = app.now;
        let listened_before = app.profile.listened_secs;
        let plays_before = app.profile.plays;

        let ctx = egui::Context::default();
        let frames = drive(&mut app, &ctx, 3);

        assert_eq!(frames, vec![Frame::Locked; 3]);
        assert_eq!(app.pos, 42.0, "playback must not advance while locked");
        assert_eq!(app.now, now_before, "the queue must not move while locked");
        assert_eq!(app.profile.listened_secs, listened_before);
        assert_eq!(app.profile.plays, plays_before);
        assert_eq!(app.locked_frames, 3);
        assert_eq!(app.player_frames, 0, "no player work may run while locked");
        assert_eq!(
            std::fs::read(&path).unwrap(),
            before,
            "locked frames must not write the profile"
        );

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn import_page_renders_and_merged_playlists_join_the_library() {
        let path = temp_profile("import");
        let open = gate::verdict(vec![gate::Evidence {
            label: "test probe".to_string(),
            ok: true,
        }]);
        let mut app = App::for_test(open, path.clone());
        let before = app.lib.len();

        app.view = View::Import;
        let ctx = egui::Context::default();
        assert_eq!(drive(&mut app, &ctx, 2), vec![Frame::Player; 2]);

        app.imported = vec![spotify::ImportedPlaylist {
            art: library::Art::default(),
            name: "Imported Test".to_string(),
            tracks: vec![spotify::ImportedTrack {
                art: library::Art::default(),
                title: "Neon Horizon".to_string(),
                artist: "Nova Kite".to_string(),
                album: "Neon Horizon".to_string(),
                secs: 254,
                added: String::new(),
            }],
        }];
        app.rebuild_lib();
        let ctx = egui::Context::default();
        assert_eq!(drive(&mut app, &ctx, 1), vec![Frame::Player]);

        assert_eq!(app.lib.len(), before + 1);
        let last = app.lib.last().unwrap();
        assert_eq!(last.name, "Imported Test");
        assert_eq!(last.tracks.len(), 1);
        assert_eq!(last.tracks[0].key(), "Nova Kite|Neon Horizon");

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn passing_gate_runs_the_player() {
        let path = temp_profile("unlocked");
        let open = gate::verdict(vec![gate::Evidence {
            label: "test probe".to_string(),
            ok: true,
        }]);
        let mut app = App::for_test(open, path.clone());

        let ctx = egui::Context::default();
        let frames = drive(&mut app, &ctx, 1);

        assert_eq!(frames, vec![Frame::Player]);
        assert_eq!(app.player_frames, 1);
        assert_eq!(app.locked_frames, 0);

        let _ = std::fs::remove_file(&path);
    }
}

// -------------------------------------------------------------- self test

fn simulate_profile(lib: &[Collection]) -> Profile {
    let mut p = Profile::default();
    p.theme = 1;
    p.volume = 0.6;
    let plan: [(usize, usize, u32); 6] = [
        (3, 0, 3), // Synthwave Essentials - Neon Horizon (Nova Kite)
        (3, 1, 2), // Synthwave Essentials - Night Circuit (Aster Drive)
        (4, 0, 2), // Lo-Fi Beats - Kettle Steam (Low Ceiling)
        (8, 0, 2), // Paper Planes - Paper Planes (The Ferns)
        (2, 3, 1), // Chill Vibes - Warm Static (The Ferns)
        (1, 0, 1), // Daily Mix 1 - Midnight Static (Nova Kite)
    ];
    for (ci, ti, reps) in plan {
        let coll = lib[ci].clone();
        let track = coll.tracks[ti].clone();
        let genres: Vec<String> = coll.genres.iter().map(|g| g.to_string()).collect();
        for _ in 0..reps {
            p.record_play(&track.artist, &track.key(), &genres);
            p.listened_secs += track.secs as f64;
        }
    }
    p.liked.insert(lib[3].tracks[0].key());
    p.liked.insert(lib[4].tracks[1].key());
    p
}

fn selftest_report_text(p: &Profile, lib: &[Collection]) -> String {
    let mut out = String::new();
    out.push_str(&format!("plays={}
", p.plays));
    out.push_str(&format!(
        "minutes={}
",
        (p.listened_secs / 60.0).round() as u32
    ));
    out.push_str(&format!("likes={}
", p.liked.len()));

    let snap = taste::TasteSnapshot::build(p, lib);
    let artists: Vec<String> = snap
        .artists
        .iter()
        .take(5)
        .map(|(n, c)| format!("{n}:{c}"))
        .collect();
    out.push_str(&format!("top_artists={}
", artists.join(",")));

    let genres: Vec<String> = snap
        .genres
        .iter()
        .take(5)
        .map(|(n, c)| format!("{n}:{c}"))
        .collect();
    out.push_str(&format!("top_genres={}
", genres.join(",")));
    out.push_str(&format!("personality={}
", snap.personality()));

    let rec: Vec<String> = taste::recommended(&snap, lib)
        .into_iter()
        .take(4)
        .map(|i| lib[i].name.clone())
        .collect();
    out.push_str(&format!("recommended={}
", rec.join(",")));

    for n in 0..3 {
        let m = taste::mix(&snap, lib, n);
        let head: Vec<String> = m.tracks.iter().take(4).map(|t| t.key()).collect();
        out.push_str(&format!(
            "mix{}={}|{}|{}
",
            n + 1,
            m.name,
            m.subtitle,
            head.join(",")
        ));
    }

    let liked: Vec<String> = taste::liked_collection(p, lib)
        .tracks
        .iter()
        .map(|t| t.key())
        .collect();
    out.push_str(&format!("liked={}
", liked.join(",")));
    out
}

/// Self-test data lives in its own file, so seeding can never touch the live
/// profile at the default path.
fn selftest_path() -> PathBuf {
    Profile::default_path().with_file_name("selftest-taste.txt")
}

fn selftest_write() {
    let lib = library::sample();
    let path = selftest_path();
    let simulated = simulate_profile(&lib);
    simulated.save_to(&path);

    let loaded = Profile::load_from(&path);
    if loaded != simulated {
        eprintln!("selftest: profile did not survive save -> load");
        std::process::exit(1);
    }
    print!("{}", selftest_report_text(&loaded, &lib));
}

fn selftest_report() {
    let lib = library::sample();
    let loaded = Profile::load_from(&selftest_path());
    print!("{}", selftest_report_text(&loaded, &lib));
}

// ------------------------------------------------------------------- clock

fn greeting() -> &'static str {
    match local_hour() {
        5..=11 => "Good morning",
        12..=17 => "Good afternoon",
        18..=22 => "Good evening",
        _ => "Still up?",
    }
}

#[cfg(windows)]
#[repr(C)]
struct WinSystemTime {
    year: u16,
    month: u16,
    dow: u16,
    day: u16,
    hour: u16,
    minute: u16,
    second: u16,
    ms: u16,
}

#[cfg(windows)]
#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetLocalTime(t: *mut WinSystemTime);
}

fn local_hour() -> u32 {
    #[cfg(windows)]
    {
        let mut st = WinSystemTime {
            year: 0,
            month: 0,
            dow: 0,
            day: 0,
            hour: 0,
            minute: 0,
            second: 0,
            ms: 0,
        };
        unsafe { GetLocalTime(&mut st) };
        st.hour as u32
    }
    #[cfg(not(windows))]
    {
        18
    }
}

#[cfg(test)]
mod notice_tests {
    use super::*;

    fn test_app(tag: &str) -> App {
        let dir = std::env::temp_dir().join(format!("rustify-{tag}-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let open = gate::verdict(vec![gate::Evidence {
            label: "test probe".to_string(),
            ok: true,
        }]);
        App::for_test(open, dir.join("taste.txt"))
    }

    #[test]
    fn a_track_with_no_file_explains_the_silence() {
        let mut app = test_app("notice");
        // No local files and no Spotify session: this click cannot make sound.
        app.audio_dir = String::new();
        app.audio_lib = audio::Index::scan("");
        app.spotify_mode = false;
        app.conn_tx = None;
        app.play_coll(0, 0);
        let (msg, _) = app.notice.clone().expect("a notice was set");
        assert!(msg.contains("No file on this PC"), "unexpected notice: {msg}");
        assert!(
            msg.contains("preview"),
            "the click must fall back to a preview: {msg}"
        );
    }

    #[test]
    fn a_premium_refusal_is_recognised() {
        assert!(premium_error(
            "Spotify returned HTTP 403: Player command failed: Premium required"
        ));
        assert!(!premium_error(
            "Spotify returned HTTP 401: The access token expired"
        ));
    }

    #[test]
    fn local_rows_keep_the_exact_file_they_came_from() {
        let dir = std::env::temp_dir().join(format!("rustify-local-rows-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("a")).unwrap();
        std::fs::create_dir_all(dir.join("b")).unwrap();
        let one = dir.join("a").join("Same Artist - Same Song.mp3");
        let two = dir.join("b").join("Same Artist - Same Song.mp3");
        std::fs::write(&one, b"x").unwrap();
        std::fs::write(&two, b"x").unwrap();

        let index = audio::Index::scan(dir.to_str().unwrap());
        let coll = local_files_collection(&index).expect("a collection");
        assert_eq!(coll.tracks.len(), 2, "two files, two rows");
        let mut got: Vec<_> = coll.tracks.iter().filter_map(|t| t.path.clone()).collect();
        got.sort();
        let mut want = vec![one, two];
        want.sort();
        assert_eq!(got, want, "each row points at its own file");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_missing_spotify_session_falls_back_to_the_official_preview() {
        let mut app = test_app("notice-hit");
        app.spotify_mode = false;
        app.conn_tx = None;
        let hit = catalog::Hit {
            kind: catalog::HitKind::Track,
            id: "x".to_string(),
            uri: "spotify:track:x".to_string(),
            title: "Song That Is Not On This Pc".to_string(),
            artist: "Nobody".to_string(),
            album: String::new(),
            secs: 0,
            art: library::Art::default(),
        };
        app.play_hit(&hit);
        let (msg, _) = app.notice.clone().expect("a notice was set");
        assert!(msg.contains("preview"), "unexpected notice: {msg}");
        assert!(app.playing, "the preview clock runs without Spotify");
        assert_eq!(app.queue.len(), 1);
        assert_eq!(app.queue[0].title, "Song That Is Not On This Pc");
    }

    /// Stands in for the Spotify worker: everything the window sends is
    /// recorded, and `out` lets a test answer as if Spotify's Web API had.
    struct FakeLink {
        app: App,
        rx: std::sync::mpsc::Receiver<conn_worker::Request>,
        out: std::sync::mpsc::Sender<conn_worker::Out>,
    }

    fn spotify_transport(tag: &str) -> FakeLink {
        let mut app = test_app(tag);
        let (cmds, rx) = std::sync::mpsc::channel();
        let (out, outs) = std::sync::mpsc::channel();
        app.conn_tx = Some(cmds);
        app.conn_rx = Some(outs);
        app.spotify_mode = true;
        app.devices = vec![connect::Device {
            id: "dev-1".to_string(),
            name: "PDX-PC".to_string(),
            active: false,
            kind: "Computer".to_string(),
        }];
        FakeLink { app, rx, out }
    }

    fn track_hit(title: &str, artist: &str, uri: &str) -> catalog::Hit {
        catalog::Hit {
            kind: catalog::HitKind::Track,
            id: uri.rsplit(':').next().unwrap_or(uri).to_string(),
            uri: uri.to_string(),
            title: title.to_string(),
            artist: artist.to_string(),
            album: String::new(),
            secs: 0,
            art: library::Art::default(),
        }
    }

    /// The play command the fake transport received, if any.
    fn drain_play(rx: &std::sync::mpsc::Receiver<conn_worker::Request>) -> Option<(String, Option<String>)> {
        let mut found = None;
        while let Ok(req) = rx.try_recv() {
            if let conn_worker::Request::Api(connect::Command::PlayUri { uri, device }) = req {
                found = Some((uri, device));
            }
        }
        found
    }

    #[test]
    fn a_local_files_row_plays_its_file_instead_of_searching_spotify() {
        let mut fake = spotify_transport("local-row");
        let app = &mut fake.app;
        let dir = std::env::temp_dir().join(format!("rustify-local-spotify-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("Kavinsky - Nightcall.mp3");
        std::fs::write(&file, b"x").unwrap();

        app.audio_lib = audio::Index::scan(dir.to_str().unwrap());
        app.lib
            .push(local_files_collection(&app.audio_lib).expect("a local collection"));
        let ci = app.lib.len() - 1;

        app.play_coll(ci, 0);

        let mut paused = false;
        let mut searched = false;
        while let Ok(req) = fake.rx.try_recv() {
            match req {
                conn_worker::Request::Api(connect::Command::Pause) => paused = true,
                conn_worker::Request::PlaySearch(_) => searched = true,
                _ => {}
            }
        }
        assert!(paused, "Spotify must be paused before the file starts");
        assert!(!searched, "a file on this PC must not be searched on Spotify");
        assert!(app.local_override);
        assert!(!app.spotify_playing(), "the file owns the transport");
        assert_eq!(app.cur().and_then(|t| t.path.clone()), Some(file));

        // A paused Spotify poll must not stop the local clock.
        fake.out
            .send(conn_worker::Out::State(Ok(Some(connect::Playback {
                playing: false,
                ..Default::default()
            }))))
            .unwrap();
        app.poll_connect(&egui::Context::default());
        assert!(app.playing, "Spotify's pause stopped the local clock");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_spotify_row_searches_then_plays_on_the_fake_transport() {
        let mut fake = spotify_transport("search-play");
        let app = &mut fake.app;
        let title = app.lib[0].tracks[0].title.clone();
        let artist = app.lib[0].tracks[0].artist.clone();
        assert!(app.lib[0].tracks[0].path.is_none());

        app.play_coll(0, 0);

        let query = loop {
            match fake.rx.try_recv() {
                Ok(conn_worker::Request::PlaySearch(q)) => break q,
                Ok(_) => continue,
                Err(_) => panic!("no PlaySearch was sent"),
            }
        };
        assert_eq!(query, format!("{title} {artist}"));

        // The worker answers with a track hit; the window turns that into a
        // play for the device the search came back for.
        fake.out
            .send(conn_worker::Out::PlaySearch(Ok(vec![track_hit(
                &title,
                &artist,
                "spotify:track:found",
            )])))
            .unwrap();
        app.poll_connect(&egui::Context::default());

        let (uri, device) = drain_play(&fake.rx).expect("a play command");
        assert_eq!(uri, "spotify:track:found");
        assert_eq!(device.as_deref(), Some("dev-1"));
        assert!(app.playing);
    }

    #[test]
    fn waiting_for_a_device_opens_spotify_then_plays() {
        let mut fake = spotify_transport("device-wait");
        let app = &mut fake.app;
        app.devices.clear();

        app.spotify_play(&track_hit("Nightcall", "Kavinsky", "spotify:track:wait"));

        let mut asked_devices = false;
        while let Ok(req) = fake.rx.try_recv() {
            match req {
                conn_worker::Request::Devices => asked_devices = true,
                conn_worker::Request::Api(connect::Command::PlayUri { .. }) => {
                    panic!("played without a device")
                }
                _ => {}
            }
        }
        assert!(asked_devices, "the window asks which clients are available");
        assert!(app.pending_play_uri.is_some());
        assert!(app.wake.is_some());

        // Time passes with no client: Spotify is opened for the owner instead
        // of leaving a notice that asks them to do it themselves.
        if let Some(wake) = &mut app.wake {
            wake.since = std::time::Instant::now() - std::time::Duration::from_millis(1300);
        }
        app.tick_wake();
        let (msg, _) = app.notice.clone().expect("a notice");
        assert!(msg.contains("Opening Spotify"), "{msg}");
        assert!(app.wake.as_ref().is_some_and(|w| w.opened));
        assert!(app.pending_play_uri.is_some());
        assert!(
            drain_play(&fake.rx).is_none(),
            "nothing played without a device"
        );

        // A client appears; the waiting play lands on it.
        app.devices = vec![connect::Device {
            id: "dev-1".to_string(),
            name: "PDX-PC".to_string(),
            active: false,
            kind: "Computer".to_string(),
        }];
        app.tick_pending_play();
        let (uri, device) = drain_play(&fake.rx).expect("a play command");
        assert_eq!(uri, "spotify:track:wait");
        assert_eq!(device.as_deref(), Some("dev-1"));
        assert!(app.pending_play_uri.is_none());
        assert!(app.wake.is_none());
    }
}
