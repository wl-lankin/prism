# Prism - Design

Date: 2026-10-20
Owner: Wolfgang Linz (https://wolfgang-linz.de)
Status: approved in brainstorming, pending plan

> One player, every color of sound.

## 1. Intent and why

Wolfgang listens to music all day while programming. Today that listening is spread over
four places: a browser tab for internet radio, a browser tab for his self-hosted Navidrome
server (~30,000+ tracks, all organized in albums), a browser tab for YouTube Music, and the
separate Apple Music app. There is no small, always-reachable player and no single place for
his favorites.

Prism is one desktop app for Windows and macOS that brings all of these sources together, sits
quietly next to the editor all day, and is controlled from a small mini player in the
bottom-right corner of the screen. It must feel premium and distinctive: CleanMyMac is the bar
for polish, in particular its big action button that sits half outside the window.

Success: Wolfgang closes those browser tabs and the Apple Music app for good, and the app is
something he enjoys looking at.

## 2. Goals, non-goals, anti-goals

### Goals (v1.0.0)
- Play local files in practically every audio format (see 5.2).
- Navidrome (Subsonic API) as a first-class source that stays fast with 30k+ tracks.
- Internet radio, with stations near the user as the default view plus worldwide search.
- YouTube Music, integrated as an embedded web view (see 5.6).
- Album covers everywhere they exist.
- One combined Favorites library across sources.
- "Flow": a one-click endless mix from favorites across sources.
- A main window plus a mini player in the bottom-right corner, both carrying the half-out
  Flow ring.
- Public GitHub repo with a polished README, release v1.0.0, and installers for Windows and
  macOS, each also published under a versionless file name.
- About dialog listing Wolfgang Linz with a link to https://wolfgang-linz.de.

### v1.1 (planned, not in v1.0.0)
- Apple Music via MusicKit (Wolfgang is getting an Apple Developer membership now).
- macOS signing and notarization with that membership.

### Non-goals
- Spotify (dropped by the owner "for now").
- Mobile apps and Linux builds.
- Tag editing, CD ripping, podcasts, DJ or mixing features, an equalizer, lyrics (all later
  ideas at most).
- Syncing Prism's own state between devices (Navidrome stars already carry across devices).

### Anti-goals (failure even if it "works")
- Feels like a web page in a frame: browser-like scrolling, flashes of white, default form
  controls, generic UI.
- Lags with the 30k-track Navidrome library: scrolling, search, or opening an album stutters.
- The mini player gets in the way of coding: steals focus, covers the editor, or pops up
  without being asked.
- Credentials leak: passwords in plain-text config, or a developer key built into the public
  app.
- A logo that looks like a generic "AI" icon. No sparkle glyphs anywhere (the owner rejected
  that on another app).

## 3. Constraints

- Platforms: Windows 11 and macOS (Intel + Apple Silicon, universal DMG).
- Repo: `wl-lankin/prism`, public, code and releases in the same repo. Local path
  `C:\Projects\private\Prism`, branch `main`. Commits use the repo-local identity
  `Wolfgang Linz <27097016+wl-lankin@users.noreply.github.com>`.
- First release is `v1.0.0`.
- Every release asset is also published without the version number, so the website can link
  to `https://github.com/wl-lankin/prism/releases/latest/download/<name>`:
  - `Prism-setup.exe` (Windows NSIS installer)
  - `Prism.dmg` (macOS universal)
- No Apple Developer key inside the app or repo (public repo means anyone could extract it).
- No hyphen-like long dashes (em dash, en dash) in any text, UI, docs or commits; use "-".
- No session links or AI attribution lines in commits, README, or release notes.
- The owner's Windows reports `prefers-reduced-motion: reduce`. Short feedback transitions
  (hover, fades, about 120 ms) must still run; reduced motion only switches off large motion.

## 4. Decided by the owner

- Name **Prism**, the "Prism" visual direction: neutral graphite glass and one spectral accent
  ring (red, amber, mint, blue). The mockup lives in `docs/superpowers/mockups/identity.html`.
- The big half-out button is **Flow** (option B): when idle it shows "FLOW" and one click
  starts an endless mix from favorites across all sources. While playing it becomes
  play/pause, with progress drawn around the ring.
- The mini player has the same Flow ring, half outside its left edge
  (`docs/superpowers/mockups/mini-v2.html`).
- The main window layout follows `docs/superpowers/mockups/layout.html`: a sidebar with
  You (Home, Favorites, Playlists) and Sources (Navidrome, YouTube Music, Radio near you,
  Local files; Apple Music joins in v1.1). Home mixes favorites from all sources, with a
  small colored source badge on each cover.
- Sources: local files, Navidrome, radio, YouTube Music in v1.0.0; Apple Music in v1.1;
  no Spotify.
- Ship v1.0.0 first, Apple Music in v1.1.
- A public GitHub repo with README and releases; versionless asset copies; About dialog with
  his name and site.
- Logo: Higgsfield may be used if needed, but credits are scarce. See 7.1.

## 5. The product

### 5.1 Source colors (used for sidebar dots and cover badges)
- Navidrome: mint `#4dffb2`
- Apple Music (v1.1): red `#ff4d6d`
- YouTube Music: amber `#ffc94d`
- Radio: blue `#4da6ff`
- Local files: neutral `#cccccc`

### 5.2 Local files and formats
- The user adds folders. Prism scans them and watches them for changes (add, remove, rename).
- Formats: at minimum MP3, AAC/M4A, ALAC, FLAC, WAV, AIFF, Ogg Vorbis, Opus, WMA, APE,
  WavPack, Musepack, TTA, DSD (DSF/DFF, played as PCM), MKA/WebM audio. Also a file dropped
  onto the window plays, even if it is outside the library.
- Tags and embedded art are read natively. Cover order: embedded art, then
  `cover.*`/`folder.*`/`front.*` in the album folder, then an online lookup
  (MusicBrainz + Cover Art Archive) cached on disk. The online lookup can be switched off.
- Prism registers itself under "Open with" for audio types, never as the default app
  without the user choosing it.

### 5.3 Audio engine
- Playback runs in a native Rust engine, never in the web view's `<audio>` element (macOS
  WebKit cannot play Ogg/Opus/APE/WMA and the like).
