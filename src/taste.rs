// The personalization engine: everything that turns recorded plays into
// rankings, a personality line, generated mixes and recommendations. Kept free
// of UI types so `cargo test` can exercise it headlessly.
use std::collections::{BTreeMap, BTreeSet};

use crate::library::{self, Collection, Kind, Track};
use crate::profile::Profile;
use crate::theme::rgb;

fn ranked(map: &BTreeMap<String, u32>) -> Vec<(String, u32)> {
    let mut v: Vec<(String, u32)> = map.iter().map(|(k, c)| (k.clone(), *c)).collect();
    v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    v
}

fn merge(played: &[(String, u32)], imported: &[(String, u32)]) -> Vec<(String, u32)> {
    let mut m: BTreeMap<String, u32> = BTreeMap::new();
    for (k, v) in played.iter().chain(imported.iter()) {
        *m.entry(k.clone()).or_insert(0) += *v;
    }
    ranked(&m)
}

/// One imported track replayed through the real profile recorder. It proves the
/// sample and the UI read the same recording path.
#[derive(Clone, Debug)]
pub struct TasteSample {
    pub playlist: String,
    pub title: String,
    pub artist: String,
    pub genres: Vec<String>,
    pub artist_plays: u32,
    pub genre_plays: u32,
}

/// Every personalization number the app shows, built in one place from real
/// plays plus the inferred genres of imported playlists. The taste page, home
/// shelves, mixes, reports and checks all read this snapshot instead of
/// recomputing from the raw profile.
#[derive(Clone, Debug, Default)]
pub struct TasteSnapshot {
    pub plays: u32,
    pub liked: BTreeSet<String>,
    pub played_genres: Vec<(String, u32)>,
    pub imported_genres: Vec<(String, u32)>,
    pub genres: Vec<(String, u32)>,
    pub played_artists: Vec<(String, u32)>,
    pub imported_artists: Vec<(String, u32)>,
    pub artists: Vec<(String, u32)>,
    pub imported_playlists: usize,
    pub imported_tracks: usize,
    pub sample: Option<TasteSample>,
}

impl TasteSnapshot {
    pub fn build(p: &Profile, lib: &[Collection]) -> Self {
        let played_genres = ranked(&p.genre_plays);
        let played_artists = ranked(&p.artist_plays);

        let imported: Vec<&Collection> = lib.iter().filter(|c| c.imported()).collect();
        let imported_playlists = imported.len();
        let imported_tracks: usize = imported.iter().map(|c| c.tracks.len()).sum();

        let mut genre_counts: BTreeMap<String, u32> = BTreeMap::new();
        let mut artist_counts: BTreeMap<String, u32> = BTreeMap::new();
        for c in &imported {
            for t in &c.tracks {
                *artist_counts.entry(t.artist.clone()).or_insert(0) += 1;
                for g in &c.genres {
                    if *g != "mix" {
                        *genre_counts.entry((*g).to_string()).or_insert(0) += 1;
                    }
                }
            }
        }
        let imported_genres = ranked(&genre_counts);
        let imported_artists = ranked(&artist_counts);

        // Replay one imported track through the real recorder, so the sample
        // shown by reports is proof that playback feeds this same profile path.
        let mut probe = p.clone();
        let sample = imported
            .iter()
            .find(|c| !c.tracks.is_empty())
            .map(|c| {
                let t = &c.tracks[0];
                let genres: Vec<String> = c.genres.iter().map(|g| g.to_string()).collect();
                probe.record_play(&t.artist, &t.key(), &genres);
                TasteSample {
                    playlist: c.name.clone(),
                    title: t.title.clone(),
                    artist: t.artist.clone(),
                    artist_plays: *probe.artist_plays.get(&t.artist).unwrap_or(&0),
                    genre_plays: genres
                        .iter()
                        .map(|g| *probe.genre_plays.get(g).unwrap_or(&0))
                        .sum(),
                    genres,
                }
            });

        Self {
            plays: p.plays,
            liked: p.liked.clone(),
            genres: merge(&played_genres, &imported_genres),
            artists: merge(&played_artists, &imported_artists),
            played_genres,
            imported_genres,
            played_artists,
            imported_artists,
            imported_playlists,
            imported_tracks,
            sample,
        }
    }

