// Spotify playlist import. Two routes:
//  * OAuth 2.0 Authorization Code + PKCE against the Spotify Web API, using the
//    user's own Client ID (no client secret). HTTPS goes through the curl.exe
//    that ships with Windows, so no TLS stack has to be compiled in.
//  * CSV import (for example an Exportify export), which needs no API app.
// The result is stored locally and merged into the app's library.
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use eframe::egui::Color32;
use serde_json::Value;

use crate::library::{Art, Collection, Kind, Track};
use crate::theme::rgb;

pub const DEFAULT_PORT: u16 = 8899;
pub const SCOPES: &str = "playlist-read-private playlist-read-collaborative";

#[derive(Clone, Debug)]
pub struct Endpoints {
    pub authorize: String,
    pub token: String,
    pub api: String,
}

impl Default for Endpoints {
    fn default() -> Self {
        Self {
            authorize: "https://accounts.spotify.com/authorize".to_string(),
            token: "https://accounts.spotify.com/api/token".to_string(),
            api: "https://api.spotify.com/v1".to_string(),
        }
    }
}

/// Everything the import flow needs besides the Client ID. Tests point this at
/// a local stand-in and turn the real browser launch off.
#[derive(Clone, Debug)]
pub struct Options {
    pub endpoints: Endpoints,
    pub port: u16,
    pub open_browser: bool,
    pub timeout: Duration,
    /// Set by the UI to abandon a sign-in that is waiting for the browser.
    /// Without it, one failed attempt holds the callback port until timeout.
    pub cancel: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            endpoints: Endpoints::default(),
            port: DEFAULT_PORT,
            open_browser: true,
            timeout: Duration::from_secs(300),
            cancel: None,
        }
    }
}

pub enum Msg {
    Log(String),
    Done(Result<Vec<ImportedPlaylist>, String>),
    One(Result<ImportedPlaylist, String>),
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ImportedTrack {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub secs: u32,
    /// YYYY-MM-DD. Stamped with the import date when the source has none, so
    /// the Date Added column never has to invent a value.
    pub added: String,
    /// Album cover from the CDN, when the import could see one.
    pub art: Art,
    /// Spotify track URI, used to remove this exact row from its playlist.
    /// Empty for CSV imports and stores written before this column existed.
    pub uri: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ImportedPlaylist {
    pub name: String,
    pub tracks: Vec<ImportedTrack>,
    /// Playlist cover, when the import could see one.
    pub art: Art,
    /// Spotify playlist id. Empty means this is a local list (CSV or hand
    /// built), which can only be edited in Rustify.
    pub id: String,
}

impl ImportedPlaylist {
    pub fn total_secs(&self) -> u32 {
        self.tracks.iter().map(|t| t.secs).sum()
    }
}

pub fn default_path() -> PathBuf {
    crate::profile::Profile::default_path().with_file_name("imported-playlists.txt")
}

fn clean(s: &str) -> String {
    s.replace(['\t', '\r', '\n'], " ").trim().to_string()
}

/// True for a CSV cell that already looks like a date, so an export that
/// carries "Added At" fills the column without a column-name guess.
fn is_date(cell: &str) -> bool {
    let c = cell.trim();
    c.len() >= 10
        && c.as_bytes()[..4].iter().all(u8::is_ascii_digit)
        && (c.as_bytes()[4] == b'-' || c.as_bytes()[4] == b'/')
}

/// Today as YYYY-MM-DD. The civil-date conversion is Howard Hinnant's
/// `civil_from_days`, which keeps a whole date crate out of the build.
pub fn today_iso() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    iso_from_days(secs.div_euclid(86_400))
}

pub fn iso_from_days(days: i64) -> String {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}")
}

pub fn save(path: &Path, list: &[ImportedPlaylist]) {
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let mut out = String::from("v1\n");
    for p in list {
        out.push_str(&format!(
            "playlist\t{}\t{}\t{}\n",
            clean(&p.name),
            clean(p.art.largest().unwrap_or("")),
            clean(&p.id)
        ));
        for t in &p.tracks {
            out.push_str(&format!(
                "track\t{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
                clean(&t.title),
                clean(&t.artist),
                clean(&t.album),
                t.secs,
                clean(&t.added),
                clean(t.art.largest().unwrap_or("")),
                clean(&t.uri)
            ));
        }
    }
    let _ = std::fs::write(path, out);
}

pub fn load(path: &Path) -> Vec<ImportedPlaylist> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let mut out: Vec<ImportedPlaylist> = Vec::new();
    for line in text.lines() {
        let mut parts = line.split('\t');
        match parts.next().unwrap_or("") {
            "playlist" => {
                let name = parts.next().unwrap_or("Untitled").to_string();
                let art = Art::of(parts.next().unwrap_or(""));
                // Older stores stop after the cover; they stay local-only
                // until the playlist is imported again from Spotify.
                let id = parts.next().unwrap_or("").to_string();
                out.push(ImportedPlaylist {
                    name,
                    tracks: Vec::new(),
                    art,
                    id,
                });
            }
            "track" => {
                if let Some(p) = out.last_mut() {
                    let title = parts.next().unwrap_or("").to_string();
                    if title.is_empty() {
                        continue;
                    }
                    p.tracks.push(ImportedTrack {
                        title,
                        artist: parts.next().unwrap_or("").to_string(),
                        album: parts.next().unwrap_or("").to_string(),
                        secs: parts.next().and_then(|s| s.parse().ok()).unwrap_or(0),
                        // Older stores predate the column: an empty date is
                        // shown as unknown rather than guessed.
                        added: parts.next().unwrap_or("").to_string(),
                        art: Art::of(parts.next().unwrap_or("")),
                        uri: parts.next().unwrap_or("").to_string(),
                    });
                }
            }
            _ => {}
        }
    }
    out
}

fn palette(i: usize) -> (Color32, Color32) {
    const P: [(u32, u32); 6] = [
        (0x1e5b8f, 0x7fd8e8),
        (0x5a1f6e, 0xff7ad9),
        (0x2f4f3f, 0xa8d8b9),
        (0x4a3c2a, 0xd9b98a),
        (0x3a2352, 0x8f7bd6),
        (0x1f3f4a, 0x9fd8e0),
    ];
    let (a, b) = P[i % P.len()];
    (rgb(a), rgb(b))
}

