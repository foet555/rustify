// Catalogue built from real Spotify artists, albums and tracks, tagged with
// genres so the taste profile has something to learn from.
use eframe::egui::Color32;

use crate::theme::rgb;

/// Cover artwork as Spotify reports it: CDN URLs kept next to their widths, so
/// a 40px sidebar thumb asks for the 64px image instead of downloading a 640px
/// JPEG. Empty means "no artwork known", which is what makes the generated
/// gradient appear.
#[derive(Clone, Default, PartialEq, Debug)]
pub struct Art {
    pub images: Vec<(u32, String)>,
}

impl Art {
    pub fn new(images: Vec<(u32, String)>) -> Self {
        let mut images = images;
        images.sort_by_key(|(w, _)| *w);
        Self { images }
    }

    /// A single URL whose width is unknown (an older store row, a share link).
    pub fn of(url: &str) -> Self {
        let url = url.trim();
        if url.is_empty() {
            return Self::default();
        }
        Self::new(vec![(0, url.to_string())])
    }

    pub fn is_empty(&self) -> bool {
        self.images.is_empty()
    }

    /// The smallest image that still covers `want`, else the largest there is.
    pub fn pick(&self, want: u32) -> Option<&str> {
        let big_enough = self
            .images
            .iter()
            .filter(|(w, _)| *w >= want)
            .min_by_key(|(w, _)| *w);
        big_enough
            .or_else(|| self.images.iter().max_by_key(|(w, _)| *w))
            .map(|(_, url)| url.as_str())
    }

    pub fn largest(&self) -> Option<&str> {
        self.images
            .iter()
            .max_by_key(|(w, _)| *w)
            .map(|(_, url)| url.as_str())
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Playlist,
    Album,
    Liked,
}

impl Kind {
    pub fn label(&self) -> &'static str {
        match self {
            Kind::Album => "ALBUM",
            Kind::Liked => "LIKED SONGS",
            Kind::Playlist => "PLAYLIST",
        }
    }
}

#[derive(Clone)]
pub struct Track {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub secs: u32,
    pub c1: Color32,
    pub c2: Color32,
    /// The day this track entered the library as YYYY-MM-DD, when the source
    /// reports it. Empty means unknown, which is honest: Spotify only gives
    /// that date for playlists you own.
    pub added: String,
    /// Album cover, when Spotify has told us about one.
    pub art: Art,
    /// The file on this PC this row was built from, when it has one. Rows that
    /// came from a folder scan keep it so playback always opens exactly that
    /// file, even when two songs share a title.
    pub path: Option<std::path::PathBuf>,
}

impl Track {
    /// Stable identity used for likes, play counts and history.
    pub fn key(&self) -> String {
        format!("{}|{}", self.artist, self.title)
    }
}

#[derive(Clone)]
pub struct Collection {
    pub name: String,
    pub kind: Kind,
    pub subtitle: String,
    pub c1: Color32,
    pub c2: Color32,
    pub genres: Vec<&'static str>,
    pub tracks: Vec<Track>,
    /// Playlist or album cover, when known.
    pub art: Art,
}

impl Collection {
    /// True for playlists merged in from Spotify rather than the built-in catalogue.
    pub fn imported(&self) -> bool {
        self.subtitle.starts_with("Imported")
    }

    pub fn new(
        name: &str,
        kind: Kind,
        subtitle: String,
        c1: Color32,
        c2: Color32,
        genres: Vec<&'static str>,
        tracks: Vec<Track>,
    ) -> Self {
        Self {
            name: name.to_string(),
            kind,
            subtitle,
            c1,
            c2,
            genres,
            tracks,
            art: Art::default(),
        }
    }

    /// Stable identity for pinned collections: the kind is part of the key so
    /// an album and a playlist that share a name stay separate rows.
    pub fn key(&self) -> String {
        format!("{}|{}", self.kind.label().to_lowercase(), self.name)
    }

