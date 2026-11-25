//! Searching Spotify's whole catalogue through the official Web API.
//!
//! One `GET /v1/search` covers the three things the client shows side by side:
//! tracks, albums and artists. Everything here is a pure function over the JSON
//! so the parser can be checked against a recorded response without a socket,
//! and the network layer stays the same injectable `Transport` playback uses.
use serde_json::Value;

use crate::connect::Transport;
use crate::library::Art;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum HitKind {
    Track,
    Album,
    Artist,
}

impl HitKind {
    pub fn label(&self) -> &'static str {
        match self {
            HitKind::Track => "Song",
            HitKind::Album => "Album",
            HitKind::Artist => "Artist",
        }
    }
}

/// One row of search results, already shaped like the library's own types.
#[derive(Clone, Debug, PartialEq)]
pub struct Hit {
    pub kind: HitKind,
    pub id: String,
    pub uri: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub secs: u32,
    pub art: Art,
}

/// Percent-encodes a query for the `q` parameter. Only the characters that
/// would break the URL are touched, so a plain search stays readable.
pub fn encode_query(q: &str) -> String {
    let mut out = String::with_capacity(q.len() * 3);
    for b in q.trim().bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

pub fn search_url(api: &str, query: &str) -> String {
    format!(
        "{api}/search?q={}&type=track,album,artist&limit=8&market=from_token",
        encode_query(query)
    )
}

/// Reads Spotify's `images` array into cover URLs by width. The largest is
/// last in the API but sorting makes no assumptions.
fn art_of(value: &Value) -> Art {
    let mut images: Vec<(u32, String)> = Vec::new();
    if let Some(list) = value.get("images").and_then(Value::as_array) {
        for img in list {
            let url = img.get("url").and_then(Value::as_str).unwrap_or_default();
            if url.is_empty() {
                continue;
            }
            let width = img.get("width").and_then(Value::as_u64).unwrap_or(0) as u32;
            images.push((width, url.to_string()));
        }
    }
    images.sort_by_key(|(w, _)| *w);
    Art { images }
}

fn artists_of(value: &Value) -> String {
    value
        .get("artists")
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .filter_map(|a| a.get("name").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_default()
}

fn track_hit(item: &Value) -> Option<Hit> {
    let title = item.get("name").and_then(Value::as_str)?.to_string();
    let album = item.get("album")?;
    Some(Hit {
        kind: HitKind::Track,
        id: item
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        uri: item.get("uri").and_then(Value::as_str).unwrap_or("").to_string(),
        artist: artists_of(item),
        album: album
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        secs: item
            .get("duration_ms")
            .and_then(Value::as_u64)
            .map(|ms| (ms / 1000) as u32)
            .unwrap_or(0),
        title,
        // A track wears its album's cover.
        art: art_of(album),
    })
}

fn album_hit(item: &Value) -> Option<Hit> {
    let title = item.get("name").and_then(Value::as_str)?.to_string();
    Some(Hit {
        kind: HitKind::Album,
        id: item
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        uri: item.get("uri").and_then(Value::as_str).unwrap_or("").to_string(),
        artist: artists_of(item),
        album: String::new(),
        secs: 0,
        title,
        art: art_of(item),
    })
}

fn artist_hit(item: &Value) -> Option<Hit> {
    let title = item.get("name").and_then(Value::as_str)?.to_string();
    Some(Hit {
        kind: HitKind::Artist,
        id: item
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        uri: item.get("uri").and_then(Value::as_str).unwrap_or("").to_string(),
        artist: item
            .get("genres")
            .and_then(Value::as_array)
            .map(|g| {
                g.iter()
                    .filter_map(Value::as_str)
                    .take(2)
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .unwrap_or_default(),
        album: String::new(),
        secs: 0,
        title,
        art: art_of(item),
    })
}

/// Parses a `/v1/search` response into at most `limit` hits, tracks first so
/// the thing you typed a song name for is at the top.
pub fn parse_search(json: &str, limit: usize) -> Result<Vec<Hit>, String> {
    let v: Value = serde_json::from_str(json).map_err(|e| format!("bad search JSON: {e}"))?;
    let mut out = Vec::new();
    for (key, build) in [
        ("tracks", track_hit as fn(&Value) -> Option<Hit>),
        ("albums", album_hit),
        ("artists", artist_hit),
    ] {
        let Some(items) = v
            .pointer(&format!("/{key}/items"))
            .and_then(Value::as_array)
        else {
            continue;
        };
        for item in items {
            // Spotify returns nulls for markets where an item is unavailable.
            if item.is_null() {
                continue;
            }
            if let Some(hit) = build(item) {
                if !hit.uri.is_empty() {
                    out.push(hit);
                }
            }
        }
    }
    out.truncate(limit);
    Ok(out)
}

/// One search request against the account's session.
pub fn search<T: Transport>(
    http: &T,
    api: &str,
    token: &str,
    query: &str,
) -> Result<Vec<Hit>, String> {
    if query.trim().is_empty() {
        return Ok(Vec::new());
    }
    let response = http.get(&search_url(api, query), token)?;
    if !(200..300).contains(&response.status) {
        return Err(crate::spotify::response_error(&response));
    }
    parse_search(&response.body, 20)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spotify::HttpResponse;
    use std::cell::RefCell;

    /// A recorded `/v1/search` payload: the shape Spotify really returns,
    /// trimmed to a few items per section.
    pub const RECORDED: &str = r#"{
      "tracks": { "items": [
        {
          "id": "0VjIjW4GlUZAMYd2vXMi3b",
          "uri": "spotify:track:0VjIjW4GlUZAMYd2vXMi3b",
          "name": "Blinding Lights",
          "duration_ms": 200040,
          "artists": [{"name": "The Weeknd"}],
          "album": {
            "name": "After Hours",
            "images": [
              {"url": "https://i.scdn.co/image/ab67616d000048518863bc11d2aa12b54f5aeb36", "width": 64, "height": 64},
              {"url": "https://i.scdn.co/image/ab67616d00001e028863bc11d2aa12b54f5aeb36", "width": 300, "height": 300},
              {"url": "https://i.scdn.co/image/ab67616d0000b2738863bc11d2aa12b54f5aeb36", "width": 640, "height": 640}
            ]
          }
        },
        {
          "id": "2Fxmhks0bxGSBdJ92vM42m",
          "uri": "spotify:track:2Fxmhks0bxGSBdJ92vM42m",
          "name": "bad guy",
          "duration_ms": 194088,
          "artists": [{"name": "Billie Eilish"}, {"name": "Justin Bieber"}],
          "album": {"name": "WHEN WE ALL FALL ASLEEP", "images": []}
        },
        null
      ]},
      "albums": { "items": [
        {
          "id": "4yP0hdKOZPNshxUOjY0cZj",
          "uri": "spotify:album:4yP0hdKOZPNshxUOjY0cZj",
          "name": "After Hours",
          "artists": [{"name": "The Weeknd"}],
          "images": [{"url": "https://i.scdn.co/image/ab67616d0000b2738863bc11d2aa12b54f5aeb36", "width": 640, "height": 640}]
        }
      ]},
      "artists": { "items": [
        {
          "id": "1Xyo4u8uXC1ZmMpatF05PJ",
          "uri": "spotify:artist:1Xyo4u8uXC1ZmMpatF05PJ",
          "name": "The Weeknd",
          "genres": ["canadian pop", "pop", "r&b"],
          "images": [{"url": "https://i.scdn.co/image/ab6761610000e5eb9e5be0fd6a2c5e7f6f8f6a1d", "width": 320, "height": 320}]
        }
      ]}
    }"#;

    #[test]
    fn recorded_response_becomes_playable_hits_with_real_covers() {
        let hits = parse_search(RECORDED, 20).unwrap();
        assert_eq!(hits.len(), 4, "two tracks, one album, one artist");
        assert_eq!(hits[0].kind, HitKind::Track);
        assert_eq!(hits[0].title, "Blinding Lights");
        assert_eq!(hits[0].artist, "The Weeknd");
        assert_eq!(hits[0].album, "After Hours");
        assert_eq!(hits[0].secs, 200);
        assert_eq!(hits[0].uri, "spotify:track:0VjIjW4GlUZAMYd2vXMi3b");
        assert_eq!(
            hits[0].art.pick(64),
            Some("https://i.scdn.co/image/ab67616d000048518863bc11d2aa12b54f5aeb36")
        );
        assert_eq!(
            hits[0].art.pick(300),
            Some("https://i.scdn.co/image/ab67616d00001e028863bc11d2aa12b54f5aeb36")
        );
        // Several artists keep their billing order.
        assert_eq!(hits[1].artist, "Billie Eilish, Justin Bieber");
        assert!(hits[1].art.is_empty(), "no images means the gradient placeholder");
        // The null item is skipped, not rendered as an empty row.
        assert_eq!(hits[2].kind, HitKind::Album);
        assert_eq!(hits[3].kind, HitKind::Artist);
        assert_eq!(hits[3].artist, "canadian pop, pop");
    }

    #[test]
    fn broken_json_is_an_error_the_caller_can_fall_back_from() {
        assert!(parse_search("<html>gateway timeout</html>", 20).is_err());
        // A well-formed but empty page is not an error: it is zero results.
        assert!(parse_search("{}", 20).unwrap().is_empty());
    }

    #[test]
    fn the_query_is_percent_encoded() {
        assert_eq!(encode_query("blinding lights"), "blinding+lights");
        assert_eq!(encode_query("AC/DC & co."), "AC%2FDC+%26+co.");
        let url = search_url("https://api.spotify.com/v1", "bad guy");
        assert!(url.starts_with("https://api.spotify.com/v1/search?q=bad+guy"));
        assert!(url.contains("type=track,album,artist"));
    }

    #[test]
    fn search_sends_one_bearer_request_to_the_search_endpoint() {
        struct Fake {
            seen: RefCell<Vec<(String, String)>>,
            status: u16,
            body: String,
        }
        impl Transport for Fake {
            fn get(&self, url: &str, token: &str) -> Result<HttpResponse, String> {
                self.seen
                    .borrow_mut()
                    .push((url.to_string(), token.to_string()));
                Ok(HttpResponse {
                    status: self.status,
                    body: self.body.clone(),
                })
            }
            fn post_form(
                &self,
                _url: &str,
                _form: &[(&str, &str)],
            ) -> Result<HttpResponse, String> {
                unreachable!("search never posts a form")
            }
            fn send_json(
                &self,
                _m: &str,
                _u: &str,
                _t: &str,
                _b: Option<&str>,
            ) -> Result<HttpResponse, String> {
                unreachable!("search is a GET")
            }
        }
        let api = "https://api.spotify.com/v1";
        let fake = Fake {
            seen: RefCell::new(Vec::new()),
            status: 200,
            body: RECORDED.to_string(),
        };
        let hits = search(&fake, api, "tok", "the weeknd").unwrap();
        assert_eq!(hits.len(), 4);
        let seen = fake.seen.borrow();
        assert_eq!(seen.len(), 1, "one request per search");
        assert_eq!(
            seen[0].0,
            "https://api.spotify.com/v1/search?q=the+weeknd&type=track,album,artist&limit=8&market=from_token"
        );
        assert_eq!(seen[0].1, "tok");

        // An API error is reported, so the UI can fall back to local search.
        let failing = Fake {
            seen: RefCell::new(Vec::new()),
            status: 401,
            body: r#"{"error":{"status":401,"message":"The access token expired"}}"#.to_string(),
        };
        let err = search(&failing, api, "tok", "x").unwrap_err();
        assert!(err.contains("expired"), "{err}");

        // An empty query never leaves the machine.
        let quiet = Fake {
            seen: RefCell::new(Vec::new()),
            status: 200,
            body: "{}".to_string(),
        };
        assert!(search(&quiet, api, "tok", "   ").unwrap().is_empty());
        assert!(quiet.seen.borrow().is_empty());
    }
}
