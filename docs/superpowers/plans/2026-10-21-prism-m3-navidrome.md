# Prism M3 (Navidrome) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Navidrome (Subsonic API) becomes a first-class source: a signed-in server whose 30k+ tracks live in Prism's local index (synced in the background, browsable and searchable offline), stream through the native engine with seeking, gapless and crossfade, star and unstar both ways, scrobble, show their covers and their playlists, and join Flow.

**Architecture:** A new `src-tauri/src/navidrome/` module owns everything that talks to the server: a blocking Subsonic client, keychain credentials, the sync passes (library, stars, playlists, scrobble queue) and one background service thread that schedules them and reports a status. Navidrome rows live in the existing `tracks` / `albums` tables with `source = 'navidrome'` and ids `navidrome:<subsonic id>`, so every query, FTS, favorites, playlists and Flow see a plain union with the local library. Streams enter the engine as a new `Input::Http`: a progressive, range-capable download buffer that symphonia reads as a `MediaSource` (FFmpeg reads the same buffer through stdin), so the M2 read-ahead, gapless and crossfade paths work unchanged.

**Tech Stack:** M2 stack plus `keyring` 3 (features `windows-native`, `apple-native`), `md-5` (RustCrypto, the release line that shares `digest` with `sha1 = 0.11`), `reqwest` 0.12 (the copy Task 18 added; add feature `rustls-tls-native-roots` next to `blocking`, `rustls-tls`, `json`), dev-dependency `tiny_http` 0.12 for the fake Subsonic server.

**Spec:** `docs/superpowers/specs/2026-10-20-prism-design.md` (5.4, 5.7, 5.8, 5.9, 5.10, 5.12, 8). Roadmap: `docs/superpowers/plans/2026-10-20-prism-m1-foundation.md` (row M3). Previous plan and its constraints: `docs/superpowers/plans/2026-10-20-prism-m2-everyday-player.md`. M2 ledger with rulings and deferred minors: `.superpowers/sdd/2026-10-20-prism-m2-everyday-player/progress.md`.

## Global Constraints

- Platforms: Windows 11 and macOS 11+ (universal). Developed and checked on Windows; anything Windows-only is behind `cfg(target_os = "windows")` with a macOS counterpart or a no-op, and every such pair is named in its task.
- Repo `C:\Projects\private\Prism`. Work on a new branch `PRISM-3` cut from the `PRISM-2` HEAD when execution starts. Parallel worktrees use branches `PRISM-3NN` (NN = task number) cut from `PRISM-3` and merge back in task order. Repo-local identity `Wolfgang Linz <27097016+wl-lankin@users.noreply.github.com>`. Commit, never push. Commit messages `PRISM-3: <Description>`.
- App identifier `de.wolfganglinz.prism`, productName `Prism`, version `1.0.0`, MIT, publisher "Wolfgang Linz", homepage `https://wolfgang-linz.de`.
- No em dash or en dash anywhere (code, UI strings, docs, commits). No AI attribution or session links anywhere. No sparkle glyphs.
- Palette: background `#0d0d10`, surface `#141418`, raised `#1f1f25`, text `#f3f3f5`; spectrum `#ff4d6d`, `#ffc94d`, `#4dffb2`, `#4da6ff`. Source colors: navidrome `#4dffb2`, apple `#ff4d6d`, youtube `#ffc94d`, radio `#4da6ff`, local `#cccccc`.
- Reduced motion: only large motion is switched off; hover and fade transitions keep running at 120 ms (`--dur`).
- Window look (owner decision): no Mica, no vibrancy, no OS shadow, no DWM border or rounded system corners. Main Flow ring 96 px half out of the card's bottom edge; mini ring 76 px half out of its left edge.
- Closing the main window hides it together with its taskbar button and opens the mini player (owner decision, spec 5.10). Nothing in M3 may regress this.
- The mini player never steals focus and never appears without a user action.
- Every dev run sets `PRISM_DATA_DIR` to a fresh temp folder AND passes a Tauri `--config` override with its own identifier (`de.wolfganglinz.prism.dev<n>`). Never touch `%APPDATA%` / `%LOCALAPPDATA%\de.wolfganglinz.prism`. Keychain entries are keyed by the identifier (Task 3), so a dev run never reads or writes the owner's real Navidrome password.
- Agents never inject OS input (mouse, keyboard, window moves of other windows). Visual checks only through CDP screenshots and DOM events of the agent's own instance; real-input checks go to the owner hand check (Task 20).
- Ports: the main copy uses Vite 1440 / HMR 1441. Each parallel worktree agent picks its own Vite, HMR and CDP ports in 1500-1599 through an uncommitted override config it deletes afterwards. Never kill processes by image name (`taskkill /IM node.exe`, `bash.exe` or any other); stop only the PIDs you started.
- Write files with Write/Edit, never Bash heredocs. Frontend tests are `src/**/*.spec.ts`.
- Commands: `npm test`, `npm run check`, `cargo test --manifest-path src-tauri/Cargo.toml [filter]`, `cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings`. The local `ffmpeg` / `ffprobe` on PATH are fine for development.
- Migrations are append-only: the owner has a real library DB. M3 adds exactly one, `0008_navidrome.sql` (Task 1). Never edit an earlier migration.
- Network: besides MusicBrainz / Cover Art Archive (M2, unchanged), the only host Prism contacts is the Navidrome server the user configured. Test servers bind `127.0.0.1:0` only.
- Credentials: the password lives only in the OS keychain and in memory. It never appears in `settings.json`, logs, error messages, `Debug` output, events, process command lines, URLs passed to FFmpeg, fixtures or screenshots. The same applies to the Subsonic token and salt.
- Never use the owner's real library, server, accounts or credentials in tests, fixtures or screenshots. The fake account is `demo` / `demo-pass`. The one live test (Task 19) is `#[ignore]` and reads its server from env vars only.
- Every new UI string goes into both `en` and `de` in `src/lib/i18n.ts` (`i18n.spec.ts` checks keys, placeholders and dashes).

## Review Focus

1. **The network drops or the server goes down mid-track, with a long queue or a Flow running:** playback must not burn through the queue with ten "could not be played" errors. The player pauses on that track with one "Navidrome is not reachable" notice, and Play resumes where it stopped once the server is back. Pinned in Task 9 (`offline_mid_track_pauses_at_position`, `offline_errors_do_not_count_toward_the_failure_cap`) and Task 13 (`offline_navidrome_tracks_leave_the_pool`).
2. **A first sync of 30k tracks that is interrupted** (server restart, laptop sleeps, app quit): nothing already indexed is deleted, the partial index stays browsable, and the next sync completes. Pinned in Task 4 (`interrupted_pass_deletes_nothing`, `zero_songs_never_wipes_the_index`).
3. **Switching to another server or user, or disconnecting:** no tracks, stars, playlists or queued stars of the old server survive, and nothing queued for the old server is sent to the new one. Pinned in Task 4 (`server_change_wipes_the_index`) and Task 5 (`switching_server_wipes_old_rows`, `disconnect_wipes_and_forgets`).
4. **Hearting while offline, then quitting:** the star survives the restart and reaches the server later; a star removed on the server meanwhile on another item is still imported. Pinned in Task 10 (`unstar_while_offline_survives_restart_and_flushes`, `pending_local_change_wins_over_server`).
5. **Credentials leaking through error text:** reqwest errors print the request URL (with `t=` and `s=`), `Input` derives `Debug`, FFmpeg command lines are visible to other users. Pinned in Task 2 (`errors_never_contain_the_token`), Task 7 (`input_debug_hides_url`) and Task 8 (`ffmpeg_args_carry_no_url_or_token`).

---

## File Structure

```
src-tauri/src/db/migrations/0008_navidrome.sql   albums.artist_id, source-order index, narrower FTS update
                                                  trigger, navidrome_state, star / scrobble queues,
                                                  remote playlists
src-tauri/src/secret.rs                 Secret, SecretUrl (redacted Debug/Display)
src-tauri/src/navidrome/mod.rs          NavidromeService: account, status, scheduler thread, cycle
src-tauri/src/navidrome/client.rs       SubsonicClient (auth, calls, JSON types, error mapping)
src-tauri/src/navidrome/keychain.rs     CredentialStore, KeyringStore, MemoryStore
src-tauri/src/navidrome/sync.rs         library sync passes, server switch wipe
src-tauri/src/navidrome/stars.rs        two-way stars: queue, flush, import
src-tauri/src/navidrome/scrobble.rs     ScrobbleTracker, scrobbler sink, queue flush
src-tauri/src/navidrome/playlists.rs    remote playlists sync and queries
src-tauri/src/navidrome/covers.rs       RemoteCovers over getCoverArt
src-tauri/src/navidrome/stream.rs       Navidrome opener, NavidromeSource (quality, transcoded seek)
src-tauri/src/navidrome/testserver.rs   #[cfg(test)] fake Subsonic server (tiny_http)
src-tauri/src/audio/http.rs             HttpStream: progressive range-capable buffer, Offline marker
src-tauri/tests/fixtures/subsonic/*.json hand-written Subsonic / OpenSubsonic responses, fake data
src/lib/stores/navidrome.svelte.ts      status store, failure texts
src/lib/components/SourceBadge.svelte   colored source dot on covers and rows
src/lib/components/SourceEmpty.svelte   empty / not connected / first sync state of a source view
```

Modified: `library/{model,query,favorites,playlists,scan}.rs`, `audio/{source,symph,ffmpeg,mod}.rs`, `player/{engine,mod}.rs`, `flow/{mod,deck}.rs`, `thumbs.rs`, `protocol.rs`, `media_controls.rs`, `settings.rs`, `commands.rs`, `lib.rs`, `Cargo.toml`; frontend `types.ts`, `ipc.ts`, `format.ts`, `stores/{library,nav,playlists,favorites}`, `Sidebar`, `Settings`, `FavoritesView`, `ArtistView`, `ArtistList`, `AlbumGrid`, `SongList`, `SearchPalette`, `PlaylistsView`, `PlaylistView`, `TransportStrip`, `NowPlaying`, `MiniPlayer`, `App.svelte`.

