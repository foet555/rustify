// Discord rich presence: the "Listening to ..." card on the owner's profile.
//
// This only talks to the Discord client already running on this PC, over its
// local IPC pipe. Nothing is uploaded, no track audio is touched, and the
// payload is just "this song, this artist, this position" - the same text the
// player bar already shows. An Application ID is required because that is how
// Discord knows which app the card belongs to; the user creates one in the
// Discord developer portal and names it Rustify.
use std::io::{Read, Write};
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub const OP_HANDSHAKE: u32 = 0;
pub const OP_FRAME: u32 = 1;

/// WHAT is playing, and how far in. Enough to draw the card, nothing more.
#[derive(Clone, Debug, PartialEq)]
pub struct Now {
    pub track: String,
    pub artist: String,
    pub playing: bool,
    pub elapsed_ms: u64,
}

impl Now {
    /// Presence is only re-sent when this changes, so a moving playhead does
    /// not spam Discord twice a second.
    pub fn signature(&self) -> String {
        format!("{}|{}|{}", self.track, self.artist, self.playing)
    }
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"").replace(['\n', '\r'], " ")
}

/// Discord truncates long strings itself, but sending 300 characters of a
/// YouTube upload title is pointless.
fn clip(s: &str) -> String {
    if s.chars().count() <= 120 {
        return s.to_string();
    }
    s.chars().take(117).collect::<String>() + "..."
}

/// `[op][len][json]`, little endian, which is what the client reads.
pub fn frame(op: u32, json: &str) -> Vec<u8> {
    let body = json.as_bytes();
    let mut out = Vec::with_capacity(body.len() + 8);
    out.extend_from_slice(&op.to_le_bytes());
    out.extend_from_slice(&(body.len() as u32).to_le_bytes());
    out.extend_from_slice(body);
    out
}

