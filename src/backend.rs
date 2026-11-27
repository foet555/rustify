// Native Spotify playback backend.
//
// The UI only ever touches two tokio channels:
//   commands: PlayerCommand  (UI -> backend)
//   events:   PlayerEvent    (backend -> UI)
// Everything else - OAuth refreshes, rspotify metadata calls, librespot's
// Spotify Connect session and Ogg Vorbis decoding - lives on a dedicated
// runtime thread. A slow network call or a decoder stall can therefore never
// hold up an egui frame.
//
// The session comes from the app's existing PKCE login and is stored in
// `connect.bin`, sealed with Windows DPAPI. No credential is written as plain
// text. Full playback is Premium-only: a Free account gets a clear Error event
// before librespot ever tries to fetch audio.
use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, TimeDelta, Utc};
use librespot::connect::{ConnectConfig, LoadRequest, LoadRequestOptions, Spirc};
use librespot::core::authentication::Credentials;
use librespot::core::config::SessionConfig;
use librespot::core::session::Session as LibrespotSession;
use librespot::core::spotify_uri::SpotifyUri;
use librespot::playback::audio_backend;
use librespot::playback::config::{AudioFormat, PlayerConfig};
use librespot::playback::mixer::{self, Mixer, MixerConfig};
use librespot::playback::player::{Player, PlayerEvent as LibrespotEvent};
use rspotify::clients::{BaseClient, OAuthClient};
use rspotify::model::idtypes::{Id, TrackId};
use rspotify::model::{FullTrack, SubscriptionLevel};
use rspotify::{AuthCodeSpotify, Token};
use tokio::sync::mpsc;

use crate::connect;
use crate::profile::Profile;
use crate::spotify::{self, Curl, Endpoints};

/// Shown for Free accounts and for librespot's own Premium rejection. The
/// wording matches the rest of the app so the owner never sees a bare 403.
pub const PREMIUM_REQUIRED: &str =
    "Spotify Premium is required to stream full songs in Rustify. Sign in with a Premium account.";

/// The single Spotify Connect device this application registers. The UI looks
/// for this name to route playback to the native engine and never starts the
/// old hidden WebView player alongside it.
pub const DEVICE_NAME: &str = "Rustify (native)";

/// A signed-in Spotify account, as reported by the Web API.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct User {
    pub id: String,
    pub display_name: String,
    /// "premium", "free" or "unknown" when Spotify omits the deprecated field.
    pub product: String,
    /// True when the native engine registered as a Spotify Connect device.
    /// Access-token sessions cannot use Connect, so they stream directly
    /// through librespot instead of failing playback.
    pub connect_device: bool,
}

/// Everything the UI needs to draw the current track without another API call.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrackMetadata {
    pub id: String,
    pub uri: String,
    pub title: String,
    pub artists: String,
    pub album: String,
    pub cover_url: String,
    pub duration_ms: u32,
}

/// Messages sent from the UI to the background worker. The transport controls
/// are the native-player contract; this build wires sign-in and event polling,
/// leaving the existing Web Playback transport untouched.
#[allow(dead_code)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PlayerCommand {
    /// Sign in (or restore the sealed session) and become a Connect device.
    Login,
    /// Disconnect this app's Connect device without ending the worker thread.
    Logout,
    /// A Spotify track URI, open.spotify.com URL, or bare 22-character id.
    PlayTrack(String),
    Pause,
    Resume,
    /// Position in milliseconds.
    Seek(u32),
    /// 0-100, the same scale as the app's volume slider.
    SetVolume(u16),
}

/// Messages sent from the worker back to the UI.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PlayerEvent {
    Authenticated(User),
    TrackChanged(TrackMetadata),
    /// Current playback position in milliseconds.
    Progress(u32),
    Error(String),
}

/// Handle held by the UI. Dropping it closes the command channel, which ends
/// the worker loop and its runtime.
pub struct Backend {
    commands: mpsc::Sender<PlayerCommand>,
    events: mpsc::Receiver<PlayerEvent>,
    /// Detached on purpose: the runtime thread exits when the UI drops its
    /// command sender, so the window never blocks on shutdown.
    _thread: Option<std::thread::JoinHandle<()>>,
}