## Decisions pinned here

- **Ids.** Track `navidrome:<song id>`, album `navidrome:<album id>`, artist `navidrome:<artist id>`. Server ids are stable, so nothing joins `TRACK_ID_REFS` or the rekey. Cover ids `nd-<coverArt>` (only `[A-Za-z0-9_.-]{1,128}`, otherwise no cover). Tracks use their album's `coverArt` when the album is known (one fetch per album, not per song).
- **Artists.** `Artist` gains `id` and `source`. A local artist's id is its name (so every M2 favorite row stays valid); a Navidrome artist's id is `navidrome:<artistId>`. The artist page stays keyed by name (a union over sources).
- **Sync.** Change detection by `getIndexes` `lastModified`. A full pass (albums via `getAlbumList2` `alphabeticalByName`, songs via `search3` with an empty query, 500 per page; per-album `getAlbum` fallback when `search3` returns nothing) runs when `lastModified` changed, after an incomplete pass, or when the last full pass is over 24 h old. Deletions only after a complete pass. Stars and playlists are refreshed every cycle (one and 1 + changed-count calls).
- **Schedule.** One cycle 3 s after start (when configured), every 30 min, on "Sync now" and on connect. Order: flush stars, library, import stars, playlists, flush scrobbles. Offline: ping every 60 s, full cycle once it answers.
- **Offline.** An unreachable server sets the status `offline`, keeps the whole index browsable, serves no new covers (503, not cached), drops Navidrome tracks from new Flow refills, and pauses (not skips) a Navidrome track that cannot stream.
- **Streaming.** Quality `original` requests `format=raw`; `320` / `192` / `128` request `format=mp3&maxBitRate=<n>`. Byte-seekable streams (206, or 200 with `Content-Length` and `Accept-Ranges: bytes`) seek through Range requests; other streams seek by reopening with `timeOffset=<s>`. FFmpeg never sees a URL: it reads the buffer through `pipe:0`.
- **TLS.** rustls with webpki plus native roots (a private CA the OS trusts works). No "accept invalid certificate" switch in v1.0.0; a self-signed server gets a clear `tls` error that suggests a trusted certificate or plain `http://` on the LAN.
- **Stars.** A heart on a Navidrome item writes the local favorite at once (source `navidrome`) and queues the intent (latest intent per item wins). The queue is flushed right away and retried after 5 s, 30 s, 2 min, then every cycle. Import: server stars are the truth for every item without a pending local intent; a pending local intent wins.
- **Scrobble.** `scrobble?submission=false` when a Navidrome track becomes audible; `submission=true&time=<start ms>` once it has played min(50 %, 4 min) of a track of at least 30 s. Failed submissions are queued in the DB; now-playing is best effort.

---

### Task 1: Schema 0008, source-aware queries, artists with id and source

**Files:**
- Create: `src-tauri/src/db/migrations/0008_navidrome.sql`
- Modify: `src-tauri/src/db/mod.rs`, `src-tauri/src/library/model.rs`, `src-tauri/src/library/query.rs`, `src-tauri/src/library/favorites.rs`, `src-tauri/src/library/scan.rs` (`reconcile_albums`), `src-tauri/src/commands.rs`

**Interfaces:**
- Produces:
  - `0008_navidrome.sql`: `ALTER TABLE albums ADD COLUMN artist_id TEXT`; `idx_albums_artist_id ON albums (artist_id)`; `idx_tracks_source_order ON tracks (source, <the columns of idx_tracks_order>)`; `DROP TRIGGER tracks_fts_au` and recreate it as `AFTER UPDATE OF title, artist, album ON tracks` (same body); tables `navidrome_state (key TEXT PRIMARY KEY, value TEXT NOT NULL)`, `navidrome_star_queue (kind TEXT NOT NULL, item_id TEXT NOT NULL, star INTEGER NOT NULL, queued_at INTEGER NOT NULL, attempts INTEGER NOT NULL DEFAULT 0, PRIMARY KEY (kind, item_id))`, `navidrome_scrobble_queue (id INTEGER PRIMARY KEY, track_id TEXT NOT NULL, played_at INTEGER NOT NULL)`, `remote_playlists (id TEXT PRIMARY KEY, name TEXT NOT NULL, owner TEXT, changed TEXT, track_count INTEGER NOT NULL DEFAULT 0, duration_ms INTEGER NOT NULL DEFAULT 0, updated_at INTEGER NOT NULL)`, `remote_playlist_tracks (playlist_id TEXT NOT NULL REFERENCES remote_playlists(id) ON DELETE CASCADE, position INTEGER NOT NULL, track_id TEXT NOT NULL, PRIMARY KEY (playlist_id, position))`. Each table gets the house-style comment (no foreign key to tracks, kept when the track leaves the index).
  - `model.rs`: `pub const NAVIDROME_PREFIX: &str = "navidrome:"`, `pub fn navidrome_id(raw: &str) -> String`, `pub fn navidrome_raw(id: &str) -> Option<&str>`, `pub const NAVIDROME_COVER_PREFIX: &str = "nd-"`, `pub fn navidrome_cover_id(cover_art: &str) -> Option<String>`, `pub fn navidrome_cover_raw(id: &str) -> Option<&str>`. `Artist { id: String, source: Source, name, album_count, track_count, cover_id }`.
  - `query.rs`: `artist_select(filter)` groups by `a.source, COALESCE(a.artist_id, a.artist)` and returns `id = COALESCE(a.artist_id, a.artist)` and `source`; `tracks_range(conn, offset, limit, source: Option<Source>)`, `track_count(conn, source: Option<Source>)`.
  - `favorites::artists` resolves artist favorites by `Artist.id` (local ids are names, so old rows still match).
  - Commands `library_tracks_range(offset, limit, source: Option<Source>)` and `library_track_count(source: Option<Source>)` (`source` omitted = all).
- Rule: `reconcile_albums` only rebuilds albums of `source = 'local'` tracks (a Navidrome track written before its album must never get a local album row).

- [ ] **Step 1: Write the failing tests:** `db::migrates_fresh_db_to_latest` stays generic; new `navidrome_tables_exist` (each table and `albums.artist_id` present after migrate), `fts_update_trigger_ignores_other_columns` (the trigger SQL in `sqlite_master` names `UPDATE OF title, artist, album`; updating `duration_ms` keeps `tracks_fts MATCH` results unchanged), `source_tracks_range_uses_index` (`EXPLAIN QUERY PLAN` of the source-filtered range query names `idx_tracks_source_order`, no `USE TEMP B-TREE`), `track_count_by_source` (2 local + 3 navidrome rows: `None` 5, `Local` 2, `Navidrome` 3), `artists_split_by_source_with_ids` (same name in both sources gives two rows; local id is the name, navidrome id `navidrome:ar-1`), `local_artist_favorites_keep_working`, `navidrome_artist_favorite_resolves_by_id`, `reconcile_ignores_navidrome_tracks`, `navidrome_ids_round_trip`, `cover_id_rejects_unsafe_chars` (`"al-1_abc"` ok, `"../x"`, `"a b"`, 129 chars rejected).
- [ ] **Step 2: Run** `cargo test --manifest-path src-tauri/Cargo.toml db:: library::` -> FAIL.
- [ ] **Step 3: Implement** the migration, model helpers, queries and command arguments. Existing callers of `tracks_range` / `track_count` pass `None`.
- [ ] **Step 4: Run** the full `cargo test` -> PASS.
- [ ] **Step 5: Commit** `PRISM-3: Schema for Navidrome, source-aware track queries, artists with id and source`.

### Task 2: Subsonic client and fake server

**Files:**
- Create: `src-tauri/src/secret.rs`, `src-tauri/src/navidrome/mod.rs` (module list only), `src-tauri/src/navidrome/client.rs`, `src-tauri/src/navidrome/testserver.rs`, `src-tauri/tests/fixtures/subsonic/{ping.json, error_40.json, error_41.json, getIndexes.json, getAlbumList2.json, getAlbum.json, search3.json, getStarred2.json, getPlaylists.json, getPlaylist.json}`
- Modify: `src-tauri/Cargo.toml`, `src-tauri/src/lib.rs` (`pub mod navidrome; pub mod secret;`)

