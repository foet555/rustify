// Playback control for the Spotify app the owner is already signed into.
//
// This is the official Spotify Web API "player" surface: Rustify reads the
// current playback state and sends play/pause/next/previous/seek/volume
// commands. No audio ever passes through this program - Spotify streams to its
// own client - so nothing here copies, downloads or redistributes a track.
// Playback control needs a Premium account, which is why the UI says so.
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

use crate::spotify::{self, Curl, Endpoints, HttpResponse, Options};

/// Playback control alone. Kept apart from the import scopes so a login is
/// never asked for more than the feature being turned on.
pub const SCOPES: &str = "user-read-playback-state user-modify-playback-state";

/// Anything the module needs from the network, so tests can answer without a
/// socket and the app can use curl.exe.
pub trait Transport {
    fn get(&self, url: &str, token: &str) -> Result<HttpResponse, String>;
    fn post_form(&self, url: &str, form: &[(&str, &str)]) -> Result<HttpResponse, String>;
    fn send_json(
        &self,
        method: &str,
        url: &str,
        token: &str,
        body: Option<&str>,
    ) -> Result<HttpResponse, String>;
}

impl Transport for Curl {
    fn get(&self, url: &str, token: &str) -> Result<HttpResponse, String> {
        <Curl as spotify::Http>::get(self, url, token)
    }

    fn post_form(&self, url: &str, form: &[(&str, &str)]) -> Result<HttpResponse, String> {
        <Curl as spotify::Http>::post_form(self, url, form)
    }