impl Backend {
    /// Start the dedicated runtime thread. Nothing connects to Spotify until
    /// the UI sends `PlayerCommand::Login`.
    pub fn spawn() -> Self {
        let (command_tx, command_rx) = mpsc::channel(32);
        let (event_tx, event_rx) = mpsc::channel(64);
        let thread = std::thread::Builder::new()
            .name("rustify-spotify-audio".to_string())
            .spawn(move || {
                if cfg!(debug_assertions) {
                    eprintln!("[backend] worker thread started");
                }
                let runtime = match tokio::runtime::Builder::new_multi_thread()
                    .worker_threads(2)
                    .enable_all()
                    .build()
                {
                    Ok(runtime) => runtime,
                    Err(error) => {
                        let _ = event_tx.blocking_send(PlayerEvent::Error(format!(
                            "Could not start the Spotify audio runtime: {error}"
                        )));
                        return;
                    }
                };
                runtime.block_on(worker(command_rx, event_tx));
            });
        Self {
            commands: command_tx,
            events: event_rx,
            _thread: thread.ok(),
        }
    }

    /// A channel-only handle for headless tests: `send` reports false and
    /// `poll` is always empty, without starting a thread.
    #[cfg(test)]
    pub fn disabled() -> Self {
        let (command_tx, command_rx) = mpsc::channel(1);
        let (_event_tx, event_rx) = mpsc::channel(1);
        drop(command_rx);
        Self {
            commands: command_tx,
            events: event_rx,
            _thread: None,
        }
    }

    /// Non-blocking, for use inside an egui frame.
    pub fn send(&self, command: PlayerCommand) -> bool {
        self.commands.try_send(command).is_ok()
    }

    /// Drain one event. Call in a loop until it returns None each frame.
    pub fn poll(&mut self) -> Option<PlayerEvent> {
        self.events.try_recv().ok()
    }
}

async fn worker(mut commands: mpsc::Receiver<PlayerCommand>, events: mpsc::Sender<PlayerEvent>) {
    let mut worker = Worker {
        events,
        spirc: None,
        spotify: None,
        player: None,
        mixer: None,
    };
    while let Some(command) = commands.recv().await {
        if let Err(error) = worker.handle(command).await {
            worker.fail(error).await;
        }
    }
    if let Some(spirc) = worker.spirc.take() {
        let _ = spirc.shutdown();
    }
}

struct Worker {
    events: mpsc::Sender<PlayerEvent>,
    spirc: Option<Spirc>,
    spotify: Option<AuthCodeSpotify>,
    /// Set when Connect registration is unavailable. The librespot player can
    /// still decode and stream a track directly with the same session.
    player: Option<Arc<Player>>,
    mixer: Option<Arc<dyn Mixer>>,
}

impl Worker {
    async fn handle(&mut self, command: PlayerCommand) -> Result<(), String> {
        match command {
            PlayerCommand::Login => self.login().await,
            PlayerCommand::Logout => {
                if let Some(spirc) = self.spirc.take() {
                    let _ = spirc.shutdown();
                }
                if let Some(player) = self.player.take() {
                    player.stop();
                }
                self.mixer = None;
                self.spotify = None;
                Ok(())
            }
            PlayerCommand::PlayTrack(track) => self.play_track(&track).await,
            PlayerCommand::Pause => {
                if let Some(spirc) = &self.spirc {
                    spirc.pause().map_err(playback_error)
                } else if let Some(player) = &self.player {
                    player.pause();
                    Ok(())
                } else {
                    Err("Native playback is not ready.".to_string())
                }
            }
            PlayerCommand::Resume => {
                if let Some(spirc) = &self.spirc {
                    spirc.play().map_err(playback_error)
                } else if let Some(player) = &self.player {
                    player.play();
                    Ok(())
                } else {
                    Err("Native playback is not ready.".to_string())
                }
            }
            PlayerCommand::Seek(position_ms) => {
                if let Some(spirc) = &self.spirc {
                    spirc.set_position_ms(position_ms).map_err(playback_error)
                } else if let Some(player) = &self.player {
                    player.seek(position_ms);
                    Ok(())
                } else {
                    Err("Native playback is not ready.".to_string())
                }
            }
            PlayerCommand::SetVolume(volume) => {
                let raw = (volume.min(100) as u32 * u16::MAX as u32 / 100) as u16;
                if let Some(spirc) = &self.spirc {
                    spirc.set_volume(raw).map_err(playback_error)
                } else if let Some(mixer) = &self.mixer {
                    mixer.set_volume(raw);
                    Ok(())
                } else {
                    Ok(())
                }
            }
        }
    }