    pub fn total_secs(&self) -> u32 {
        self.tracks.iter().map(|t| t.secs).sum()
    }

    /// Running time on its own, for the places that already printed the song
    /// count and would otherwise say "41 songs, 41 songs".
    pub fn runtime(&self) -> String {
        let mins = self.total_secs() / 60;
        if mins == 0 {
            // Unknown (folder-scanned files report no duration).
            return String::new();
        }
        if mins >= 60 {
            format!("{} hr {} min", mins / 60, mins % 60)
        } else {
            format!("{mins} min")
        }
    }

}

pub const GENRES: [&str; 11] = [
    "pop",
    "hiphop",
    "rock",
    "electronic",
    "synthwave",
    "chill",
    "lofi",
    "rnb",
    "indie",
    "dance",
    "mix",
];

pub fn genre_colors(g: &str) -> (Color32, Color32) {
    let (a, b) = match g {
        "pop" => (0x6e1f5a, 0xff9ad9),
        "hiphop" => (0x3a2a1f, 0xe0b37a),
        "rock" => (0x4a1f1f, 0xff8a7a),
        "electronic" => (0x1e5b8f, 0x7fd8e8),
        "synthwave" => (0x5a1f6e, 0xff7ad9),
        "chill" => (0x2f4f3f, 0xa8d8b9),
        "lofi" => (0x4a3c2a, 0xd9b98a),
        "rnb" => (0x3a2352, 0xc0a6ff),
        "indie" => (0x2b3b2f, 0xbfe08a),
        "dance" => (0x1f3f4a, 0x9fd8e0),
        _ => (0x5038a0, 0xcbb6ff),
    };
    (rgb(a), rgb(b))
}