pub fn handshake(client_id: &str) -> String {
    format!(r#"{{"v":1,"client_id":"{}"}}"#, escape(client_id))
}

pub fn activity(now: &Now) -> String {
    let artist = if now.artist.trim().is_empty() {
        "Rustify".to_string()
    } else {
        format!("by {}", clip(&now.artist))
    };
    let mut act = format!(
        r#"{{"type":2,"details":"{}","state":"{}""#,
        escape(&clip(&now.track)),
        escape(&artist)
    );
    // A start timestamp makes Discord draw the real elapsed time; while paused
    // there is no honest position to show, so it is left out.
    if now.playing {
        let start = now_ms().saturating_sub(now.elapsed_ms as i64);
        act.push_str(&format!(r#","timestamps":{{"start":{start}}}"#));
    }
    act.push('}');
    act
}

pub fn set_activity(now: &Now, nonce: &str) -> String {
    format!(
        r#"{{"cmd":"SET_ACTIVITY","args":{{"pid":{},"activity":{}}},"nonce":"{}"}}"#,
        std::process::id(),
        activity(now),
        escape(nonce)
    )
}

pub fn clear_activity(nonce: &str) -> String {
    format!(
        r#"{{"cmd":"SET_ACTIVITY","args":{{"pid":{}}},"nonce":"{}"}}"#,
        std::process::id(),
        escape(nonce)
    )
}

#[cfg(windows)]
pub fn connect() -> Result<std::fs::File, String> {
    let mut last = "Discord is not running".to_string();
    for i in 0..10 {
        let path = format!(r"\\.\pipe\discord-ipc-{i}");
        match std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&path)
        {
            Ok(file) => return Ok(file),
            Err(e) => last = e.to_string(),
        }
    }
    Err(format!("start Discord to share what you are playing ({last})"))
}

#[cfg(not(windows))]
pub fn connect() -> Result<std::fs::File, String> {
    Err("Discord presence is wired up for Windows only".to_string())
}

pub struct Handle {
    pub tx: Sender<Option<Now>>,
    pub status: Receiver<String>,
}

/// Keeps the Discord link alive on its own thread: reconnects when Discord
/// restarts and only writes when the signature actually changes.
pub fn spawn(client_id: String) -> Handle {
    let (tx, rx) = channel::<Option<Now>>();
    let (stx, srx) = channel::<String>();
    std::thread::spawn(move || {
        let mut link: Option<std::fs::File> = None;
        let mut sent: Option<String> = None;
        let mut nonce = 0u32;
        let mut next_try = Instant::now();
        loop {
            if link.is_none() && Instant::now() >= next_try {
                match connect() {
                    Ok(mut file) => {
                        let hello = frame(OP_HANDSHAKE, &handshake(&client_id));
                        if file.write_all(&hello).and_then(|_| file.flush()).is_ok() {
                            // Drain whatever Discord answers so its pipe never
                            // fills up while we are not listening.
                            if let Ok(mut reader) = file.try_clone() {
                                std::thread::spawn(move || {
                                    let mut buf = [0u8; 1024];
                                    while reader.read(&mut buf).map(|n| n > 0).unwrap_or(false) {}
                                });
                            }
                            let _ = stx.send("Connected to Discord".to_string());
                            link = Some(file);
                            sent = None;
                        } else {
                            let _ = stx.send("Discord refused the handshake".to_string());
                            next_try = Instant::now() + Duration::from_secs(15);
                        }
                    }
                    Err(e) => {
                        let _ = stx.send(e);
                        next_try = Instant::now() + Duration::from_secs(15);
                    }
                }
            }

            match rx.recv_timeout(Duration::from_millis(900)) {
                Ok(now) => {
                    let Some(file) = link.as_mut() else {
                        continue;
                    };
                    let signature = now.as_ref().map(|n| n.signature());
                    if signature == sent {
                        continue;
                    }
                    nonce = nonce.wrapping_add(1);
                    let json = match &now {
                        Some(n) => set_activity(n, &format!("rustify-{nonce}")),
                        None => clear_activity(&format!("rustify-{nonce}")),
                    };
                    if file
                        .write_all(&frame(OP_FRAME, &json))
                        .and_then(|_| file.flush())
                        .is_err()
                    {
                        let _ = stx.send("Discord closed the connection; reconnecting".to_string());
                        link = None;
                        sent = None;
                        next_try = Instant::now();
                        continue;
                    }
                    sent = signature;
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return,
            }
        }
    });
    Handle { tx, status: srx }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now(track: &str, artist: &str, playing: bool) -> Now {
        Now {
            track: track.to_string(),
            artist: artist.to_string(),
            playing,
            elapsed_ms: 0,
        }
    }

    #[test]
    fn frames_are_opcode_then_length_then_json() {
        let bytes = frame(OP_FRAME, r#"{"a":1}"#);
        assert_eq!(&bytes[0..4], &1u32.to_le_bytes());
        assert_eq!(&bytes[4..8], &7u32.to_le_bytes());
        assert_eq!(&bytes[8..], br#"{"a":1}"#);
        assert_eq!(bytes.len(), 15);
    }

    #[test]
    fn handshake_names_the_app_once() {
        let json = handshake("1234567890");
        assert!(json.contains(r#""v":1"#));
        assert!(json.contains(r#""client_id":"1234567890""#));
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["v"], 1);
    }

    #[test]
    fn activity_is_a_listening_card_with_the_track_and_artist() {
        let json = set_activity(&now("Nightcall", "Kavinsky", true), "n1");
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["cmd"], "SET_ACTIVITY");
        assert_eq!(v["args"]["activity"]["type"], 2);
        assert_eq!(v["args"]["activity"]["details"], "Nightcall");
        assert_eq!(v["args"]["activity"]["state"], "by Kavinsky");
        assert!(v["args"]["activity"]["timestamps"]["start"].is_i64());
        assert_eq!(v["args"]["pid"], std::process::id());
        assert_eq!(v["nonce"], "n1");
    }

    #[test]
    fn paused_cards_have_no_made_up_position() {
        let json = activity(&now("Espresso", "Sabrina Carpenter", false));
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert!(v.get("timestamps").is_none());
        assert_eq!(v["details"], "Espresso");
    }

    #[test]
    fn quotes_and_newlines_cannot_break_the_frame() {
        let json = activity(&now("He said \"hi\"\nthere", "", false));
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["details"], "He said \"hi\" there");
        assert_eq!(v["state"], "Rustify");
    }

    #[test]
    fn long_titles_are_clipped_not_sent_whole() {
        let long = "a".repeat(400);
        let v: serde_json::Value = serde_json::from_str(&activity(&now(&long, "x", false))).unwrap();
        let details = v["details"].as_str().unwrap();
        assert_eq!(details.chars().count(), 120);
        assert!(details.ends_with("..."));
    }

    #[test]
    fn same_song_same_state_is_the_same_signature() {
        let a = now("A", "B", true);
        let b = now("A", "B", true);
        assert_eq!(a.signature(), b.signature());
        assert_ne!(a.signature(), now("A", "B", false).signature());
    }

    #[test]
    fn clearing_sends_no_activity_payload() {
        let v: serde_json::Value = serde_json::from_str(&clear_activity("n2")).unwrap();
        assert!(v["args"].get("activity").is_none());
        assert_eq!(v["cmd"], "SET_ACTIVITY");
    }
}