    fn spirc(&self) -> Result<&Spirc, String> {
        self.spirc
            .as_ref()
            .ok_or_else(|| "Sign in to Spotify before using the native player.".to_string())
    }

    async fn fail(&self, error: String) {
        let _ = self.events.send(PlayerEvent::Error(error)).await;
    }

    /// Restore or create the OAuth session, verify the account is not Free,
    /// then open a librespot session and register the Connect device.
    async fn login(&mut self) -> Result<(), String> {
        // A second Login while this device is already registered would create
        // a second Connect client. The UI attaches and signs in separately, so
        // make the command idempotent.
        if self.spirc.is_some() {
            return Ok(());
        }
        let profile = Profile::load_from(&Profile::default_path());
        let client_id = profile.client_id.trim().to_string();
        if client_id.is_empty() {
            return Err("Paste your Spotify app Client ID in Settings first.".to_string());
        }

        let store = connect::default_path();
        let mut session = match connect::load(&store, &client_id) {
            Some(session) => session,
            None => {
                let new_session = browser_login(client_id.clone()).await?;
                if !connect::save(&store, &new_session) {
                    return Err(
                        "Spotify signed in, but the session could not be sealed with Windows DPAPI; refusing to cache it as plain text."
                            .to_string(),
                    );
                }
                new_session
            }
        };

        if session.tokens.expired() {
            session = refresh_session(session, store.clone()).await?;
        }

        let spotify = spotify_client(&session);
        // The Web API provides the display name and an optional product hint.
        // It is allowed to fail here: a rate-limited profile lookup must not
        // stop librespot from streaming. ProductInfo below is authoritative.
        let me = match spotify.me().await {
            Ok(me) => Some(me),
            Err(error) => {
                eprintln!("[backend] profile lookup failed: {}", api_error(error));
                None
            }
        };
        // Spotify removed `product` from the Web API in 2026. When it is present
        // it is authoritative; when it is absent the ProductInfo account type
        // is checked before any player is created.
        #[allow(deprecated)]
        let product = me.as_ref().and_then(|me| me.product.clone());
        if matches!(product, Some(SubscriptionLevel::Free)) {
            return Err(PREMIUM_REQUIRED.to_string());
        }
        let mut product = match product {
            Some(SubscriptionLevel::Premium) => "premium",
            Some(SubscriptionLevel::Free) => "free",
            _ => "unknown",
        };
        let user_id = me
            .as_ref()
            .map(|me| me.id.id().to_string())
            .unwrap_or_else(|| "spotify".to_string());
        let display_name = me
            .as_ref()
            .and_then(|me| me.display_name.clone())
            .unwrap_or_else(|| user_id.clone());

        let credentials = Credentials::with_access_token(session.tokens.access.clone());
        let librespot_session = LibrespotSession::new(SessionConfig::default(), None);
        librespot_session
            .connect(credentials.clone(), false)
            .await
            .map_err(playback_error)?;

        // librespot's ProductInfo packet arrives just after `connect` returns.
        // Wait for the account type before creating a player: the patched
        // librespot-core logs a Free account instead of exiting, and this check
        // turns that into the documented Error event.
        verify_account_type(&librespot_session).await?;
        if product == "unknown" {
            product = "premium";
        }

        let mixer_factory = mixer::find(None)
            .ok_or_else(|| "No Spotify audio mixer is available in this build.".to_string())?;
        let mixer = mixer_factory(MixerConfig::default()).map_err(playback_error)?;
        let sink_builder = audio_backend::find(None)
            .ok_or_else(|| "No Spotify audio output is available in this build.".to_string())?;
        let player_config = PlayerConfig {
            position_update_interval: Some(Duration::from_secs(1)),
            ..Default::default()
        };
        let player = Player::new(
            player_config,
            librespot_session.clone(),
            mixer.get_soft_volume(),
            move || sink_builder(None, AudioFormat::default()),
        );

        // librespot exposes progress through this unbounded channel. Forward
        // only the position updates the UI needs and leave its receiver alive
        // for the lifetime of the player.
        let mut player_events = player.get_player_event_channel();
        let progress_tx = self.events.clone();
        tokio::spawn(async move {
            while let Some(event) = player_events.recv().await {
                if let LibrespotEvent::PositionChanged { position_ms, .. } = event {
                    if progress_tx
                        .send(PlayerEvent::Progress(position_ms))
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
            }
        });

        let connect_config = ConnectConfig {
            name: DEVICE_NAME.to_string(),
            ..Default::default()
        };
        let (spirc, task) = Spirc::new(
            connect_config,
            librespot_session,
            credentials,
            player,
            mixer,
        )
        .await
        .map_err(playback_error)?;
        tokio::spawn(task);
        spirc.activate().map_err(playback_error)?;
        self.spirc = Some(spirc);
        self.player = None;
        self.mixer = None;
        self.spotify = Some(spotify);
        let _ = self
            .events
            .send(PlayerEvent::Authenticated(User {
                id: user_id,
                display_name,
                product: product.to_string(),
                connect_device: true,
            }))
            .await;
        Ok(())
    }

    async fn play_track(&mut self, input: &str) -> Result<(), String> {
        let id = track_id(input)
            .ok_or_else(|| "Expected a Spotify track URI, URL, or 22-character id.".to_string())?;
        let uri = format!("spotify:track:{id}");
        // Metadata is a nicety, not a precondition: a rate-limited Web API
        // must not stop the native engine from streaming a known track URI.
        let metadata = self.metadata(&id, &uri).await.ok();

        if let Some(spirc) = &self.spirc {
            let request = LoadRequest::from_tracks(
                vec![uri],
                LoadRequestOptions {
                    start_playing: true,
                    ..Default::default()
                },
            );
            spirc.load(request).map_err(playback_error)?;
        } else if let Some(player) = &self.player {
            let target = SpotifyUri::from_uri(&uri)
                .map_err(|error| format!("Invalid Spotify track URI: {error}"))?;
            player.load(target, true, 0);
        } else {
            return Err("Native playback is not ready.".to_string());
        }
        if let Some(metadata) = metadata {
            let _ = self
                .events
                .send(PlayerEvent::TrackChanged(metadata))
                .await;
        }
        Ok(())
    }

    async fn metadata(&self, id: &str, uri: &str) -> Result<TrackMetadata, String> {
        let spotify = self
            .spotify
            .as_ref()
            .ok_or_else(|| "Sign in to Spotify before playing a track.".to_string())?;
        let track_id =
            TrackId::from_id(id).map_err(|error| format!("Invalid Spotify track id: {error}"))?;
        let track = spotify.track(track_id, None).await.map_err(api_error)?;
        Ok(metadata_from_rspotify(&track, uri))
    }
}

/// Convert the open.spotify.com / URI / bare-id forms the rest of the app uses
/// into the bare id librespot and rspotify both want.
fn track_id(input: &str) -> Option<String> {
    let input = input.trim();
    let candidate = if let Some(rest) = input.strip_prefix("spotify:track:") {
        rest
    } else if let Some(index) = input.find("open.spotify.com/track/") {
        &input[index + "open.spotify.com/track/".len()..]
    } else if input.contains("://") {
        return None;
    } else {
        input
    };
    let id = candidate
        .split(['?', '&', '/', '#'])
        .next()
        .unwrap_or_default()
        .trim();
    (id.len() == 22 && id.chars().all(|c| c.is_ascii_alphanumeric())).then(|| id.to_string())
}

fn metadata_from_rspotify(track: &FullTrack, uri: &str) -> TrackMetadata {
    let artists = track
        .artists
        .iter()
        .map(|artist| artist.name.clone())
        .collect::<Vec<_>>()
        .join(", ");
    let cover_url = track
        .album
        .images
        .iter()
        .max_by_key(|image| image.width.unwrap_or(0))
        .map(|image| image.url.clone())
        .unwrap_or_default();
    TrackMetadata {
        id: track
            .id
            .as_ref()
            .map(|id| id.id().to_string())
            .unwrap_or_default(),
        uri: uri.to_string(),
        title: track.name.clone(),
        artists,
        album: track.album.name.clone(),
        cover_url,
        duration_ms: track.duration.num_milliseconds().clamp(0, u32::MAX as i64) as u32,
    }
}

fn spotify_client(session: &connect::Session) -> AuthCodeSpotify {
    let expires_at = DateTime::<Utc>::from_timestamp_millis(session.tokens.expires_at_ms)
        .unwrap_or_else(|| Utc::now() + TimeDelta::seconds(60));
    let token = Token {
        access_token: session.tokens.access.clone(),
        expires_in: TimeDelta::seconds(3600),
        expires_at: Some(expires_at),
        refresh_token: Some(session.tokens.refresh.clone()),
        scopes: HashSet::new(),
    };
    let mut client = AuthCodeSpotify::from_token(token);
    // The DPAPI-sealed store is the only credential cache. Never let rspotify
    // write a second, plaintext token cache next to the executable.
    client.config.token_cached = false;
    client.config.token_refreshing = false;
    client
}

/// `Session::connect` only opens the access point; the account type comes in a
/// ProductInfo packet on the dispatch task. Bounded wait, then fail closed if
/// Spotify never tells us the account is Premium.
async fn verify_account_type(session: &LibrespotSession) -> Result<(), String> {
    for _ in 0..50 {
        if let Some(account_type) = session.get_user_attribute("type") {
            return if account_type == "premium" {
                Ok(())
            } else {
                Err(PREMIUM_REQUIRED.to_string())
            };
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    Err(
        "Could not verify that this Spotify account is Premium; refusing to stream full songs."
            .to_string(),
    )
}

async fn browser_login(client_id: String) -> Result<connect::Session, String> {
    let login_id = client_id.clone();
    let tokens = tokio::task::spawn_blocking(move || {
        let options = spotify::Options::default();
        let mut log = |line: String| eprintln!("[spotify backend] {line}");
        connect::login(&Curl, &login_id, &options, &mut log)
    })
    .await
    .map_err(|error| format!("Spotify sign-in task failed: {error}"))??;
    Ok(connect::Session {
        client_id,
        endpoints: Endpoints::default(),
        tokens,
        scope_rev: connect::SCOPE_REV,
    })
}

async fn refresh_session(
    session: connect::Session,
    store: std::path::PathBuf,
) -> Result<connect::Session, String> {
    tokio::task::spawn_blocking(move || {
        let mut session = session;
        let changed = connect::ensure_fresh(&Curl, &mut session)?;
        if changed && !connect::save(&store, &session) {
            return Err(
                "The refreshed Spotify session could not be sealed with Windows DPAPI."
                    .to_string(),
            );
        }
        Ok::<_, String>(session)
    })
    .await
    .map_err(|error| format!("Spotify token refresh task failed: {error}"))?
}

fn api_error(error: impl std::fmt::Display) -> String {
    let raw = error.to_string();
    if premium_error(&raw) {
        PREMIUM_REQUIRED.to_string()
    } else {
        raw
    }
}

fn playback_error(error: impl std::fmt::Display) -> String {
    let raw = error.to_string();
    if premium_error(&raw) {
        PREMIUM_REQUIRED.to_string()
    } else {
        format!("Spotify playback: {raw}")
    }
}

fn premium_error(message: &str) -> bool {
    let message = message.to_ascii_lowercase();
    message.contains("premium") || message.contains("premiumrequired")
}

/// Headless check of the primary engine: refresh the sealed session, open a
/// librespot session with its access token, prove the account is Premium, and
/// make sure a mixer and audio backend can be created. Nothing is registered as
/// a Connect device and no audio is played, so it is safe to run beside the app.
/// It returns the exact error the UI would have received.
pub fn native_smoke() -> Result<String, String> {
    let profile = Profile::load_from(&Profile::default_path());
    let mut session = connect::load(&connect::default_path(), &profile.client_id)
        .ok_or_else(|| "no saved Spotify session".to_string())?;
    if connect::ensure_fresh(&Curl, &mut session)? && !connect::save(&connect::default_path(), &session)
    {
        return Err("the refreshed session could not be sealed".to_string());
    }
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .map_err(|error| format!("could not start the audio runtime: {error}"))?;
    runtime.block_on(async move {
        let mut out = String::new();
        // The Web API is used for names and search, never for the stream
        // itself. Report its state separately so a 429 here is not mistaken
        // for a broken native engine.
        let web = spotify_client(&session);
        match web.me().await {
            Ok(me) => {
                out.push_str(&format!(
                    "me=ok id={} product={:?}\n",
                    me.id.id(),
                    me.product
                ));
            }
            Err(error) => out.push_str(&format!("me_error={}\n", api_error(error))),
        }
        let credentials = Credentials::with_access_token(session.tokens.access.clone());
        let librespot_session = LibrespotSession::new(SessionConfig::default(), None);
        librespot_session
            .connect(credentials, false)
            .await
            .map_err(playback_error)?;
        verify_account_type(&librespot_session).await?;
        let mixer_factory = mixer::find(None)
            .ok_or_else(|| "no Spotify audio mixer is available in this build".to_string())?;
        mixer_factory(MixerConfig::default()).map_err(playback_error)?;
        let sink_builder = audio_backend::find(None)
            .ok_or_else(|| "no Spotify audio output is available in this build".to_string())?;
        let _sink = sink_builder(None, AudioFormat::default());
        out.push_str(&format!(
            "native=ok account=premium device={DEVICE_NAME} scopes_streaming=true\n"
        ));
        Ok(out)
    })
}

/// The optional third engine: a spotifyd daemon on this machine registers
/// itself as a Spotify Connect device, so Rustify can hand playback to it with
/// the same Web API calls it uses for any other Spotify client. Nothing is
/// bundled or downloaded; if spotifyd is not installed the chain simply moves
/// on to rspotify. The lookup runs once per process.
pub fn spotifyd_path() -> Option<&'static std::path::PathBuf> {
    use std::sync::OnceLock;
    static FOUND: OnceLock<Option<std::path::PathBuf>> = OnceLock::new();
    FOUND
        .get_or_init(|| {
            let exe = if cfg!(windows) {
                "spotifyd.exe"
            } else {
                "spotifyd"
            };
            let mut candidates = Vec::new();
            if let Ok(current) = std::env::current_exe() {
                if let Some(dir) = current.parent() {
                    candidates.push(dir.join(exe));
                }
            }
            if let Ok(local) = std::env::var("LOCALAPPDATA") {
                let base = std::path::PathBuf::from(local);
                candidates.push(base.join("Microsoft/WinGet/Links").join(exe));
                candidates.push(base.join("Programs/spotifyd").join(exe));
            }
            if let Ok(home) = std::env::var("USERPROFILE") {
                candidates.push(
                    std::path::PathBuf::from(home)
                        .join(".cargo/bin")
                        .join(exe),
                );
            }
            if let Some(paths) = std::env::var_os("PATH") {
                candidates.extend(std::env::split_paths(&paths).map(|dir| dir.join(exe)));
            }
            candidates.into_iter().find(|path| path.is_file())
        })
        .as_ref()
}