- Decoding uses a native decoder library for the common formats, with a bundled FFmpeg as
  the fallback for everything else. The same pipeline plays Navidrome streams and radio
  streams (including HLS and ICY now-playing metadata).
- Gapless playback for albums. Crossfade is an optional setting, on by default only inside
  Flow.
- ReplayGain support (track/album/off).
- Output device selection; follows the system default device when it changes.
- System integration: media keys, Windows media overlay (SMTC), macOS Now Playing / Control
  Center, with title, artist and cover.

### 5.4 Navidrome (Subsonic API)
- Connection: server URL, username, password. Auth uses the Subsonic token+salt scheme. The
  password is stored in the OS keychain (Windows Credential Manager / macOS Keychain).
- The app keeps a local index (albums, artists, tracks, genres, stars) in SQLite, synced in
  the background with paging and refreshed incrementally. Browsing and search hit the local
  index, so they stay instant with 30k+ tracks.
- Covers are fetched at the needed size and cached on disk.
- Streaming quality: original, or a transcoded bitrate chosen in settings.
- Playback reports "now playing" and scrobbles back to Navidrome.
- Support covers the Subsonic API in general, so Airsonic and Gonic should also work, but
  Navidrome is the tested target.

### 5.5 Radio
- Station directory: the free Radio Browser API (community mirrors; no key needed).
- "Near you": the location comes from a coarse IP lookup on first start, with a manual
  city/country override in settings. Nearby stations are sorted by distance and popularity.
- Worldwide search by name, country, genre and tag.
- Stations can be favorited. Now-playing titles come from the stream metadata when the
  station sends them.