pub fn to_collections(list: &[ImportedPlaylist]) -> Vec<Collection> {
    list.iter()
        .enumerate()
        .map(|(i, p)| {
            let (c1, c2) = palette(i);
            let tracks: Vec<Track> = p
                .tracks
                .iter()
                .map(|t| Track {
                    title: t.title.clone(),
                    artist: t.artist.clone(),
                    album: t.album.clone(),
                    secs: t.secs,
                    c1,
                    c2,
                    added: t.added.clone(),
                    art: t.art.clone(),
                    path: None,
                })
                .collect();
            let mut coll = Collection::new(
                &p.name,
                Kind::Playlist,
                // Short enough to fit a card without being cut off mid-word.
                format!("Imported \u{b7} {} songs", p.tracks.len()),
                c1,
                c2,
                Vec::new(),
                tracks,
            );
            // A playlist without its own cover borrows the first track's, the
            // same way the client fills an empty playlist tile.
            coll.art = if p.art.is_empty() {
                p.tracks
                    .iter()
                    .find(|t| !t.art.is_empty())
                    .map(|t| t.art.clone())
                    .unwrap_or_default()
            } else {
                p.art.clone()
            };
            coll
        })
        .collect()
}

// ------------------------------------------------------------------- crypto

pub fn sha256(data: &[u8]) -> [u8; 32] {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
        0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
        0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
        0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
        0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
        0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
        0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
    ];

    let mut h: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19,
    ];

    let bit_len = (data.len() as u64) * 8;
    let mut msg = data.to_vec();
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bit_len.to_be_bytes());

    for chunk in msg.chunks(64) {
        let mut w = [0u32; 64];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([
                chunk[i * 4],
                chunk[i * 4 + 1],
                chunk[i * 4 + 2],
                chunk[i * 4 + 3],
            ]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }

        let (mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh) =
            (h[0], h[1], h[2], h[3], h[4], h[5], h[6], h[7]);

        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let t1 = hh
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(maj);
            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b;
            b = a;
            a = t1.wrapping_add(t2);
        }

        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
        h[5] = h[5].wrapping_add(f);
        h[6] = h[6].wrapping_add(g);
        h[7] = h[7].wrapping_add(hh);
    }

    let mut out = [0u8; 32];
    for (i, v) in h.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&v.to_be_bytes());
    }
    out
}

pub fn base64url(data: &[u8]) -> String {
    const T: &[u8; 64] =
        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::new();
    for chunk in data.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
        out.push(T[((n >> 18) & 63) as usize] as char);
        out.push(T[((n >> 12) & 63) as usize] as char);
        if chunk.len() > 1 {
            out.push(T[((n >> 6) & 63) as usize] as char);
        }
        if chunk.len() > 2 {
            out.push(T[(n & 63) as usize] as char);
        }
    }
    out
}

pub fn challenge_for(verifier: &str) -> String {
    base64url(&sha256(verifier.as_bytes()))
}

fn random_bytes(buf: &mut [u8]) -> bool {
    #[cfg(windows)]
    {
        #[link(name = "bcryptprimitives")]
        unsafe extern "system" {
            fn ProcessPrng(data: *mut u8, len: usize) -> i32;
        }
        return unsafe { ProcessPrng(buf.as_mut_ptr(), buf.len()) != 0 };
    }
    #[cfg(not(windows))]
    {
        let _ = buf;
        false
    }
}

/// A PKCE verifier: 32 random bytes, base64url encoded (43 chars).
pub fn new_verifier() -> String {
    let mut bytes = [0u8; 32];
    if !random_bytes(&mut bytes) {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let seed = format!("{nanos}-{}-{}", std::process::id(), new_verifier_seed());
        bytes = sha256(seed.as_bytes());
    }
    base64url(&bytes)
}

fn new_verifier_seed() -> usize {
    let local = 0u8;
    &local as *const u8 as usize
}

// --------------------------------------------------------------------- oauth

pub fn redirect_uri(port: u16) -> String {
    format!("http://127.0.0.1:{port}/callback")
}

fn enc(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

fn urldecode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("");
                if let Ok(v) = u8::from_str_radix(hex, 16) {
                    out.push(v);
                    i += 3;
                } else {
                    out.push(bytes[i]);
                    i += 1;
                }
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).to_string()
}

#[cfg(test)]
pub fn auth_url(client_id: &str, redirect: &str, challenge: &str, state: &str) -> String {
    auth_url_at(
        &Endpoints::default(),
        client_id,
        redirect,
        challenge,
        state,
    )
}

pub fn auth_url_at(
    endpoints: &Endpoints,
    client_id: &str,
    redirect: &str,
    challenge: &str,
    state: &str,
) -> String {
    auth_url_scoped(endpoints, client_id, redirect, challenge, state, SCOPES)
}

/// Same authorize URL with an explicit scope list, so playback control can ask
/// for its own scopes without widening what the importer requests.
pub fn auth_url_scoped(
    endpoints: &Endpoints,
    client_id: &str,
    redirect: &str,
    challenge: &str,
    state: &str,
    scope: &str,
) -> String {
    format!(
        "{}?response_type=code&client_id={}&redirect_uri={}&scope={}&state={}&code_challenge_method=S256&code_challenge={}",
        endpoints.authorize,
        enc(client_id),
        enc(redirect),
        enc(scope),
        enc(state),
        enc(challenge)
    )
}

/// The browser receives the URL as one argument. `cmd /c start` would split
/// an OAuth URL at every `&`, which is exactly how the sign-in used to die
/// before Spotify ever saw it.
#[cfg(windows)]
fn browser_args(url: &str) -> [&str; 2] {
    ["url.dll,FileProtocolHandler", url]
}

pub(crate) fn open_browser(url: &str) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let mut cmd = std::process::Command::new("rundll32.exe");
        cmd.args(browser_args(url));
        cmd.creation_flags(0x0800_0000);
        let _ = cmd.spawn();
    }
    #[cfg(not(windows))]
    {
        let _ = url;
    }
}

pub(crate) const CANCELLED: &str = "Spotify sign-in cancelled";

pub(crate) fn is_cancelled(cancel: Option<&std::sync::atomic::AtomicBool>) -> bool {
    cancel.is_some_and(|flag| flag.load(std::sync::atomic::Ordering::SeqCst))
}

fn reply(stream: &mut TcpStream, status: &str, body: &str) {
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(response.as_bytes());
    let _ = stream.flush();
}

