//! Phone pairing: the one-time check that ties a Rustify install to its
//! owner's phone. The PC shows a six-digit code on its lock screen, the phone
//! posts that code back over the local network, and the PC remembers the phone.
//!
//! The listener is tiny on purpose: one request shape, one code, and it shuts
//! down with the app. There is no cloud service and no account involved.
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream, UdpSocket};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// One port for every Rustify install. Only one PC needs to pair at a time.
pub const PORT: u16 = 47653;

pub fn path() -> PathBuf {
    crate::profile::Profile::default_path().with_file_name("paired-phone.txt")
}

/// The phone's name when this PC has already been paired.
pub fn paired() -> Option<String> {
    let text = std::fs::read_to_string(path()).ok()?;
    let name = text.lines().next().unwrap_or("").trim().to_string();
    (!name.is_empty()).then_some(name)
}

pub fn remember(name: &str) {
    let path = path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(path, format!("{name}\n"));
}

pub fn forget() {
    let _ = std::fs::remove_file(path());
}

fn make_code() -> String {
    let mut x = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(1)
        ^ (u64::from(std::process::id()) << 32);
    // Hand-rolled xorshift: this is a one-time door code, not cryptography.
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    format!("{:06}", x % 900_000 + 100_000)
}

/// Best-effort LAN address: asking the routing table which interface would be
/// used for the internet is enough, and no packet is sent.
fn lan_address() -> String {
    if let Ok(socket) = UdpSocket::bind("0.0.0.0:0") {
        if socket.connect("8.8.8.8:80").is_ok() {
            if let Ok(addr) = socket.local_addr() {
                return format!("{}:{PORT}", addr.ip());
            }
        }
    }
    format!("127.0.0.1:{PORT}")
}

pub struct Server {
    pub code: String,
    pub address: String,
    #[allow(dead_code)]
    pub port: u16,
    pub listening: bool,
    stop: Arc<AtomicBool>,
    rx: Receiver<String>,
}

impl Server {
    pub fn start() -> Self {
        Self::start_on(PORT, make_code())
    }

    fn start_on(port: u16, code: String) -> Self {
        let (tx, rx) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let listener = TcpListener::bind(("0.0.0.0", port));
        let listening = listener.is_ok();
        let bound = listener
            .as_ref()
            .ok()
            .and_then(|l| l.local_addr().ok())
            .map(|a| a.port())
            .unwrap_or(port);
        if let Ok(listener) = listener {
            let _ = listener.set_nonblocking(true);
            let code = code.clone();
            let stop = stop.clone();
            std::thread::spawn(move || serve(listener, code, tx, stop));
        }
        let address = if port == PORT {
            lan_address()
        } else {
            format!("127.0.0.1:{bound}")
        };
        Self {
            code,
            address,
            port: bound,
            listening,
            stop,
            rx,
        }
    }