### 5.6 YouTube Music
- YouTube Music has no official playback API. Prism embeds music.youtube.com in an in-app
  web view. The user signs in there themselves; Prism never sees or stores the password.
- No extra window (owner decision, 2026-10-20): the YouTube Music view lives inside the main
  window's content area (a child web view), shown when "YouTube Music" is selected in the
  sidebar and hidden, not destroyed, when the user navigates away, so playback continues and
  Prism's controls keep driving it.
- Sign-in fallback (owner decision): if Google refuses sign-in inside the embedded view
  ("This browser or app may not be secure"), Prism opens sign-in in a separate pop-up
  window that shares the same session store, then returns to the embedded view signed in.
  This is a known risk; the builder spikes it early on both platforms.
- YouTube Music is a closed view (builder's call, from the review): it shares transport
  control only. Its tracks do not enter Prism's queue, playlists, Home, search or history.
- Prism reads the now-playing state from the page and drives play/pause/next/previous, so
  the mini player, media keys and the Flow ring control it too.
- While YouTube Music plays, Prism's own engine pauses, and the other way round: only one
  thing plays at a time.

### 5.7 Favorites
- One combined Favorites view across all sources, filterable by source.
- Favorites can be tracks, albums, artists and radio stations (builder's call). On
  Navidrome all three item kinds map to Subsonic stars.
- Hearting a Navidrome track stars it on the server. Starred items on the server show up
  as favorites. Two-way.
- Local files and radio stations are favorited in Prism's own database.
- YouTube Music favorites are not merged in v1.0.0. There is no reliable way to read them,
  so they stay inside the YouTube Music view.
- Apple Music (v1.1): hearting adds to the Apple Music library or marks it as loved.

### 5.8 Flow
- Idle click on the ring starts Flow: an endless queue built from the combined favorites
  (tracks from local and Navidrome; radio stations are not mixed in). A favorited album or
  artist contributes all of its tracks to the pool (builder's call).
- Rules: no repeats until the pool is used up; no two tracks in a row from the same artist
  where avoidable; light weighting toward tracks played less recently. When the pool is
  small (under ~100 tracks), Flow pads it with tracks from the same artists and genres in
  the library.
- Right-click (or long-press) on the ring offers Flow variants: from favorites (default),
  from the whole library, from one genre.
- While playing, the ring is play/pause with progress around it. Its colors follow the
  current cover (dominant colors mapped onto the spectral gradient), and its glow reacts
  subtly to loudness.

### 5.9 Main window
- Frameless, custom title bar, graphite glass look (Mica on Windows 11, vibrancy on macOS),
  the Flow ring half outside the bottom edge.
- Sidebar: Home, Favorites, Playlists, then the Sources.
- Library views per source and for "All": Albums (cover grid), Artists, Songs (virtualized
  list), Genres. Album detail with a large cover and a backdrop blurred from it.
- Now Playing view: big cover, queue, source and quality ("Navidrome · FLAC 24/96").
- Global search on Ctrl+K / Cmd+K across the local index, the Navidrome index and the radio
  directory.
- Playlists: Prism's own playlists can mix local and Navidrome tracks. Navidrome playlists
  are shown and playable (read-only in v1.0.0).
- Queue and playlists hold tracks only. Playing a radio station replaces the queue with that
  one stream; "next" then does nothing (builder's call).
- The "All" views are a plain union of local and Navidrome. The owner confirmed the two
  never hold the same music, so no duplicate matching is needed.

### 5.10 Mini player
- Matches `mini-v2.html`: about 310 px wide, cover, title, artist, source and quality,
  heart, previous, next, volume, and the Flow ring half outside the left edge.
- It never steals focus while the user types elsewhere, and it never pops up on its own.
- Windows: closing the main window hides it, the app keeps running, and the mini player
  takes over the app's taskbar button. Clicking that taskbar button toggles the mini player
  docked above the bottom-right of the taskbar. Left-click on the tray icon also toggles it.
  A button on the mini player reopens the main window. The main window's taskbar thumbnail
  carries previous / play-pause / next buttons.
- macOS: a menu bar icon toggles the mini player directly under it. Clicking the Dock icon
  opens the main window.
- A global shortcut toggles the mini player (default Ctrl+Alt+P / Cmd+Option+P,
  configurable).
- Option: "Keep mini player on top".

### 5.11 About dialog
Opened from the logo in the sidebar and from Settings. It shows:
- the logo and the app version
- "Made by Wolfgang Linz" with a link to https://wolfgang-linz.de (opens in the browser)
- a link to the GitHub repo
- open-source licenses

### 5.12 Settings
Library folders, Navidrome connection, streaming quality, radio location, playback
(output device, gapless, crossfade, ReplayGain), mini player and shortcut, language,
online cover lookup, updates, and "Start with system" (off by default).

### 5.13 Language
English and German from v1.0.0. The app follows the system language and can be overridden
in settings.

### 5.14 Updates
Signed auto-update from this repo's GitHub releases (Tauri updater, `latest.json`). It
downloads in the background and installs on quit, or right away from a "Restart to update"
pill. This follows the same pattern as the owner's app Glint.

## 6. Technical frame (builder's call within it)

- Tauri 2 + Svelte 5 + TypeScript + Vite, with Rust for the engine, library index,
  source clients and OS integration. This is the same stack family as the owner's apps Kern
  (Svelte) and Glint (release pipeline).
- SQLite for the library index, favorites, playlists and play history, in the app data
  directory.
- Specific crates and libraries (decoder, output, tags, media controls, keychain) are the
  builder's choice. FFmpeg licensing must allow redistribution (an LGPL build), and the
  license is listed in the About dialog.

## 7. Brand, README and release

### 7.1 Logo and icon
- The concept from the chosen mockup: a white outline prism with an incoming beam,
  refracted into three spectral sound waves (red, amber, mint, blue), on a graphite rounded
  square `#141418`.
- It is built by hand as SVG (a source file in the repo); app icons for all sizes come from
  it via `tauri icon`. This needs no Higgsfield credits.
- Higgsfield (if it gets connected) is used for at most one optional README hero image, and
  only after the owner approves the spend. Nothing in v1.0.0 depends on it.

### 7.2 README
Logo and tagline, screenshots of the main window and the mini player (taken from the
running app with demo data, never with the owner's real library or account names),
download buttons that point to the versionless `latest/download` links, a feature list, a
supported formats table, a Navidrome setup note, the macOS first-launch note for v1.0.0
(right-click > Open, unsigned), a credits line for Wolfgang Linz with his site, and the
license.

### 7.3 License
MIT (builder default; the owner can change it before the first push).

### 7.4 Release pipeline
- GitHub Actions on an annotated tag `v*`: builds Windows (NSIS, x64) and macOS (universal
  DMG), creates a draft release, uploads the versioned assets plus the versionless copies,
  writes `latest.json` for the updater, and then publishes.
- The installers are unsigned in v1.0.0 (only the Tauri updater signature). macOS
  notarization arrives with the Apple Developer membership in v1.1.
- Release notes live in `docs/releases/v<x>.md`.
- The updater key pair lives at `%USERPROFILE%\.tauri\prism-updater.key` (+ `.password`,
  `.pub`). The repo secrets `TAURI_SIGNING_PRIVATE_KEY(_PASSWORD)` are set by the owner if
  the tooling blocks setting them.

## 8. Quality bar

- With 30k Navidrome tracks indexed: the Songs list scrolls smoothly, search answers in
  under 100 ms, and an album opens instantly from the index.
- No white flash on start. The window appears themed from its first frame.
- Tests for the engine, the index, the Subsonic client (against a mocked server), Flow
  rules and favorites sync (two-way stars), plus frontend unit tests.
- Never use the owner's real Navidrome credentials in tests or screenshots.

## 9. Left to the builder

The milestone split, crate choices, database schema, IPC shape, exact animation timings,
the icon set, the German wording (reviewed by the owner), and test tooling.