pub(crate) fn wait_for_code(
    listener: &TcpListener,
    state: &str,
    timeout: Duration,
    cancel: Option<&std::sync::atomic::AtomicBool>,
) -> Result<String, String> {
    listener
        .set_nonblocking(true)
        .map_err(|e| format!("cannot watch the redirect port: {e}"))?;
    let start = Instant::now();
    loop {
        if is_cancelled(cancel) {
            return Err(CANCELLED.to_string());
        }
        if start.elapsed() > timeout {
            return Err("Timed out waiting for Spotify sign-in".to_string());
        }
        match listener.accept() {
            Ok((mut stream, _)) => {
                let mut buf = [0u8; 8192];
                let n = stream.read(&mut buf).unwrap_or(0);
                let request = String::from_utf8_lossy(&buf[..n]).to_string();
                let path = request
                    .lines()
                    .next()
                    .and_then(|l| l.split_whitespace().nth(1))
                    .unwrap_or("");
                let route = path.split('?').next().unwrap_or("");
                if route != "/callback" {
                    reply(
                        &mut stream,
                        "404 Not Found",
                        "<html><body><h2>Not found</h2></body></html>",
                    );
                    continue;
                }
                let query = path.split_once('?').map(|(_, q)| q).unwrap_or("");

                let mut code = String::new();
                let mut got_state = String::new();
                let mut error = String::new();
                for pair in query.split('&') {
                    let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
                    let v = urldecode(v);
                    match k {
                        "code" => code = v,
                        "state" => got_state = v,
                        "error" => error = v,
                        _ => {}
                    }
                }

                if !error.is_empty() {
                    if got_state == state {
                        reply(
                            &mut stream,
                            "200 OK",
                            "<html><body style=\"font-family:Segoe UI,sans-serif;padding:40px\"><h2>Sign-in failed</h2><p>Go back to Rustify and try again.</p></body></html>",
                        );
                        return Err(format!("Spotify returned an error: {error}"));
                    }
                    reply(
                        &mut stream,
                        "400 Bad Request",
                        "<html><body><h2>This sign-in is no longer current.</h2><p>Close this tab and try again from Rustify.</p></body></html>",
                    );
                    continue;
                }
                if code.is_empty() || got_state != state {
                    // A browser preconnect, a favicon request, or a callback
                    // from an attempt that was already cancelled and retried.
                    // Answer it without ending the sign-in waiting now.
                    reply(
                        &mut stream,
                        "400 Bad Request",
                        "<html><body><h2>This sign-in is no longer current.</h2><p>Close this tab and try again from Rustify.</p></body></html>",
                    );
                    continue;
                }
                if is_cancelled(cancel) {
                    reply(
                        &mut stream,
                        "400 Bad Request",
                        "<html><body><h2>Sign-in cancelled</h2></body></html>",
                    );
                    return Err(CANCELLED.to_string());
                }
                reply(
                    &mut stream,
                    "200 OK",
                    "<html><body style=\"font-family:Segoe UI,sans-serif;background:#121212;color:#fff;padding:40px\"><h2 style=\"color:#1db954\">Rustify is connected</h2><p>You can close this tab and go back to the app.</p></body></html>",
                );
                return Ok(code);
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(120));
            }
            Err(e) => return Err(format!("redirect listener failed: {e}")),
        }
    }
}

pub struct HttpResponse {
    pub status: u16,
    pub body: String,
}

impl HttpResponse {
    pub fn ok(&self) -> bool {
        (200..300).contains(&self.status)
    }
}

pub trait Http {
    fn get(&self, url: &str, token: &str) -> Result<HttpResponse, String>;
    fn post_form(&self, url: &str, form: &[(&str, &str)]) -> Result<HttpResponse, String>;
}

pub struct Curl;

pub(crate) fn curl_escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

pub(crate) fn response_error(response: &HttpResponse) -> String {
    let reason = serde_json::from_str::<Value>(&response.body)
        .ok()
        .and_then(|v| {
            v.pointer("/error/message")
                .and_then(|m| m.as_str())
                .or_else(|| v.get("error_description").and_then(|m| m.as_str()))
                .or_else(|| v.get("error").and_then(|m| m.as_str()))
                .map(str::to_string)
        })
        .unwrap_or_else(|| response.body.trim().chars().take(200).collect());
    if reason.is_empty() {
        format!("Spotify returned HTTP {}", response.status)
    } else {
        format!("Spotify returned HTTP {}: {reason}", response.status)
    }
}

#[cfg(windows)]
pub(crate) fn run_curl(config: &str) -> Result<HttpResponse, String> {
    use std::os::windows::process::CommandExt;
    let mut child = std::process::Command::new("curl")
        .args([
            "--silent",
            "--show-error",
            "--config",
            "-",
            "--write-out",
            "\n%{http_code}",
        ])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .creation_flags(0x0800_0000)
        .spawn()
        .map_err(|e| format!("could not start curl.exe: {e}"))?;
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(config.as_bytes());
    }
    let out = child
        .wait_with_output()
        .map_err(|e| format!("curl failed: {e}"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        return Err(if err.is_empty() {
            "Spotify request failed".to_string()
        } else {
            err
        });
    }
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    let (body, status) = match text.rsplit_once('\n') {
        Some((body, code)) => (body.to_string(), code.trim().parse().unwrap_or(0)),
        None => (text, 0),
    };
    Ok(HttpResponse { status, body })
}

#[cfg(not(windows))]
pub(crate) fn run_curl(_config: &str) -> Result<HttpResponse, String> {
    Err("HTTP is only wired up on Windows (uses curl.exe)".to_string())
}

/// Raw bytes over HTTPS, for album art and preview clips. curl writes the body
/// to a temp file so binary data never passes through a String, while the
/// status code still comes back on stdout.
pub fn fetch_bytes(url: &str) -> Result<Vec<u8>, String> {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let path = std::env::temp_dir().join(format!("rustify-art-{}-{stamp}", std::process::id()));
    let config = format!(
        "url = \"{}\"\noutput = \"{}\"\nheader = \"Accept: */*\"\nheader = \"User-Agent: Mozilla/5.0 (Windows NT 10.0; Win64; x64) Rustify\"\n",
        curl_escape(url),
        curl_escape(&path.display().to_string())
    );
    let result = (|| {
        let response = run_curl(&config)?;
        if response.status < 200 || response.status >= 300 {
            return Err(format!("download returned HTTP {}", response.status));
        }
        let bytes = std::fs::read(&path)
            .map_err(|e| format!("the downloaded file could not be read: {e}"))?;
        if bytes.is_empty() {
            return Err("the downloaded file was empty".to_string());
        }
        Ok(bytes)
    })();
    let _ = std::fs::remove_file(&path);
    result
}

