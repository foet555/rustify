# Rustify

A desktop music player for Windows, built in Rust with [egui](https://github.com/emilk/egui).

Rustify plays the audio files on your PC, and it can play full songs from your Spotify account
inside its own window through a native Spotify Connect player. Spotify decrypts and streams the
audio; Rustify only drives it, so the Spotify desktop app does not have to be open. Anything
Rustify plays on its own is a file you own or an official 30-second preview.

## Download

[**Releases →**](https://github.com/foet555/rustify/releases/latest) has a ready-to-run Windows
build. Unzip `Rustify-win64.zip` anywhere and run `spotify-rust.exe`; settings are kept in
`%APPDATA%\spotify-rust`. Full-song playback needs Spotify Premium and a Spotify app Client ID of
your own — [Connect your Spotify app](#connect-your-spotify-app) walks through it.

## What it does

- **Local library** – scans Music, Downloads, Desktop and Documents for MP3, WAV, M4A, OGG and
  FLAC, or a folder you pick yourself.
- **Spotify previews** – a track that is not on your disk streams the official 30-second preview
  that Deezer and iTunes publish. No account needed, nothing is ripped or re-hosted.
- **Spotify playback** – with Premium, a native Spotify Connect player called **Rustify (native)**
  streams full songs inside Rustify with no desktop app or hidden browser running. Play, pause,
  skip, seek and volume all go through Spotify's Web API. The device picker can still send
  playback to your phone or to the Spotify desktop app when you prefer that.
- **Playlist import and editing** – copy the playlists you own or collaborate on with your own
  Spotify app, add any public playlist by link, or import an Exportify CSV. Hover a row in one of
  your own playlists and press the small x (or right-click and choose **Remove from playlist**) to
  remove that track from Spotify and from Rustify.
- **Ordered autoplay** – when a track ends, Rustify plays the next row from the same playlist or
  collection, even when you started in the middle of the list. It stops after the last track
  instead of wrapping around.
- **Tutorial** – **Settings → Tutorial** has a six-step quick start and a **Replay welcome** button.
- **Queue, search, likes and play counts** – plus a taste profile that builds itself from what you
  actually listen to.
- **Themes** – built-in palettes plus a `theme.toml` that reloads while the app is open.
- **Discord presence** – optional "Listening to …" card on your Discord profile.

## Requirements

| | |
|---|---|
| OS | Windows 10 or 11 (uses the `curl.exe` that ships with Windows, and the Win32 clipboard) |
| Toolchain | Rust 1.85 or newer (edition 2024) |
| Spotify | Any account for previews and imports; **Premium** for full-song playback |
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

`build.rs` copies `vendor/webview2/WebView2Loader.dll` next to the built exe. The binary still
links that loader on the GNU target, so keep the DLL beside the exe; without it the program exits
at startup with a missing-library error. Playback itself uses the native player, not WebView2.

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
6. Click **Connect playback control**. Your browser opens Spotify's consent page; approve the
   streaming, playback and playlist-edit scopes and the tab will report "Rustify is connected".

The playlist-edit scopes are new in 1.2.0. A session saved by an older build asked for fewer
scopes, so Rustify drops it and asks you to connect once more. That is expected after this
upgrade and only has to be done once.

If you ever see `redirect_uri: Not matching configuration`, the URI from step 2 is missing from
that app's Redirect URIs (or was added but not saved). Fix it in the dashboard and connect again.

Development-mode apps are limited to the account that owns them plus a few invited users, and that
owner needs an active Premium subscription. That is a Spotify restriction, not a Rustify one.

## Playing full songs

1. Make sure **Use Spotify playback** is ticked on the *Import from Spotify* page (it appears once
   the account is connected).
2. Play from Rustify. The native backend connects as a Spotify Connect device named
   **Rustify (native)** and the track starts there. The *Import from Spotify* page reports
   "In-app player ready" when the device is live.
3. To send playback somewhere else, open the device picker in the player bar (the monitor icon)
   and pick your phone or the desktop app. Your choice sticks until that device disappears; press
   play again to come back to Rustify's own player.

While the Spotify switch is off, or for any track that is not in your Spotify account, Rustify
falls back to local files and 30-second previews.

Full-song playback needs Spotify Premium. A Free account still gets previews, and the player
reports the account restriction as a normal error instead of closing the app.

## Importing playlists

- **Copy playlists** – with the Client ID set, *Connect and copy playlists* pulls the playlists you
  own or collaborate on into your library. It is saved locally in
  `%APPDATA%\spotify-rust\imported-playlists.txt`.
- **Public link** – in Spotify use *Share → Copy link* on any playlist and paste the link. No app
  or login needed.
- **CSV** – export a playlist with [Exportify](https://exportify.net) and drop the CSV on the
  window (or paste its path).

Imports are snapshots. Re-import when the playlist changes. Removing a song from a linked
playlist updates Spotify first; if Spotify refuses the edit (for example because the playlist is
not yours, or the new permission was not granted), Rustify leaves the local list alone and shows
the reason.

## Local files

Rustify indexes MP3, WAV, M4A, OGG and FLAC from Music, Downloads, Desktop and Documents, or from a
single folder you choose. Everything is read locally; the app never uploads your files.

## Themes and Discord

- **Settings** lists the built-in themes. The current `theme.toml` is shown with its path; edit and
  save it while the app is open and the window repaints. Its location is
  `%APPDATA%\spotify-rust\theme.toml`.
- **Tutorial** under **Settings → Tutorial** walks through importing music, connecting Spotify,
  playing a track, using the queue and removing a song. **Replay welcome** opens it again later.
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
| `In-app player: authentication error` | the saved session predates the current scopes; Disconnect, then Connect again |
| `In-app player: account error` | the account is not Premium, so Spotify refuses full-song playback |
| `Spotify playback: ...` in the console | the native player reported an error; the app stays open and previews still work |
| Removing a song says Spotify refused | the playlist is not yours, or Spotify did not grant the playlist-edit permission; reconnect and approve it |
| Playback lands on another device | that device was picked by hand; open the device picker and choose **Rustify (native)** |
| `spotify-rust.exe` opens a console window | you are running the debug build; use `target/release` |
| Imports fail with a Client ID error | paste the Client ID first, then reconnect |

## Development

```sh
cargo test              # unit tests, no network or Spotify account needed
cargo build             # debug build with a console for diagnostics
```

`src/` is split by area: `main.rs` (UI and state), `spotify.rs` (OAuth, API, imports),
`connect.rs` (playback control and playlist edits), `backend.rs` (the native librespot worker),
`preview.rs` (30-second clips), `library.rs`/`catalog.rs` (indexing), `audio.rs` (local playback),
`theme.rs`/`themefile.rs`, `discord.rs`, `profile.rs`, `taste.rs`, `art.rs`, `gate.rs`.

The binary also takes a few read-only report flags for checking a real library without opening the
window, for example `--library-report`, `--taste-report` and `--spotify-check`. They print to the
console, so run them from a terminal with the debug build.

## Legal

Rustify does not download, record or redistribute Spotify audio. Previews are the official clips
Deezer and iTunes publish for exactly this purpose. Full tracks are streamed by Spotify's native
client on your own account, under Spotify's terms.

## License

MIT — see [LICENSE](LICENSE). You may use, change and share Rustify, including commercially, as
long as the copyright notice stays with it.
