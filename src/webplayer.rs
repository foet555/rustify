//! Spotify's Web Playback SDK, hosted in a hidden WebView2 child of the main
//! window.
//!
//! Spotify decrypts and renders the audio itself; nothing here touches a
//! stream. Rustify only asks the player to connect, then addresses the device
//! id it reports through the ordinary Web API, so the desktop app is no longer
//! needed to make sound.
//!
//! The page talks to the app through the same loopback server that served it:
//! it polls `/token`, reports `/ready` and pushes diagnostics to `/log`. That
//! avoids depending on a host message bridge for something as important as the
//! access token.
#![cfg(windows)]

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};

/// Served to the webview over loopback.
const PAGE: &str = r#"<!doctype html>
<meta charset="utf-8">
<title>Rustify player</title>
<style>html,body{margin:0;background:#000;overflow:hidden}</style>
<script>
window.__log = m => { try { fetch('/log?m=' + encodeURIComponent(m)); } catch (e) {} };
window.addEventListener('error', e => __log('pageerror: ' + e.message), true);
window.addEventListener('load', () => __log('page-loaded'));
setTimeout(() => { if (!window.__sdkReady) __log('sdk-missing'); }, 6000);
</script>
<script src="https://sdk.scdn.co/spotify-player.js" onerror="__log('sdk-failed')"></script>
<script>
__log('page-script');
async function getToken() {
  for (;;) {
    try {
      const r = await fetch('/token', { cache: 'no-store' });
      const t = (await r.text()).trim();
      if (t) return t;
    } catch (e) {}
    await new Promise(r => setTimeout(r, 400));
  }
}
window.onSpotifyWebPlaybackSDKReady = () => {
  window.__sdkReady = true;
  __log('sdk-ready');
  const player = new Spotify.Player({
    name: 'Rustify',
    getOAuthToken: cb => { getToken().then(cb); },
    volume: 0.7
  });
  player.addListener('ready', ({ device_id }) => {
    fetch('/ready?id=' + encodeURIComponent(device_id));
  });
  player.addListener('not_ready', () => fetch('/ready?id='));
  player.addListener('initialization_error', ({ message }) => {
    __log('init: ' + message);
    fetch('/error?m=' + encodeURIComponent(message));
  });
  player.addListener('authentication_error', ({ message }) => {
    __log('auth: ' + message);
    fetch('/auth?m=' + encodeURIComponent(message));
  });
  player.addListener('account_error', ({ message }) => {
    __log('account: ' + message);
    fetch('/account?m=' + encodeURIComponent(message));
  });
  player.addListener('playback_error', ({ message }) => {
    __log('playback: ' + message);
    fetch('/error?m=' + encodeURIComponent(message));
  });
  player.addListener('player_state_changed', s => {
    if (!s) return;
    fetch('/state?paused=' + (s.paused ? '1' : '0') + '&position=' + s.position + '&duration=' + s.duration);
  });
  player.connect().then(ok => __log('connect ' + ok));
};
</script>
"#;

/// Something the player page reported.
pub enum Event {
    /// The player is ready and Spotify routed it to this device id.
    Ready(String),
    NotReady,
    /// Bad token or missing scope: the saved session has to be redone.
    Auth(String),
    /// Free accounts cannot use the SDK.
    Account(String),
    Error(String),
}

#[derive(Default)]
struct Bridge {
    token: Option<String>,
    events: Vec<Event>,
}

pub struct Player {
    bridge: Arc<Mutex<Bridge>>,
    _view: wry::WebView,
}

impl Player {
    /// Builds the hidden webview inside the running Rustify window and starts
    /// loading the page. The window must already exist.
    pub fn start() -> Result<Self, String> {
        init_com();
        let bridge = Arc::new(Mutex::new(Bridge::default()));
        let url = serve(bridge.clone())?;
        let window = main_window()?;
        let view = wry::WebViewBuilder::new()
            .with_url(&url)
            .with_on_page_load_handler(|event, url| {
                if cfg!(debug_assertions) {
                    let what = match event {
                        wry::PageLoadEvent::Started => "started",
                        wry::PageLoadEvent::Finished => "finished",
                    };
                    eprintln!("webplayer page {what} {url}");
                }
            })
            .with_bounds(wry::Rect {
                // Off the client area: the player needs a live view to keep
                // its audio graph running, but nothing of it should be seen.
                position: wry::dpi::LogicalPosition::new(-16.0, -16.0).into(),
                size: wry::dpi::LogicalSize::new(8.0, 8.0).into(),
            })
            .build_as_child(&window)
            .map_err(|e| format!("WebView2 could not start: {e}"))?;
        Ok(Self {
            bridge,
            _view: view,
        })
    }

    /// Drains what the page reported since the last frame.
    pub fn poll(&mut self) -> Vec<Event> {
        self.bridge
            .lock()
            .map(|mut b| std::mem::take(&mut b.events))
            .unwrap_or_default()
    }

    /// True until the page has a token it can use.
    pub fn needs_token(&self) -> bool {
        self.bridge
            .lock()
            .map(|b| b.token.is_none())
            .unwrap_or(false)
    }

    /// Publishes a fresh access token; the page picks it up on its next poll.
    pub fn set_token(&self, token: &str) {
        if let Ok(mut bridge) = self.bridge.lock() {
            bridge.token = Some(token.to_string());
        }
    }

    /// Makes the page ask again, used before the token expires.
    pub fn clear_token(&self) {
        if let Ok(mut bridge) = self.bridge.lock() {
            bridge.token = None;
        }
    }
}

/// Serves the page and the tiny bridge endpoints on a loopback port picked by
/// the OS. No audio goes through here.
fn serve(bridge: Arc<Mutex<Bridge>>) -> Result<String, String> {
    let listener = TcpListener::bind(("127.0.0.1", 0))
        .map_err(|e| format!("could not serve the player page: {e}"))?;
    let port = listener
        .local_addr()
        .map_err(|e| format!("could not read the player port: {e}"))?
        .port();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else {
                continue;
            };
            handle(stream, &bridge);
        }
    });
    Ok(format!("http://127.0.0.1:{port}/"))
}

