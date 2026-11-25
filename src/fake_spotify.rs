//! Test-only local stand-in for Spotify's OAuth and Web API hosts. It speaks
//! just enough HTTP/1.1 for curl to talk to it, serves a two-page playlist
//! list, a paginated playlist, an episode-only playlist, and can fail the token
//! exchange or the API call on demand.
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use serde_json::json;

pub struct FakePlan {
    pub token_status: u16,
    pub token_body: String,
    pub playlists_expired: bool,
}

impl FakePlan {
    pub fn ok() -> Self {
        Self {
            token_status: 200,
            token_body:
                r#"{"access_token":"fake-access-token","token_type":"Bearer","expires_in":3600}"#
                    .to_string(),
            playlists_expired: false,
        }
    }
}

pub struct FakeSpotify {
    pub base: String,
    pub seen: Arc<Mutex<Vec<String>>>,
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl FakeSpotify {
    pub fn start(plan: FakePlan) -> Self {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind fake spotify");
        let port = listener.local_addr().unwrap().port();
        let base = format!("http://127.0.0.1:{port}");
        listener.set_nonblocking(true).unwrap();

        let stop = Arc::new(AtomicBool::new(false));
        let seen = Arc::new(Mutex::new(Vec::new()));
        let stop_inner = stop.clone();
        let seen_inner = seen.clone();
        let base_inner = base.clone();

        let handle = std::thread::spawn(move || {
            while !stop_inner.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        // Accepted sockets may inherit the listener's non-blocking
                        // mode on Windows; force blocking so the whole request is
                        // read before it is routed.
                        let _ = stream.set_nonblocking(false);
                        let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
                        let (method, path, _body) = read_request(&mut stream);
                        seen_inner
                            .lock()
                            .unwrap()
                            .push(format!("{method} {path}"));
                        let route = path.split('?').next().unwrap_or("").to_string();
                        let (status, body) = match route.as_str() {
                            "/api/token" => (plan.token_status, plan.token_body.clone()),
                            "/v1/me/playlists" => {
                                if plan.playlists_expired {
                                    (
                                        401,
                                        json!({"error": {"status": 401, "message": "The access token expired"}})
                                            .to_string(),
                                    )
                                } else if path.contains("offset=2") {
                                    (200, playlists_page2())
                                } else {
                                    (200, playlists_page1(&base_inner))
                                }
                            }
                            "/v1/playlists/p1/items" => {
                                if path.contains("offset=1") {
                                    (200, p1_page2())
                                } else {
                                    (200, p1_page1(&base_inner))
                                }
                            }
                            "/v1/playlists/p2/items" => (200, p2_episodes()),
                            _ => (
                                404,
                                json!({"error": {"status": 404, "message": "not found"}}).to_string(),
                            ),
                        };
                        write_response(&mut stream, status, &body);
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    Err(_) => break,
                }
            }
        });

        Self {
            base,
            seen,
            stop,
            handle: Some(handle),
        }
    }
}

impl Drop for FakeSpotify {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

fn playlists_page1(base: &str) -> String {
    json!({
        "items": [
            {"id": "p1", "name": "Night Drive", "items": {"total": 2}},
            {"id": "p2", "name": "Podcast Junk", "items": {"total": 2}}
        ],
        "next": format!("{base}/v1/me/playlists?offset=2")
    })
    .to_string()
}

fn playlists_page2() -> String {
    json!({
        "items": [
            {"id": "p3", "name": "Empty Shelf", "items": {"total": 0}}
        ],
        "next": null
    })
    .to_string()
}

fn p1_page1(base: &str) -> String {
    json!({
        "items": [
            {"item": {"name": "Neon Horizon", "artists": [{"name": "Nova Kite"}], "album": {"name": "Neon Horizon"}, "duration_ms": 254000}}
        ],
        "next": format!("{base}/v1/playlists/p1/items?offset=1")
    })
    .to_string()
}

fn p1_page2() -> String {
    json!({
        "items": [
            {"item": {"name": "Afterglow", "artists": [{"name": "Nova Kite"}], "album": {"name": "Neon Horizon"}, "duration_ms": 258000}}
        ],
        "next": null
    })
    .to_string()
}

fn p2_episodes() -> String {
    json!({
        "items": [
            {"item": {"name": "Episode One", "type": "episode", "duration_ms": 600000}},
            {"item": null}
        ],
        "next": null
    })
    .to_string()
}

fn read_request(stream: &mut TcpStream) -> (String, String, String) {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                buf.extend_from_slice(&chunk[..n]);
                if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                    let head = String::from_utf8_lossy(&buf[..pos]).to_string();
                    let len = head
                        .lines()
                        .find_map(|line| {
                            line.to_ascii_lowercase()
                                .strip_prefix("content-length:")
                                .map(|v| v.trim().parse::<usize>().unwrap_or(0))
                        })
                        .unwrap_or(0);
                    let body_start = pos + 4;
                    while buf.len() < body_start + len {
                        match stream.read(&mut chunk) {
                            Ok(0) => break,
                            Ok(n) => buf.extend_from_slice(&chunk[..n]),
                            Err(_) => break,
                        }
                    }
                    let body = String::from_utf8_lossy(&buf[body_start..]).to_string();
                    let mut parts = head.lines().next().unwrap_or("").split_whitespace();
                    return (
                        parts.next().unwrap_or("").to_string(),
                        parts.next().unwrap_or("").to_string(),
                        body,
                    );
                }
            }
            Err(_) => break,
        }
    }
    (String::new(), String::new(), String::new())
}

fn write_response(stream: &mut TcpStream, status: u16, body: &str) {
    let response = format!(
        "HTTP/1.1 {status} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        if status == 200 { "OK" } else { "Error" },
        body.len(),
        body
    );
    let _ = stream.write_all(response.as_bytes());
    let _ = stream.flush();
}