    fn send_json(
        &self,
        method: &str,
        url: &str,
        token: &str,
        body: Option<&str>,
    ) -> Result<HttpResponse, String> {
        let mut config = format!(
            "url = \"{}\"\nrequest = \"{}\"\nheader = \"Authorization: Bearer {}\"\n",
            spotify::curl_escape(url),
            method,
            spotify::curl_escape(token)
        );
        match body {
            Some(body) => {
                config.push_str("header = \"Content-Type: application/json\"\ndata-binary = \"");
                config.push_str(&spotify::curl_escape(body));
                config.push_str("\"\n");
            }
            None => config.push_str("header = \"Content-Length: 0\"\n"),
        }
        spotify::run_curl(&config)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Tokens {
    pub access: String,
    pub refresh: String,
    pub expires_at_ms: i64,
}

impl Tokens {
    fn from_response(v: &Value, previous_refresh: &str) -> Result<Self, String> {
        let access = v
            .get("access_token")
            .and_then(|t| t.as_str())
            .ok_or_else(|| "Spotify returned no access token".to_string())?
            .to_string();
        // A refresh response may leave out the refresh token: keep the one we
        // already had instead of losing the session.
        let refresh = v
            .get("refresh_token")
            .and_then(|t| t.as_str())
            .filter(|t| !t.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| previous_refresh.to_string());
        let secs = v
            .get("expires_in")
            .and_then(Value::as_u64)
            .unwrap_or(3600)
            .max(60);
        Ok(Self {
            access,
            refresh,
            expires_at_ms: now_ms() + (secs as i64 - 30) * 1000,
        })
    }

    pub fn expired(&self) -> bool {
        now_ms() >= self.expires_at_ms
    }
}

#[derive(Clone, Debug)]
pub struct Session {
    pub client_id: String,
    pub endpoints: Endpoints,
    pub tokens: Tokens,
}

/// Where a whole track stands right now, straight from Spotify.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Playback {
    pub playing: bool,
    pub progress_ms: u64,
    pub duration_ms: u64,
    pub track: String,
    pub artist: String,
    pub device: String,
    pub volume: Option<u8>,
}

/// One Spotify client the account is signed in on (phone, desktop, speaker).
#[derive(Clone, Debug)]
pub struct Device {
    pub id: String,
    pub name: String,
    pub active: bool,
    pub kind: String,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Command {
    Play,
    Pause,
    Next,
    Previous,
    Seek(u64),
    Volume(u8),
    /// Hand playback to another Spotify client, by device id.
    Transfer(String),
    /// Play a track from search results, by URI. A device id makes Spotify
    /// start it there without needing a separate transfer first.
    PlayUri {
        uri: String,
        device: Option<String>,
    },
    /// Play an album or artist from search results, by context URI.
    PlayContext {
        uri: String,
        device: Option<String>,
    },
    /// Append a searched track to the queue.
    QueueUri(String),
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn is_ok(response: &HttpResponse) -> bool {
    (200..300).contains(&response.status)
}

/// Spotify's player errors are all actionable, so say what to do about them
/// instead of showing a bare status code.
pub fn api_error(status: u16, body: &str) -> String {
    match status {
        401 => "Spotify sign-in expired; connect playback control again".to_string(),
        403 => "Spotify Premium is required to control playback from another app".to_string(),
        404 => "Open Spotify on this PC and start a song, then try again".to_string(),
        429 => "Spotify is rate limiting requests; wait a moment".to_string(),
        _ => {
            let reason = serde_json::from_str::<Value>(body)
                .ok()
                .and_then(|v| {
                    v.pointer("/error/message")
                        .and_then(|m| m.as_str())
                        .map(str::to_string)
                })
                .unwrap_or_else(|| body.trim().chars().take(200).collect());
            if reason.is_empty() {
                format!("Spotify returned HTTP {status}")
            } else {
                format!("Spotify returned HTTP {status}: {reason}")
            }
        }
    }
}

// ---------------------------------------------------------------------- auth

/// Full Authorization Code + PKCE login asking for the playback scopes. The
/// refresh token it returns is what keeps the session alive afterwards.
pub fn login<T: Transport>(
    http: &T,
    client_id: &str,
    options: &Options,
    log: &mut dyn FnMut(String),
) -> Result<Tokens, String> {
    let client_id = client_id.trim();
    if client_id.is_empty() {
        return Err("Paste your Spotify app Client ID first".to_string());
    }

    let verifier = spotify::new_verifier();
    let challenge = spotify::challenge_for(&verifier);
    let state = spotify::new_verifier();
    let listener = std::net::TcpListener::bind(("127.0.0.1", options.port))
        .map_err(|e| format!("cannot listen on 127.0.0.1:{}: {e}", options.port))?;
    let port = listener
        .local_addr()
        .map_err(|e| format!("cannot read the redirect port: {e}"))?
        .port();
    let redirect = spotify::redirect_uri(port);

    let url = spotify::auth_url_scoped(
        &options.endpoints,
        client_id,
        &redirect,
        &challenge,
        &state,
        SCOPES,
    );
    log("Opening Spotify sign-in for playback control...".to_string());
    log(format!("Redirect URI must be exactly {redirect}"));
    log(format!("If no browser opens, paste this URL: {url}"));
    if options.open_browser {
        spotify::open_browser(&url);
    }

    let code = spotify::wait_for_code(
        &listener,
        &state,
        options.timeout,
        options.cancel.as_deref(),
    )?;
    if spotify::is_cancelled(options.cancel.as_deref()) {
        return Err(spotify::CANCELLED.to_string());
    }
    log("Authorised. Exchanging the code for a token...".to_string());
    let response = http.post_form(
        &options.endpoints.token,
        &[
            ("grant_type", "authorization_code"),
            ("code", &code),
            ("redirect_uri", &redirect),
            ("client_id", client_id),
            ("code_verifier", &verifier),
        ],
    )?;
    let v: Value = serde_json::from_str(&response.body).unwrap_or(Value::Null);
    let tokens = Tokens::from_response(&v, "").map_err(|e| {
        if !is_ok(&response) {
            spotify::response_error(&response)
        } else {
            e
        }
    })?;
    if tokens.refresh.is_empty() {
        return Err(
            "Spotify did not return a refresh token, so the session would end immediately"
                .to_string(),
        );
    }
    if spotify::is_cancelled(options.cancel.as_deref()) {
        return Err(spotify::CANCELLED.to_string());
    }
    log("Playback control is ready.".to_string());
    Ok(tokens)
}

/// Swaps the refresh token for a new access token.
pub fn refresh<T: Transport>(
    http: &T,
    endpoints: &Endpoints,
    client_id: &str,
    refresh_token: &str,
) -> Result<Tokens, String> {
    if refresh_token.is_empty() {
        return Err("Connect playback control again to get a lasting Spotify session".to_string());
    }
    let response = http.post_form(
        &endpoints.token,
        &[
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
            ("client_id", client_id),
        ],
    )?;
    if !is_ok(&response) {
        return Err(api_error(response.status, &response.body));
    }
    let v: Value = serde_json::from_str(&response.body).unwrap_or(Value::Null);
    Tokens::from_response(&v, refresh_token)
}

/// Refreshes only when the access token is about to expire. Returns true when
/// new tokens were obtained, so the caller knows to save them.
pub fn ensure_fresh<T: Transport>(http: &T, session: &mut Session) -> Result<bool, String> {
    if !session.tokens.expired() {
        return Ok(false);
    }
    let tokens = refresh(
        http,
        &session.endpoints,
        &session.client_id,
        &session.tokens.refresh,
    )?;
    session.tokens = tokens;
    Ok(true)
}

// ------------------------------------------------------------------- player

fn parse_playback(body: &str) -> Option<Playback> {
    let v: Value = serde_json::from_str(body).ok()?;
    let item = v.get("item").filter(|i| !i.is_null())?;
    let track = item.get("name").and_then(Value::as_str)?.to_string();
    let artist = item
        .get("artists")
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .filter_map(|a| a.get("name").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_default();
    let device = v.get("device").cloned().unwrap_or(Value::Null);
    Some(Playback {
        playing: v.get("is_playing").and_then(Value::as_bool).unwrap_or(false),
        progress_ms: v.get("progress_ms").and_then(Value::as_u64).unwrap_or(0),
        duration_ms: item
            .get("duration_ms")
            .and_then(Value::as_u64)
            .unwrap_or(0),
        track,
        artist,
        device: device
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("Spotify")
            .to_string(),
        volume: device
            .get("volume_percent")
            .and_then(Value::as_u64)
            .map(|v| v.min(100) as u8),
    })
}

/// Current playback. `None` means Spotify answered "nothing is active" (204),
/// which is normal when the client is closed rather than an error.
pub fn state<T: Transport>(
    http: &T,
    api: &str,
    token: &str,
) -> Result<Option<Playback>, String> {
    let response = http.get(&format!("{api}/me/player"), token)?;
    if response.status == 204 {
        return Ok(None);
    }
    if !is_ok(&response) {
        return Err(api_error(response.status, &response.body));
    }
    Ok(parse_playback(&response.body))
}

/// Every client the account can play on. Used by the device switcher in the
/// player bar, so the owner can hand playback to their phone without leaving
/// Rustify.
pub fn devices<T: Transport>(http: &T, api: &str, token: &str) -> Result<Vec<Device>, String> {
    let response = http.get(&format!("{api}/me/player/devices"), token)?;
    if !is_ok(&response) {
        return Err(api_error(response.status, &response.body));
    }
    let v: Value = serde_json::from_str(&response.body)
        .map_err(|_| "Spotify returned an unreadable device list".to_string())?;
    let mut out = Vec::new();
    for d in v.get("devices").and_then(Value::as_array).into_iter().flatten() {
        let id = d.get("id").and_then(Value::as_str).unwrap_or_default();
        if id.is_empty() {
            continue;
        }
        out.push(Device {
            id: id.to_string(),
            name: d
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("Unknown device")
                .to_string(),
            active: d.get("is_active").and_then(Value::as_bool).unwrap_or(false),
            kind: d
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
        });
    }
    Ok(out)
}

pub fn command<T: Transport>(
    http: &T,
    api: &str,
    token: &str,
    command: Command,
) -> Result<(), String> {
    let (method, url, body): (&str, String, Option<String>) = match command {
        Command::Play => ("PUT", format!("{api}/me/player/play"), None),
        Command::Pause => ("PUT", format!("{api}/me/player/pause"), None),
        Command::Next => ("POST", format!("{api}/me/player/next"), None),
        Command::Previous => ("POST", format!("{api}/me/player/previous"), None),
        Command::Seek(ms) => (
            "PUT",
            format!("{api}/me/player/seek?position_ms={ms}"),
            None,
        ),
        Command::Volume(pct) => (
            "PUT",
            format!("{api}/me/player/volume?volume_percent={}", pct.min(100)),
            None,
        ),
        // Moving playback keeps whatever was playing paused on the old device
        // until the new one takes over; Spotify resumes it itself.
        Command::Transfer(id) => (
            "PUT",
            format!("{api}/me/player"),
            Some(json!({ "device_ids": [id], "play": true }).to_string()),
        ),
        Command::PlayUri { uri, device } => {
            let url = match device {
                Some(id) if !id.is_empty() => format!(
                    "{api}/me/player/play?device_id={}",
                    crate::catalog::encode_query(&id)
                ),
                _ => format!("{api}/me/player/play"),
            };
            (
                "PUT",
                url,
                Some(json!({ "uris": [uri] }).to_string()),
            )
        }
        Command::PlayContext { uri, device } => {
            let url = match device {
                Some(id) if !id.is_empty() => format!(
                    "{api}/me/player/play?device_id={}",
                    crate::catalog::encode_query(&id)
                ),
                _ => format!("{api}/me/player/play"),
            };
            (
                "PUT",
                url,
                Some(json!({ "context_uri": uri }).to_string()),
            )
        }
        Command::QueueUri(uri) => (
            "POST",
            format!("{api}/me/player/queue?uri={}", crate::catalog::encode_query(&uri)),
            None,
        ),
    };
    let response = http.send_json(method, &url, token, body.as_deref())?;
    if !is_ok(&response) {
        return Err(api_error(response.status, &response.body));
    }
    Ok(())
}

// -------------------------------------------------------------- token store

/// True when the session itself is gone (a new sign-in is needed), as opposed
/// to a command that failed for a recoverable reason.
pub fn fatal(message: &str) -> bool {
    message.contains("sign-in expired") || message.contains("Connect playback control again")
}

pub fn default_path() -> PathBuf {
    crate::profile::Profile::default_path().with_file_name("connect.bin")
}

fn to_json(session: &Session) -> String {
    json!({
        "client_id": session.client_id,
        "access_token": session.tokens.access,
        "refresh_token": session.tokens.refresh,
        "expires_at_ms": session.tokens.expires_at_ms,
    })
    .to_string()
}

fn from_json(text: &str, endpoints: Endpoints, expect_client: &str) -> Option<Session> {
    let v: Value = serde_json::from_str(text).ok()?;
    let client_id = v.get("client_id").and_then(Value::as_str)?.to_string();
    // A token minted for a different Spotify app cannot be refreshed by the
    // current Client ID, so treat it as gone rather than failing at runtime.
    if !expect_client.is_empty() && client_id != expect_client {
        return None;
    }
    let refresh = v.get("refresh_token").and_then(Value::as_str)?.to_string();
    if refresh.is_empty() {
        return None;
    }
    Some(Session {
        client_id,
        endpoints,
        tokens: Tokens {
            access: v.get("access_token").and_then(Value::as_str)?.to_string(),
            refresh,
            expires_at_ms: v.get("expires_at_ms").and_then(Value::as_i64).unwrap_or(0),
        },
    })
}

/// Saves the session encrypted with the owner's Windows profile (DPAPI). If the
/// OS cannot protect it, nothing is written: a refresh token is a credential
/// and does not belong in a plain text file.
pub fn save(path: &Path, session: &Session) -> bool {
    let plain = to_json(session);
    let Some(sealed) = protect(plain.as_bytes()) else {
        return false;
    };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    std::fs::write(path, sealed).is_ok()
}

pub fn load(path: &Path, client_id: &str) -> Option<Session> {
    let sealed = std::fs::read(path).ok()?;
    let plain = unprotect(&sealed)?;
    let text = String::from_utf8(plain).ok()?;
    from_json(&text, Endpoints::default(), client_id)
}

pub fn forget(path: &Path) {
    let _ = std::fs::remove_file(path);
}

#[cfg(windows)]
pub fn protect(plain: &[u8]) -> Option<Vec<u8>> {
    if plain.is_empty() || plain.len() > 64 * 1024 {
        return None;
    }
    let mut input = DataBlob {
        cb_data: plain.len() as u32,
        pb_data: plain.as_ptr() as *mut u8,
    };
    let mut output = DataBlob {
        cb_data: 0,
        pb_data: std::ptr::null_mut(),
    };
    let ok = unsafe {
        CryptProtectData(
            &mut input,
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            0,
            &mut output,
        )
    };
    if ok == 0 || output.pb_data.is_null() {
        return None;
    }
    let out = unsafe { std::slice::from_raw_parts(output.pb_data, output.cb_data as usize) }.to_vec();
    unsafe { LocalFree(output.pb_data as *mut core::ffi::c_void) };
    Some(out)
}

#[cfg(windows)]
pub fn unprotect(sealed: &[u8]) -> Option<Vec<u8>> {
    if sealed.is_empty() {
        return None;
    }
    let mut input = DataBlob {
        cb_data: sealed.len() as u32,
        pb_data: sealed.as_ptr() as *mut u8,
    };
    let mut output = DataBlob {
        cb_data: 0,
        pb_data: std::ptr::null_mut(),
    };
    let mut descr: *mut u16 = std::ptr::null_mut();
    let ok = unsafe {
        CryptUnprotectData(
            &mut input,
            &mut descr,
            std::ptr::null(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            0,
            &mut output,
        )
    };
    if !descr.is_null() {
        unsafe { LocalFree(descr as *mut core::ffi::c_void) };
    }
    if ok == 0 || output.pb_data.is_null() {
        return None;
    }
    let out = unsafe { std::slice::from_raw_parts(output.pb_data, output.cb_data as usize) }.to_vec();
    unsafe { LocalFree(output.pb_data as *mut core::ffi::c_void) };
    Some(out)
}

#[cfg(not(windows))]
pub fn protect(_plain: &[u8]) -> Option<Vec<u8>> {
    None
}

#[cfg(not(windows))]
pub fn unprotect(_sealed: &[u8]) -> Option<Vec<u8>> {
    None
}

#[cfg(windows)]
#[repr(C)]
struct DataBlob {
    cb_data: u32,
    pb_data: *mut u8,
}

#[cfg(windows)]
#[link(name = "crypt32")]
unsafe extern "system" {
    fn CryptProtectData(
        data_in: *const DataBlob,
        descr: *const u16,
        entropy: *const DataBlob,
        reserved: *mut core::ffi::c_void,
        prompt: *mut core::ffi::c_void,
        flags: u32,
        data_out: *mut DataBlob,
    ) -> i32;
    fn CryptUnprotectData(
        data_in: *const DataBlob,
        descr: *mut *mut u16,
        entropy: *const DataBlob,
        reserved: *mut core::ffi::c_void,
        prompt: *mut core::ffi::c_void,
        flags: u32,
        data_out: *mut DataBlob,
    ) -> i32;
}

#[cfg(windows)]
#[link(name = "kernel32")]
unsafe extern "system" {
    fn LocalFree(mem: *mut core::ffi::c_void) -> *mut core::ffi::c_void;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::time::Duration;

    struct Fake {
        status: u16,
        body: String,
        seen: RefCell<Vec<(String, String, Option<String>)>>,
    }

    impl Fake {
        fn reply(status: u16, body: &str) -> Self {
            Self {
                status,
                body: body.to_string(),
                seen: RefCell::new(Vec::new()),
            }
        }

        fn last(&self) -> (String, String, Option<String>) {
            self.seen.borrow().last().cloned().expect("a request")
        }
    }

    impl Transport for Fake {
        fn get(&self, url: &str, _token: &str) -> Result<HttpResponse, String> {
            self.seen
                .borrow_mut()
                .push(("GET".to_string(), url.to_string(), None));
            Ok(HttpResponse {
                status: self.status,
                body: self.body.clone(),
            })
        }

        fn post_form(&self, url: &str, form: &[(&str, &str)]) -> Result<HttpResponse, String> {
            self.seen.borrow_mut().push((
                "POST".to_string(),
                url.to_string(),
                Some(
                    form.iter()
                        .map(|(k, v)| format!("{k}={v}"))
                        .collect::<Vec<_>>()
                        .join("&"),
                ),
            ));
            Ok(HttpResponse {
                status: self.status,
                body: self.body.clone(),
            })
        }

        fn send_json(
            &self,
            method: &str,
            url: &str,
            _token: &str,
            body: Option<&str>,
        ) -> Result<HttpResponse, String> {
            self.seen.borrow_mut().push((
                method.to_string(),
                url.to_string(),
                body.map(str::to_string),
            ));
            Ok(HttpResponse {
                status: self.status,
                body: self.body.clone(),
            })
        }
    }

    const API: &str = "https://api.spotify.com/v1";

    #[test]
    fn a_cancelled_sign_in_releases_the_callback_port() {
        let flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
        let options = crate::spotify::Options {
            port: 0,
            open_browser: false,
            cancel: Some(flag),
            ..crate::spotify::Options::default()
        };
        let err = super::login(&Curl, "cid", &options, &mut |_m: String| {}).unwrap_err();
        assert!(err.contains("cancelled"), "{err}");
    }

    #[test]
    fn state_reads_the_track_the_device_and_the_position() {
        let body = r#"{"is_playing":true,"progress_ms":61000,"item":{"name":"Nightcall","artists":[{"name":"Kavinsky"}],"duration_ms":258000},"device":{"name":"PDX-PC","volume_percent":64}}"#;
        let fake = Fake::reply(200, body);
        let playing = state(&fake, API, "tok").unwrap().expect("a track");
        assert_eq!(playing.track, "Nightcall");
        assert_eq!(playing.artist, "Kavinsky");
        assert!(playing.playing);
        assert_eq!(playing.progress_ms, 61000);
        assert_eq!(playing.duration_ms, 258000);
        assert_eq!(playing.device, "PDX-PC");
        assert_eq!(playing.volume, Some(64));
        assert_eq!(fake.last().1, format!("{API}/me/player"));
    }

    #[test]
    fn nothing_active_is_normal_and_not_an_error() {
        let fake = Fake::reply(204, "");
        assert_eq!(state(&fake, API, "tok").unwrap(), None);
        // A 200 with no item (Spotify does this mid-switch) is also "nothing".
        let empty = Fake::reply(200, r#"{"is_playing":false,"item":null}"#);
        assert_eq!(state(&empty, API, "tok").unwrap(), None);
    }

    #[test]
    fn every_command_hits_its_documented_endpoint() {
        let cases = [
            (Command::Play, "PUT", "/me/player/play"),
            (Command::Pause, "PUT", "/me/player/pause"),
            (Command::Next, "POST", "/me/player/next"),
            (Command::Previous, "POST", "/me/player/previous"),
            (Command::Seek(1500), "PUT", "/me/player/seek?position_ms=1500"),
            (
                Command::Volume(40),
                "PUT",
                "/me/player/volume?volume_percent=40",
            ),
        ];
        for (command, method, path) in cases {
            let fake = Fake::reply(200, "");
            command_of(&fake, command.clone()).unwrap();
            let (got_method, url, body) = fake.last();
            assert_eq!(got_method, method, "{command:?}");
            assert_eq!(url, format!("{API}{path}"), "{command:?}");
            assert!(body.is_none(), "{command:?} should send no body");
        }
    }

    fn command_of(fake: &Fake, command: Command) -> Result<(), String> {
        super::command(fake, API, "tok", command)
    }

    #[test]
    fn playing_a_track_names_the_device_in_the_request() {
        let fake = Fake::reply(200, "");
        command_of(
            &fake,
            Command::PlayUri {
                uri: "spotify:track:abc".to_string(),
                device: Some("dev-1".to_string()),
            },
        )
        .unwrap();
        let (method, url, body) = fake.last();
        assert_eq!(method, "PUT");
        assert_eq!(url, format!("{API}/me/player/play?device_id=dev-1"));
        assert_eq!(body.unwrap(), r#"{"uris":["spotify:track:abc"]}"#);
    }

    #[test]
    fn player_errors_say_what_to_do_about_them() {
        let premium = Fake::reply(403, r#"{"error":{"status":403,"message":"Player command failed: Premium required"}}"#);
        let err = command_of(&premium, Command::Play).unwrap_err();
        assert!(err.contains("Premium"), "{err}");

        let no_device = Fake::reply(404, r#"{"error":{"status":404,"message":"Device not found"}}"#);
        let err = state(&no_device, API, "tok").unwrap_err();
        assert!(err.contains("Open Spotify"), "{err}");

        let expired = Fake::reply(401, "{}");
        assert!(api_error(401, "{}").contains("sign-in expired"));
        assert!(state(&expired, API, "tok").is_err());
    }

    #[test]
    fn expired_tokens_are_refreshed_and_the_session_survives() {
        let fake = Fake::reply(
            200,
            r#"{"access_token":"new-access","expires_in":3600}"#,
        );
        let mut session = Session {
            client_id: "cid".to_string(),
            endpoints: Endpoints::default(),
            tokens: Tokens {
                access: "old".to_string(),
                refresh: "keep-me".to_string(),
                expires_at_ms: 0,
            },
        };
        assert!(ensure_fresh(&fake, &mut session).unwrap());
        assert_eq!(session.tokens.access, "new-access");
        // Spotify often omits refresh_token on refresh; the old one must stay.
        assert_eq!(session.tokens.refresh, "keep-me");
        assert!(!session.tokens.expired());

        let form = fake.last().2.unwrap();
        assert!(form.contains("grant_type=refresh_token"));
        assert!(form.contains("refresh_token=keep-me"));

        // A fresh token means no second call.
        let before = fake.seen.borrow().len();
        assert!(!ensure_fresh(&fake, &mut session).unwrap());
        assert_eq!(fake.seen.borrow().len(), before);
    }

    #[test]
    fn a_dead_refresh_token_is_reported_as_expired_sign_in() {
        let fake = Fake::reply(400, r#"{"error":"invalid_grant"}"#);
        let mut session = Session {
            client_id: "cid".to_string(),
            endpoints: Endpoints::default(),
            tokens: Tokens {
                access: "old".to_string(),
                refresh: "dead".to_string(),
                expires_at_ms: 0,
            },
        };
        let err = ensure_fresh(&fake, &mut session).unwrap_err();
        assert!(err.contains("HTTP 400"), "{err}");
    }

    #[cfg(windows)]
    #[test]
    fn the_session_file_is_encrypted_and_round_trips() {
        let path = std::env::temp_dir().join(format!("rustify-connect-{}.bin", std::process::id()));
        let session = Session {
            client_id: "cid".to_string(),
            endpoints: Endpoints::default(),
            tokens: Tokens {
                access: "access-secret".to_string(),
                refresh: "refresh-secret".to_string(),
                expires_at_ms: 1234,
            },
        };
        assert!(save(&path, &session), "DPAPI should be available");
        let raw = std::fs::read(&path).unwrap();
        // The credential must not be readable off disk as plain text.
        let raw_text = String::from_utf8_lossy(&raw);
        assert!(!raw_text.contains("refresh-secret"));

        let loaded = load(&path, "cid").expect("session reloads");
        assert_eq!(loaded.tokens.refresh, "refresh-secret");
        assert_eq!(loaded.tokens.expires_at_ms, 1234);

        // A different Spotify app cannot use this session.
        assert!(load(&path, "other-app").is_none());
        forget(&path);
        assert!(load(&path, "cid").is_none());
    }

    /// Stands in for the browser: hits the loopback listener the login bound.
    fn simulate_browser(redirect: &str, query: &str) {
        use std::io::Write;
        let host = redirect.trim_start_matches("http://").split('/').next().unwrap();
        let mut stream = std::net::TcpStream::connect(host).expect("connect callback listener");
        stream
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        let _ = stream.write_all(
            format!("GET /callback?{query} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n")
                .as_bytes(),
        );
        let mut response = String::new();
        use std::io::Read;
        let _ = stream.read_to_string(&mut response);
        assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    }

    fn query_value(url: &str, key: &str) -> String {
        let query = url.split_once('?').unwrap().1;
        for pair in query.split('&') {
            let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
            if k == key {
                return v.to_string();
            }
        }
        panic!("no {key} in {url}");
    }

    #[test]
    fn login_asks_for_playback_scopes_and_keeps_the_refresh_token() {
        use crate::fake_spotify::{FakePlan, FakeSpotify};

        let mut plan = FakePlan::ok();
        plan.token_body = r#"{"access_token":"live-access","refresh_token":"live-refresh","expires_in":3600}"#
            .to_string();
        let fake = FakeSpotify::start(plan);
        let options = Options {
            endpoints: Endpoints {
                authorize: format!("{}/authorize", fake.base),
                token: format!("{}/api/token", fake.base),
                api: format!("{}/v1", fake.base),
            },
            port: 0,
            open_browser: false,
            timeout: Duration::from_secs(5),
            cancel: None,
        };

        // Run the login on its own thread and stream its log lines out, so the
        // browser can be simulated the moment the callback port is announced.
        let (line_tx, line_rx) = std::sync::mpsc::channel::<String>();
        let (done_tx, done_rx) = std::sync::mpsc::channel::<Result<Tokens, String>>();
        std::thread::spawn(move || {
            let http = Curl;
            let mut log = move |m: String| {
                let _ = line_tx.send(m);
            };
            let result = login(&http, "cid", &options, &mut log);
            let _ = done_tx.send(result);
        });

        let start = std::time::Instant::now();
        let mut redirect = String::new();
        let mut url = String::new();
        let mut lines: Vec<String> = Vec::new();
        while redirect.is_empty() || url.is_empty() {
            assert!(
                start.elapsed() < Duration::from_secs(10),
                "login never announced its redirect; log: {lines:?}"
            );
            let Ok(line) = line_rx.recv_timeout(Duration::from_secs(3)) else {
                continue;
            };
            if let Some(r) = line.strip_prefix("Redirect URI must be exactly ") {
                redirect = r.to_string();
            }
            if let Some(u) = line.strip_prefix("If no browser opens, paste this URL: ") {
                url = u.to_string();
            }
            lines.push(line);
        }

        // Playback control must ask for its own scopes and nothing more.
        assert!(url.starts_with(&format!("{}/authorize?", fake.base)));
        assert_eq!(
            query_value(&url, "scope"),
            "user-read-playback-state%20user-modify-playback-state"
        );
        assert!(url.contains("code_challenge_method=S256"));

        let state = query_value(&url, "state");
        simulate_browser(&redirect, &format!("code=test-code&state={state}"));

        let tokens = done_rx
            .recv_timeout(Duration::from_secs(20))
            .expect("login finished")
            .unwrap_or_else(|e| panic!("login failed: {e}; log: {lines:?}"));
        assert_eq!(tokens.access, "live-access");
        assert_eq!(tokens.refresh, "live-refresh");
        assert!(!tokens.expired());

        let posts = fake.seen.lock().unwrap().clone();
        assert!(posts.iter().any(|p| p == "POST /api/token"), "{posts:?}");
    }
}