    pub fn personality(&self) -> String {
        personality_for(self.top_genre())
    }

    /// The genre this listener actually plays, not the genre of everything
    /// sitting in the library. Importing hundreds of playlists must not make
    /// the app claim a taste the person never listened to, so this only reads
    /// recorded plays and returns `None` until there is something real to say.
    pub fn top_genre(&self) -> Option<&str> {
        self.played_genres.first().map(|(g, _)| g.as_str())
    }

    pub fn genre_weight(&self, g: &str) -> u32 {
        self.genres
            .iter()
            .find(|(name, _)| name == g)
            .map(|(_, v)| *v)
            .unwrap_or(0)
    }

    pub fn artist_weight(&self, a: &str) -> u32 {
        self.artists
            .iter()
            .find(|(name, _)| name == a)
            .map(|(_, v)| *v)
            .unwrap_or(0)
    }
}

fn personality_for(top: Option<&str>) -> String {
    match top {
        Some(g) => match g {
            "pop" => "Chart chaser - hooks first, always".to_string(),
            "hiphop" => "Bar hunter - you listen to the verses".to_string(),
            "rock" => "Amp up - guitars and big rooms".to_string(),
            "rnb" => "Smooth operator - late night, soft light".to_string(),
            "dance" => "Floor filler - built for the drop".to_string(),
            "synthwave" => "Neon dreamer - chrome, arpeggios and 2am drives".to_string(),
            "electronic" => "Circuit chaser - you like your kicks squared".to_string(),
            "indie" => "Bedroom romantic - guitars with a little dust".to_string(),
            "chill" => "Slow burner - low volume, long evenings".to_string(),
            "lofi" => "Study companion - tape hiss is your white noise".to_string(),
            "study" => "Deep worker - music as background architecture".to_string(),
            "folk" => "Acoustic heart - warm strings, honest words".to_string(),
            "ambient" => "Texture seeker - you listen to the room".to_string(),
            "mix" => "Open ears - a bit of everything".to_string(),
            _ => "Still exploring".to_string(),
        },
        None => "Open ears - play something and this page will start learning".to_string(),
    }
}

pub fn score(s: &TasteSnapshot, c: &Collection) -> u32 {
    let genre: u32 = c.genres.iter().map(|g| s.genre_weight(g)).sum();
    let artist: u32 = c.tracks.iter().map(|t| s.artist_weight(&t.artist)).sum();
    let liked = c.tracks.iter().filter(|t| s.liked.contains(&t.key())).count() as u32;
    genre * 3 + artist * 2 + liked * 5
}

pub fn recommended(s: &TasteSnapshot, lib: &[Collection]) -> Vec<usize> {
    let mut scored: Vec<(usize, u32)> = lib.iter().enumerate().map(|(i, c)| (i, score(s, c))).collect();
    scored.sort_by(|a, b| b.1.cmp(&a.1));
    scored.into_iter().map(|(i, _)| i).collect()
}

pub fn track_by_key<'a>(lib: &'a [Collection], key: &str) -> Option<&'a Track> {
    lib.iter()
        .flat_map(|c| c.tracks.iter())
        .find(|t| t.key() == key)
}

pub fn coll_for_key(lib: &[Collection], key: &str) -> Option<usize> {
    lib.iter()
        .position(|c| c.tracks.iter().any(|t| t.key() == key))
}

/// Genre tags of the catalogue entries that contain `track`, primary tag first.
/// Generated views (Liked Songs, mixes, genres) have no tags of their own, so
/// this is how a play inside them is attributed.
/// Genres for artists that turn up in imported playlists but are not in the
/// built-in catalogue. Every value stays inside `library::GENRES`.
const ARTIST_HINTS: &[(&str, &[&str])] = &[
    ("toby fox", &["electronic"]),
    ("c418", &["electronic", "chill"]),
    ("lena raine", &["electronic"]),
    ("shirobon", &["electronic"]),
    ("kordhell", &["electronic", "hiphop"]),
    ("crim3s", &["electronic"]),
    ("dyan dxddy", &["electronic"]),
    ("superpotato", &["electronic"]),
    ("thefatrat", &["electronic", "dance"]),
    ("tobu", &["electronic", "dance"]),
    ("itro", &["electronic", "dance"]),
    ("deaf kev", &["electronic", "dance"]),
    ("omfg", &["electronic", "dance"]),
    ("different heaven", &["electronic", "dance"]),
    ("jim yosef", &["electronic", "dance"]),
    ("imagine dragons", &["rock", "pop"]),
    ("boywithuke", &["pop", "indie"]),
    ("gigi perez", &["indie"]),
    ("vance joy", &["indie"]),
    ("mxmtoon", &["indie"]),
    ("blackbear", &["pop", "rnb"]),
    ("hisohkah", &["lofi"]),
    ("my head is empty", &["lofi"]),
    ("405okced", &["chill"]),
    ("leon riskin", &["chill"]),
    ("will stetson", &["pop"]),
];

