# Rustify

A desktop music player for Windows, built in Rust with [egui](https://github.com/emilk/egui).

Rustify plays the audio files on your PC, and it can hand playback over to the Spotify app you
are already signed into. Full songs only ever play through Spotify itself; anything Rustify plays
on its own is a file you own or an official 30-second preview.

## What it does

- **Local library** – scans Music, Downloads, Desktop and Documents for MP3, WAV, M4A, OGG and
  FLAC, or a folder you pick yourself.
- **Spotify previews** – a track that is not on your disk streams the official 30-second preview
  that Deezer and iTunes publish. No account needed, nothing is ripped or re-hosted.
- **Spotify playback control** – with a Spotify app of your own and Premium, Rustify can play,
  pause, skip, seek and change volume on the Spotify app running on your PC. Spotify plays the
  audio.
- **Playlist import** – copy the playlists you own or collaborate on with your own Spotify app,
  add any public playlist by link, or import an Exportify CSV.
- **Queue, search, likes and play counts** – plus a taste profile that builds itself from what you
  actually listen to.
- **Themes** – built-in palettes plus a `theme.toml` that reloads while the app is open.
- **Discord presence** – optional "Listening to …" card on your Discord profile.

## Requirements

| | |
|---|---|
| OS | Windows 10 or 11 (uses the `curl.exe` that ships with Windows, and the Win32 clipboard) |
| Toolchain | Rust 1.85 or newer (edition 2024) |
| Spotify | Any account for previews and imports; **Premium** for playback control |
| Optional | Discord desktop app for rich presence |

## Build and run

```sh
git clone <this repo>
cd spotify-rust
cargo build --release
./target/release/spotify-rust.exe
```

Release builds run without a console window. Debug builds keep the console so the headless checks
below can print, so use `cargo run` only while developing.

A note on the toolchain: `.cargo/config.toml` adds `vendor/mingw` to the linker path for the
`x86_64-pc-windows-gnu` target. It supplies the `bcryptprimitives` import library that some
MinGW Rust sysroots are missing. With the MSVC toolchain (`rustup default stable-msvc`) the file is
ignored and you can delete it.

## Connect your Spotify app

Spotify only lets a registered application use its Web API, so you need to create a (free) app
once and paste its Client ID into Rustify. There is no client secret involved; PKCE is used.

1. Open <https://developer.spotify.com/dashboard> and log in.
2. **Create app**. Any name and description will do (for example `Rustify`). For *Redirect URI* put
   exactly:

   ```
   http://127.0.0.1:8899/callback
   ```

   `localhost`, `https` or a trailing slash will not match — Spotify compares the string byte for
   byte.
3. Under *Which API/SDKs are you planning to use?* tick **Web API**. Save.
4. Open the app's **Basic Information** page and copy the **Client ID**.
5. In Rustify go to **Import from Spotify** and paste the Client ID into the *Client ID* field.
   It is stored in `%APPDATA%\spotify-rust\taste.txt` and is not a secret.
6. Click **Connect playback control**. Your browser opens Spotify's consent page; approve the two
   playback scopes and the tab will report "Rustify is connected".

If you ever see `redirect_uri: Not matching configuration`, the URI from step 2 is missing from
that app's Redirect URIs (or was added but not saved). Fix it in the dashboard and connect again.

Development-mode apps are limited to the account that owns them plus a few invited users, and that
owner needs an active Premium subscription. That is a Spotify restriction, not a Rustify one.

## Playing full songs

1. Make sure **Use Spotify playback** is ticked on the *Import from Spotify* page (it appears once
   the account is connected).
2. Open the Spotify desktop app on the same PC and start a song once, so the app has an active
   device.
3. Play from Rustify. The commands go to Spotify; the audio, the quality and the device stay
   Spotify's.

While that switch is off, or for any track that is not in your Spotify account, Rustify falls back
to local files and 30-second previews.

## Importing playlists

- **Copy playlists** – with the Client ID set, *Connect and copy playlists* pulls the playlists you
  own or collaborate on into your library. It is saved locally in
  `%APPDATA%\spotify-rust\imported-playlists.txt`.
- **Public link** – in Spotify use *Share → Copy link* on any playlist and paste the link. No app
  or login needed.
- **CSV** – export a playlist with [Exportify](https://exportify.net) and drop the CSV on the
  window (or paste its path).

Imports are snapshots. Re-import when the playlist changes.

## Local files

Rustify indexes MP3, WAV, M4A, OGG and FLAC from Music, Downloads, Desktop and Documents, or from a
single folder you choose. Everything is read locally; the app never uploads your files.

## Themes and Discord

- **Settings** lists the built-in themes. The current `theme.toml` is shown with its path; edit and
  save it while the app is open and the window repaints. Its location is
  `%APPDATA%\spotify-rust\theme.toml`.
- **Discord presence** takes an Application ID from an app you create at
  <https://discord.com/developers/applications> (name it Rustify). It only talks to the Discord
  client already running on your PC.

## Where your data lives

Everything is under `%APPDATA%\spotify-rust\`:

| File | Contents |
|---|---|
| `taste.txt` | settings: Client ID, theme choice, likes and play counts |
| `connect.bin` | the Spotify playback token from the consent flow |
| `imported-playlists.txt` | the last copied playlists |
| `previews\` | cached 30-second preview clips |
| `theme.toml` | theme overrides |

Deleting that folder resets the app; nothing is stored anywhere else.

## Troubleshooting

| Symptom | Cause |
|---|---|
| `redirect_uri: Not matching configuration` | the callback URL is not saved in the Spotify app's Redirect URIs |
| `Timed out waiting for Spotify sign-in` | the consent page was closed or never finished; click Connect again |
| `Open Spotify on this PC and start a song` | Spotify has no active device yet |
| `spotify-rust.exe` opens a console window | you are running the debug build; use `target/release` |
| Imports fail with a Client ID error | paste the Client ID first, then reconnect |

## Development

```sh
cargo test              # unit tests, no network or Spotify account needed
cargo build             # debug build with a console for diagnostics
```

`src/` is split by area: `main.rs` (UI and state), `spotify.rs` (OAuth, API, imports),
`connect.rs` (playback control), `preview.rs` (30-second clips), `library.rs`/`catalog.rs`
(indexing), `audio.rs` (playback), `theme.rs`/`themefile.rs`, `discord.rs`, `profile.rs`,
`taste.rs`, `art.rs`, `gate.rs`.

The binary also takes a few read-only report flags for checking a real library without opening the
window, for example `--library-report`, `--taste-report` and `--spotify-check`. They print to the
console, so run them from a terminal with the debug build.

## Legal

Rustify does not download, record or redistribute Spotify audio. Previews are the official clips
Deezer and iTunes publish for exactly this purpose. Full tracks are played by the Spotify client on
your own account, under Spotify's terms.