/// Fetch a public page (no bearer token), used for Spotify's share/embed pages.
fn get_public(url: &str) -> Result<HttpResponse, String> {
    let config = format!(
        "url = \"{}\"\nheader = \"Accept: text/html,application/xhtml+xml\"\nheader = \"Accept-Language: en-US,en;q=0.9\"\nheader = \"User-Agent: Mozilla/5.0 (Windows NT 10.0; Win64; x64) Rustify\"\n",
        curl_escape(url)
    );
    run_curl(&config)
}

impl Http for Curl {
    fn get(&self, url: &str, token: &str) -> Result<HttpResponse, String> {
        let config = format!(
            "url = \"{}\"\nheader = \"Authorization: Bearer {}\"\nheader = \"Accept: application/json\"\n",
            curl_escape(url),
            curl_escape(token)
        );
        run_curl(&config)
    }

    fn post_form(&self, url: &str, form: &[(&str, &str)]) -> Result<HttpResponse, String> {
        let mut config = format!("url = \"{}\"\n", curl_escape(url));
        for (k, v) in form {
            config.push_str(&format!(
                "data-urlencode = \"{}={}\"\n",
                curl_escape(k),
                curl_escape(v)
            ));
        }
        run_curl(&config)
    }
}

#[cfg(test)]
mod http_tests {
    use super::*;

    #[test]
    fn response_error_reads_api_and_oauth_payloads() {
        let api = HttpResponse {
            status: 401,
            body: r#"{"error":{"status":401,"message":"The access token expired"}}"#.to_string(),
        };
        assert!(response_error(&api).contains("The access token expired"));

        let oauth = HttpResponse {
            status: 400,
            body: r#"{"error":"invalid_grant","error_description":"Invalid authorization code"}"#
                .to_string(),
        };
        assert!(response_error(&oauth).contains("Invalid authorization code"));

        let html = HttpResponse {
            status: 502,
            body: "<html>bad gateway</html>".to_string(),
        };
        assert!(response_error(&html).contains("HTTP 502"));
    }
}

// ---------------------------------------------------------------------- json

pub struct PlaylistRef {
    pub id: String,
    pub name: String,
    pub total: u64,
    /// The playlist's own cover, when the account endpoint reported one.
    pub art: Art,
}

/// Spotify cover art comes either as `images` (Web API objects) or as
/// `coverArt.sources` (the embed page). Both shapes are read here; an empty
/// result is what paints the generated gradient placeholder.
fn art_of(value: &Value) -> Art {
    let mut images = Vec::new();
    if let Some(list) = value.get("images").and_then(Value::as_array) {
        images = images_of(list);
    }
    if images.is_empty() {
        if let Some(list) = value
            .pointer("/coverArt/sources")
            .and_then(Value::as_array)
        {
            images = images_of(list);
        }
    }
    Art::new(images)
}

fn images_of(list: &[Value]) -> Vec<(u32, String)> {
    let mut out = Vec::new();
    for img in list {
        let url = img.get("url").and_then(Value::as_str).unwrap_or_default();
        if url.is_empty() {
            continue;
        }
        let width = img.get("width").and_then(Value::as_u64).unwrap_or(0) as u32;
        out.push((width, url.to_string()));
    }
    out
}

pub fn parse_playlists(json: &str) -> Result<(Vec<PlaylistRef>, Option<String>), String> {
    let v: Value = serde_json::from_str(json).map_err(|e| format!("bad playlist JSON: {e}"))?;
    let mut out = Vec::new();
    if let Some(items) = v.get("items").and_then(|i| i.as_array()) {
        for p in items {
            let Some(id) = p.get("id").and_then(|x| x.as_str()) else {
                continue;
            };
            let name = p
                .get("name")
                .and_then(|x| x.as_str())
                .unwrap_or("Untitled playlist");
            // 2026 shape is items.total; older responses used tracks.total.
            let total = p
                .pointer("/items/total")
                .or_else(|| p.pointer("/tracks/total"))
                .and_then(|t| t.as_u64())
                .unwrap_or(0);
            out.push(PlaylistRef {
                id: id.to_string(),
                name: name.to_string(),
                total,
                art: art_of(p),
            });
        }
    }
    let next = v.get("next").and_then(|n| n.as_str()).map(str::to_string);
    Ok((out, next))
}

pub fn parse_items(json: &str) -> Result<(Vec<ImportedTrack>, Option<String>), String> {
    let v: Value = serde_json::from_str(json).map_err(|e| format!("bad playlist items JSON: {e}"))?;
    let mut out = Vec::new();
    if let Some(items) = v.get("items").and_then(|i| i.as_array()) {
        for entry in items {
            // 2026 shape is item; older responses used track.
            let item = entry
                .get("item")
                .or_else(|| entry.get("track"))
                .unwrap_or(entry);
            if item.is_null() {
                continue;
            }
            if item.get("type").and_then(|t| t.as_str()) == Some("episode") {
                continue;
            }
            let title = item.get("name").and_then(|x| x.as_str()).unwrap_or("");
            if title.is_empty() {
                continue;
            }
            let artist = item
                .get("artists")
                .and_then(|a| a.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|x| x.get("name").and_then(|n| n.as_str()))
                        .collect::<Vec<_>>()
                        .join(", ")
                })
                .unwrap_or_default();
            let album = item
                .get("album")
                .and_then(|a| a.get("name"))
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .to_string();
            let ms = item
                .get("duration_ms")
                .and_then(|d| d.as_u64())
                .unwrap_or(0);
            let uri = item
                .get("uri")
                .and_then(Value::as_str)
                .map(str::to_string)
                .or_else(|| {
                    item.get("id")
                        .and_then(Value::as_str)
                        .filter(|id| !id.is_empty())
                        .map(|id| format!("spotify:track:{id}"))
                })
                .unwrap_or_default();
            out.push(ImportedTrack {
                title: title.to_string(),
                artist,
                album,
                secs: (ms / 1000) as u32,
                added: String::new(),
                // A track wears its album's cover, exactly like the client.
                art: item.get("album").map(art_of).unwrap_or_default(),
                uri,
            });
        }
    }
    let next = v.get("next").and_then(|n| n.as_str()).map(str::to_string);
    Ok((out, next))
}

// ------------------------------------------------- public playlist by link