/// Title/artist words that point at a genre when the artist is unknown.
const TEXT_HINTS: &[(&str, &[&str])] = &[
    ("lofi", &["lofi"]),
    ("lo-fi", &["lofi"]),
    ("slowed", &["lofi"]),
    ("ambien", &["chill"]),
    ("chill", &["chill"]),
    ("calm", &["chill"]),
    ("lullaby", &["chill"]),
    ("remix", &["electronic"]),
    ("chiptune", &["electronic"]),
    ("8-bit", &["electronic"]),
    ("undertale", &["electronic"]),
    ("minecraft", &["electronic"]),
    ("fnaf", &["electronic", "chill"]),
    ("phonk", &["hiphop"]),
    ("hip hop", &["hiphop"]),
    ("hip-hop", &["hiphop"]),
    ("metal", &["rock"]),
    ("rock", &["rock"]),
    ("guitar", &["rock"]),
    ("edm", &["electronic", "dance"]),
    ("dance", &["dance"]),
    ("r&b", &["rnb"]),
    ("soul", &["rnb"]),
    ("synth", &["synthwave"]),
    ("indie", &["indie"]),
];

fn add_genres(found: &mut Vec<&'static str>, genres: &[&'static str]) {
    for g in genres {
        if !found.contains(g) {
            found.push(g);
        }
    }
}

/// Imported playlists arrive without tags. Give each one real genres: catalogue
/// matches first, then artist/title hints, and only "mix" when nothing at all
/// is known about its tracks.
pub fn assign_imported_genres(lib: &mut [Collection]) {
    use std::collections::BTreeMap;
    const MAX_GENRES: usize = 3;
    let mut by_key: BTreeMap<String, Vec<&'static str>> = BTreeMap::new();
    let mut by_artist: BTreeMap<String, Vec<&'static str>> = BTreeMap::new();
    for c in lib
        .iter()
        .filter(|c| !c.imported() && c.genres.iter().any(|g| *g != "mix"))
    {
        let genres: Vec<&'static str> = c.genres.iter().copied().filter(|g| *g != "mix").collect();
        for t in &c.tracks {
            by_key
                .entry(t.key())
                .or_insert_with(|| genres.clone());
            by_artist
                .entry(t.artist.clone())
                .or_insert_with(|| genres.clone());
        }
    }
    for c in lib.iter_mut().filter(|c| c.imported() && c.genres.is_empty()) {
        let mut found: Vec<&'static str> = Vec::new();
        for t in &c.tracks {
            match by_key.get(&t.key()).or_else(|| by_artist.get(&t.artist)) {
                Some(genres) => add_genres(&mut found, genres),
                None => {
                    let artist = t.artist.to_lowercase();
                    if let Some((_, genres)) = ARTIST_HINTS
                        .iter()
                        .find(|(needle, _)| artist.contains(*needle))
                    {
                        add_genres(&mut found, *genres);
                    }
                    let text = format!("{} {}", t.title, t.artist).to_lowercase();
                    for &(needle, genres) in TEXT_HINTS {
                        if text.contains(needle) {
                            add_genres(&mut found, genres);
                        }
                    }
                }
            }
        }
        found.truncate(MAX_GENRES);
        if found.is_empty() {
            found.push("mix");
        }
        c.genres = found;
    }
}