**Interfaces:**
- Produces:
  - `secret.rs`: `pub struct Secret(String)` and `pub struct SecretUrl(String)`, both `Clone`, `PartialEq`, `Eq`, with `Debug` and `Display` printing `***` / `<redacted url>` and `fn expose(&self) -> &str`.
  - `client.rs`: `API_VERSION = "1.16.1"`, `CLIENT_NAME = "Prism"`, `CONNECT_TIMEOUT = 8 s`, `API_TIMEOUT = 20 s`, `PAGE = 500`, `MAX_COVER_BYTES = 8 MiB`.
  - `pub struct Account { pub url: String, pub user: String, pub password: Secret }` (`url` normalized).
  - `pub fn normalize_url(input: &str) -> Result<String, NavidromeError>`: trims; no scheme -> `https://` prepended; only `http` / `https`; query and fragment rejected; trailing `/` removed; a path prefix kept (reverse proxy). Calls go to `<url>/rest/<method>`.
  - `pub(crate) fn token(password: &str, salt: &str) -> String` = lowercase hex md5(password + salt); every request gets a fresh 12-hex-char salt from `rand` and the params `u, t, s, v, c, f=json`.
  - `#[serde(rename_all = "lowercase")] pub enum StreamQuality { Original, #[serde(rename = "320")] Kbps320, #[serde(rename = "192")] Kbps192, #[serde(rename = "128")] Kbps128 }`, `Default` = `Original`, `fn max_bitrate(self) -> Option<u32>`.
  - `pub enum NavidromeError { InvalidUrl(String), WrongCredentials, TokenAuthUnsupported, NotAuthorized, NotFound, Unreachable(String), Tls(String), NotSubsonic(String), Server { code: u32, message: String } }` with `fn code(&self) -> &'static str` (`invalidUrl`, `wrongCredentials`, `tokenAuthUnsupported`, `notAuthorized`, `notFound`, `unreachable`, `tls`, `notSubsonic`, `server`) and `fn is_offline(&self) -> bool` (only `Unreachable`). Mapping: Subsonic code 40 and HTTP 401 -> `WrongCredentials`; 41, 42, 43, 44 -> `TokenAuthUnsupported`; 50 -> `NotAuthorized`; 70 -> `NotFound`; others -> `Server`; a non-2xx status or a body without `subsonic-response` -> `NotSubsonic("HTTP <n>" | "not a Subsonic server")`; reqwest timeout / connect / DNS -> `Unreachable`, except an error whose source chain mentions `certificate` -> `Tls`. Every reqwest error goes through `without_url()` first. `pub(crate) fn classify(err: &(dyn std::error::Error + 'static)) -> NavidromeError`.
  - `#[derive(Serialize)] #[serde(rename_all = "camelCase")] pub struct NavidromeFailure { pub code: &'static str, pub detail: Option<String> }` and `impl From<&NavidromeError>`.
  - JSON types (`serde`, `rename_all = "camelCase"`, every field `#[serde(default)]`): `Child` (song: `id, title, album, artist, album_id, artist_id, track, disc_number, year, genre, cover_art, suffix, content_type, duration` (s)`, bit_rate, sampling_rate, bit_depth, display_album_artist, album_artists: Vec<ArtistRef>, replay_gain: Option<ReplayGainInfo>, starred: Option<String>`), `AlbumId3 { id, name, artist, artist_id, cover_art, song_count, duration, year, genre, starred }`, `ArtistId3 { id, name, starred }`, `Starred2 { song, album, artist }`, `PlaylistSummary { id, name, owner, song_count, duration, changed, cover_art }`, `PlaylistWithSongs { summary fields, entry: Vec<Child> }`, `ServerInfo { server_type, server_version, api_version, open_subsonic }` (camelCase out).
  - `pub enum StarTarget { Song(String), Album(String), Artist(String) }` (raw ids; `id=`, `albumId=`, `artistId=`).
  - `pub struct SubsonicClient`, `SubsonicClient::new(account: Account) -> anyhow::Result<Self>`, `pub(crate) fn with_timeouts(account, connect: Duration, api: Duration)`, and methods `ping() -> Result<ServerInfo, NavidromeError>`, `indexes_last_modified() -> Result<i64, _>`, `album_list2(offset: u32, size: u32) -> Result<Vec<AlbumId3>, _>`, `album(id: &str) -> Result<(AlbumId3, Vec<Child>), _>`, `search3_songs(offset: u32, count: u32) -> Result<Vec<Child>, _>` (`query=""`, `artistCount=0`, `albumCount=0`), `starred2() -> Result<Starred2, _>`, `star(target: &StarTarget, on: bool) -> Result<(), _>`, `scrobble(id: &str, time_ms: i64, submission: bool) -> Result<(), _>`, `playlists() -> Result<Vec<PlaylistSummary>, _>`, `playlist(id: &str) -> Result<PlaylistWithSongs, _>`, `cover_art(id: &str, size: u32) -> Result<Vec<u8>, _>`, `stream_url(id: &str, quality: StreamQuality, time_offset_s: Option<u32>) -> SecretUrl` (adds `format=raw` or `format=mp3&maxBitRate=<n>`, `estimateContentLength=false`), `stream_http() -> reqwest::blocking::Client` (connect timeout, no total timeout, `tcp_keepalive` 15 s).
  - `testserver.rs` (`#[cfg(test)] pub(crate)`): `FAKE_USER = "demo"`, `FAKE_PASSWORD = "demo-pass"`, `FakeLibrary::synthetic(albums: usize, songs_per_album: usize) -> FakeLibrary` (fake names "Demo Artist <n>"), `with_audio(song_id, bytes: Vec<u8>)`, `with_wav(song_id, seconds: u32)` (generated sine), `FakeServer::start(lib) / start_at(lib, prefix: &str)`, `url() -> String`, `account() -> Account`, `requests() -> Vec<Recorded { method: String, params: Vec<(String, String)>, range: Option<String> }>`, `set_fault(Fault)`, `edit(|&mut FakeLibrary|)` (also bumps `lastModified`), `starred() -> (HashSet songs, albums, artists)`, `scrobbles() -> Vec<(String, bool, Option<i64>)>`, `stop()` / `restart()` (same port). An `#[ignore]` test `serve_for_ui` starts `FakeServer` with `synthetic(60, 10)` and audio for every song, prints its URL and the fake account, and serves for 10 minutes: the UI tasks' visual checks connect their own dev instance to it. `enum Fault { None, WrongPassword, Status(u16), Html, Refuse, Stall { method: &'static str, after_bytes: usize, wait: Duration }, CutOnce { method: &'static str, after_bytes: usize }, NoRanges, Search3Empty, Code(u32) }`. It validates `t` against md5(`demo-pass` + `s`) and answers error 40 otherwise; serves the methods above plus `stream` (Range-aware unless `NoRanges`, honors `timeOffset` for `with_wav` songs) and `getCoverArt` (a generated PNG of `size` px).
- Fixtures are hand-written in the Navidrome / OpenSubsonic response shape with fake data only.

- [ ] **Step 1: Write the failing tests** in `client.rs`: `token_matches_the_subsonic_example` (`token("sesame", "c19b2d") == "26719a1196d2a940705a59634eb18eab"`), `salt_is_fresh_per_request` (two pings, different `s`), `normalize_url_cases` (`"nas:4533"` -> `"https://nas:4533"`, `"http://nas:4533/"` -> `"http://nas:4533"`, `"https://host/music/"` -> `"https://host/music"`, `"ftp://x"`, `""`, `"https://h/?a=1"` -> `InvalidUrl`), `ping_reads_server_info`, `base_url_keeps_a_path_prefix` (`start_at("/music")`), `wrong_password_maps_to_wrong_credentials`, `code_41_maps_to_token_auth_unsupported`, `refused_maps_to_unreachable`, `timeout_maps_to_unreachable` (`Stall` on ping, `with_timeouts(.., 300 ms)`), `html_maps_to_not_subsonic`, `certificate_errors_map_to_tls` (`classify` on a fake error chain ending in "invalid peer certificate: UnknownIssuer"), `errors_never_contain_the_token` (for every fault, `Display` and `Debug` of the error contain neither `demo-pass`, the token, the salt, `t=` nor `s=`), `account_debug_hides_password`, `search3_pages_all_songs` (1 200 songs: 3 requests with `songOffset` 0, 500, 1000), `star_targets_use_the_right_param`, `stream_url_quality_params`, `fixtures_parse` (every fixture file deserializes into its type).
- [ ] **Step 2: Run** `cargo test --manifest-path src-tauri/Cargo.toml navidrome::client` -> FAIL.
- [ ] **Step 3: Implement** `secret.rs`, the client, the fixtures and the fake server; add `md-5`, `tiny_http` (dev) and the reqwest feature to `Cargo.toml`.
- [ ] **Step 4: Run** the same filter -> PASS; full `cargo test` and clippy -> PASS.
- [ ] **Step 5: Commit** `PRISM-3: Subsonic client with token auth, error mapping and a fake server`.

### Task 3: Keychain credentials and Navidrome settings

**Files:**
- Create: `src-tauri/src/navidrome/keychain.rs`
- Modify: `src-tauri/Cargo.toml` (`keyring = { version = "3", features = ["windows-native", "apple-native"] }`), `src-tauri/src/settings.rs`, `src-tauri/src/commands.rs` (`settings_set`)

**Interfaces:**
- Consumes: `Secret`, `StreamQuality` (Task 2).
- Produces:
  - `pub trait CredentialStore: Send + Sync { fn get(&self, url: &str, user: &str) -> anyhow::Result<Option<Secret>>; fn set(&self, url: &str, user: &str, password: &Secret) -> anyhow::Result<()>; fn delete(&self, url: &str, user: &str) -> anyhow::Result<()>; }`
  - `pub struct KeyringStore`, `KeyringStore::new(identifier: &str)`, `fn service(&self) -> &str` = `"<identifier>.navidrome"`; the keyring account is `pub fn account_key(url, user) -> String` = `"<user>@<url>"`. `NoEntry` maps to `Ok(None)` / a no-op delete. Windows: Credential Manager (`windows-native`); macOS: Keychain (`apple-native`).
  - `pub struct MemoryStore` (in-memory, `pub` for tests of other modules).
  - `Settings` gains `navidrome_url: Option<String>`, `navidrome_user: Option<String>`, `navidrome_quality: StreamQuality` (JSON `navidromeUrl`, `navidromeUser`, `navidromeQuality`). An unknown `navidromeQuality` value falls back to `original` without discarding the rest of the file (lenient field deserializer).
  - `SettingsStore::set_navidrome_account(&self, account: Option<(String, String)>) -> anyhow::Result<Settings>`; `settings_set` drops `navidromeUrl` and `navidromeUser` from any patch (only connect / disconnect write them). The quality needs no `Change`: the opener (Task 8) reads it at every open.
- Rule: unit tests never construct `KeyringStore` except for the pure `service()` check.

- [ ] **Step 1: Write the failing tests:** `keyring_service_uses_identifier` (`"de.wolfganglinz.prism.dev1"` -> `"de.wolfganglinz.prism.dev1.navidrome"`), `memory_store_round_trip`, `settings_navidrome_defaults` (`None`, `None`, `original`), `unknown_quality_keeps_other_settings` (file with `"navidromeQuality": "999"` and `"gapless": false`: gapless stays false, no `.bad` file), `patch_cannot_set_the_account` (a `settings_set`-style patch with `navidromeUrl` leaves it unchanged; tested on the pure strip helper `fn strip_account_keys(patch: &mut Value)`), `set_navidrome_account_saves_url_and_user_only` (the written JSON has no `password` key).
- [ ] **Step 2: Run** `cargo test --manifest-path src-tauri/Cargo.toml settings:: navidrome::keychain` -> FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** -> PASS.
- [ ] **Step 5: Commit** `PRISM-3: Navidrome password in the OS keychain, account and quality settings`.

### Task 4: Library sync

**Files:**
- Create: `src-tauri/src/navidrome/sync.rs`
- Modify: `src-tauri/src/navidrome/mod.rs`

**Interfaces:**
- Consumes: `SubsonicClient`, `FakeServer` (Task 2); schema and id helpers (Task 1).
- Produces:
  - `pub fn server_key(url: &str, user: &str) -> String` (lowercased url, `\u{1f}`, user).
  - `pub fn ensure_server(db: &Db, key: &str) -> anyhow::Result<bool>`: when `navidrome_state.server` differs, runs `wipe_navidrome` and stores the key; returns whether it wiped.
  - `pub fn wipe_navidrome(tx: &rusqlite::Transaction) -> anyhow::Result<()>`: deletes `source = 'navidrome'` tracks and albums, favorites with `source = 'navidrome'`, both queues, `remote_playlists` (cascade), `track_stats` rows of `navidrome:` ids, and every `navidrome_state` key except `server`.
  - `pub struct SyncProgress { pub done: u32, pub total: Option<u32> }` (Serialize camelCase); `pub struct SyncReport { pub full_pass: bool, pub tracks: u32, pub albums: u32, pub changed: bool }`; `pub enum SyncError { Api(NavidromeError), Db(anyhow::Error), Refused(String) }`.
  - `pub fn sync_library(db: &Db, client: &SubsonicClient, now_ms: i64, progress: &dyn Fn(SyncProgress), changed: &dyn Fn()) -> Result<SyncReport, SyncError>`.
  - `pub fn song_row(song: &Child, album_cover: Option<&str>) -> Track` and `pub fn album_row(album: &AlbumId3) -> (Album, Option<String>)` (pure; the second value is the `navidrome:` artist id).
  - `navidrome_state` keys: `server`, `last_modified`, `last_full_sync`, `last_sync`.
- Rules:
  - The full-pass condition and the pass order are the ones under "Decisions pinned here". Network calls run outside the DB lock; each page is written in its own transaction under the lock with `INSERT ... ON CONFLICT(id) DO UPDATE SET ... WHERE` (any mapped column differs), so unchanged rows are not rewritten, and each id goes into `temp.nd_seen_tracks` / `temp.nd_seen_albums`.
  - After both passes complete, rows of the source not seen are deleted. A pass that sees zero songs while the index holds Navidrome tracks fails with `SyncError::Refused` and deletes nothing. Any error leaves the written rows, deletes nothing and does not store `last_modified`, so the next cycle repeats the full pass.
  - Mapping: `format` = lowercase `suffix` (or the `contentType` subtype); `duration_ms = duration * 1000`; `album_artist` = `displayAlbumArtist`, else `albumArtists[0].name`, else `artist`; replay gain fields from `replayGain`; `path`, `folder`, `mtime`, `size` NULL.
  - `changed()` fires after the first written page, then at most every 2 s, and once at the end when anything changed. `progress` reports songs done; `total` is the sum of `songCount` from the album pass.

- [ ] **Step 1: Write the failing tests** (fake server, in-memory DB): `first_sync_indexes_everything` (20 albums x 12 songs: 240 navidrome tracks, 20 albums with `artist_id`, FTS finds a fake title), `second_sync_without_changes_skips_song_pass` (no `search3` in the recorded requests), `changed_server_updates_and_deletes` (`edit` renames a song and removes an album: title updated, that album and its tracks gone), `interrupted_pass_deletes_nothing` (`CutOnce` on page 2 of 3 after an edit that removed songs: error, nothing deleted, `last_modified` unchanged; the next sync completes and deletes them), `search3_unsupported_falls_back_to_albums` (`Search3Empty`), `zero_songs_never_wipes_the_index`, `server_change_wipes_the_index` (local rows and local favorites untouched), `local_rows_untouched_by_sync`, `unchanged_rows_are_not_rewritten` (a temp `AFTER UPDATE ON tracks` counter trigger stays 0 across a forced full pass), `song_row_maps_fields` (from `search3.json`, including OpenSubsonic `bitDepth`, `samplingRate`, `replayGain`, `displayAlbumArtist`), `tracks_use_the_album_cover`.
- [ ] **Step 2: Run** `cargo test --manifest-path src-tauri/Cargo.toml navidrome::sync` -> FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** -> PASS.
- [ ] **Step 5: Commit** `PRISM-3: Navidrome library sync with paging, change detection and safe deletions`.

### Task 5: Navidrome service, status and commands

**Files:**
- Modify: `src-tauri/src/navidrome/mod.rs`, `src-tauri/src/commands.rs`, `src-tauri/src/lib.rs`

**Interfaces:**
- Consumes: Tasks 2, 3, 4.
- Produces:
  - `pub const EVENT_STATUS: &str = "navidrome://status";`
  - `#[serde(rename_all = "lowercase")] pub enum LinkState { Off, Syncing, Online, Offline, Error }`.
  - `#[serde(rename_all = "camelCase")] pub struct NavidromeStatus { pub configured: bool, pub url: Option<String>, pub user: Option<String>, pub state: LinkState, pub last_sync: Option<i64>, pub track_count: u32, pub album_count: u32, pub progress: Option<SyncProgress>, pub error: Option<NavidromeFailure> }`.
  - `pub enum NavidromeEvent { Status(NavidromeStatus), LibraryChanged, FavoritesChanged, PlaylistsChanged }`; `pub type NavidromeEmit = Arc<dyn Fn(NavidromeEvent) + Send + Sync>`.
  - `pub struct ServiceOptions { pub first_delay: Duration, pub interval: Duration, pub offline_retry: Duration }` (default 3 s, 30 min, 60 s).
  - `NavidromeService::spawn(db: Arc<Db>, settings: Arc<SettingsStore>, creds: Arc<dyn CredentialStore>, emit: NavidromeEmit) -> Arc<NavidromeService>` and `spawn_with_options(.., ServiceOptions)`. Methods: `status()`, `client() -> Option<Arc<SubsonicClient>>`, `online_flag() -> Arc<AtomicBool>` (true only in `Syncing` / `Online`), `test(url, user, password: Secret) -> Result<ServerInfo, NavidromeError>`, `connect(url, user, password: Secret) -> Result<NavidromeStatus, NavidromeError>`, `disconnect() -> anyhow::Result<()>`, `sync_now()`, `kick_flush()`, `shutdown()`.
  - `fn cycle(&self)`: flush stars (Task 10), `sync_library`, import stars (Task 10), playlists (Task 12), flush scrobbles (Task 11). Tasks 10 to 12 add their steps here in that order.
  - Commands (registered in `lib.rs`): `navidrome_status() -> NavidromeStatus`, `navidrome_test(url, user, password) -> Result<ServerInfo, NavidromeFailure>`, `navidrome_connect(url, user, password) -> Result<NavidromeStatus, NavidromeFailure>`, `navidrome_disconnect() -> Result<(), String>`, `navidrome_sync() -> Result<(), String>`.
  - `AppState.navidrome: Arc<NavidromeService>`; `KeyringStore::new(&app.config().identifier)`; the Tauri emit maps the events to `navidrome://status`, `library://changed`, `favorites://changed`, `playlists://changed`.
- Rules: connect normalizes the URL, pings with the given password, and only on success stores the password, saves url and user, runs `ensure_server`, and starts a cycle. Unreachable -> `Offline` (index kept, retried every `offline_retry`). Credential, TLS, not-Subsonic errors -> `Error` with the failure, no automatic retry until `sync_now` or `connect`. Disconnect deletes the keychain entry, clears url and user, wipes the Navidrome rows, state `Off`. A saved account whose keychain entry is missing -> `Error` with `wrongCredentials`.

- [ ] **Step 1: Write the failing tests** (fake server, `MemoryStore`, temp settings file, recording emit, short `ServiceOptions`): `connect_stores_password_in_keychain_only` (settings file has url and user, no password text anywhere in it), `connect_with_wrong_password_saves_nothing`, `startup_with_saved_account_syncs` (status reaches `Online`, tracks indexed), `server_down_goes_offline_and_keeps_index` (`Refuse` + `sync_now`: `Offline`, count unchanged; `restart` -> back `Online` within two retries), `switching_server_wipes_old_rows` (connect to a second fake server), `disconnect_wipes_and_forgets`, `status_events_carry_no_password` (serialized events never contain `demo-pass`), `missing_keychain_entry_is_an_error_state`.
- [ ] **Step 2: Run** `cargo test --manifest-path src-tauri/Cargo.toml navidrome::tests` -> FAIL.
- [ ] **Step 3: Implement** the service and wire it into `setup`.
- [ ] **Step 4: Run** the full `cargo test` -> PASS.
- [ ] **Step 5: Commit** `PRISM-3: Navidrome service with background sync, status and connect commands`.

### Task 6: Navidrome covers through the resize gate

**Files:**
- Create: `src-tauri/src/navidrome/covers.rs`
- Modify: `src-tauri/src/thumbs.rs`, `src-tauri/src/protocol.rs`, `src-tauri/src/media_controls.rs`, `src-tauri/src/lib.rs`

**Interfaces:**
- Consumes: `SubsonicClient::cover_art` (Task 2), `NavidromeService` (Task 5), `navidrome_cover_raw` (Task 1).
- Produces:
  - `thumbs.rs`: `pub trait RemoteCovers: Send + Sync { fn fetch(&self, cover_art: &str, size: u32) -> Result<Vec<u8>, ResizeError>; }`, `ResizeError::Unreachable` (new), `ResizeGate::set_remote(&self, remote: Arc<dyn RemoteCovers>)`.
  - For an `nd-` id with no cached file, the gate fetches the cover at the requested size (96, 300 or 600), then flattens, bounds and JPEG-encodes it exactly like a local cover into `<cache>/<size>/<id>.jpg`. Same-key requests share one fetch; at most 6 fetches run at once, and a fetch does not hold a resize permit while it waits on the network. `Unreachable` and `Unknown` results write nothing.
  - `navidrome::covers::ServerCovers(Arc<NavidromeService>)`: returns `Unreachable` without a request while the service is `Offline` or `Off`; `NotFound` -> `Unknown`; other API errors -> `Unreachable`.
  - `protocol::handle`: `Unreachable` -> 503 with `Cache-Control: no-store`.
- Grouped minors: an `InFlight` entry is removed by a drop guard, so a panic in fetch or resize wakes every waiter with `Internal` and the next call tries again (M2 Task 6); `protocol::register` and `media_controls` use `try_state` (M2 Task 22); `media_controls` remembers a failed `(track id, cover id)` for 60 s instead of retrying it on every push (M2 Task 18).

- [ ] **Step 1: Write the failing tests:** `remote_cover_fetched_once_per_size` (16 threads on `nd-al-1` at 300: one fetch; 96 is a separate fetch), `remote_cover_is_requested_at_the_needed_size` (fake server records `getCoverArt` with `size=300`), `offline_remote_is_not_cached` (`Unreachable`, nothing written; a later success writes the file), `panic_in_fetch_does_not_leak_inflight`, `protocol_maps_unreachable_to_503_no_store`, `non_hex_local_ids_still_rejected`, `offline_service_makes_no_cover_requests`.
- [ ] **Step 2: Run** `cargo test --manifest-path src-tauri/Cargo.toml thumbs:: protocol:: navidrome::covers` -> FAIL.
- [ ] **Step 3: Implement**; `setup` calls `thumbs.set_remote(Arc::new(ServerCovers(..)))`.
- [ ] **Step 4: Run** -> PASS.
- [ ] **Step 5: Commit** `PRISM-3: Navidrome covers fetched at the needed size and cached on disk`.

### Task 7: Progressive HTTP stream buffer

**Files:**
- Create: `src-tauri/src/audio/http.rs`
- Modify: `src-tauri/src/audio/source.rs`, `src-tauri/src/audio/mod.rs`, `src-tauri/src/lib.rs` (spill dir cleanup at startup)

**Interfaces:**
- Consumes: `SecretUrl` (Task 2), the fake server (Task 2).
- Produces:
  - `source.rs`: `pub enum Input { File(PathBuf), Http(HttpInput) }`; `#[derive(Clone)] pub struct HttpInput { pub url: SecretUrl, pub label: String, pub extension: Option<String>, pub config: Arc<StreamConfig> }` with `PartialEq` on `url` and `label` and a `Debug` that prints the label only. `describe(input)` uses the label.
  - `http.rs`: `pub struct StreamConfig { pub client: reqwest::blocking::Client, pub spill_dir: PathBuf, pub stall: Duration, pub retries: u32 }`; `STALL_TIMEOUT = 20 s`, `RETRIES = 3` (backoff 1, 2, 4 s), `RESTART_GAP = 1 MiB`.
  - `pub struct HttpStream` implementing `Read`, `Seek` and symphonia's `MediaSource` (`is_seekable`, `byte_len`). `HttpStream::open(input: &HttpInput) -> anyhow::Result<HttpStream>` (returns after the response headers), `seekable(&self) -> bool`, `len(&self) -> Option<u64>`, `cursor(&self) -> HttpStream` (a second reader at 0 over the same buffer).
  - `#[derive(Debug)] pub struct Offline(pub String)` (`std::error::Error`) and `pub fn is_offline(err: &anyhow::Error) -> bool` (any error in the chain is `Offline`); re-exported from `audio`.
  - `pub(crate) struct RangeSet` with `insert(start, end)`, `covers(start, end) -> bool`, `run_end(from) -> Option<u64>`.
  - `pub fn cleanup_spill_dir(dir: &Path)` removes `.part` files older than one day.
- Rules: one download thread per stream writes 64 KiB chunks into `<spill_dir>/<random>.part` (deleted when the last cursor drops) and records them in the `RangeSet`; readers wait on a `Condvar` up to `stall`. A read more than `RESTART_GAP` past the downloaded run, or before it, on a seekable stream restarts the download with `Range: bytes=<pos>-`. A body error or a body shorter than `Content-Length` is retried with a Range from the end of the run (seekable) up to `retries` times; otherwise the error surfaces as `Offline` once the buffered bytes are read. Connect / DNS / timeout failures are `Offline`; HTTP errors are plain errors.

- [ ] **Step 1: Write the failing tests:** `reads_the_whole_body` (2 MB generated body equals the source bytes), `seek_ahead_restarts_with_a_range_request`, `seek_back_inside_the_buffer_makes_no_request`, `cut_connection_resumes_with_range` (`CutOnce` after 100 KB: full body intact, 2 requests, the second with `Range`), `cut_without_range_support_reports_offline_after_buffer` (`NoRanges` + `CutOnce`), `stall_reports_offline_within_the_timeout` (`stall` 300 ms: error within 1 s, `is_offline` true), `refused_connection_is_offline`, `http_404_is_not_offline`, `spill_file_removed_on_drop`, `range_set_merges_and_reports_runs`, `input_debug_hides_url` (`format!("{:?}", Input::Http(..))` has no `http`, `t=` or token).
- [ ] **Step 2: Run** `cargo test --manifest-path src-tauri/Cargo.toml audio::http` -> FAIL.
- [ ] **Step 3: Implement.** Existing `let Input::File(path) = input;` sites become `match` arms returning an error for `Http` until Task 8.
- [ ] **Step 4: Run** the full `cargo test` -> PASS.
- [ ] **Step 5: Commit** `PRISM-3: Progressive HTTP stream buffer with range resume and offline detection`.

### Task 8: Stream opener: symphonia and FFmpeg over HTTP, quality, transcoded seek

**Files:**
- Create: `src-tauri/src/navidrome/stream.rs`
- Modify: `src-tauri/src/audio/symph.rs`, `src-tauri/src/audio/ffmpeg.rs`, `src-tauri/src/audio/source.rs`, `src-tauri/src/lib.rs`

**Interfaces:**
- Consumes: `HttpStream`, `HttpInput`, `Offline` (Task 7), `SubsonicClient::stream_url` / `stream_http` (Task 2), `NavidromeService::client` (Task 5), `Settings::navidrome_quality` (Task 3).
- Produces:
  - `SymphoniaSource::open_media(source: Box<dyn MediaSource>, extension: Option<&str>, label: &str) -> anyhow::Result<Option<Self>>`; `open_if_supported` dispatches `File` and `Http` to it.
  - `ffmpeg.rs`: `pub(crate) fn ffmpeg_args(input: &Input, rate: u32, start_ms: u64) -> Vec<OsString>`. For `Http` the input is `-i pipe:0` with `-ss` after `-i`; stdin is piped and a feeder thread copies from `HttpStream::cursor()` until EOF or a broken pipe. Seeking restarts FFmpeg and the feeder over a fresh cursor (the bytes already buffered are not downloaded again).
  - `navidrome/stream.rs`: `pub fn navidrome_opener(service: Arc<NavidromeService>, settings: Arc<SettingsStore>, spill_dir: PathBuf, fallback: Opener) -> Opener` (non-Navidrome tracks go to `fallback`); `pub struct NavidromeSource` implementing `AudioSource`: byte-seekable streams seek through the inner source; others reopen with `timeOffset = ms / 1000` and drop the remaining `ms % 1000` worth of frames; a reopened stream with a different spec is an error. `duration_ms` falls back to the track's.
  - Not connected -> `Offline("Navidrome is not connected")`. The extension hint is the track's `format` for `original`, `mp3` otherwise.
- Grouped minor (M2 Task 5): dropping an FFmpeg source kills the child at once and its feeder thread ends on the broken pipe, so skip-spam leaves no detached children behind.

- [ ] **Step 1: Write the failing tests:** `plays_a_navidrome_track_end_to_end` (fake `with_audio` of `tagged.flac`: about 1 s of frames), `original_quality_requests_raw`, `transcoded_quality_requests_mp3_at_bitrate` (`192`: `format=mp3`, `maxBitRate=192`), `byte_seek_uses_range`, `transcoded_seek_uses_time_offset` (`with_wav(5 s)` + `NoRanges`; `seek(1500)` -> a request with `timeOffset=1`; frames left about 3.5 s within 5 %), `ffmpeg_fallback_reads_stdin` (`opus.opus` through the fake; skipped without ffmpeg), `ffmpeg_args_carry_no_url_or_token` (args contain `pipe:0`, none contains `http`, `rest/` or `t=`), `dropping_an_ffmpeg_stream_ends_the_feeder` (feeder exit counter within 2 s), `not_connected_is_offline`, `local_tracks_use_the_fallback`.
- [ ] **Step 2: Run** `cargo test --manifest-path src-tauri/Cargo.toml audio:: navidrome::stream` -> FAIL.
- [ ] **Step 3: Implement**; `setup` builds the player with `navidrome_opener(.., default_opener())` and the spill dir `<cache>/streams` (cleaned with `cleanup_spill_dir`). The navidrome service is created before the player for this.
- [ ] **Step 4: Run** the full `cargo test` -> PASS.
- [ ] **Step 5: Commit** `PRISM-3: Navidrome streams play through symphonia or FFmpeg, with quality and seeking`.

### Task 9: Engine: offline pauses instead of skipping; network sources stay gapless

**Files:**
- Modify: `src-tauri/src/player/engine.rs`, `src-tauri/src/player/mod.rs`, `src/lib/types.ts` (`PlayerError.kind`)

**Interfaces:**
- Consumes: `audio::is_offline` (Task 7).
- Produces:
  - `PlayerError { track_id, message, kind: ErrorKind }`, `#[serde(rename_all = "lowercase")] pub enum ErrorKind { Track, Offline }`. TS: `kind: 'track' | 'offline'`.
  - The opener result carries `OpenFailure { message: String, offline: bool }` instead of a bare `String`.
- Rules: an offline failure (open, or a decode error with `is_offline`) does not skip and does not count toward `MAX_FAILURES_IN_A_ROW`. The player stays on that queue entry, status `Paused`, position kept (mid-track) or 0 (open), one `Error` with `kind: Offline`. The next play / toggle reopens the entry and seeks to the kept position. If an offline failure hits the pre-opened next track, the current track plays to its end and the player then pauses on the next entry. Every other error behaves as in M2.

- [ ] **Step 1: Write the failing tests** with a `NetSource` fake (open delay and per-read delay; can be told to fail with `Offline` after N frames): `offline_open_pauses_on_that_track`, `offline_mid_track_pauses_at_position` (position within 300 ms of where it failed), `resume_after_offline_reopens_at_position`, `offline_errors_do_not_count_toward_the_failure_cap` (12 offline failures, then a working source plays), `offline_next_track_lets_the_current_one_finish`, `network_tracks_are_gapless` (two `NetSource`s with 30 ms read delay: one continuous run), `slow_network_open_of_next_does_not_gap` (next opens in 1 s, inside `PRELOAD_MS`), and the M2 `ordinary_errors_still_skip` path unchanged.
- [ ] **Step 2: Run** `cargo test --manifest-path src-tauri/Cargo.toml player::` -> FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** the full `cargo test` and `npm test` -> PASS.
- [ ] **Step 5: Commit** `PRISM-3: An unreachable server pauses playback instead of skipping the queue`.

### Task 10: Two-way stars

**Files:**
- Create: `src-tauri/src/navidrome/stars.rs`
- Modify: `src-tauri/src/navidrome/mod.rs` (`cycle`, flush scheduling), `src-tauri/src/commands.rs` (`favorites_set`), `src-tauri/src/library/favorites.rs`

**Interfaces:**
- Consumes: `SubsonicClient::{star, starred2}`, `StarTarget` (Task 2), `NavidromeService` (Task 5), `navidrome_star_queue` (Task 1).
- Produces:
  - `pub fn target(kind: FavKind, id: &str) -> Option<StarTarget>` (`None` for non-Navidrome ids).
  - `pub fn set_favorite(conn: &Connection, kind: FavKind, id: &str, on: bool, now_ms: i64) -> anyhow::Result<bool>`: local row (`source = 'navidrome'`) plus queue upsert; a queued opposite intent for the same item is replaced.
  - `pub struct PendingStar { pub kind: FavKind, pub item_id: String, pub star: bool, pub attempts: u32 }`, `pub fn pending(conn: &Connection) -> anyhow::Result<Vec<PendingStar>>`.
  - `pub struct FlushOutcome { pub sent: u32, pub dropped: u32, pub offline: bool }`, `pub fn flush(db: &Db, client: &SubsonicClient) -> FlushOutcome`: one request per item, oldest first, network calls outside the DB lock. Offline stops the flush (rows stay). `NotFound` drops the row and, for a star, the local favorite. Other API errors drop the row (the next import restores the server state).
  - `pub fn import(conn: &mut Connection, starred: &Starred2, now_ms: i64) -> anyhow::Result<bool>`: for items without a pending intent, adds server stars missing locally (`added_at` from the server's `starred` timestamp, else `now_ms`) and removes local Navidrome favorites the server no longer stars.
  - `favorites_set`: a `navidrome:` id goes to `stars::set_favorite` and `kick_flush()`; it fails with "Navidrome is not connected" while the service is `Off`. Local ids keep the M2 path.
  - Flush retry schedule after an offline flush: 5 s, 30 s, 2 min, then every cycle.
- Grouped minors (M2 Task 9): `favorites_set` writes through a write helper instead of `read()`; `favorites::set` (test-only) is removed in favor of `set_changed`.

- [ ] **Step 1: Write the failing tests:** `heart_writes_locally_and_queues`, `flush_sends_star_and_clears_queue` (fake `starred()` holds the song), `album_and_artist_map_to_album_id_and_artist_id`, `local_favorites_never_reach_the_server`, `toggle_twice_offline_sends_only_the_last_intent`, `unstar_while_offline_survives_restart_and_flushes` (file DB; a second service instance flushes it), `server_stars_are_imported`, `server_unstar_removes_local_favorite`, `pending_local_change_wins_over_server`, `deleted_item_drops_the_queue_entry` (fake `Code(70)` on star).
- [ ] **Step 2: Run** `cargo test --manifest-path src-tauri/Cargo.toml navidrome::stars library::favorites` -> FAIL.
- [ ] **Step 3: Implement**; add the flush and import steps to `cycle` and emit `FavoritesChanged` when the import changed anything.
- [ ] **Step 4: Run** -> PASS.
- [ ] **Step 5: Commit** `PRISM-3: Two-way Navidrome stars with an offline queue`.

### Task 11: Scrobbling

**Files:**
- Create: `src-tauri/src/navidrome/scrobble.rs`
- Modify: `src-tauri/src/navidrome/mod.rs` (`cycle`), `src-tauri/src/lib.rs` (player sinks)

**Interfaces:**
- Consumes: `PlayerEvent` (M2), `SubsonicClient::scrobble` (Task 2), `navidrome_scrobble_queue` (Task 1).
- Produces:
  - `pub const MIN_TRACK_MS: u64 = 30_000; pub const MAX_THRESHOLD_MS: u64 = 240_000;`
  - `pub enum ScrobbleAction { NowPlaying { id: String }, Submit { id: String, played_at: i64 } }` (raw Subsonic ids).
  - `pub struct ScrobbleTracker`, `ScrobbleTracker::new()`, `fn on_event(&mut self, event: &PlayerEvent, now_ms: i64) -> Vec<ScrobbleAction>`.
  - `pub fn spawn_scrobbler(service: Arc<NavidromeService>, db: Arc<Db>) -> EventSink` (own thread; `Level` events ignored; a failed `Submit` is queued; `NowPlaying` is dropped when it fails).
  - `pub fn flush_queue(db: &Db, client: &SubsonicClient) -> anyhow::Result<u32>` (oldest first, at most 50 per cycle, `submission=true&time=<played_at>`).
- Rules: only `navidrome:` tracks. `TrackChanged` starts a play (played_at = now) and emits `NowPlaying`. Listened time grows from consecutive `State` events of the same track while `Playing`, by forward position deltas of at most 2 000 ms. `Submit` fires once per play when listened >= min(duration / 2, `MAX_THRESHOLD_MS`) and duration >= `MIN_TRACK_MS`.

- [ ] **Step 1: Write the failing tests** (pure tracker): `now_playing_on_track_change`, `submits_at_half_for_short_tracks`, `submits_at_four_minutes_for_long_tracks`, `seeking_forward_does_not_count`, `pause_does_not_count`, `under_30_seconds_never_submits`, `local_tracks_are_ignored`, `submits_once_per_play`, `replaying_the_track_is_a_new_play`; with the fake server: `offline_submission_is_queued_and_flushed` (fake `scrobbles()` gets `(id, true, Some(played_at))`).
- [ ] **Step 2: Run** `cargo test --manifest-path src-tauri/Cargo.toml navidrome::scrobble` -> FAIL.
- [ ] **Step 3: Implement**; add the scrobbler to the player's `fan_out` and `flush_queue` to `cycle`.
- [ ] **Step 4: Run** -> PASS.
- [ ] **Step 5: Commit** `PRISM-3: Now playing and scrobbles to Navidrome`.

### Task 12: Navidrome playlists (backend)

**Files:**
- Create: `src-tauri/src/navidrome/playlists.rs`
- Modify: `src-tauri/src/navidrome/mod.rs` (`cycle`), `src-tauri/src/commands.rs`, `src-tauri/src/library/playlists.rs`, `src-tauri/src/lib.rs`

**Interfaces:**
- Consumes: `SubsonicClient::{playlists, playlist}` (Task 2), `PlaylistEntry` (M2), remote tables (Task 1).
- Produces:
  - `#[serde(rename_all = "camelCase")] pub struct RemotePlaylist { pub id: String, pub name: String, pub owner: Option<String>, pub track_count: u32, pub duration_ms: u64, pub cover_ids: Vec<String>, pub updated_at: i64 }` (`id` = `navidrome:<playlist id>`).
  - `pub fn sync_playlists(db: &Db, client: &SubsonicClient) -> Result<bool, SyncError>`: `getPlaylists`, then `getPlaylist` only for new playlists or a changed `changed` value; playlists gone from the server are deleted.
  - `pub fn list(conn: &Connection) -> anyhow::Result<Vec<RemotePlaylist>>` (one query, up to 4 distinct cover ids in playlist order), `pub fn entries(conn: &Connection, id: &str) -> anyhow::Result<Vec<PlaylistEntry>>`.
  - Commands `navidrome_playlists() -> Vec<RemotePlaylist>`, `navidrome_playlist_entries(id: String) -> Vec<PlaylistEntry>`. There is no write path (read-only in v1.0.0).
- Grouped minors (M2 Task 14): `playlists_list` builds its summaries in one query instead of one per playlist; `playlist_rename`, `playlist_remove` and `playlist_move` return whether anything changed and the commands emit `playlists://changed` only then.

- [ ] **Step 1: Write the failing tests:** `playlists_synced_with_entries`, `unchanged_playlist_not_refetched` (second sync: no `getPlaylist` request), `removed_playlist_disappears`, `entries_of_tracks_missing_from_index_are_kept` (`track: None`), `wipe_clears_remote_playlists`, `prism_playlist_holds_navidrome_tracks` (add a `navidrome:` id to a Prism playlist; it resolves; after a wipe the entry stays with `track: None`), `noop_rename_reports_no_change`, `playlists_list_counts_and_covers_unchanged` (the M2 expectations hold with the one-query version).
- [ ] **Step 2: Run** `cargo test --manifest-path src-tauri/Cargo.toml navidrome::playlists library::playlists` -> FAIL.
- [ ] **Step 3: Implement**; the cycle emits `PlaylistsChanged` when `sync_playlists` changed anything.
- [ ] **Step 4: Run** -> PASS.
- [ ] **Step 5: Commit** `PRISM-3: Navidrome playlists synced read-only, quieter playlist events`.

### Task 13: Flow with Navidrome

**Files:**
- Modify: `src-tauri/src/flow/mod.rs`, `src-tauri/src/flow/deck.rs`, `src-tauri/src/lib.rs`

**Interfaces:**
- Consumes: `albums.artist_id` (Task 1), `NavidromeService::online_flag` (Task 5).
- Produces: `FlowService::spawn(db: Arc<Db>, player: PlayerHandle, navidrome_online: Arc<AtomicBool>)`; `pool(conn, variant, include_navidrome: bool)`.
- Rules: the favorites pool also takes `t.album_id IN (SELECT a.id FROM albums a WHERE a.artist_id IN (SELECT item_id FROM favorites WHERE kind = 'artist'))`. While the flag is false, every variant and the padding add `t.source <> 'navidrome'`; the flag is read at every refill, so a Flow that started online stops dealing Navidrome tracks once the server goes away and picks them up again when it is back.
- Grouped minor (M2 Task 11): the deck's artist spacing compares names case-insensitively.

- [ ] **Step 1: Write the failing tests:** `navidrome_favorites_join_the_pool` (track, album and `navidrome:` artist favorites each contribute), `offline_navidrome_tracks_leave_the_pool`, `refill_follows_the_online_flag`, `spacing_ignores_artist_case`.
- [ ] **Step 2: Run** `cargo test --manifest-path src-tauri/Cargo.toml flow::` -> FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** -> PASS.
- [ ] **Step 5: Commit** `PRISM-3: Flow mixes Navidrome favorites and skips them while offline`.

### Task 14: Frontend data layer

**Files:**
- Create: `src/lib/stores/navidrome.svelte.ts`, `src/lib/stores/navidrome.spec.ts`
- Modify: `src/lib/types.ts`, `src/lib/ipc.ts`, `src/lib/ipc.spec.ts`, `src/lib/stores/library.svelte.ts`, `src/lib/stores/library.spec.ts`, `src/lib/stores/nav.svelte.ts`, `src/lib/stores/settings.svelte.ts`, `src/lib/stores/playlists.svelte.ts`, `src/lib/i18n.ts`

**Interfaces:**
- Consumes: the commands and events of Tasks 1, 5, 9, 12.
- Produces:
  - `types.ts`: `Artist { id; source; name; albumCount; trackCount; coverId }`; `StreamQuality = 'original' | '320' | '192' | '128'`; `Settings` gains `navidromeUrl: string | null`, `navidromeUser: string | null`, `navidromeQuality: StreamQuality`; `LinkState`; `NavidromeErrorCode` (the nine codes of Task 2); `NavidromeFailure { code; detail: string | null }`; `NavidromeStatus`; `ServerInfo`; `SyncProgress`; `RemotePlaylist`.
  - `ipc.ts`: `navidromeStatus()`, `navidromeTest(url, user, password)`, `navidromeConnect(url, user, password)`, `navidromeDisconnect()`, `navidromeSync()`, `navidromePlaylists()`, `navidromePlaylistEntries(id)`, `onNavidromeStatus(cb)`; `libraryTrackCount(source?: Source)`, `libraryTracksRange(offset, limit, source?: Source)`.
  - `stores/navidrome.svelte.ts`: `navidrome: NavidromeStatus` ($state, starts `off`), `initNavidrome(): Promise<() => void>`, `failureText(failure: NavidromeFailure): string` (key `navidrome.error.<code>`, `detail` appended for `server` and `notSubsonic`), `isNavidromeError(err: unknown): err is NavidromeFailure`.
  - `nav.svelte.ts`: `Nav.source?: Source`; `go(view: View, param?: string, source?: Source)` (a different source is a different place for the trail).
  - `library.svelte.ts`: `trackCounts: { all: number; local: number; navidrome: number } | null` (replaces `trackCount`; three counts per reload), `albumsOf(source?: Source): Album[]`, `artistsOf(source?: Source): Artist[]`.
  - `playlists.svelte.ts`: `playlists.remote: RemotePlaylist[] | null`, loaded with the Prism playlists on every reload.
  - `i18n.ts`: `navidrome.error.*` (9 keys) and `navidrome.offlineNotice` in `en` and `de`.

- [ ] **Step 1: Write the failing tests:** `navidrome.spec.ts` (status follows `navidrome://status`; `failureText` gives a non-empty text for every code in both languages; `isNavidromeError`), `library.spec.ts` (`albumsOf('navidrome')` filters; `trackCounts` fills from three calls), `ipc.spec.ts` (`libraryTracksRange(0, 50, 'navidrome')` invokes with `source: 'navidrome'`; without a source the arg is absent), a `nav` case (`go('albums', undefined, 'navidrome')` then `back()` returns to the all-albums view).
- [ ] **Step 2: Run** `npm test` -> FAIL.
- [ ] **Step 3: Implement**; update every `library.trackCount` use to `trackCounts.all` and every artist `{#each}` key to `artist.id`.
- [ ] **Step 4: Run** `npm test` and `npm run check` -> PASS.
- [ ] **Step 5: Commit** `PRISM-3: Frontend types, IPC and stores for Navidrome`.

### Task 15: Settings: Navidrome section

**Files:**
- Modify: `src/lib/components/Settings.svelte`, `src/lib/components/settings.spec.ts`, `src/lib/i18n.ts`

**Interfaces:**
- Consumes: Task 14.
- Produces: a "Navidrome" section between Library and Playback:
  - Not connected: Server URL (placeholder `https://music.example.com`), Username, Password (`type="password"`, never prefilled), buttons "Test connection" and "Connect". Test shows "Connected to Navidrome 0.5x (API 1.16.1)" or the mapped failure inline under the fields.
  - Connected: url and user as text, a status line (dot in the source color; "Synced 4 min ago, 31 204 songs" / "Syncing 12 000 of 31 204" / "Server not reachable, showing the saved library" / the failure text), "Sync now", "Disconnect" (inline confirm row: "Sign out and remove the Navidrome library from Prism?"), and a note that the password is kept in Windows Credential Manager / the macOS Keychain (by platform).
  - "Streaming quality": `Segmented` with Original / 320 / 192 / 128 kbps, saved as `navidromeQuality`.
  - The password field is cleared after a connect attempt, successful or not.

- [ ] **Step 1: Write the failing tests** in `settings.spec.ts` (mocked IPC): `test_connection_shows_server_version`, `wrong_credentials_show_the_mapped_message`, `connect_clears_the_password_field`, `connected_shows_status_and_sync_now`, `disconnect_needs_the_confirm_row`, `quality_segmented_saves_navidromeQuality`, `password_is_never_prefilled`.
- [ ] **Step 2: Run** `npm test -- settings` -> FAIL.
- [ ] **Step 3: Implement** with the existing Settings row, `Segmented` and button styles.
- [ ] **Step 4: Run** `npm test` and `npm run check` -> PASS. Visual check: own dev instance (constraints above) connected to `cargo test --manifest-path src-tauri/Cargo.toml serve_for_ui -- --ignored --nocapture` (Task 2), CDP screenshot of the section in both states.
- [ ] **Step 5: Commit** `PRISM-3: Navidrome settings: connection, sync status, streaming quality`.

### Task 16: Sidebar, source views, source badges and offline state

**Files:**
- Create: `src/lib/components/SourceBadge.svelte`, `src/lib/components/SourceEmpty.svelte`
- Modify: `src/lib/components/Sidebar.svelte`, `src/App.svelte`, `src/lib/components/AlbumGrid.svelte`, `src/lib/components/ArtistList.svelte`, `src/lib/components/SongList.svelte`, `src/lib/components/SearchPalette.svelte`, `src/lib/components/views.spec.ts`, `src/lib/i18n.ts`

**Interfaces:**
- Consumes: Task 14.
- Produces:
  - Sidebar: "Local files" -> `go('albums', undefined, 'local')`, "Navidrome" (no longer "soon") -> `go('albums', undefined, 'navidrome')`; a source entry is lit when `nav.source` matches; the Library entries (All) are lit only when `nav.source` is unset. The Navidrome dot is hollow with the accessible name "Navidrome (offline)" / "(sync failed)" in those states.
  - Source views: `albums`, `artists`, `songs` with `nav.source` set show the source name as title and a `Segmented` Albums / Artists / Songs switch; lists come from `albumsOf` / `artistsOf` / `libraryTracksRange(.., source)`; `SongList` gets a `source?: Source` prop. The empty check uses `trackCounts[source]`.
  - `SourceEmpty.svelte` (props `source: Source`): Navidrome not configured -> "Connect your Navidrome server" + button to Settings; first sync running -> progress line; offline with an empty index -> the offline text. Local keeps `EmptyLibrary`.
  - `SourceBadge.svelte` (props `source: Source`, `size = 10`): a dot in the source color with a 1.5 px `--surface` ring and the source name as `title` / accessible label. `AlbumGrid` and `SongList` get `showSource = false`; it is true in Home, the All views, Favorites and the search palette, false in source views.

- [ ] **Step 1: Write the failing tests** in `views.spec.ts`: `navidrome_entry_is_enabled_and_opens_its_albums`, `local_entry_passes_source_local`, `navidrome_view_lists_only_navidrome_albums`, `songs_view_requests_its_source`, `badges_in_all_views_not_in_source_views`, `not_configured_navidrome_offers_settings`, `offline_dot_has_the_offline_label`, `artist_rows_keyed_by_id` (two artists with one name, different sources, both render).
- [ ] **Step 2: Run** `npm test -- views` -> FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** `npm test` and `npm run check` -> PASS; CDP screenshot of the Navidrome albums view with fake data.
- [ ] **Step 5: Commit** `PRISM-3: Navidrome in the sidebar with its own views, source badges, offline state`.

### Task 17: Favorites, hearts and quality labels for Navidrome

**Files:**
- Modify: `src/lib/components/FavoritesView.svelte`, `src/lib/components/ArtistView.svelte`, `src/lib/format.ts`, `src/lib/format.spec.ts`, `src/lib/components/TransportStrip.svelte`, `src/lib/components/NowPlaying.svelte`, `src/lib/components/MiniPlayer.svelte`, `src/App.svelte` (offline notice), `src/lib/components/views.spec.ts`, `src/lib/i18n.ts`

**Interfaces:**
- Consumes: Tasks 9, 10, 14.
- Produces:
  - FavoritesView: artists filtered by `artist.source` with the chips and hearted by `artist.id`; the Navidrome chip appears when any Navidrome track, album or artist is a favorite (closes the M2 Task 10 note).
  - ArtistView: the heart uses the Navidrome artist's id when an artist of that name exists in Navidrome, else the name.
  - `format.ts`: `qualityLabel(track: QualityInput, stream?: StreamQuality)` and `sourceQualityLabel(track, sourceName, stream?)`. A Navidrome track with `stream` other than `original` shows `MP3 <n>` when the track is lossless, its bitrate is unknown, or its bitrate is above `<n>`; otherwise the original label. Transport, Now Playing and the mini player pass `settings.navidromeQuality`.
  - App: a `player://error` with `kind: 'offline'` shows `navidrome.offlineNotice` once instead of the error text.

- [ ] **Step 1: Write the failing tests:** `format.spec.ts` (`FLAC 24/96` at `192` -> `MP3 192`; `MP3 128` at `320` -> `MP3 128`; local tracks ignore `stream`; `original` unchanged), `views.spec.ts` (`navidrome_chip_appears_with_navidrome_favorites`, `artist_favorites_filter_by_source`, `artist_view_hearts_the_navidrome_id`, `offline_error_shows_one_notice`).
- [ ] **Step 2: Run** `npm test` -> FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** `npm test` and `npm run check` -> PASS.
- [ ] **Step 5: Commit** `PRISM-3: Navidrome favorites, artist hearts and streamed quality labels`.

### Task 18: Playlists UI: Navidrome playlists read-only

**Files:**
- Modify: `src/lib/components/PlaylistsView.svelte`, `src/lib/components/PlaylistView.svelte`, `src/lib/components/views.spec.ts`, `src/lib/i18n.ts`

**Interfaces:**
- Consumes: `playlists.remote`, `navidromePlaylistEntries` (Task 14).
- Produces:
  - PlaylistsView: a "Navidrome" section under the Prism playlists when `playlists.remote` is non-empty, cards with the mosaic cover, a small lock icon and "Read-only" label, opening `go('playlist', 'navidrome:<id>')`.
  - PlaylistView: an id starting with `navidrome:` loads `navidromePlaylistEntries` and runs read-only: Play, Shuffle, row play, add to queue and "Add to playlist" (into a Prism playlist) stay; rename, delete, drag, remove and the Delete key are absent; the header shows the owner.

- [ ] **Step 1: Write the failing tests:** `navidrome_section_lists_remote_playlists`, `remote_playlist_is_read_only` (no rename field, no delete, no drag handles; Delete key removes nothing), `remote_playlist_plays_its_entries` (`playerPlay` with the resolved ids, missing ones skipped), `remote_tracks_can_go_into_a_prism_playlist`.
- [ ] **Step 2: Run** `npm test -- views` -> FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** `npm test` and `npm run check` -> PASS.
- [ ] **Step 5: Commit** `PRISM-3: Navidrome playlists shown and playable, read-only`.

### Task 19: Scale checks and the live smoke test

**Files:**
- Modify: `src-tauri/src/navidrome/sync.rs` (tests), `src-tauri/src/library/query.rs` (tests), `src-tauri/src/navidrome/client.rs` (tests)

**Interfaces:**
- Consumes: Tasks 1, 2, 4.
- Produces tests only:
  - `sync_30k_synthetic` (`#[ignore]`, run with `--release`): `FakeLibrary::synthetic(3000, 10)` first sync, prints the duration; budget 60 s.
  - `search_with_30k_tracks_under_100ms` and `songs_page_with_30k_under_20ms` (`#[cfg_attr(debug_assertions, ignore)]`): 30 000 navidrome rows plus 2 000 local rows; `query::search` for a two-word query and `tracks_range(.., 15000, 200, Some(Navidrome))` under their budgets (median of 5).
  - `live_server_smoke` (`#[ignore]`): reads `PRISM_LIVE_SUBSONIC_URL`, `PRISM_LIVE_SUBSONIC_USER`, `PRISM_LIVE_SUBSONIC_PASSWORD` (skips with a message when unset); read-only: ping, one `getAlbumList2` page of 5, one `search3` page, one `getCoverArt` at 96, the first 64 KiB of one stream. Never stars, scrobbles or writes. Prints counts and timings only, never names or URLs.

- [ ] **Step 1: Write the tests.**
- [ ] **Step 2: Run** `cargo test --manifest-path src-tauri/Cargo.toml --release -- --ignored sync_30k_synthetic search_with_30k songs_page_with_30k` -> PASS within budget (a miss is a finding: fix the query or index before going on, never raise the budget).
- [ ] **Step 3: Run** the full `cargo test`, `npm test`, `npm run check`, clippy -> PASS.
- [ ] **Step 4: Commit** `PRISM-3: Scale tests for a 30k-track index and an opt-in live smoke test`.

### Task 20: Owner hand check

- [ ] **Step 1:** Build a release-mode dev bundle the owner starts himself (agents never start it against the real data dir, the real identifier or the real server).
- [ ] **Step 2:** Ask the owner to check with his own Navidrome (~30k tracks) and note the numbers: time of the first sync (budget 60 s on the LAN), scrolling the Navidrome Songs list and the album grid, search speed, an album opening instantly, streaming at Original and at 192, seeking far into a FLAC and into a transcoded track, gapless album playback and a Flow crossfade with Navidrome tracks, starring in Prism appearing in the Navidrome web UI and the reverse after "Sync now", a scrobble showing in Navidrome, Navidrome playlists, and the offline case (stop the server or unplug the network mid-track: one notice, paused, library still browsable; reconnect: play resumes). Also: the password is not in `settings.json`, and the M2 close-to-mini behavior still holds.
- [ ] **Step 3:** Collect feedback, fix in worktrees, merge, and record the outcome in the ledger and the Prism memory entry. Then write the M4 plan against the real code.

---

## Deferred M2 minors

Folded in: artists without a source (Task 10 note) -> Tasks 1, 17; InFlight leak on panic, bare `state::<AppState>()` in `protocol.rs`, `cover_for` retry on every push -> Task 6; FFmpeg children left behind by skip-spam -> Task 8; `favorites_set` via `read()`, test-only `favorites::set` -> Task 10; playlists N+1 summaries and events on no-op changes -> Task 12; deck artist spacing byte-exact -> Task 13.

Left out (code M3 does not otherwise touch, or not worth the risk now): scanner and watcher items (transient read failures cached, prune on early returns, nested changes in offline folders, folder key normalization, case-differing paths, guard on a deliberately emptied folder); rekey merge details (winner by rowid, lost play counts, A->B->C chain test); startup and quarantine items (backup before VACUUM, non-atomic quarantine, `rfd` features, dialog over a visible window); engine and crossfade timing items (track change at fade start, outgoing gain frozen, device switch cuts the fade, `Append` resets `Skipped`, out-of-range Jump, timing-sensitive drop tests); level meter and mini player items (meter after device loss, hidden mini web view cost, on-top bit, macOS NSPanel, which belongs to M6); window region and tray items; Now Playing and drag-and-drop UI items; `settings_set` reading `before` outside the lock; the second reqwest copy (0.13 comes from Tauri; Prism stays on the 0.12 line as instructed); the old `_placeholder.jpg` orphan; cover lookup retry timer; Flow staying set on an idle queue.

## Execution notes

- Order and waves (each wave from the merged result of the previous one): A = 1, 2, 3 (independent); B = 4, 6, 7; C = 5, 8; D = 9, 10, 11, 12, 13; E = 14; F = 15, 16, 17, 18; G = 19; then 20. Merge in task number order inside a wave; Task 1 is the only migration.
- Tasks 5, 8, 10, 11, 12 and 13 all touch `lib.rs` / `navidrome/mod.rs` `cycle`; the controller resolves those merges by keeping both sides in the order the Task 5 `cycle` rule gives.