    /// The name of the phone that just paired, once.
    pub fn poll(&self) -> Option<String> {
        match self.rx.try_recv() {
            Ok(name) => Some(name),
            Err(TryRecvError::Empty) | Err(TryRecvError::Disconnected) => None,
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

fn serve(listener: TcpListener, code: String, tx: Sender<String>, stop: Arc<AtomicBool>) {
    let mut attempts = 0u32;
    while !stop.load(Ordering::Relaxed) {
        match listener.accept() {
            Ok((mut stream, _)) => {
                // Accepted sockets inherit non-blocking mode on Windows; the
                // body usually arrives in a later packet, so wait for it.
                let _ = stream.set_nonblocking(false);
                let _ = stream.set_read_timeout(Some(Duration::from_secs(3)));
                let request = read_request(&mut stream);
                let body = request.split("\r\n\r\n").nth(1).unwrap_or("");
                let given = field(body, "code");
                let device = {
                    let name = field(body, "name");
                    if name.trim().is_empty() {
                        "Phone".to_string()
                    } else {
                        name
                    }
                };
                attempts += 1;
                let ok = attempts <= 30 && given.trim() == code;
                let reply = if ok { "paired" } else { "wrong code" };
                let _ = write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: text/plain; charset=utf-8\r\n\
                     Content-Length: {}\r\nConnection: close\r\n\r\n{}",
                    reply.len(),
                    reply
                );
                let _ = stream.flush();
                if ok {
                    let _ = tx.send(device);
                    return;
                }
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(60));
            }
            Err(_) => std::thread::sleep(Duration::from_millis(200)),
        }
    }
}

/// Reads a full HTTP request, not just the first packet. A phone (and
/// PowerShell) may send the headers and the body in separate writes; the body
/// still belongs to this request and must not be dropped.
fn read_request(stream: &mut TcpStream) -> String {
    let mut data = Vec::new();
    let mut byte = [0u8; 1];
    while !data.ends_with(b"\r\n\r\n") {
        match stream.read(&mut byte) {
            Ok(0) => break,
            Ok(_) => data.push(byte[0]),
            Err(_) => break,
        }
    }
    let want = content_length(&String::from_utf8_lossy(&data));
    if want > 0 {
        let mut body = vec![0u8; want];
        let mut filled = 0;
        while filled < body.len() {
            match stream.read(&mut body[filled..]) {
                Ok(0) => break,
                Ok(n) => filled += n,
                Err(_) => break,
            }
        }
        data.extend_from_slice(&body[..filled]);
    }
    String::from_utf8_lossy(&data).to_string()
}

#[cfg(test)]
#[test]
fn content_length_is_read_case_insensitively() {
    assert_eq!(
        content_length("POST /pair HTTP/1.1\r\ncontent-length: 22\r\n"),
        22
    );
}

fn content_length(head: &str) -> usize {
    head.lines()
        .find_map(|line| {
            let (key, value) = line.split_once(':')?;
            if key.eq_ignore_ascii_case("content-length") {
                value.trim().parse().ok()
            } else {
                None
            }
        })
        .unwrap_or(0)
}

fn field(body: &str, key: &str) -> String {
    body.split('&')
        .find_map(|pair| pair.strip_prefix(key).and_then(|rest| rest.strip_prefix('=')))
        .map(decode)
        .unwrap_or_default()
}

fn decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b'%' if i + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("");
                match u8::from_str_radix(hex, 16) {
                    Ok(byte) => {
                        out.push(byte);
                        i += 3;
                    }
                    Err(_) => {
                        out.push(bytes[i]);
                        i += 1;
                    }
                }
            }
            byte => {
                out.push(byte);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn post(address: &str, body: &str) -> String {
        let mut stream = TcpStream::connect(address).expect("connect to pairing");
        write!(
            stream,
            "POST /pair HTTP/1.1\r\nHost: rustify\r\nContent-Length: {}\r\n\r\n{}",
            body.len(),
            body
        )
        .unwrap();
        let mut reply = String::new();
        stream.read_to_string(&mut reply).unwrap();
        reply
    }

    #[test]
    fn the_code_pairs_the_phone_and_a_wrong_code_does_not() {
        let server = Server::start_on(0, "123456".to_string());
        let address = format!("127.0.0.1:{}", server.port);

        let wrong = post(&address, "code=000000&name=Phone");
        assert!(wrong.contains("wrong code"), "{wrong}");
        assert!(server.poll().is_none(), "a wrong code must not pair");

        let right = post(&address, "code=123456&name=Pixel%209");
        assert!(right.contains("paired"), "{right}");
        assert_eq!(server.poll().as_deref(), Some("Pixel 9"));
    }

    #[test]
    fn a_body_sent_after_the_headers_still_pairs() {
        let server = Server::start_on(0, "654321".to_string());
        let address = format!("127.0.0.1:{}", server.port);
        let mut stream = TcpStream::connect(address).unwrap();
        write!(
            stream,
            "POST /pair HTTP/1.1\r\nHost: rustify\r\nContent-Length: 22\r\n\r\n"
        )
        .unwrap();
        stream.flush().unwrap();
        std::thread::sleep(Duration::from_millis(80));
        stream.write_all(b"code=654321&name=Split").unwrap();
        let mut reply = String::new();
        stream.read_to_string(&mut reply).unwrap();
        assert!(reply.contains("paired"), "{reply}");
        assert_eq!(server.poll().as_deref(), Some("Split"));
    }

    #[test]
    fn the_phone_name_survives_percent_encoding() {
        assert_eq!(decode("My+Phone%20%26+Tablet"), "My Phone & Tablet");
    }
}