pub fn genres_for(lib: &[Collection], track: &Track) -> Vec<String> {
    let key = track.key();
    let mut out: Vec<String> = Vec::new();
    for c in lib {
        if c.tracks.iter().any(|t| t.key() == key) {
            for g in &c.genres {
                if !out.iter().any(|x| x == g) {
                    out.push((*g).to_string());
                }
            }
        }
    }
    // A track that also sits in Liked Songs must not carry that playlist's
    // catch-all "mix" tag when real genre tags exist elsewhere.
    if out.iter().any(|g| g != "mix") {
        out.retain(|g| g != "mix");
    }
    out
}

pub fn recent_collections(p: &Profile, lib: &[Collection], n: usize) -> Vec<usize> {
    let mut seen = BTreeSet::new();
    p.recent
        .iter()
        .filter_map(|k| coll_for_key(lib, k))
        .filter(|i| seen.insert(*i))
        .take(n)
        .collect()
}

fn push_unique(tracks: &mut Vec<Track>, seen: &mut BTreeSet<String>, t: &Track) {
    if seen.insert(t.key()) {
        tracks.push(t.clone());
    }
}

pub fn liked_collection(p: &Profile, lib: &[Collection]) -> Collection {
    let mut tracks = Vec::new();
    let mut seen = BTreeSet::new();
    for c in lib {
        for t in &c.tracks {
            if p.liked.contains(&t.key()) {
                push_unique(&mut tracks, &mut seen, t);
            }
        }
    }
    let n = tracks.len();
    Collection::new(
        "Liked Songs",
        Kind::Liked,
        format!("{n} liked songs"),
        rgb(0x5038a0),
        rgb(0xcbb6ff),
        vec!["mix"],
        tracks,
    )
}

pub fn mix(s: &TasteSnapshot, lib: &[Collection], n: usize) -> Collection {
    let (c1, c2) = [(0x1e5b8f, 0x7fd8e8), (0x5a1f6e, 0xff7ad9), (0x2f4f3f, 0xa8d8b9)][n % 3];
    let title = format!("Daily Mix {}", n + 1);

    if lib.is_empty() {
        return Collection::new(
            &title,
            Kind::Playlist,
            "No tracks".to_string(),
            rgb(c1),
            rgb(c2),
            vec!["mix"],
            Vec::new(),
        );
    }

    let artists = &s.artists;
    let genres = &s.genres;
    let mut tracks: Vec<Track> = Vec::new();
    let mut seen = BTreeSet::new();
    let mut subtitle = String::from("Starts learning as soon as you play something");

    let mut chosen_genre = None;
    if let Some((artist, _)) = artists.get(n % artists.len().max(1)) {
        for c in lib {
            for t in &c.tracks {
                if &t.artist == artist {
                    push_unique(&mut tracks, &mut seen, t);
                }
            }
        }
        if !tracks.is_empty() {
            subtitle = format!("Because you like {artist}");
            // Keep the lean coherent: the genre comes from that artist's own
            // tagged collection, imported or catalogue.
            let mut best: Option<(&Collection, usize)> = None;
            for c in lib {
                let hits = c.tracks.iter().filter(|t| &t.artist == artist).count();
                if hits > 0
                    && c.genres.iter().any(|g| *g != "mix")
                    && best.map_or(true, |(_, most)| hits > most)
                {
                    best = Some((c, hits));
                }
            }
            chosen_genre = best
                .and_then(|(c, _)| c.genres.iter().find(|g| **g != "mix").copied())
                .map(|g| g.to_string());
        }
    }
    if chosen_genre.is_none() {
        chosen_genre = genres
            .get(n % genres.len().max(1))
            .map(|(g, _)| g.clone());
    }

    if let Some(genre) = chosen_genre {
        if tracks.len() < 12 {
            for c in lib {
                if c.genres.iter().any(|g| *g == genre.as_str()) {
                    for t in &c.tracks {
                        if tracks.len() < 14 {
                            push_unique(&mut tracks, &mut seen, t);
                        }
                    }
                }
            }
        }
        if tracks.len() > 4 {
            subtitle = format!("{subtitle}, leaning {genre}");
        }
    }

    let mut idx = n;
    let mut guard = 0;
    while tracks.len() < 10 && guard < lib.len() * 2 {
        let c = &lib[idx % lib.len()];
        for t in &c.tracks {
            if tracks.len() < 10 {
                push_unique(&mut tracks, &mut seen, t);
            }
        }
        idx += 1;
        guard += 1;
    }

    Collection::new(
        &title,
        Kind::Playlist,
        subtitle,
        rgb(c1),
        rgb(c2),
        vec!["mix"],
        tracks,
    )
}