fn coll(
    name: &str,
    kind: Kind,
    album: &str,
    subtitle: &str,
    colors: (u32, u32),
    genres: &[&'static str],
    list: &[(&str, &str, u32)],
) -> Collection {
    let (col1, col2) = (rgb(colors.0), rgb(colors.1));
    let tracks = list
        .iter()
        .enumerate()
        .map(|(i, (title, artist, secs))| {
            let (a, b) = if i % 2 == 0 {
                (col1, col2)
            } else {
                (col2.gamma_multiply(0.85), col1)
            };
            Track {
                title: title.to_string(),
                artist: artist.to_string(),
                album: album.to_string(),
                secs: *secs,
                c1: a,
                c2: b,
                added: String::new(),
                art: Art::default(),
                path: None,
            }
        })
        .collect();

    Collection::new(
        name,
        kind,
        subtitle.to_string(),
        col1,
        col2,
        genres.to_vec(),
        tracks,
    )
}

pub fn sample() -> Vec<Collection> {
    vec![
        coll(
            "Liked Songs",
            Kind::Liked,
            "Liked Songs",
            "Everything you hit the heart on",
            (0x5038a0, 0xcbb6ff),
            &["mix"],
            &[
                ("Blinding Lights", "The Weeknd", 200),
                ("Espresso", "Sabrina Carpenter", 175),
                ("Not Like Us", "Kendrick Lamar", 274),
                ("Dreams", "Fleetwood Mac", 257),
                ("Nightcall", "Kavinsky", 258),
                ("Feather", "Nujabes, Cise Starr, Akin", 195),
                ("Pink + White", "Frank Ocean", 184),
                ("Midnight City", "M83", 244),
                ("Good Luck, Babe!", "Chappell Roan", 218),
                ("Money Trees", "Kendrick Lamar", 386),
                ("Space Song", "Beach House", 320),
                ("Get Lucky", "Daft Punk, Pharrell Williams, Nile Rodgers", 369),
            ],
        ),
        coll(
            "Pop Right Now",
            Kind::Playlist,
            "Pop Right Now",
            "The biggest hooks on repeat",
            (0x6e1f5a, 0xff9ad9),
            &["pop"],
            &[
                ("Espresso", "Sabrina Carpenter", 175),
                ("BIRDS OF A FEATHER", "Billie Eilish", 210),
                ("Good Luck, Babe!", "Chappell Roan", 218),
                ("Houdini", "Dua Lipa", 185),
                ("As It Was", "Harry Styles", 167),
                ("Anti-Hero", "Taylor Swift", 200),
                ("vampire", "Olivia Rodrigo", 219),
                ("we can't be friends (wait for your love)", "Ariana Grande", 228),
                ("Kill Bill", "SZA", 153),
                ("Blinding Lights", "The Weeknd", 200),
            ],
        ),
        coll(
            "Hip-Hop Heat",
            Kind::Playlist,
            "Hip-Hop Heat",
            "Rap's loudest right now",
            (0x3a2a1f, 0xe0b37a),
            &["hiphop"],
            &[
                ("Not Like Us", "Kendrick Lamar", 274),
                ("SICKO MODE", "Travis Scott", 312),
                ("God's Plan", "Drake", 198),
                ("No Role Modelz", "J. Cole", 292),
                ("redrum", "21 Savage", 271),
                ("Like That", "Future, Metro Boomin, Kendrick Lamar", 267),
                ("EARFQUAKE", "Tyler, The Creator", 190),
                ("Paint The Town Red", "Doja Cat", 231),
                ("Creepin'", "Metro Boomin, The Weeknd, 21 Savage", 221),
                ("Drip Too Hard", "Lil Baby, Gunna", 145),
            ],
        ),
        coll(
            "Rock Classics",
            Kind::Playlist,
            "Rock Classics",
            "Guitars that never get old",
            (0x4a1f1f, 0xff8a7a),
            &["rock"],
            &[
                ("Bohemian Rhapsody", "Queen", 354),
                ("Smells Like Teen Spirit", "Nirvana", 301),
                ("Stairway to Heaven", "Led Zeppelin", 482),
                ("Back In Black", "AC/DC", 255),
                ("Wish You Were Here", "Pink Floyd", 334),
                ("Paint It Black", "The Rolling Stones", 202),
                ("Do I Wanna Know?", "Arctic Monkeys", 272),
                ("Creep", "Radiohead", 238),
                ("Everlong", "Foo Fighters", 250),
                ("Dreams", "Fleetwood Mac", 257),
            ],
        ),
        coll(
            "Lo-Fi & Chill",
            Kind::Playlist,
            "Lo-Fi & Chill",
            "Dusty drums and slow motion",
            (0x4a3c2a, 0xd9b98a),
            &["lofi", "chill"],
            &[
                ("Feather", "Nujabes, Cise Starr, Akin", 195),
                ("Aruarian Dance", "Nujabes", 264),
                ("Time: The Donut of the Heart", "J Dilla", 100),
                ("Roygbiv", "Boards of Canada", 151),
                ("A Walk", "Tycho", 308),
                ("Kerala", "Bonobo", 235),
                ("Two Thousand and Seventeen", "Four Tet", 245),
                ("Soon It Will Be Cold Enough", "Emancipator", 240),
                ("Sunset Lover", "Petit Biscuit", 237),
                ("Intro", "The xx", 128),
            ],
        ),
        coll(
            "Synthwave Nights",
            Kind::Playlist,
            "Synthwave Nights",
            "Neon, chrome and analog drums",
            (0x5a1f6e, 0xff7ad9),
            &["synthwave", "electronic"],
            &[
                ("Nightcall", "Kavinsky", 258),
                ("A Real Hero", "College, Electric Youth", 267),
                ("Sunset", "The Midnight", 297),
                ("Vampires", "The Midnight", 322),
                ("Tech Noir", "Gunship", 331),
                ("Turbo Killer", "Carpenter Brut", 205),
                ("Midnight City", "M83", 244),
                ("On the Run", "Timecop1983", 237),
                ("Running in the Night", "FM-84, Ollie Wride", 253),
                ("Sentient", "Perturbator", 266),
            ],
        ),
        coll(
            "2010s Throwbacks",
            Kind::Playlist,
            "2010s Throwbacks",
            "The decade that refused to leave",
            (0x1e5b8f, 0x7fd8e8),
            &["pop", "dance"],
            &[
                ("Teenage Dream", "Katy Perry", 227),
                ("Bad Romance", "Lady Gaga", 294),
                ("We Found Love", "Rihanna, Calvin Harris", 215),
                ("Summer", "Calvin Harris", 223),
                ("Uptown Funk", "Mark Ronson, Bruno Mars", 270),
                ("Get Lucky", "Daft Punk, Pharrell Williams, Nile Rodgers", 369),
                ("Somebody That I Used to Know", "Gotye, Kimbra", 244),
                ("Pumped Up Kicks", "Foster the People", 239),
                ("Summertime Sadness", "Lana Del Rey", 265),
                ("What Makes You Beautiful", "One Direction", 199),
            ],
        ),
        coll(
            "Chill Hits",
            Kind::Playlist,
            "Chill Hits",
            "Soft edges, low volume",
            (0x2f4f3f, 0xa8d8b9),
            &["chill", "rnb", "indie"],
            &[
                ("Pink + White", "Frank Ocean", 184),
                ("Best Part", "Daniel Caesar, H.E.R.", 209),
                ("Good News", "Mac Miller", 320),
                ("Sunflower", "Rex Orange County", 177),
                ("Sofia", "Clairo", 188),
                ("Show Me How", "Men I Trust", 215),
                ("Apocalypse", "Cigarettes After Sex", 290),
                ("ocean eyes", "Billie Eilish", 200),
                ("Mystery of Love", "Sufjan Stevens", 248),
                ("Space Song", "Beach House", 320),
            ],
        ),
        coll(
            "Random Access Memories",
            Kind::Album,
            "Random Access Memories",
            "Daft Punk - 2013",
            (0x14243d, 0x5ad2f0),
            &["electronic", "dance"],
            &[
                ("Give Life Back to Music", "Daft Punk", 274),
                ("The Game of Love", "Daft Punk", 322),
                ("Giorgio by Moroder", "Daft Punk", 544),
                ("Instant Crush", "Daft Punk, Julian Casablancas", 337),
                ("Lose Yourself to Dance", "Daft Punk, Pharrell Williams", 353),
                ("Touch", "Daft Punk, Paul Williams", 498),
                ("Get Lucky", "Daft Punk, Pharrell Williams, Nile Rodgers", 369),
                ("Doin' It Right", "Daft Punk, Panda Bear", 251),
                ("Contact", "Daft Punk", 383),
            ],
        ),
        coll(
            "good kid, m.A.A.d city",
            Kind::Album,
            "good kid, m.A.A.d city",
            "Kendrick Lamar - 2012",
            (0x2b3b2f, 0xbfe08a),
            &["hiphop"],
            &[
                ("Sherane a.k.a Master of Ceremonies", "Kendrick Lamar", 274),
                ("Bitch, Don't Kill My Vibe", "Kendrick Lamar", 310),
                ("Backseat Freestyle", "Kendrick Lamar", 212),
                ("The Art of Peer Pressure", "Kendrick Lamar", 324),
                ("Money Trees", "Kendrick Lamar, Jay Rock", 386),
                ("Poetic Justice", "Kendrick Lamar, Drake", 300),
                ("good kid", "Kendrick Lamar", 214),
                ("m.A.A.d city", "Kendrick Lamar, MC Eiht", 350),
                ("Swimming Pools (Drank)", "Kendrick Lamar", 313),
                ("Sing About Me, I'm Dying of Thirst", "Kendrick Lamar", 723),
                ("Real", "Kendrick Lamar, Anna Wise", 263),
                ("Compton", "Kendrick Lamar, Dr. Dre", 248),
            ],
        ),
    ]
}