fn handle(mut stream: TcpStream, bridge: &Arc<Mutex<Bridge>>) {
    let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(2)));
    let mut buf = [0u8; 4096];
    let n = stream.read(&mut buf).unwrap_or(0);
    let request = String::from_utf8_lossy(&buf[..n]).to_string();
    let line = request.lines().next().unwrap_or("");
    let mut parts = line.split_whitespace();
    let _method = parts.next().unwrap_or("");
    let target = parts.next().unwrap_or("/");
    let (path, query) = match target.split_once('?') {
        Some((p, q)) => (p, q),
        None => (target, ""),
    };

    match path {
        "/token" => {
            let token = bridge
                .lock()
                .ok()
                .and_then(|b| b.token.clone())
                .unwrap_or_default();
            reply(&mut stream, "text/plain; charset=utf-8", token.as_bytes());
        }
        "/ready" => {
            let id = param(query, "id");
            if let Ok(mut b) = bridge.lock() {
                b.events.push(if id.is_empty() {
                    Event::NotReady
                } else {
                    Event::Ready(id)
                });
            }
            reply(&mut stream, "text/plain", b"ok");
        }
        "/auth" | "/account" | "/error" => {
            let message = param(query, "m");
            let event = match path {
                "/auth" => Event::Auth(message),
                "/account" => Event::Account(message),
                _ => Event::Error(message),
            };
            if cfg!(debug_assertions) {
                eprintln!("webplayer page reported {path}: {}", param(query, "m"));
            }
            if let Ok(mut b) = bridge.lock() {
                b.events.push(event);
            }
            reply(&mut stream, "text/plain", b"ok");
        }
        "/state" => {
            if cfg!(debug_assertions) {
                eprintln!(
                    "webplayer state paused={} position={} duration={}",
                    param(query, "paused"),
                    param(query, "position"),
                    param(query, "duration")
                );
            }
            reply(&mut stream, "text/plain", b"ok");
        }
        "/log" => {
            if cfg!(debug_assertions) {
                eprintln!("webplayer page: {}", param(query, "m"));
            }
            reply(&mut stream, "text/plain", b"ok");
        }
        _ => reply(&mut stream, "text/html; charset=utf-8", PAGE.as_bytes()),
    }
}

fn reply(stream: &mut TcpStream, kind: &str, body: &[u8]) {
    let header = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(header.as_bytes());
    let _ = stream.write_all(body);
    let _ = stream.flush();
}

/// `id` or `m` out of a raw query string, percent-decoded.
fn param(query: &str, key: &str) -> String {
    for pair in query.split('&') {
        let (k, v) = match pair.split_once('=') {
            Some(kv) => kv,
            None => continue,
        };
        if k == key {
            let bytes = v.as_bytes();
            let mut out = Vec::with_capacity(bytes.len());
            let mut i = 0;
            while i < bytes.len() {
                match bytes[i] {
                    b'%' if i + 2 < bytes.len() => {
                        let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("");
                        match u8::from_str_radix(hex, 16) {
                            Ok(b) => {
                                out.push(b);
                                i += 3;
                            }
                            Err(_) => {
                                out.push(bytes[i]);
                                i += 1;
                            }
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
            return String::from_utf8_lossy(&out).to_string();
        }
    }
    String::new()
}

/// The main window, by the title the app sets. It exists by the time the player
/// starts (the first frame has already been shown).
fn main_window() -> Result<MainWindow, String> {
    let title: Vec<u16> = "Rustify\0".encode_utf16().collect();
    let hwnd = unsafe { FindWindowW(std::ptr::null(), title.as_ptr()) };
    if hwnd == 0 {
        return Err("the main window could not be found".to_string());
    }
    Ok(MainWindow(hwnd))
}

struct MainWindow(isize);

impl raw_window_handle::HasWindowHandle for MainWindow {
    fn window_handle(
        &self,
    ) -> Result<raw_window_handle::WindowHandle<'_>, raw_window_handle::HandleError> {
        use raw_window_handle::{RawWindowHandle, Win32WindowHandle, WindowHandle};
        let handle = std::num::NonZeroIsize::new(self.0)
            .ok_or(raw_window_handle::HandleError::Unavailable)?;
        let raw = RawWindowHandle::Win32(Win32WindowHandle::new(handle));
        Ok(unsafe { WindowHandle::borrow_raw(raw) })
    }
}

/// WebView2 is COM; winit may have set the apartment up already, in which case
/// the changed-mode error is harmless and simply means "same thread, other
/// mode".
fn init_com() {
    unsafe {
        CoInitializeEx(std::ptr::null(), 0x2 | 0x4);
    }
}

#[link(name = "user32")]
unsafe extern "system" {
    fn FindWindowW(class: *const u16, title: *const u16) -> isize;
}

#[link(name = "ole32")]
unsafe extern "system" {
    fn CoInitializeEx(reserved: *const std::ffi::c_void, flags: u32) -> i32;
}