/// Accepts a share URL, a spotify: URI, or a bare 22-character playlist id.
pub fn parse_playlist_id(input: &str) -> Option<String> {
    let text = input.trim();
    let tail = if let Some(rest) = text.split("spotify:playlist:").nth(1) {
        rest
    } else if let Some(pos) = text.find("/playlist/") {
        &text[pos + "/playlist/".len()..]
    } else {
        text
    };
    let id: String = tail.chars().take_while(|c| c.is_ascii_alphanumeric()).collect();
    if id.len() == 22 {
        Some(id)
    } else {
        None
    }
}

fn replace_nbsp(s: &str) -> String {
    s.replace('\u{a0}', " ").trim().to_string()
}

/// Read the playlist out of Spotify's public embed page (`__NEXT_DATA__`).
pub fn parse_embed(html: &str) -> Result<ImportedPlaylist, String> {
    let marker = "id=\"__NEXT_DATA__\"";
    let start = html
        .find(marker)
        .ok_or_else(|| "Spotify did not send any playlist data".to_string())?;
    let open = start + html[start..].find('>').ok_or("bad Spotify page")? + 1;
    let close = open + html[open..].find("</script>").ok_or("bad Spotify page")?;
    let value: Value =
        serde_json::from_str(&html[open..close]).map_err(|e| format!("bad Spotify page data: {e}"))?;
    let page = value
        .get("props")
        .and_then(|p| p.get("pageProps"))
        .ok_or_else(|| "Spotify did not send any playlist data".to_string())?;
    if page.get("status").is_some() {
        return Err(
            "Spotify will not show this playlist without signing in (it is private)".to_string(),
        );
    }
    let entity = page
        .pointer("/state/data/entity")
        .ok_or_else(|| "Spotify did not send any playlist data".to_string())?;
    let name = entity
        .get("name")
        .and_then(|n| n.as_str())
        .map(replace_nbsp)
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| "Spotify playlist".to_string());
    let mut tracks = Vec::new();
    if let Some(list) = entity.get("trackList").and_then(|t| t.as_array()) {
        for item in list {
            let title = item.get("title").and_then(|t| t.as_str()).unwrap_or("");
            if title.trim().is_empty() {
                continue;
            }
            let artist = item
                .get("subtitle")
                .and_then(|s| s.as_str())
                .map(replace_nbsp)
                .unwrap_or_default();
            let secs = item
                .get("duration")
                .and_then(|d| d.as_u64())
                .map(|ms| (ms / 1000).max(1) as u32)
                .unwrap_or(0);
            let uri = item
                .get("uri")
                .and_then(Value::as_str)
                .map(str::to_string)
                .or_else(|| {
                    item.get("id")
                        .and_then(Value::as_str)
                        .filter(|id| !id.is_empty())
                        .map(|id| format!("spotify:track:{id}"))
                })
                .unwrap_or_default();
            tracks.push(ImportedTrack {
                title: replace_nbsp(title),
                artist,
                album: "Spotify".to_string(),
                secs,
                added: String::new(),
                art: art_of(item),
                uri,
            });
        }
    }
    let id = entity
        .get("uri")
        .and_then(Value::as_str)
        .and_then(|uri| uri.strip_prefix("spotify:playlist:"))
        .or_else(|| entity.get("id").and_then(Value::as_str))
        .unwrap_or("")
        .to_string();
    Ok(ImportedPlaylist {
        name,
        tracks,
        art: art_of(entity),
        id,
    })
}

/// Copy any public playlist into the library with just its share link: no
/// developer app, no login. Private playlists still need the OAuth route.
pub fn import_public_playlist(
    link: &str,
    log: &mut dyn FnMut(String),
) -> Result<ImportedPlaylist, String> {
    let id = parse_playlist_id(link).ok_or_else(|| {
        "Paste a Spotify playlist link, for example https://open.spotify.com/playlist/...".to_string()
    })?;
    log(format!("Reading public playlist {id} from Spotify..."));
    let url = format!("https://open.spotify.com/embed/playlist/{id}");
    let response = get_public(&url)?;
    if response.status == 404 {
        return Err(
            "That playlist is private (or gone), so Spotify only shows it to a signed-in account"
                .to_string(),
        );
    }
    if !response.ok() {
        return Err(response_error(&response));
    }
    let mut playlist = parse_embed(&response.body)?;
    // The link already names the playlist even when the embed page omitted
    // its URI; keep the id so playback and removal can target it later.
    if playlist.id.is_empty() {
        playlist.id = id;
    }
    log(format!(
        "Found {} ({} tracks).",
        playlist.name,
        playlist.tracks.len()
    ));
    Ok(playlist)
}

#[cfg(test)]
pub fn fetch_all<H: Http>(
    http: &H,
    token: &str,
    log: &mut dyn FnMut(String),
) -> Result<Vec<ImportedPlaylist>, String> {
    fetch_all_at(http, &Endpoints::default().api, token, log)
}

pub fn fetch_all_at<H: Http>(
    http: &H,
    api: &str,
    token: &str,
    log: &mut dyn FnMut(String),
) -> Result<Vec<ImportedPlaylist>, String> {
    let mut next = Some(format!("{api}/me/playlists?limit=50"));
    let mut refs: Vec<PlaylistRef> = Vec::new();
    let mut pages = 0;
    while let Some(url) = next {
        pages += 1;
        if pages > 20 {
            break;
        }
        let response = http.get(&url, token)?;
        if !response.ok() {
            return Err(response_error(&response));
        }
        let (page, following) = parse_playlists(&response.body)?;
        refs.extend(page);
        next = following;
    }

    let mut out = Vec::new();
    for p in refs.iter().take(200) {
        if p.total == 0 {
            out.push(ImportedPlaylist {
                art: p.art.clone(),
                name: p.name.clone(),
                tracks: Vec::new(),
                id: p.id.clone(),
            });
            continue;
        }
        log(format!("Importing {} ({} tracks)", p.name, p.total));
        let mut tracks = Vec::new();
        let mut next = Some(format!("{api}/playlists/{}/items?limit=100", p.id));
        let mut pages = 0;
        while let Some(url) = next {
            pages += 1;
            if pages > 25 || tracks.len() >= 2500 {
                break;
            }
            let response = match http.get(&url, token) {
                Ok(response) => response,
                Err(e) => {
                    log(format!("Skipped {}: {e}", p.name));
                    break;
                }
            };
            if !response.ok() {
                let message = response_error(&response);
                if response.status == 401 || response.status == 403 {
                    return Err(message);
                }
                log(format!("Skipped {}: {message}", p.name));
                break;
            }
            let (page, following) = parse_items(&response.body)?;
            tracks.extend(page);
            next = following;
        }
        out.push(ImportedPlaylist {
            art: p.art.clone(),
            name: p.name.clone(),
            tracks,
            id: p.id.clone(),
        });
    }
    Ok(out)
}