pub fn genre_collection(lib: &[Collection], genre: &'static str) -> Collection {
    let mut tracks = Vec::new();
    let mut seen = BTreeSet::new();
    for c in lib {
        if c.genres.iter().any(|g| *g == genre) {
            for t in &c.tracks {
                push_unique(&mut tracks, &mut seen, t);
            }
        }
    }
    let (c1, c2) = library::genre_colors(genre);
    let n = tracks.len();
    Collection::new(
        genre,
        Kind::Playlist,
        format!("{n} tracks tagged {genre}"),
        c1,
        c2,
        vec![genre],
        tracks,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn temp_path(tag: &str) -> PathBuf {
        static N: AtomicUsize = AtomicUsize::new(0);
        let n = N.fetch_add(1, Ordering::SeqCst);
        std::env::temp_dir().join(format!(
            "spotify-rust-{}-{}-{}.txt",
            tag,
            std::process::id(),
            n
        ))
    }

    fn play(p: &mut Profile, lib: &[Collection], ci: usize, ti: usize, reps: u32) {
        let coll = &lib[ci];
        let track = &coll.tracks[ti];
        let genres: Vec<String> = coll.genres.iter().map(|g| g.to_string()).collect();
        for _ in 0..reps {
            p.record_play(&track.artist, &track.key(), &genres);
            p.listened_secs += track.secs as f64;
        }
    }

    fn keys(c: &Collection) -> Vec<String> {
        c.tracks.iter().map(|t| t.key()).collect()
    }

    fn imported(name: &str, list: &[(&str, &str)]) -> Collection {
        Collection {
            name: name.to_string(),
            kind: Kind::Playlist,
            subtitle: "Imported from Spotify - test".to_string(),
            c1: rgb(0x222222),
            c2: rgb(0x333333),
            genres: Vec::new(),
            art: crate::library::Art::default(),
            tracks: list
                .iter()
                .map(|(title, artist)| Track {
                    title: title.to_string(),
                    artist: artist.to_string(),
                    album: String::new(),
                    secs: 180,
                    c1: rgb(0x222222),
                    c2: rgb(0x333333),
                    added: String::new(),
                    art: crate::library::Art::default(),
                    path: None,
                })
                .collect(),
        }
    }

    #[test]
    fn imported_genres_are_real_with_mix_only_as_last_resort() {
        let mut lib = library::sample();
        lib.push(imported(
            "Imported Dance",
            &[("Cloud 9", "Tobu"), ("Time Lapse", "TheFatRat")],
        ));
        lib.push(imported("Imported Mystery", &[("Untitled", "Nobody At All")]));
        lib.push(imported(
            "Imported Catalogue",
            &[("Giorgio by Moroder", "Daft Punk")],
        ));
        lib.push(imported(
            "Imported Game",
            &[("Undertale Theme", "Unknown Composer")],
        ));
        assign_imported_genres(&mut lib);

        let by_name = |n: &str| lib.iter().find(|c| c.name == n).unwrap();

        let dance = by_name("Imported Dance");
        assert!(dance.genres.contains(&"electronic"));
        assert!(dance.genres.contains(&"dance"));
        assert!(!dance.genres.contains(&"mix"));

        let catalogue = by_name("Imported Catalogue");
        assert!(catalogue.genres.contains(&"electronic"));
        assert!(catalogue.genres.contains(&"dance"));

        let game = by_name("Imported Game");
        assert_eq!(game.genres, vec!["electronic"]);

        let mystery = by_name("Imported Mystery");
        assert_eq!(mystery.genres, vec!["mix"]);

        // A play from an imported track now records the real genre, not mix.
        let mut p = Profile::default();
        let t = &dance.tracks[0];
        p.record_play(&t.artist, &t.key(), &dance.genres.iter().map(|g| g.to_string()).collect::<Vec<_>>());
        assert!(p.genre_plays.keys().any(|g| g != "mix"));
        assert_eq!(p.artist_plays.get("Tobu"), Some(&1));
    }

    #[test]
    fn snapshot_is_the_single_source_for_imports_and_plays() {
        let mut lib = library::sample();
        lib.push(imported(
            "Imported Dance",
            &[("Cloud 9", "Tobu"), ("Time Lapse", "TheFatRat")],
        ));
        assign_imported_genres(&mut lib);

        let mut p = Profile::default();
        p.record_play("Queen", "Queen|Bohemian Rhapsody", &["rock".to_string()]);

        let s = TasteSnapshot::build(&p, &lib);
        assert_eq!(s.plays, 1);
        assert_eq!(s.played_genres[0].0, "rock");
        assert_eq!(
            s.imported_genres
                .iter()
                .find(|(g, _)| g == "electronic")
                .map(|(_, n)| *n),
            Some(2)
        );
        assert!(s.genres.iter().any(|(g, _)| g == "rock"));
        assert!(s.genre_weight("electronic") >= 2);
        assert_eq!(
            s.imported_artists
                .iter()
                .find(|(a, _)| a == "Tobu")
                .map(|(_, n)| *n),
            Some(1)
        );
        let sample = s.sample.as_ref().expect("imported sample");
        assert_eq!(sample.artist, "Tobu");
        assert!(sample.genre_plays > 0, "sample must record its genre");

        // Mixes, recommendations and the personality line all read the same snapshot.
        let m = mix(&s, &lib, 0);
        assert!(!m.tracks.is_empty());
        assert!(m.subtitle.contains(&s.artists[0].0));
        assert!(m.subtitle.contains("leaning rock"));
        assert_eq!(recommended(&s, &lib).len(), lib.len());
        assert_eq!(s.personality(), personality_for(s.top_genre()));

        // With only unknown imports there is no real genre to show and "mix"
        // stays the honest fallback for the sample.
        let mut unknown = library::sample();
        unknown.push(imported("Imported Mystery", &[("Untitled", "Nobody At All")]));
        assign_imported_genres(&mut unknown);
        let s = TasteSnapshot::build(&Profile::default(), &unknown);
        assert!(s.imported_genres.is_empty());
        assert_eq!(s.sample.as_ref().unwrap().genres, vec!["mix"]);
        assert!(s.personality().contains("Open ears"));
    }

    #[test]
    fn profile_round_trip_preserves_stats_rankings_and_mixes() {
        let lib = library::sample();
        let mut p = Profile::default();
        p.theme = 2;
        p.volume = 0.42;
        play(&mut p, &lib, 3, 0, 3); // synthwave / Nova Kite
        play(&mut p, &lib, 3, 1, 2); // synthwave / Aster Drive
        play(&mut p, &lib, 4, 0, 2); // lofi / Low Ceiling
        play(&mut p, &lib, 8, 0, 1); // folk / The Ferns
        play(&mut p, &lib, 2, 3, 1); // chill / Vela Grey
        p.liked.insert(lib[3].tracks[0].key());
        p.liked.insert(lib[4].tracks[1].key());

        let path = temp_path("roundtrip");
        p.save_to(&path);
        let loaded = Profile::load_from(&path);

        assert_eq!(p, loaded, "profile must survive save -> load unchanged");
        assert_eq!(p.plays, loaded.plays);
        assert_eq!(p.listened_secs, loaded.listened_secs);
        assert_eq!(p.liked, loaded.liked);
        assert_eq!(p.recent, loaded.recent);

        let sa = TasteSnapshot::build(&p, &lib);
        let sb = TasteSnapshot::build(&loaded, &lib);
        assert_eq!(sa.personality(), sb.personality());
        assert_eq!(sa.genres, sb.genres);
        assert_eq!(sa.artists, sb.artists);
        assert_eq!(recommended(&sa, &lib), recommended(&sb, &lib));

        for n in 0..3 {
            let a = mix(&sa, &lib, n);
            let b = mix(&sb, &lib, n);
            assert_eq!(a.name, b.name);
            assert_eq!(a.subtitle, b.subtitle);
            assert_eq!(keys(&a), keys(&b));
            assert!(!a.tracks.is_empty());
        }

        let la = liked_collection(&p, &lib);
        let lb = liked_collection(&loaded, &lib);
        assert_eq!(keys(&la), keys(&lb));
        assert_eq!(la.tracks.len(), 2);
        assert_eq!(la.kind, Kind::Liked);

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn recorded_plays_change_rankings_personality_and_mixes() {
        let lib = library::sample();

        let mut rock = Profile::default();
        play(&mut rock, &lib, 3, 0, 4); // Rock Classics: Bohemian Rhapsody, Queen

        let mut lofi = Profile::default();
        play(&mut lofi, &lib, 4, 0, 4); // Lo-Fi & Chill: Feather, Nujabes

        let rs = TasteSnapshot::build(&rock, &lib);
        let ls = TasteSnapshot::build(&lofi, &lib);

        assert_eq!(rs.top_genre(), Some("rock"));
        assert_eq!(ls.top_genre(), Some("lofi"));
        assert_ne!(rs.genres, ls.genres);

        assert!(rs.personality().contains("Amp up"));
        assert!(ls.personality().contains("Study companion"));
        assert_ne!(rs.personality(), ls.personality());

        assert_eq!(rs.artists[0].0, "Queen");
        assert_eq!(ls.artists[0].0, "Nujabes, Cise Starr, Akin");
        assert_ne!(rs.artists, ls.artists);

        let ms = mix(&rs, &lib, 0);
        let ml = mix(&ls, &lib, 0);
        assert_ne!(keys(&ms), keys(&ml));
        assert_eq!(ms.tracks[0].artist, "Queen");
        assert_eq!(ml.tracks[0].artist, "Nujabes, Cise Starr, Akin");
        assert!(ms.subtitle.contains("Queen"));
        assert!(ml.subtitle.contains("Nujabes"));

        // Recommended for you follows the same evidence.
        let rr = recommended(&rs, &lib);
        let rl = recommended(&ls, &lib);
        assert_ne!(rr, rl);
        assert!(lib[rr[0]].genres.iter().any(|g| *g == "rock"));
        assert!(lib[rl[0]]
            .genres
            .iter()
            .any(|g| *g == "lofi" || *g == "chill"));
    }

    #[test]
    fn rankings_follow_the_latest_evidence() {
        let lib = library::sample();
        let mut p = Profile::default();
        play(&mut p, &lib, 4, 0, 2); // lofi 2
        play(&mut p, &lib, 3, 0, 3); // rock 3
        let s = TasteSnapshot::build(&p, &lib);
        assert_eq!(s.top_genre(), Some("rock"));
        assert!(s.personality().contains("Amp up"));

        play(&mut p, &lib, 4, 1, 5); // now lofi 7
        let s = TasteSnapshot::build(&p, &lib);
        assert_eq!(s.top_genre(), Some("lofi"));
        assert!(s.personality().contains("Study companion"));
        assert_eq!(s.artists[0].0, "Nujabes");

        let m = mix(&s, &lib, 0);
        assert!(m.subtitle.contains("Nujabes"));
        assert_eq!(m.tracks[0].artist, "Nujabes");
    }

    #[test]
    fn likes_drive_liked_songs_and_boost_recommendations() {
        let lib = library::sample();
        let mut p = Profile::default();
        assert!(liked_collection(&p, &lib).tracks.is_empty());

        // A liked track in a collection nobody played should still lift it.
        let before = recommended(&TasteSnapshot::build(&p, &lib), &lib);
        let target = 7usize; // Chill Hits
        for t in lib[target].tracks.iter().take(2) {
            p.liked.insert(t.key());
        }
        let after = recommended(&TasteSnapshot::build(&p, &lib), &lib);
        assert_ne!(before, after);

        let liked = liked_collection(&p, &lib);
        assert_eq!(liked.tracks.len(), 2);
        let mut got = keys(&liked);
        got.sort();
        let mut want = vec![lib[target].tracks[0].key(), lib[target].tracks[1].key()];
        want.sort();
        assert_eq!(got, want);
        // Track-level genre lookup still attributes those likes to their real tags.
        assert!(genres_for(&lib, &lib[target].tracks[0]).contains(&"chill".to_string()));
        assert!(score(&TasteSnapshot::build(&p, &lib), &lib[target]) > 0);
    }
}