pub fn run_import_with_options(
    client_id: &str,
    options: &Options,
    log: &mut dyn FnMut(String),
) -> Result<Vec<ImportedPlaylist>, String> {
    let http = Curl;
    run_import_with(&http, client_id, options, log)
}

/// Runs the whole PKCE dance. Split from `run_import` so tests can inject a fake
/// transport and a local stand-in for Spotify's servers, and so the local
/// listener is bound before the browser is opened.
pub fn run_import_with<H: Http>(
    http: &H,
    client_id: &str,
    options: &Options,
    log: &mut dyn FnMut(String),
) -> Result<Vec<ImportedPlaylist>, String> {
    let client_id = client_id.trim();
    if client_id.is_empty() {
        return Err("Paste your Spotify app Client ID first".to_string());
    }

    let verifier = new_verifier();
    let challenge = challenge_for(&verifier);
    let state = new_verifier();
    let listener = TcpListener::bind(("127.0.0.1", options.port))
        .map_err(|e| format!("cannot listen on 127.0.0.1:{}: {e}", options.port))?;
    let port = listener
        .local_addr()
        .map_err(|e| format!("cannot read the redirect port: {e}"))?
        .port();
    let redirect = redirect_uri(port);

    let url = auth_url_at(&options.endpoints, client_id, &redirect, &challenge, &state);
    log("Opening Spotify sign-in in your browser...".to_string());
    log(format!("Redirect URI must be exactly {redirect}"));
    log(format!("If no browser opens, paste this URL: {url}"));
    if options.open_browser {
        open_browser(&url);
    }

    let code = wait_for_code(&listener, &state, options.timeout, options.cancel.as_deref())?;
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
    let token = match v.get("access_token").and_then(|t| t.as_str()) {
        Some(token) => token.to_string(),
        None => {
            let reason = v
                .get("error_description")
                .and_then(|e| e.as_str())
                .or_else(|| v.get("error").and_then(|e| e.as_str()))
                .map(str::to_string)
                .unwrap_or_else(|| {
                    if response.status != 200 {
                        response_error(&response)
                    } else {
                        "no access token in the response".to_string()
                    }
                });
            return Err(format!("Spotify login failed: {reason}"));
        }
    };

    log("Fetching your playlists...".to_string());
    fetch_all_at(http, &options.endpoints.api, &token, log)
}

// ----------------------------------------------------------------------- csv

pub fn parse_csv(text: &str) -> Vec<Vec<String>> {
    let mut rows = Vec::new();
    let mut row = Vec::new();
    let mut field = String::new();
    let mut in_quotes = false;
    let mut chars = text.chars().peekable();

    while let Some(c) = chars.next() {
        if in_quotes {
            match c {
                '"' => {
                    if chars.peek() == Some(&'"') {
                        field.push('"');
                        chars.next();
                    } else {
                        in_quotes = false;
                    }
                }
                other => field.push(other),
            }
        } else {
            match c {
                '"' => in_quotes = true,
                ',' => {
                    row.push(field.trim().to_string());
                    field.clear();
                }
                '\r' => {}
                '\n' => {
                    row.push(field.trim().to_string());
                    field.clear();
                    if row.iter().any(|f| !f.is_empty()) {
                        rows.push(std::mem::take(&mut row));
                    } else {
                        row.clear();
                    }
                }
                other => field.push(other),
            }
        }
    }
    if !field.is_empty() || !row.is_empty() {
        row.push(field.trim().to_string());
        if row.iter().any(|f| !f.is_empty()) {
            rows.push(row);
        }
    }
    rows
}

fn parse_duration(s: &str) -> u32 {
    let s = s.trim();
    if let Some((m, sec)) = s.split_once(':') {
        let minutes: u32 = m.trim().parse().unwrap_or(0);
        let seconds: u32 = sec.trim().parse().unwrap_or(0);
        return minutes * 60 + seconds;
    }
    let n: u64 = s.parse().unwrap_or(0);
    if n > 1000 {
        (n / 1000) as u32
    } else {
        n as u32
    }
}

pub fn import_csv(text: &str, name: &str) -> Result<ImportedPlaylist, String> {
    let rows = parse_csv(text);
    if rows.len() < 2 {
        return Err("the CSV has no track rows".to_string());
    }
    let header: Vec<String> = rows[0].iter().map(|h| h.to_lowercase()).collect();
    let find = |needles: &[&str]| {
        header
            .iter()
            .position(|h| needles.iter().any(|n| h.contains(n)))
    };
    let title_i = find(&["track name", "title"]).ok_or("no Track Name column")?;
    let artist_i = find(&["artist name", "artist"]).ok_or("no Artist Name column")?;
    let album_i = find(&["album name", "album"]).unwrap_or(title_i);
    let duration_i = find(&["duration"]);
    // Exportify ships the cover URL as its own column.
    let art_i = find(&["album image url", "image url", "cover"]);

    let mut tracks = Vec::new();
    for row in &rows[1..] {
        let title = row.get(title_i).map(|s| s.trim().to_string()).unwrap_or_default();
        if title.is_empty() {
            continue;
        }
        tracks.push(ImportedTrack {
            title,
            artist: row.get(artist_i).cloned().unwrap_or_default(),
            album: row.get(album_i).cloned().unwrap_or_default(),
            secs: duration_i
                .and_then(|i| row.get(i))
                .map(|s| parse_duration(s))
                .unwrap_or(0),
            added: row
                .iter()
                .find(|c| is_date(c))
                .cloned()
                .unwrap_or_default(),
            art: art_i
                .and_then(|i| row.get(i))
                .map(|u| Art::of(u))
                .unwrap_or_default(),
            uri: String::new(),
        });
    }
    if tracks.is_empty() {
        return Err("no usable rows in the CSV".to_string());
    }
    Ok(ImportedPlaylist {
        art: Art::default(),
        name: clean(name),
        tracks,
        id: String::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    #[test]
    fn sha256_matches_known_vectors() {
        assert_eq!(
            hex(&sha256(b"abc")),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            hex(&sha256(b"")),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn pkce_matches_rfc7636_example() {
        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        assert_eq!(
            challenge_for(verifier),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
        let generated = new_verifier();
        assert_eq!(generated.len(), 43);
        assert!(generated
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'));
        assert_eq!(challenge_for(&generated).len(), 43);
    }

    #[cfg(windows)]
    #[test]
    fn the_browser_gets_the_whole_oauth_url_as_one_argument() {
        let url = "https://accounts.spotify.com/authorize?client_id=abc&scope=user-read-playback-state%20user-modify-playback-state&state=xyz";
        assert_eq!(browser_args(url)[1], url, "cmd must never see the & separators");
    }

    #[test]
    fn a_stale_callback_does_not_kill_the_new_sign_in() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind callback");
        let port = listener.local_addr().expect("callback address").port();
        let waiter = std::thread::spawn(move || {
            wait_for_code(&listener, "NEW_STATE", Duration::from_secs(5), None)
        });

        let mut stale = std::net::TcpStream::connect(("127.0.0.1", port)).expect("stale connect");
        stale
            .write_all(b"GET /callback?code=old&state=OLD_STATE HTTP/1.1\r\nHost: localhost\r\n\r\n")
            .expect("write stale");
        let mut buf = [0u8; 1024];
        let _ = stale.read(&mut buf);

        let mut fresh = std::net::TcpStream::connect(("127.0.0.1", port)).expect("fresh connect");
        fresh
            .write_all(b"GET /callback?code=fresh&state=NEW_STATE HTTP/1.1\r\nHost: localhost\r\n\r\n")
            .expect("write fresh");
        let _ = fresh.read(&mut buf);

        let code = waiter.join().expect("join callback").expect("fresh code");
        assert_eq!(code, "fresh");
    }

    #[test]
    fn auth_url_carries_pkce_and_scopes() {
        let url = auth_url(
            "abc123",
            "http://127.0.0.1:8899/callback",
            "CHALLENGE",
            "STATE",
        );
        assert!(url.starts_with("https://accounts.spotify.com/authorize?"));
        assert!(url.contains("response_type=code"));
        assert!(url.contains("client_id=abc123"));
        assert!(url.contains("redirect_uri=http%3A%2F%2F127.0.0.1%3A8899%2Fcallback"));
        assert!(url.contains("code_challenge_method=S256"));
        assert!(url.contains("code_challenge=CHALLENGE"));
        assert!(url.contains("state=STATE"));
        assert!(url.contains("playlist-read-private%20playlist-read-collaborative"));
    }

    #[test]
    fn parses_new_playlist_shape_and_paging() {
        let body = r#"{
            "items": [
                {"id": "p1", "name": "Road trip", "items": {"total": 2}},
                {"id": "p2", "name": "Focus", "items": {"total": 0}},
                {"name": "no id"}
            ],
            "next": "https://api.spotify.com/v1/me/playlists?offset=50"
        }"#;
        let (refs, next) = parse_playlists(body).unwrap();
        assert_eq!(refs.len(), 2);
        assert_eq!(refs[0].id, "p1");
        assert_eq!(refs[0].total, 2);
        assert_eq!(refs[1].name, "Focus");
        assert!(next.unwrap().contains("offset=50"));
    }

    #[test]
    fn parses_items_new_and_old_shapes_skipping_episodes() {
        let body = r#"{
            "items": [
                {"item": {"name": "Neon Horizon", "artists": [{"name": "Nova Kite"}], "album": {"name": "Neon Horizon"}, "duration_ms": 254321}},
                {"track": {"name": "Paper Planes", "artists": [{"name": "The Ferns"}, {"name": "Vela Grey"}], "album": {"name": "Paper Planes"}, "duration_ms": 245900}},
                {"item": {"name": "A podcast", "type": "episode", "duration_ms": 1000}},
                {"item": null}
            ],
            "next": null
        }"#;
        let (tracks, next) = parse_items(body).unwrap();
        assert_eq!(tracks.len(), 2);
        assert_eq!(tracks[0].title, "Neon Horizon");
        assert_eq!(tracks[0].artist, "Nova Kite");
        assert_eq!(tracks[0].secs, 254);
        assert_eq!(tracks[1].artist, "The Ferns, Vela Grey");
        assert_eq!(tracks[1].secs, 245);
        assert!(next.is_none());
    }

    struct FakeHttp {
        pages: Mutex<Vec<(String, String)>>,
        seen: Mutex<Vec<String>>,
    }

    impl Http for FakeHttp {
        fn get(&self, url: &str, token: &str) -> Result<HttpResponse, String> {
            assert_eq!(token, "tok");
            self.seen.lock().unwrap().push(url.to_string());
            let mut pages = self.pages.lock().unwrap();
            if pages.is_empty() {
                return Err("unexpected request".to_string());
            }
            Ok(HttpResponse {
                status: 200,
                body: pages.remove(0).1,
            })
        }

        fn post_form(&self, _url: &str, _form: &[(&str, &str)]) -> Result<HttpResponse, String> {
            Err("not used".to_string())
        }
    }

    #[test]
    fn fetch_follows_paging_and_maps_playlists() {
        let first = r#"{"items":[{"id":"p1","name":"Road trip","items":{"total":2}}],"next":"https://api.spotify.com/v1/me/playlists?offset=50"}"#;
        let second = r#"{"items":[{"id":"p2","name":"Focus","items":{"total":1}}],"next":null}"#;
        let items1 = r#"{"items":[{"item":{"name":"Neon Horizon","artists":[{"name":"Nova Kite"}],"album":{"name":"Neon Horizon"},"duration_ms":254000}}],"next":"https://api.spotify.com/v1/playlists/p1/items?offset=100"}"#;
        let items1b = r#"{"items":[{"item":{"name":"Afterglow","artists":[{"name":"Nova Kite"}],"album":{"name":"Neon Horizon"},"duration_ms":258000}}],"next":null}"#;
        let items2 = r#"{"items":[{"item":{"name":"Kettle Steam","artists":[{"name":"Low Ceiling"}],"album":{"name":"Lo-Fi Beats"},"duration_ms":148000}}],"next":null}"#;

        let fake = FakeHttp {
            pages: Mutex::new(vec![
                ("a".into(), first.into()),
                ("b".into(), second.into()),
                ("c".into(), items1.into()),
                ("d".into(), items1b.into()),
                ("e".into(), items2.into()),
            ]),
            seen: Mutex::new(Vec::new()),
        };

        let mut logs = Vec::new();
        let list = fetch_all(&fake, "tok", &mut |m| logs.push(m)).unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].name, "Road trip");
        assert_eq!(list[0].tracks.len(), 2);
        assert_eq!(list[0].tracks[1].title, "Afterglow");
        assert_eq!(list[1].name, "Focus");
        assert_eq!(list[1].tracks[0].title, "Kettle Steam");
        assert!(logs.iter().any(|l| l.contains("Road trip")));

        let seen = fake.seen.lock().unwrap().clone();
        assert!(seen[0].contains("/me/playlists?limit=50"));
        assert!(seen[1].contains("offset=50"));
        assert!(seen[2].contains("/playlists/p1/items?limit=100"));
        assert!(seen[3].contains("offset=100"));
    }

    #[test]
    fn csv_import_handles_quotes_and_durations() {
        let csv = "Track URI,Track Name,Artist Name(s),Album Name,Track Duration (ms)\n\
                   spotify:track:1,\"Neon, Horizon\",Nova Kite,Neon Horizon,254000\n\
                   spotify:track:2,Paper Planes,\"The Ferns, Vela Grey\",Paper Planes,3:45\n\
                   spotify:track:3,,Nobody,Empty,60000\n";
        let pl = import_csv(csv, "My Export").unwrap();
        assert_eq!(pl.name, "My Export");
        assert_eq!(pl.tracks.len(), 2);
        assert_eq!(pl.tracks[0].title, "Neon, Horizon");
        assert_eq!(pl.tracks[0].secs, 254);
        assert_eq!(pl.tracks[1].artist, "The Ferns, Vela Grey");
        assert_eq!(pl.tracks[1].secs, 225);
    }

    #[test]
    fn an_old_store_without_ids_still_loads_as_a_local_playlist() {
        let path = std::env::temp_dir().join(format!(
            "rustify-old-import-{}-{}.txt",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.subsec_nanos())
                .unwrap_or(0)
        ));
        std::fs::write(
            &path,
            "v1\nplaylist\tOld Trip\tcover.jpg\ntrack\tNightcall\tKavinsky\tOutRun\t258\t2026-01-01\tcover.jpg\n",
        )
        .unwrap();
        let loaded = load(&path);
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].name, "Old Trip");
        assert!(loaded[0].id.is_empty());
        assert_eq!(loaded[0].tracks.len(), 1);
        assert_eq!(loaded[0].tracks[0].title, "Nightcall");
        assert!(loaded[0].tracks[0].uri.is_empty());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn imported_playlists_round_trip_and_become_collections() {
        let path = std::env::temp_dir().join(format!(
            "rustify-import-{}-{}.txt",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.subsec_nanos())
                .unwrap_or(0)
        ));
        let list = vec![
            ImportedPlaylist {
                art: Art::default(),
                name: "Road\ttrip".to_string(),
                tracks: vec![
                    ImportedTrack {
                        art: Art::default(),
                        title: "Neon Horizon".to_string(),
                        artist: "Nova Kite".to_string(),
                        album: "Neon Horizon".to_string(),
                        secs: 254,
                        added: "2026-11-22".to_string(),
                        uri: "spotify:track:neon".to_string(),
                    },
                    ImportedTrack {
                        art: Art::default(),
                        title: "Paper Planes".to_string(),
                        artist: "The Ferns".to_string(),
                        album: "Paper Planes".to_string(),
                        secs: 245,
                        added: "2026-11-22".to_string(),
                        uri: "spotify:track:paper".to_string(),
                    },
                ],
                id: "road-trip-id".to_string(),
            },
            ImportedPlaylist {
                art: Art::default(),
                name: "Empty".to_string(),
                tracks: Vec::new(),
                id: String::new(),
            },
        ];
        save(&path, &list);
        let loaded = load(&path);
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[0].name, "Road trip");
        assert_eq!(loaded[0].tracks, list[0].tracks);
        assert_eq!(loaded[0].id, "road-trip-id", "the playlist link was lost");
        assert_eq!(
            loaded[0].tracks[0].uri, "spotify:track:neon",
            "the track link was lost"
        );
        assert_eq!(loaded[1].name, "Empty");
        assert!(loaded[1].tracks.is_empty());

        let cols = to_collections(&loaded);
        assert_eq!(cols.len(), 2);
        assert_eq!(cols[0].name, "Road trip");
        assert_eq!(cols[0].tracks.len(), 2);
        assert_eq!(cols[0].tracks[0].secs, 254);
        assert_eq!(cols[0].kind, Kind::Playlist);
        assert!(cols[0].genres.is_empty());
        assert!(cols[0].imported());
        assert!(
            cols[0].subtitle.contains("2 songs"),
            "the card needs a readable count: {}",
            cols[0].subtitle
        );
        assert!(!cols[1].tracks.is_empty() || cols[1].tracks.is_empty());

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn playlist_links_and_ids_are_recognised() {
        let id = "37i9dQZF1DXcBWIGoYBM5M";
        assert_eq!(parse_playlist_id(id).as_deref(), Some(id));
        assert_eq!(
            parse_playlist_id("https://open.spotify.com/playlist/37i9dQZF1DXcBWIGoYBM5M?si=abc")
                .as_deref(),
            Some(id)
        );
        assert_eq!(
            parse_playlist_id("spotify:playlist:37i9dQZF1DXcBWIGoYBM5M").as_deref(),
            Some(id)
        );
        assert_eq!(parse_playlist_id("https://open.spotify.com/album/nope"), None);
        assert_eq!(parse_playlist_id(""), None);
    }

    #[test]
    fn embed_page_is_read_into_a_playlist() {
        let page = r#"<html><body><script id="__NEXT_DATA__" type="application/json">{"props":{"pageProps":{"state":{"data":{"entity":{"name":"Road Trip","trackList":[{"title":"Nightcall","subtitle":"Kavinsky","duration":258000},{"title":"A Real Hero","subtitle":"College, Electric Youth","duration":267000},{"title":"","duration":1000}]}}}}}}</script></body></html>"#;
        let playlist = parse_embed(page).expect("embed page should parse");
        assert_eq!(playlist.name, "Road Trip");
        assert_eq!(playlist.tracks.len(), 2);
        assert_eq!(playlist.tracks[0].title, "Nightcall");
        assert_eq!(playlist.tracks[0].secs, 258);
        assert_eq!(playlist.tracks[1].artist, "College, Electric Youth");

        let missing = r#"<html><script id="__NEXT_DATA__">{}</script></html>"#;
        assert!(parse_embed(missing).is_err());
        let private = r#"<html><script id="__NEXT_DATA__">{"props":{"pageProps":{"status":404}}}</script></html>"#;
        assert!(parse_embed(private)
            .unwrap_err()
            .contains("without signing in"));
    }
}
