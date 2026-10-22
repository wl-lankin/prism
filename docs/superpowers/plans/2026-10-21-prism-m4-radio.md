# Prism M4 (Radio) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Internet radio becomes a source: "Radio near you" opens on stations close to the user, a worldwide search finds any station by name, country, region or genre, stations can be favorited, and a playing station shows its current title from the stream. Stations play through the native engine like tracks, survive short drops without a sound, pause with one notice when the internet is gone, and never mix into the music library, Flow, playlists or play statistics.

**Architecture:** A new `src-tauri/src/radio/` module owns everything radio: a blocking Radio Browser client with mirror rotation, a coarse location (IP lookup or a manual country and region), the "near you" ranking, the station store in SQLite (only stations the user saved or played, plus favicon URLs) and one `RadioService` that answers the commands, reports clicks and refreshes saved stations. A station plays as a `Track` with `source = 'radio'` and id `radio:<stationuuid>` that the resolver builds from `radio_stations`, so the queue, the engine, media controls, the mini player and the Flow ring need no second path. Streams enter the engine as a new `Input::Live`: a bounded in-memory ring fed by one download thread that strips ICY metadata (the titles go to a title sink, not through the engine), resolves `.pls` / `.m3u` once, and reconnects on short drops. HLS stations go to FFmpeg by URL. The engine gains a live mode: no duration, no seek, no pre-open, no crossfade, `next` does nothing, and pause closes the connection so Play always joins the stream live.

**Tech Stack:** M3 stack, no new crates. `image` gains the `ico` feature (about 7 % of favicons are `.ico`). The fake Radio Browser, geo and ICY servers are hand-rolled `TcpListener` servers like the M3 fake Subsonic server, so they can stall and cut sockets.

**Spec:** `docs/superpowers/specs/2026-10-20-prism-design.md` (5.1, 5.3, 5.5, 5.7, 5.9, 5.10, 5.12, 8). Roadmap: `docs/superpowers/plans/2026-10-20-prism-m1-foundation.md` (row M4). Previous plan and its constraints: `docs/superpowers/plans/2026-10-21-prism-m3-navidrome.md`. Ledgers with rulings and deferred minors: `docs/superpowers/handoff/ledgers/`.

## Global Constraints

- Platforms: Windows 11 and macOS 11+ (universal). The owner now develops on both machines; anything platform-only is behind `cfg(target_os = ...)` with a counterpart or a no-op, and every such pair is named in its task.
- Repo root: `C:\Projects\private\Prism` on Windows, `~/Projects/Prism` on the Mac. Work on the branch `PRISM-4`, cut from the `PRISM-3` HEAD on 2026-10-21 (before the M3 hand check; its fixes are merged in later). Parallel worktrees use branches `PRISM-4NN` (NN = task number) cut from `PRISM-4` and merge back with `--no-ff` in task order. Repo-local identity `Wolfgang Linz <27097016+wl-lankin@users.noreply.github.com>`. Commit, never push. Commit messages `PRISM-4: <Description>`.
- App identifier `de.wolfganglinz.prism`, productName `Prism`, version `1.0.0`, MIT, publisher "Wolfgang Linz", homepage `https://wolfgang-linz.de`.
- No em dash or en dash anywhere (code, UI strings, docs, commits). No AI attribution or session links anywhere. No sparkle glyphs.
- Palette: background `#0d0d10`, surface `#141418`, raised `#1f1f25`, text `#f3f3f5`; spectrum `#ff4d6d`, `#ffc94d`, `#4dffb2`, `#4da6ff`. Source colors: navidrome `#4dffb2`, apple `#ff4d6d`, youtube `#ffc94d`, radio `#4da6ff`, local `#cccccc`.
- Reduced motion: only large motion is switched off; hover and fade transitions keep running at 120 ms (`--dur`). Endless loops (a live pulse, breathing dots) stop.
- Window look and closing behavior as in M3 (no Mica, no vibrancy, no OS shadow; closing the main window hides it with its taskbar button and opens the mini player). Nothing in M4 may regress this.
- The mini player never steals focus and never appears without a user action.
- Every dev run sets `PRISM_DATA_DIR` to a fresh temp folder AND passes a Tauri `--config` override with its own identifier (`de.wolfganglinz.prism.dev<n>`). Never touch the real data dirs (`%APPDATA%` / `%LOCALAPPDATA%\de.wolfganglinz.prism` on Windows, `~/Library/Application Support/de.wolfganglinz.prism` and `~/Library/Caches/de.wolfganglinz.prism` on the Mac).
- Agents never inject OS input and never move or focus the owner's windows. Visual checks only through CDP screenshots and DOM events of the agent's own instance (on the Mac, WKWebView has no CDP: visual checks there go to the owner hand check). Real-input checks go to the owner hand check (Task 13).
- Ports: the main copy uses Vite 1440 / HMR 1441. Parallel agents use 1500-1599 through an uncommitted override config they delete afterwards. Never kill processes by image name; stop only the PIDs you started.
- Write files with Write/Edit, never Bash heredocs. Frontend tests are `src/**/*.spec.ts`.
- Commands: `npm test`, `npm run check`, `cargo test --manifest-path src-tauri/Cargo.toml [filter]`, `cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings`, `cargo fmt --check`. Use exact test names for anything that can make sound (M3 incident: a broad filter played a tone).
- Migrations are append-only: the owner has a real library DB. M4 adds exactly one, `0009_radio.sql` (Task 1). Never edit an earlier migration.
- Network: M4 adds these hosts and no others: the Radio Browser mirrors (`*.api.radio-browser.info`), the location services (`get.geojs.io`, fallback `ipinfo.io`, only in automatic location mode), the stream host of a station the user plays, and favicon hosts of stations shown on screen. Every request sends `User-Agent: Prism/<version>`. Test servers bind `127.0.0.1:0` only. No test, except the one `#[ignore]` live smoke test (Task 12), reaches the internet.
- Location privacy: the detected location is rounded to one decimal (about 10 km) before it is stored, lives only in the local DB, and never appears in logs, events other than `radio_location`, error texts, fixtures or screenshots. Manual mode never calls a location service. Tests use the fake place "Testhausen" (`countryCode: "DE"`, region "Testland", lat 50.0, lon 10.0).
- Radio never enters the music library: no rows in `tracks`, `albums`, `tracks_fts`, `track_stats`, playlists or the Navidrome scrobble queue; Flow pools never see a station; the "All" views do not change.
- Every new UI string goes into both `en` and `de` in `src/lib/i18n.ts` (`i18n.spec.ts` checks keys, placeholders and dashes).
- Never use the owner's real library, server, accounts, credentials or location in tests, fixtures or screenshots.

## Review Focus

1. **A station drops mid-stream, or the internet goes away, with Radio playing:** short drops are bridged without a notice (reconnect, the ring keeps playing what it holds). When the station stays away for 20 s, the player pauses with one notice that names the station, never skips, never loops reconnect notices. Play joins the stream live again. Pinned in Task 4 (`reconnects_after_a_cut_without_an_error`, `gives_up_after_the_retry_window`) and Task 6 (`live_offline_pauses_with_one_notice`, `play_after_offline_joins_live`).
2. **An endless stream for hours:** memory and disk stay constant. No spill file, a fixed ring, metadata buffers capped. Pinned in Task 4 (`ring_stays_bounded_over_64_mib`, `no_files_written`).
3. **Hostile or broken directory data:** station names with control characters or 2 000 characters, tags lists of 300 entries, a favicon of 50 MB or an HTML page, a `.pls` that points at another `.pls` forever, an HTML error page instead of audio, ICY metadata blocks with garbage or a `metaint` of 0 or 10^9. Nothing crashes, nothing grows without bound, and a station that is not audio fails with a clear error. Pinned in Tasks 2, 4, 7.
4. **Radio never leaks into the music library** (see Global Constraints). Pinned in Task 1 (`stations_never_touch_tracks`), Task 6 (`radio_plays_are_not_recorded_or_scrobbled`) and Task 7 (`flow_never_deals_a_station`).
5. **Location privacy:** coordinates rounded and stored locally only; manual mode makes no location request; nothing personal in logs. Pinned in Task 3 (`manual_mode_never_calls_the_service`, `detected_location_is_rounded`, `errors_carry_no_coordinates`).
6. **The directory is down:** the next mirror is tried; with all mirrors down, saved and favorite stations still play from their stored URLs, "near you" shows the last cached list with a hint, and search says it cannot reach the directory. Pinned in Task 2 (`rotates_to_the_next_mirror`) and Task 7 (`saved_stations_play_without_the_directory`, `near_list_falls_back_to_cache`).

---

## File Structure

```
src-tauri/src/db/migrations/0009_radio.sql   radio_stations, radio_favicons, radio_state
src-tauri/src/radio/mod.rs                    RadioService: commands, station store, clicks,
                                              refresh, near list cache, now-playing title
src-tauri/src/radio/model.rs                  Station, StationRef, ids, name cleanup, quality
src-tauri/src/radio/directory.rs              Radio Browser client: mirrors, search, near, lists
src-tauri/src/radio/near.rs                   the "near you" ranking (pure)
src-tauri/src/radio/location.rs               IP lookup (GeoJS, ipinfo), manual override, cache
src-tauri/src/radio/store.rs                  SQL: stations, favicons, state
src-tauri/src/radio/covers.rs                 FaviconCovers for the resize gate (rb- covers)
src-tauri/src/radio/stream.rs                 radio opener, RadioSource (decoder + reopen)
src-tauri/src/radio/testserver.rs             #[cfg(test)] fake Radio Browser and geo servers
src-tauri/src/audio/live.rs                   LiveStream: ring buffer, ICY demux, playlists,
                                              reconnect, Offline marker
src-tauri/src/audio/live/testserver.rs        #[cfg(test)] fake ICY / playlist / HLS server
src-tauri/tests/fixtures/radio/*.json         hand-written Radio Browser and geo responses
src/lib/stores/radio.svelte.ts                near list, search, location, titles, saved
src/lib/components/RadioView.svelte           "Radio near you", search, saved and recent
src/lib/components/StationList.svelte         virtualized station rows
src/lib/components/StationTile.svelte         favicon on a tile, or initials in radio blue
src/lib/components/LivePill.svelte            "LIVE" in place of the seek bar
```

Modified: `library/{model,favorites,history,query}.rs`, `audio/{mod,source,ffmpeg}.rs`, `player/{engine,mod}.rs`, `navidrome/scrobble.rs` (test only), `flow/mod.rs` (test only), `thumbs.rs`, `media_controls.rs`, `settings.rs`, `commands.rs`, `lib.rs`, `Cargo.toml`; frontend `types.ts`, `ipc.ts`, `format.ts`, `i18n.ts`, `stores/{nav,favorites,player,notice}`, `Sidebar`, `Settings`, `FavoritesView`, `SearchPalette`, `TransportStrip`, `NowPlaying`, `MiniPlayer`, `FlowRing`, `QueuePanel`, `TrackMenu`, `Heart`, `App.svelte` (home row, notices).

## Decisions pinned here

- **Ids.** A station is `radio:<stationuuid>` (lowercase UUID, validated). Its cover id is `rb-<first 20 hex of sha1(favicon url)>`; the URL behind it lives in `radio_favicons`. Favorites use the new kind `station` with `source = 'radio'`.
- **What is stored.** Only stations the user played or saved (favorite) go into `radio_stations`, with every field the player needs (name, `url`, `url_resolved`, codec, bitrate, hls, favicon, country code, region, tags, homepage). Search and near results live in memory; their favicon URLs are written to `radio_favicons` so their tiles can load. Recently played keeps the last 30 non-favorite stations; older ones are pruned.
- **A station as a track.** The resolver builds `Track { id, source: Radio, title: <station name>, artist: <region, country>, album: "", album_id: "", duration_ms: 0, format: <codec>, bitrate_kbps: <bitrate or None>, cover_id: <rb- id>, path: None, replay gain None }`. Duration 0 plus `source = Radio` is what the engine calls live.
- **Directory.** Mirrors come from `https://all.api.radio-browser.info/json/servers` (names, deduplicated), cached in `radio_state` for 7 days, with the built-in fallback `de1.api.radio-browser.info`. The list is shuffled once per start; a request that fails as offline or with 5xx moves to the next mirror; 4xx does not. Every search sends `hidebroken=true`. Stations whose codec mentions a video codec (`H.264`, `H.265`, `VP8`, `VP9`) are dropped. Clicks (`/json/url/<uuid>`) are reported once per play start, best effort, never twice for one station within 24 h from this app.
- **Names.** Display names are cleaned once: control characters removed, whitespace collapsed, leading and trailing decoration (`-`, `#`, `*`, `|`, `>`, `~`, `_`, `.`, spaces) trimmed, capped at 80 characters on a char boundary with `...`. A station whose cleaned name is empty is dropped. Tags are lowercased, split on commas, trimmed, deduplicated, at most 8 kept.
- **Near you.** Radio Browser has no distance order, and only about a quarter of stations carry coordinates, so the list is built from three bands, each ordered by `clickcount` (then `votes`): (1) stations with coordinates within 75 km, (2) stations of the same region (`state`, matched case-insensitively against `/json/states` of the country), (3) the country's most clicked stations. Duplicates (same uuid, or same cleaned name plus `url_resolved`) keep their first place. At most 200 stations. The list is cached in `radio_state` for 6 hours and per location; a refresh happens on view open when it is stale, or on "Refresh".
- **Location.** Setting `radioLocation`: `{ "mode": "auto" }` (default) or `{ "mode": "manual", "countryCode": "DE", "region": "Bavaria" | null }`. Auto asks GeoJS (`https://get.geojs.io/v1/ip/geo.json`), and `https://ipinfo.io/json` when GeoJS fails, the first time the radio view needs it (not at app start, so nobody who never opens Radio is located) and again when the stored value is older than 7 days. Nothing found: the system locale's country (`de-DE` -> `DE`) without coordinates, so bands 2 and 3 still work. Manual mode skips band 1 and never calls a location service.
- **Live stream.** `Input::Live(LiveInput)`. One download thread per stream, `Icy-MetaData: 1`, connect timeout 8 s, read stall 10 s. The body goes into a 1 MiB ring (about 26 s at 320 kbps); a full ring blocks the download thread, it never drops audio and never spills to disk. ICY blocks are cut out before the ring; `StreamTitle='...'` goes to the title sink (UTF-8, else Latin-1, at most 256 chars, control characters removed, an empty or unchanged title is not reported). Playlists: a response of type `audio/x-scpls`, `audio/x-mpegurl`, `audio/mpegurl`, `application/pls+xml` or a body starting `[playlist]` / `#EXTM3U` without `#EXT-X-` is parsed once for its first http(s) entry (one level, no loop). `text/html` or another non-audio type is a plain error "This station is not sending audio". https to http redirects are allowed for radio (many stations still stream over http; nothing secret is sent), at most 5 redirects.
- **Reconnect.** A cut, a stall or a connection error is retried after 0.5, 1, 2, 4, 8 s while the total outage stays under 20 s; the ring keeps feeding the decoder meanwhile. A reconnect continues the byte stream for MP3 and ADTS AAC; for Ogg and anything FFmpeg decodes, `RadioSource` opens a fresh decoder on the new connection. After 20 s, the read fails with `Offline`, and the engine holds the station paused (one notice). A plain error (404, 410, HTML) fails at once.
- **HLS.** `hls = 1` stations, and responses whose body starts `#EXTM3U` with `#EXT-X-`, open through FFmpeg with the URL as input (`-reconnect 1 -reconnect_streamed 1 -reconnect_delay_max 4 -rw_timeout 10000000`). Radio URLs carry no Prism secret, so this does not break the M3 rule that FFmpeg never sees a Navidrome URL. HLS stations show no stream title in v1.0.0.
- **Engine live mode.** A live track has no duration and no seek, the seek bar shows LIVE, `position_ms` counts the time heard. No pre-open and no crossfade into or out of it (a hard switch). `next` and `prev` do nothing while a station plays (spec 5.9; the queue holds only that station). Pause closes the source and the connection (status Paused, held like an offline hold); Play opens the station fresh, so it never plays minutes-old audio. ReplayGain is 1.0 for stations. A station's plain open error goes through the normal error path (the queue of one ends, the player goes idle) and now shows a notice (closes the M2 gap: `kind: 'track'` errors had no UI).
- **Offline notice per source.** `PlayerError` gains `source: Source`. `OFFLINE_MESSAGE` stays for Navidrome; a radio offline error carries "The station is not reachable". The frontend picks `navidrome.offlineNotice` or `radio.offlineNotice` (with the station name) by `source`.
- **Titles.** The opener's title sink goes to the `RadioService`, which keeps the current title per station id, emits `radio://now-playing` `{ stationId, title | null }`, and hands it to the media bridge. A title belongs to the station that is current in the player: the frontend shows it only while `player.track.id === stationId`. `radio_now_playing()` returns the current pair for windows that load later (the mini player).
- **Favicons.** The resize gate gets one remote per cover prefix (`nd-` Navidrome, `rb-` radio). `FaviconCovers` fetches http(s) only, connect timeout 5 s, total 10 s, at most 1 MiB, at most 3 redirects, decodes with `image` (png, jpeg, gif, webp, bmp, ico); svg and anything that does not decode is `Unknown` (cached as missing for 7 days, so it is not asked again on every scroll). At most 4 favicon fetches run at once. **Homepage fallback (owner decision, 2026-10-21):** when a station has no favicon, or its favicon does not load or decode, and it has an http(s) homepage, Prism reads the homepage (at most 512 KiB of HTML, timeout 8 s, at most 3 redirects) and tries, in this order: `<link rel="apple-touch-icon">` (largest `sizes`), `<link rel="icon">` / `rel="shortcut icon"` (largest `sizes`, png before ico, svg skipped), `<meta property="og:image">`, then `/favicon.ico` of the homepage's origin. The first one that decodes wins. The HTML is scanned with a small bounded tag scanner (no HTML crate, no script runs). A station without favicon gets its cover id from its homepage (`rb-<sha1("home:" + homepage)>`). The tile shows the logo contained on a raised square, or the station's initials in radio blue.
- **Favorites and the rest of the app.** A heart on a station is a local favorite (`station`, source `radio`) and stores the station. Favorites gets a Stations section and a Radio chip. Home shows a "Your stations" row when any station is a favorite. Ctrl+K gets a Stations group: saved stations match at once, the directory answers after 300 ms debounce with at most 8 results (silent when the directory is down). "Add to playlist", "Add to queue", "Go to album" and "Go to artist" are absent for stations. Flow ignores stations (spec 5.8); starting Flow while a station plays replaces it.

---

### Task 1: Schema 0009, Source::Radio, the station model and favorites of kind station

**Files:**
- Create: `src-tauri/src/db/migrations/0009_radio.sql`, `src-tauri/src/radio/mod.rs` (module list only), `src-tauri/src/radio/model.rs`, `src-tauri/src/radio/store.rs`
- Modify: `src-tauri/src/db/mod.rs`, `src-tauri/src/library/model.rs`, `src-tauri/src/library/favorites.rs`, `src-tauri/src/lib.rs` (`pub mod radio;`), `src/lib/types.ts`

**Interfaces:**
- Produces:
  - `0009_radio.sql`: `radio_stations (id TEXT PRIMARY KEY, name TEXT NOT NULL, url TEXT NOT NULL, url_resolved TEXT, homepage TEXT, favicon TEXT, cover_id TEXT, country_code TEXT, region TEXT, language TEXT, tags TEXT NOT NULL DEFAULT '', codec TEXT, bitrate_kbps INTEGER, hls INTEGER NOT NULL DEFAULT 0, last_check_ok INTEGER NOT NULL DEFAULT 1, saved_at INTEGER NOT NULL, refreshed_at INTEGER, last_played_at INTEGER, last_click_at INTEGER)`, `idx_radio_stations_played ON radio_stations (last_played_at)`; `radio_favicons (cover_id TEXT PRIMARY KEY, url TEXT, homepage TEXT, missing_until INTEGER)` (at least one of `url` and `homepage` is set); `radio_state (key TEXT PRIMARY KEY, value TEXT NOT NULL)`. House-style comments (no foreign keys; a station row is kept while it is a favorite or among the recent 30).
  - `library::model::Source` gains `Radio` (`"radio"`); `from_db("radio")`. Every exhaustive `match` on `Source` is updated; `query.rs` filters that mean "local or Navidrome" stay as they are (radio has no rows there).
  - `radio::model`: `pub const RADIO_PREFIX: &str = "radio:"`, `pub fn station_id(uuid: &str) -> Option<String>` (UUID shape only, lowercased), `pub fn station_uuid(id: &str) -> Option<&str>`, `pub const RADIO_COVER_PREFIX: &str = "rb-"`, `pub fn favicon_cover_id(url: &str) -> Option<String>` (http(s) URLs up to 2 048 bytes only), `pub fn clean_name(raw: &str) -> Option<String>`, `pub fn clean_tags(raw: &str) -> Vec<String>`.
  - `#[serde(rename_all = "camelCase")] pub struct Station { id, name, url, url_resolved: Option<String>, homepage: Option<String>, favicon: Option<String>, cover_id: Option<String>, country_code: Option<String>, region: Option<String>, language: Option<String>, tags: Vec<String>, codec: Option<String>, bitrate_kbps: Option<u32>, hls: bool, last_check_ok: bool, distance_km: Option<f32> }` and `impl Station { pub fn as_track(&self) -> Track }` (the shape in Decisions; `artist` = "<region>, <country name>" from a small built-in ISO table for the 60 most common countries, else the code).
  - `radio::store`: `upsert_station(conn, &Station, now_ms) -> Result<()>`, `station(conn, id) -> Result<Option<Station>>`, `track(conn, id) -> Result<Option<Track>>`, `remember_favicons(conn, &[Station]) -> Result<()>`, `favicon_source(conn, cover_id) -> Result<Option<FaviconSource { url: Option<String>, homepage: Option<String> }>>`, `pub fn station_cover_id(favicon: Option<&str>, homepage: Option<&str>) -> Option<String>` (in `model.rs`: the favicon's id, else the homepage's), `mark_favicon_missing(conn, cover_id, until_ms)`, `mark_played(conn, id, now_ms)`, `recent(conn, limit) -> Result<Vec<Station>>`, `prune(conn, keep_recent: usize) -> Result<usize>` (deletes rows that are neither a favorite nor among the newest `keep_recent` played), `state_get(conn, key) -> Result<Option<String>>`, `state_set(conn, key, value)`.
  - `FavKind::Station` (`"station"`); `FavoriteIds.stations: Vec<String>`; `favorites::stations(conn) -> Result<Vec<Station>>` (joined with `radio_stations`, newest first). Setting a station favorite with `set_with_source(.., Source::Radio, ..)`.
  - `types.ts`: `Source` adds `'radio'`; `Station` interface; `FavoriteIds.stations`.

- [ ] **Step 1: Write the failing tests:** `radio_tables_exist`, `source_radio_round_trips`, `station_ids_validate` (a UUID upper or lower -> `radio:<lower>`; `"../x"`, `""`, 37 chars -> `None`), `favicon_cover_id_is_stable_and_safe` (same URL same id; `ftp:` and 3 000-byte URLs -> `None`), `clean_name_cases` (`" - DJ & CLUB CHARTS ---> Club"` -> `"DJ & CLUB CHARTS ---> Club"`, `"*CLUB CHARTS TOP 100 || I"` -> `"CLUB CHARTS TOP 100 || I"`, `"\u{0007}Radio\n  Eins"` -> `"Radio Eins"`, `"###"` -> `None`, a 2 000-char name -> 80 chars ending `...`, multi-byte names cut on a char boundary), `clean_tags_dedupes_and_caps`, `station_as_track_is_live` (`duration_ms == 0`, `source == Radio`, `path == None`), `prune_keeps_favorites_and_recent`, `station_favorites_list_and_resolve`, `stations_never_touch_tracks` (upsert 10 stations, favorite 3: `tracks`, `albums`, `tracks_fts` and `track_stats` stay empty; `query::track_count(None)` unchanged).
- [ ] **Step 2: Run** `cargo test --manifest-path src-tauri/Cargo.toml db:: radio:: library::favorites` -> FAIL.
- [ ] **Step 3: Implement** the migration, the model, the store and the favorites kind. `npm run check` stays clean with the widened `Source` (exhaustive switches get a `radio` branch).
- [ ] **Step 4: Run** the full `cargo test`, `npm test`, `npm run check` -> PASS.
- [ ] **Step 5: Commit** `PRISM-4: Schema for radio, Source::Radio, stations and station favorites`.

### Task 2: Radio Browser client and fake directory

**Files:**
- Create: `src-tauri/src/radio/directory.rs`, `src-tauri/src/radio/testserver.rs`, `src-tauri/tests/fixtures/radio/{servers.json, search.json, search_geo.json, byuuid.json, countries.json, states.json, tags.json, url_click.json}`
- Modify: `src-tauri/src/radio/mod.rs`

**Interfaces:**
- Produces:
  - `pub const USER_AGENT: &str = concat!("Prism/", env!("CARGO_PKG_VERSION"))`, `CONNECT_TIMEOUT = 6 s`, `API_TIMEOUT = 12 s`, `MIRROR_TTL = 7 days`, `FALLBACK_MIRROR = "de1.api.radio-browser.info"`, `MAX_RESPONSE = 8 MiB` (a larger body is an error, read with a cap).
  - `pub enum DirectoryError { Unreachable(String), Server(u16), BadResponse(String) }` with `fn code(&self) -> &'static str` (`unreachable`, `server`, `badResponse`) and `is_offline()`. Errors never carry the full request URL (host only).
  - `#[derive(Deserialize)] pub(crate) struct RawStation` with the Radio Browser fields (`stationuuid, name, url, url_resolved, homepage, favicon, tags, countrycode, state, language, codec, bitrate, hls, lastcheckok, clickcount, votes, geo_lat, geo_long, geo_distance`), every field `#[serde(default)]`, numbers lenient (a string "128" or null accepted), and `fn into_station(self) -> Option<Station>` (cleans the name and tags, drops video codecs, a bad uuid, a non-http(s) url).
  - `pub struct StationQuery { pub name: Option<String>, pub country_code: Option<String>, pub region: Option<String>, pub tag: Option<String>, pub geo: Option<(f64, f64, f64)> /* lat, lon, metres */, pub order: Order /* ClickCount | Votes | Name */, pub offset: u32, pub limit: u32 }` (limit capped at 500).
  - `pub struct Directory`, `Directory::new(mirrors: MirrorSource) -> anyhow::Result<Self>` where `pub enum MirrorSource { Discover { cached: Vec<String> }, Fixed(Vec<String>) }` (tests use `Fixed` with fake server URLs, including the scheme). Methods: `mirrors(&self) -> Vec<String>` (for the cache), `search(&StationQuery) -> Result<Vec<Station>, DirectoryError>`, `by_uuids(&[&str]) -> Result<Vec<Station>, _>` (batches of 50), `countries() -> Result<Vec<Country { code, name, station_count }>, _>`, `states(country_name: &str) -> Result<Vec<Region { name, station_count }>, _>`, `top_tags(limit) -> Result<Vec<TagCount { name, station_count }>, _>`, `click(uuid) -> Result<(), _>`.
  - Mirror rotation: the list is shuffled once in `new`; a request that fails `is_offline()` or with 5xx retries the next mirror (each mirror at most once per request); the mirror that answered is tried first next time.
  - `testserver.rs` (`#[cfg(test)] pub(crate)`): `FakeDirectory::start(stations: Vec<FakeStation>)`, `FakeStation::new(n)` (fake names "Demo Radio <n>", fake uuids, `url` pointing at a configurable stream base), builders for geo, region, codec, hls, favicon, clickcount; `url()`, `requests() -> Vec<Recorded { path, params }>`, `set_fault(Fault::{None, Status(u16), Refuse, Stall(Duration), Html, Huge})`, `stop()` / `restart()`. It implements `/json/servers`, `/json/stations/search` (filters name substring, countrycode, state, tag, geo radius with haversine, `hidebroken`, order, offset, limit), `/json/stations/byuuid`, `/json/countries`, `/json/states`, `/json/tags`, `/json/url/<uuid>`.
- Fixtures are hand-written in the Radio Browser response shape with fake data only.

- [ ] **Step 1: Write the failing tests** in `directory.rs`: `fixtures_parse`, `lenient_numbers_and_nulls` (`"bitrate": "128"`, `"geo_lat": null`, `"hls": "1"`), `bad_stations_are_dropped` (empty name after cleanup, `"codec": "AAC,H.264"`, `"url": "rtsp://..."`, a bad uuid), `search_sends_the_right_params` (geo -> `geo_lat`, `geo_long`, `geo_distance`, `has_geo_info=true`; always `hidebroken=true`), `limit_is_capped`, `user_agent_is_sent`, `rotates_to_the_next_mirror` (first mirror refused, second answers; next request goes straight to the second), `client_errors_do_not_rotate` (404 -> `Server(404)`, one request), `all_mirrors_down_is_unreachable`, `huge_body_is_refused` (9 MiB -> `BadResponse`, no OOM), `by_uuids_batches_by_50`, `click_hits_the_url_endpoint`, `errors_name_the_host_only`.
- [ ] **Step 2: Run** `cargo test --manifest-path src-tauri/Cargo.toml radio::directory` -> FAIL.
- [ ] **Step 3: Implement** the client, the fixtures and the fake server.
- [ ] **Step 4: Run** the filter 3x, then full `cargo test` and clippy -> PASS.
- [ ] **Step 5: Commit** `PRISM-4: Radio Browser client with mirror rotation and a fake directory`.

### Task 3: Location: IP lookup, manual override, settings

**Files:**
- Create: `src-tauri/src/radio/location.rs`
- Modify: `src-tauri/src/radio/testserver.rs` (fake geo endpoints), `src-tauri/src/settings.rs`, `src-tauri/src/commands.rs` (`settings_set` accepts `radioLocation`), `src/lib/types.ts`

**Interfaces:**
- Produces:
  - Settings: `radio_location: RadioLocation` with `#[serde(tag = "mode", rename_all = "camelCase")] pub enum RadioLocation { Auto, Manual { country_code: String, region: Option<String> } }`, default `Auto`, read leniently (an unknown or broken value becomes `Auto`, the rest of the file is kept; `country_code` must be two ASCII letters, uppercased, else `Auto`). No `Change` needed: the radio service reads it per request.
  - `#[serde(rename_all = "camelCase")] pub struct Place { pub country_code: String, pub region: Option<String>, pub city: Option<String>, pub lat: Option<f64>, pub lon: Option<f64>, pub origin: PlaceOrigin /* detected | manual | locale */, pub detected_at: Option<i64> }`.
  - `pub trait Locator: Send + Sync { fn locate(&self) -> Result<Place, LocateError>; }`, `pub struct IpLocator { geojs: String, ipinfo: String }` with `IpLocator::public()` (the two real URLs) and `IpLocator::at(geojs, ipinfo)` (tests). GeoJS fields `country_code, region, city, latitude, longitude` (strings or numbers); ipinfo fields `country, region, city, loc` (`"lat,lon"`). Timeouts 5 s each. Coordinates are rounded to one decimal before `locate` returns.
  - `pub fn resolve_place(setting: &RadioLocation, cached: Option<Place>, locator: &dyn Locator, locale: &str, now_ms: i64) -> (Place, bool /* store */)`: Manual -> a manual place (no call); Auto with a cached detected place younger than 7 days -> it; else locate; on failure the cached place of any age, else the locale country (`"de-DE"` -> `DE`; no country -> `US`).
  - `pub fn locale_country(locale: &str) -> Option<String>`.
  - `LocateError` carries no coordinates and no IP.
- Rule: the place is stored in `radio_state` key `place` as JSON, only when it came from a lookup.

- [ ] **Step 1: Write the failing tests:** `geojs_answer_parses` (fixture), `ipinfo_is_the_fallback` (GeoJS 500 -> ipinfo used), `detected_location_is_rounded` (50.04321, 10.0987 -> 50.0, 10.1), `manual_mode_never_calls_the_service` (a locator that panics when called), `fresh_cache_is_used`, `stale_cache_triggers_a_lookup`, `failed_lookup_keeps_the_old_place`, `locale_is_the_last_resort`, `errors_carry_no_coordinates`, `settings_radio_location_round_trips`, `broken_radio_location_falls_back_to_auto` (`{"mode":"manual","countryCode":"Deutschland"}` -> Auto, other keys kept).
- [ ] **Step 2: Run** `cargo test --manifest-path src-tauri/Cargo.toml radio::location settings::` -> FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** the filter, full `cargo test`, clippy -> PASS.
- [ ] **Step 5: Commit** `PRISM-4: Coarse location for radio, with a manual country and region`.

### Task 4: LiveStream: bounded ring, ICY metadata, playlists, reconnect

**Files:**
- Create: `src-tauri/src/audio/live.rs`, `src-tauri/src/audio/live/testserver.rs`
- Modify: `src-tauri/src/audio/mod.rs` (`pub mod live;`, re-exports), `src-tauri/src/audio/source.rs` (`Input::Live`, `describe`)

**Interfaces:**
- Produces:
  - `pub struct LiveInput { pub url: String, pub label: String /* station name, for errors */, pub codec_hint: Option<String> /* "mp3", "aac", "ogg" */, pub config: Arc<LiveConfig>, pub titles: Option<TitleSink> }`, `Debug` prints the label, `PartialEq` by url and label. `pub type TitleSink = Arc<dyn Fn(Option<String>) + Send + Sync>`.
  - `pub struct LiveConfig { pub client: reqwest::blocking::Client /* UA, connect 8 s, read timeout 12 s, no gzip, redirects limited to 5 */, pub ring: usize /* 1 MiB */, pub stall: Duration /* 10 s */, pub outage: Duration /* 20 s */, pub backoff: Vec<Duration> /* 0.5, 1, 2, 4, 8 s */ }`, `LiveConfig::new()`, `LiveConfig::for_tests(ring, stall, outage, backoff)`.
  - `Input::Live(LiveInput)`; `open_source_with` routes it to the radio opener's path only through Task 5, here `open_source` returns an error for `Input::Live` ("live inputs open through the radio opener").
  - `pub struct LiveStream`: `LiveStream::open(input: &LiveInput) -> anyhow::Result<LiveStream>` (resolves a playlist once, checks the content type, reads `icy-metaint` and `icy-name`, starts the download thread), `fn content_type(&self) -> Option<&str>`, `fn is_hls(&self) -> bool` (the answer was an HLS playlist: nothing is downloaded, the caller goes to FFmpeg with `resolved_url()`), `fn resolved_url(&self) -> &str`, `fn reader(&self) -> LiveReader` (one reader at a time; a second call cancels the first), `fn reconnected(&self) -> u64` (bumped per reconnect, so `RadioSource` can tell a joint), `fn canceller(&self) -> Canceller`. Dropping the stream stops the thread and closes the connection.
  - `pub struct LiveReader`: `Read` + `symphonia::core::io::MediaSource` (`is_seekable() == false`, `byte_len() == None`), `Seek` only to the current position. It waits on a condvar for bytes up to `stall`; past `outage` without bytes it fails with `io::Error` wrapping `audio::http::Offline("The station is not reachable")`; a plain failure wraps `StreamFailure`.
  - ICY demux: when `icy-metaint` is between 1 and 1 048 576, every `metaint` bytes one length byte (x16) and that many metadata bytes are cut out before the ring; other values disable the demux (logged once). A metadata block is parsed for `StreamTitle='...';` only.
  - Playlist parsing: `pub(crate) fn first_entry(body: &str) -> Option<String>` for PLS (`File1=`) and M3U (first non-comment http(s) line); bodies over 64 KiB are not parsed. A playlist that resolves to another playlist is an error ("This station's playlist points nowhere").
  - Errors classify like M3: connection refused, DNS failure, timeout, a cut, 502/503/504 -> `Offline`; any other status, `text/html`, `text/plain` that is not a playlist, an empty body -> `StreamFailure` with a short message (`This station is not sending audio`, `The station answered <status>`).
  - `testserver.rs` (`#[cfg(test)] pub(crate)`): `FakeRadio::start()`, `.station(path, Body::{Mp3Tone { seconds_per_loop }, Ogg, Html, Pls(target), M3u(target), Hls}, metaint: Option<usize>, titles: Vec<&str>)`, `url(path)`, `set_fault(Fault::{CutAfter(bytes), StallAfter(bytes, Duration), Refuse, Status(u16), GarbageMeta, HugeMetaint})`, `stop()`, `restart()`, `connections() -> usize`. Endless bodies are generated (an MP3 tone loop from `tests/fixtures/tagged.mp3`-like frames written by `scripts/make-fixtures.mjs` into `tests/fixtures/radio/tone.mp3`, under 40 KB).

- [ ] **Step 1: Write the failing tests:** `icy_titles_are_cut_out_and_reported` (the decoded bytes equal the plain body; titles `"A - One"`, `"B - Two"` reported once each), `latin1_title_decodes` (`"Caf\xe9"` -> `"Café"`), `garbage_metadata_never_panics`, `huge_metaint_disables_the_demux`, `pls_and_m3u_resolve_once`, `playlist_loop_is_an_error`, `html_is_not_audio`, `status_404_is_a_plain_failure`, `status_503_is_offline`, `reconnects_after_a_cut_without_an_error` (cut at 64 KiB, the reader keeps getting bytes, `reconnected() == 1`), `gives_up_after_the_retry_window` (server stopped; with `outage` 1 s the read fails `Offline` within 3 s), `ring_stays_bounded_over_64_mib` (a fast server, a slow reader: the buffer never exceeds `ring`), `no_files_written` (the temp dir and `spill_dir` stay empty), `hls_playlist_is_detected` (`is_hls()`, nothing downloaded), `drop_closes_the_connection` (`connections()` goes back to 0 within 1 s), `user_agent_and_icy_header_sent`.
- [ ] **Step 2: Run** `cargo test --manifest-path src-tauri/Cargo.toml audio::live` -> FAIL.
- [ ] **Step 3: Implement** `live.rs` and the fake server; add `tone.mp3` to `scripts/make-fixtures.mjs` and run it.
- [ ] **Step 4: Run** the filter 3x (flakiness), full `cargo test`, clippy -> PASS.
- [ ] **Step 5: Commit** `PRISM-4: Live streams with a bounded buffer, ICY titles, playlists and reconnect`.

### Task 5: The radio opener and RadioSource

**Files:**
- Create: `src-tauri/src/radio/stream.rs`
- Modify: `src-tauri/src/audio/ffmpeg.rs` (`open_url`, `open_reader`), `src-tauri/src/audio/symph.rs` (only if `open_media` needs a non-seekable hint), `src-tauri/src/audio/source.rs`

**Interfaces:**
- Consumes: Task 1 (`Station`, `store::station`), Task 4.
- Produces:
  - `FfmpegSource::open_url(url: &str, label: &str, target: StreamSpec) -> anyhow::Result<Self>` (for HLS: `-reconnect 1 -reconnect_streamed 1 -reconnect_delay_max 4 -rw_timeout 10000000 -i <url>`; the label, not the URL, appears in errors) and `FfmpegSource::open_reader(reader: Box<dyn Read + Send>, label, target)` (stdin feed, like `open_stream`).
  - `pub fn radio_opener(lookup: Arc<dyn Fn(&str) -> Option<Station> + Send + Sync>, config: Arc<LiveConfig>, titles: TitleSink, fallback: Opener) -> Opener`: tracks of `Source::Radio` open as a `RadioSource`, every other track goes to `fallback` (the Navidrome opener chain). The station is looked up by id to get `url_resolved` (else `url`), codec and hls.
  - `pub struct RadioSource`: `AudioSource` with `duration_ms() == None`, `seek` returning an error ("a live station cannot seek"). It opens symphonia over a `LiveReader` with the codec hint, falling back to FFmpeg through `open_reader`; HLS goes to `open_url`. When the decoder fails or ends while the stream is still there (a reconnect joint, an Ogg chain restart), it opens a fresh `LiveStream` and decoder, at most 3 times per minute, with at most 300 ms of silence handed out meanwhile; after that the error goes up (offline stays offline). Its `spec()` stays fixed for the source's life: a fresh decoder at another rate or channel count is resampled to the first spec (`audio::resample`).
  - Titles: `RadioSource` calls the sink with `None` when it opens, so an old title never stays on a new station.

- [ ] **Step 1: Write the failing tests:** `mp3_station_decodes_audio` (about 1 s of non-silent frames from the fake station), `ogg_station_decodes_audio`, `unknown_codec_falls_back_to_ffmpeg` (codec hint `null`, an MKA body), `hls_goes_to_ffmpeg_by_url` (fake HLS playlist with two TS segments made by ffmpeg in the fixture script), `seek_is_refused`, `duration_is_unknown`, `decoder_end_reopens_while_the_stream_lives`, `reopens_are_capped`, `offline_stays_offline` (`is_offline(err)` after the outage), `titles_reset_on_open`, `other_sources_go_to_the_fallback`, `ffmpeg_children_are_reaped` (no child left after drop).
- [ ] **Step 2: Run** `cargo test --manifest-path src-tauri/Cargo.toml radio::stream` -> FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** the filter 3x, full `cargo test`, clippy -> PASS.
- [ ] **Step 5: Commit** `PRISM-4: Radio opener: stations decode through symphonia or FFmpeg and survive joints`.

### Task 6: Engine live mode

**Files:**
- Modify: `src-tauri/src/player/engine.rs`, `src-tauri/src/player/mod.rs`, `src-tauri/src/library/history.rs`, `src-tauri/src/media_controls.rs`, `src-tauri/src/navidrome/scrobble.rs` (test only), `src/lib/types.ts` (`PlayerError.source`)

**Interfaces:**
- Consumes: Task 1, Task 5 (through the `Opener`; engine tests use a fake live source).
- Produces:
  - `resolve()` accepts `Source::Radio` tracks without a path.
  - `fn is_live(track: &Track) -> bool` = `source == Radio && duration_ms == 0`.
  - Live behavior: no pre-open of a next entry, no crossfade in or out, `seek` ignored, `seekable()` false, `next` / `prev` ignored while the current entry is live (also from media keys, tray, mini player), repeat and shuffle have no effect on it. The position is the time heard since the last (re)open.
  - Pause on a live entry drops the source (closing the connection) and holds the entry (status `Paused`, reusing the offline hold with `position_ms = 0`). Play opens it fresh; this emits no new `TrackChanged` (the same rule as the M3 resume, R2), so history and the click report see one play.
  - `PlayerError { track_id, message, kind, source }`. An offline failure of a live entry holds it with the message "The station is not reachable" and `source: Radio`; a Navidrome one keeps `OFFLINE_MESSAGE`. Offline errors still never count toward `MAX_FAILURES_IN_A_ROW`.
  - A live entry that ends without an error (the station closed the stream and `RadioSource` gave up) is treated as offline, not as the end of the queue, so the player never silently goes idle on a station.
  - ReplayGain factor 1.0 for live entries.
  - `history::spawn_recorder` ignores `radio:` ids (no `track_stats` rows). The scrobbler already ignores non-Navidrome ids; a test pins it.
  - `media_controls.rs`: for a live track, no position, no duration, seek, next and previous disabled; title = the current stream title when one is set (`MediaBridge::set_live_title(Option<String>)`), else the station name; artist = the station name when a title is shown, else the region line; the cover from the `rb-` cache path. Windows (SMTC) and macOS (MPNowPlayingInfoCenter via souvlaki) both take the same calls; any platform difference is named in the code.

- [ ] **Step 1: Write the failing tests** (engine, with a fake endless source and a fake resolver): `live_has_no_duration_and_no_seek`, `next_and_prev_do_nothing_on_live`, `no_preopen_or_crossfade_with_live` (a queue of track then station with crossfade Always: a hard switch, no pre-open job for the station before the track ends), `pause_closes_and_play_reopens_live` (the opener is called again; no second `TrackChanged`), `live_offline_pauses_with_one_notice` (one `Error { kind: Offline, source: Radio }`, status Paused), `play_after_offline_joins_live` (reopen, not at a position), `a_live_end_is_held_not_idle`, `navidrome_offline_message_is_unchanged`, `radio_plays_are_not_recorded_or_scrobbled`, `live_gain_is_unity`; media controls: `live_metadata_shows_title_and_station` (pure mapping function test).
- [ ] **Step 2: Run** `cargo test --manifest-path src-tauri/Cargo.toml player:: media_controls library::history navidrome::scrobble` -> FAIL.
- [ ] **Step 3: Implement.** Keep every M2/M3 engine test green; uid-based entries, RepeatMode and the offline hold must behave exactly as before for tracks.
- [ ] **Step 4: Run** the filter 3x, full `cargo test`, clippy, `npm run check` -> PASS.
- [ ] **Step 5: Commit** `PRISM-4: Live mode in the engine: no seek, no next, pause joins live again`.

### Task 7: RadioService, favicons and app wiring

**Files:**
- Create: `src-tauri/src/radio/near.rs`, `src-tauri/src/radio/covers.rs`
- Modify: `src-tauri/src/radio/mod.rs`, `src-tauri/src/thumbs.rs` (remotes by prefix), `src-tauri/src/navidrome/covers.rs` (`set_remote_for("nd-", ..)`), `src-tauri/src/lib.rs`, `src-tauri/Cargo.toml` (`image` feature `ico`), `src-tauri/src/flow/mod.rs` (test only)

**Interfaces:**
- Consumes: Tasks 1, 2, 3, 5, 6.
- Produces:
  - `near::rank(place: &Place, geo: Vec<Station>, region: Vec<Station>, country: Vec<Station>, max: usize) -> Vec<Station>` (the three bands of Decisions; `distance_km` set for band 1).
  - `pub struct RadioService` with `RadioService::spawn(db: Arc<Db>, settings: Arc<SettingsStore>, directory: Arc<dyn DirectoryApi>, locator: Arc<dyn Locator>, locale: String, emit: RadioEmit) -> Arc<Self>`; `DirectoryApi` is a trait over Task 2's `Directory` so tests inject the fake. Methods (all blocking, called from commands on Tauri's thread pool, never under the DB lock for network work):
    - `place(&self) -> Place` (resolves and stores per Task 3),
    - `near(&self, refresh: bool) -> NearResult { stations, place, cached: bool, stale: bool, error: Option<&'static str> }` (directory down -> the cached list with `error: Some("unreachable")`),
    - `search(&self, query: StationQuery) -> Result<Vec<Station>, DirectoryError>`,
    - `countries()`, `regions(country_code)`, `top_tags()` (cached in memory for the session),
    - `play_prepared(&self, station: Station) -> Result<String /* id */>` (upserts the station, marks it played, prunes, reports the click on a background thread at most once per 24 h per station),
    - `saved(&self) -> Saved { favorites: Vec<Station>, recent: Vec<Station> }`,
    - `set_favorite(&self, station: Station, on: bool)` (upserts first; emits `favorites://changed`),
    - `title_sink(&self) -> TitleSink` and `now_playing(&self) -> Option<NowPlaying { station_id, title }>`; the sink emits `radio://now-playing` and calls `MediaBridge::set_live_title`,
    - a refresh thread: 30 s after start and then every 24 h, `by_uuids` for every stored station; changed URLs, names, favicons and codecs are written; a station the directory no longer knows is kept as it is (it may still stream).
  - `lookup_station(db) -> Arc<dyn Fn(&str) -> Option<Station>>` for the opener, and the resolver chain in `lib.rs`: `radio:` ids resolve through `radio::store::track`, everything else through the library resolver (behind `with_dropped` as before).
  - `thumbs::ResizeGate::set_remote_for(prefix: &'static str, remote: Arc<dyn RemoteCovers>)` replaces `set_remote`; `Origin::Remote` carries the prefix; `purge_remote(cache, prefix)` purges only that prefix's files (the Navidrome wipe must not delete favicons).
  - `radio::covers::FaviconCovers { db, client }` implements `RemoteCovers` per Decisions (caps, decode check, missing cache, homepage fallback). `pub(crate) fn icon_candidates(html: &str, base: &Url) -> Vec<Url>` is the bounded tag scanner, pure and unit-tested. The gate caches the result like an `nd-` cover.
  - `lib.rs`: `RadioService::spawn` before the player; the opener chain is `radio_opener(.., navidrome_opener(.., default_opener()))`; `AppState.radio`; favicons attached to the gate.

- [ ] **Step 1: Write the failing tests:** `near_bands_in_order` (fake stations: two within 75 km, three in the region without geo, five country-wide; band order kept, each band by clickcount), `near_dedupes_across_bands`, `manual_place_skips_the_geo_band`, `near_list_falls_back_to_cache`, `near_cache_is_per_place`, `play_prepared_stores_marks_and_clicks_once` (two plays within 24 h -> one click request), `saved_stations_play_without_the_directory` (directory stopped: the resolver and the opener still get the stored station), `refresh_updates_changed_urls`, `refresh_keeps_unknown_stations`, `favorite_station_survives_prune`, `favicon_too_big_is_unknown`, `favicon_html_is_unknown_and_not_asked_again`, `ico_favicon_decodes`, `homepage_icon_used_without_favicon`, `homepage_used_when_favicon_fails`, `icon_candidates_order` (apple-touch-icon 180 before icon 32 before og:image before /favicon.ico; relative URLs resolved; svg skipped; `javascript:` and `data:` ignored), `icon_scanner_is_bounded` (a 5 MB page, an unclosed `<link` at the end, 10 000 link tags: no panic, at most 16 candidates), `navidrome_purge_keeps_favicons`, `titles_emit_and_reset`, `flow_never_deals_a_station` (favorites with tracks and stations: every dealt id is a track id).
- [ ] **Step 2: Run** `cargo test --manifest-path src-tauri/Cargo.toml radio:: thumbs:: navidrome::covers flow::` -> FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** the filter 3x, full `cargo test`, clippy -> PASS.
- [ ] **Step 5: Commit** `PRISM-4: Radio service: near you, search, saved stations, favicons and titles`.

### Task 8: Commands, IPC and the radio store

**Files:**
- Modify: `src-tauri/src/commands.rs`, `src-tauri/src/lib.rs` (invoke handler list), `src/lib/ipc.ts`, `src/lib/types.ts`
- Create: `src/lib/stores/radio.svelte.ts`, `src/lib/stores/radio.spec.ts`

**Interfaces:**
- Consumes: Task 7.
- Produces:
  - Commands: `radio_place() -> Place`, `radio_near(refresh: bool) -> NearResult`, `radio_search(query: StationQueryArgs { name?, countryCode?, region?, tag?, offset?, limit? }) -> Vec<Station>` (errors as `{ code }`), `radio_countries()`, `radio_regions(countryCode)`, `radio_tags()`, `radio_saved() -> Saved`, `radio_play(station: Station) -> ()` (prepares, then `player.play([id], 0)`), `radio_favorite(station: Station, on: bool)`, `radio_now_playing() -> Option<NowPlaying>`, `radio_redetect() -> Place` (Auto only: drops the cached place and locates again).
  - `ipc.ts` wrappers with the same names in camelCase, and `onRadioNowPlaying(cb)`.
  - `stores/radio.svelte.ts`: `radio = $state({ place, near: { stations, loading, cached, stale, error }, search: { query, stations, loading, error }, saved: { favorites, recent }, nowPlaying: { stationId, title } | null })`, `loadNear(refresh?)`, `runSearch(query)` (debounced 300 ms, the latest query wins), `loadSaved()`, `playStation(station)`, `toggleStationFavorite(station)`, `currentTitle(track)` (the title only when `track.id === nowPlaying.stationId`). Listens to `radio://now-playing` and `favorites://changed`.

- [ ] **Step 1: Write the failing tests** (`radio.spec.ts`, mocked IPC): `search_is_debounced_and_latest_wins`, `title_shows_only_for_the_current_station`, `near_error_keeps_cached_list`, `favorite_toggle_reloads_saved`; Rust: `radio_commands_are_registered` (the handler list names every command; pattern of the existing command tests).
- [ ] **Step 2: Run** `npm test -- radio` and `cargo test --manifest-path src-tauri/Cargo.toml commands` -> FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** `npm test`, `npm run check`, full `cargo test` -> PASS.
- [ ] **Step 5: Commit** `PRISM-4: Radio commands and the radio store`.

### Task 9: Radio view: near you, search, saved and recent

**Files:**
- Create: `src/lib/components/RadioView.svelte`, `src/lib/components/StationList.svelte`, `src/lib/components/StationTile.svelte`
- Modify: `src/lib/components/Sidebar.svelte`, `src/lib/stores/nav.svelte.ts` (`'radio'` view, param `near | search | saved`), `src/App.svelte` (route), `src/lib/components/Icon.svelte` (radio, live, refresh, location icons), `src/lib/components/views.spec.ts`, `src/lib/i18n.ts`

**Interfaces:**
- Consumes: Task 8.
- Produces:
  - Sidebar: the entry is labeled just "Radio" (owner decision, 2026-10-21; EN and DE), enabled (no "soon"), dot `--src-radio`, opens `go('radio', 'near')`, lit on every radio tab.
  - RadioView header: title, the place line ("Near Testhausen, Germany" / "Germany, Bavaria" / "Germany" for locale or manual), "Refresh", and a segmented control Near you | Search | Saved (the existing `Segmented`).
  - Near you: `StationList` of `radio.near.stations`; distance shown for band 1 ("12 km"); a quiet hint line when the list is cached or stale and the directory is down; the empty state explains and links to Settings > Radio.
  - Search: one search field (name), a country select (from `radio_countries`, "Anywhere" first), a region select (enabled once a country is chosen), a genre select (top 40 tags); results load 100 at a time with "Show more"; errors in plain words ("The radio directory cannot be reached right now.").
  - Saved: Favorites (stations) and "Recently played", each a `StationList`.
  - `StationList`: virtualized like `SongList` (the same row height and scroll behavior), each row: `StationTile` (40 px), cleaned name, tags (first three) and country or region, `MP3 128` / `AAC 64` quality, a heart, a play button (`RowPlay`), the playing row marked like a playing song. Enter and double click play; the context menu offers Play, Favorite and "Open homepage" (through the opener plugin, http(s) only).
  - `StationTile`: the `rb-` cover through the existing cover URL helper at 96 px, `object-fit: contain` on `--raised`, falling back to the initials (two letters) in radio blue on `--surface`.
- i18n keys (EN and DE): `source.radio` (exists; becomes "Radio" in EN and DE), `radio.title`, `radio.tab.near`, `radio.tab.search`, `radio.tab.saved`, `radio.near.place`, `radio.near.country`, `radio.near.distance`, `radio.near.cached`, `radio.near.empty`, `radio.refresh`, `radio.search.placeholder`, `radio.search.country`, `radio.search.anywhere`, `radio.search.region`, `radio.search.genre`, `radio.search.more`, `radio.search.empty`, `radio.error.unreachable`, `radio.saved.favorites`, `radio.saved.recent`, `radio.saved.empty`, `radio.menu.homepage`.

- [ ] **Step 1: Write the failing tests** (`views.spec.ts`): `sidebar_radio_entry_opens_near`, `near_lists_stations_with_distance`, `near_shows_cached_hint`, `search_filters_reach_ipc`, `region_select_waits_for_country`, `saved_tab_lists_favorites_and_recent`, `row_play_plays_the_station`, `tile_falls_back_to_initials`, `homepage_menu_only_for_http_links`.
- [ ] **Step 2: Run** `npm test -- views` -> FAIL.
- [ ] **Step 3: Implement.** Visual check per the implementer rules with the fake directory: start it with `cargo test --manifest-path src-tauri/Cargo.toml radio::testserver::serve_for_ui -- --exact --ignored --nocapture` (Task 2 adds this `#[ignore]` server with 300 fake stations and a fake ICY stream, and prints the env vars to set) and point the dev instance at it through `PRISM_RADIO_DIRECTORY` and `PRISM_RADIO_GEO` (debug builds only, like `PRISM_DATA_DIR`; Task 7 reads them in `lib.rs`).
- [ ] **Step 4: Run** `npm test`, `npm run check` -> PASS.
- [ ] **Step 5: Commit** `PRISM-4: Radio near you, station search and saved stations`.

### Task 10: Player surfaces in live mode, titles and error notices

**Files:**
- Create: `src/lib/components/LivePill.svelte`
- Modify: `src/lib/components/TransportStrip.svelte`, `src/lib/components/NowPlaying.svelte`, `src/lib/components/MiniPlayer.svelte`, `src/lib/components/FlowRing.svelte`, `src/lib/components/QueuePanel.svelte`, `src/lib/components/TrackMenu.svelte`, `src/lib/components/Heart.svelte`, `src/lib/format.ts`, `src/lib/format.spec.ts`, `src/App.svelte` (notices), `src/lib/stores/player.svelte.ts`, `src/lib/components/mini.spec.ts`, `src/lib/components/views.spec.ts`, `src/lib/i18n.ts`

**Interfaces:**
- Consumes: Tasks 6, 8.
- Produces:
  - `format.ts`: `isLive(track)`, `sourceQualityLabel` gives `Radio · MP3 128` (codec and bitrate when known, `Radio` alone otherwise).
  - TransportStrip, Now Playing and the mini player: for a live track the seek bar is replaced by `LivePill` ("LIVE", a soft pulse that stops under reduced motion), the time shows the time heard, next and previous are disabled with a tooltip, the title line is the stream title (when known) and the second line the station name; without a title, the station name and the region line. The heart writes a station favorite.
  - Now Playing: the cover backdrop uses the favicon (or the radio blue tile), and the seek bar colors fall back to the spectrum.
  - FlowRing: while a station plays, the ring is play/pause with a full, steady arc (no progress); the glow still follows the level.
  - QueuePanel: a station shows as one row with the LIVE pill, no drag handle, no "up next" section.
  - TrackMenu: for stations only Favorite and "Open homepage"; no add to queue, add to playlist, go to album or artist.
  - Notices (App): `player://error` with `kind: 'offline'` shows `navidrome.offlineNotice` for `source: 'navidrome'` and `radio.offlineNotice` (with the station name) for `source: 'radio'`, once per hold; `kind: 'track'` now shows `player.trackFailed` with the title (closes the M2 gap) and, for a station, `radio.stationFailed`.
- i18n keys: `radio.live`, `radio.liveHeard`, `radio.noSkip`, `radio.offlineNotice`, `radio.stationFailed`, `player.trackFailed`, `radio.menu.favorite`.

- [ ] **Step 1: Write the failing tests:** `format.spec.ts` (`radio_quality_label`, `radio_label_without_bitrate`), `views.spec.ts` (`live_track_shows_pill_not_seekbar`, `live_disables_next_and_prev`, `stream_title_is_the_title_line`, `title_of_another_station_is_ignored`, `station_menu_has_no_queue_items`, `radio_offline_notice_names_the_station`, `track_error_shows_a_notice`), `mini.spec.ts` (`mini_shows_live_and_title`).
- [ ] **Step 2: Run** `npm test` -> FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** `npm test`, `npm run check` -> PASS.
- [ ] **Step 5: Commit** `PRISM-4: Live stations in the player, mini player and ring, with titles and notices`.

### Task 11: Favorites, Home, search palette and the Radio settings

**Files:**
- Modify: `src/lib/components/FavoritesView.svelte`, `src/lib/stores/favorites.svelte.ts`, `src/App.svelte` (Home row), `src/lib/components/SearchPalette.svelte`, `src/lib/components/Settings.svelte`, `src/lib/components/settings.spec.ts`, `src/lib/components/views.spec.ts`, `src/lib/i18n.ts`

**Interfaces:**
- Consumes: Task 8.
- Produces:
  - FavoritesView: a Stations section (a `StationList` without virtualization below 50 rows) and a Radio chip (shown when any station is a favorite) in the source filter.
  - Home: a "Your stations" row of up to 12 favorite station tiles (StationTile at 96 px with name), shown only when there is at least one.
  - SearchPalette: a fourth group "Stations": saved stations (favorites and recent) matching the query at once, then up to 8 directory results after the debounce; directory errors hide the group silently. Enter plays the station.
  - Settings > Radio (between Navidrome and Playback): Location as a segmented control Automatic | Manual. Automatic shows the detected place ("Testhausen, Germany", or "Not found yet") with "Detect again" and one privacy line ("Prism asks geojs.io once a week for a rough location from your IP address. It stays on this computer."). Manual shows a country select and a region select; choosing them writes `radioLocation` and reloads the near list.
- i18n keys: `favorites.stations`, `favorites.chip.radio`, `home.stations`, `search.stations`, `settings.radio`, `settings.radio.location`, `settings.radio.auto`, `settings.radio.manual`, `settings.radio.detected`, `settings.radio.notFound`, `settings.radio.redetect`, `settings.radio.privacy`, `settings.radio.country`, `settings.radio.region`, `settings.radio.anyRegion`.

- [ ] **Step 1: Write the failing tests:** `views.spec.ts` (`favorites_lists_stations_and_radio_chip`, `home_row_only_with_station_favorites`, `palette_shows_saved_stations_first`, `palette_hides_stations_on_directory_error`, `palette_enter_plays_a_station`), `settings.spec.ts` (`radio_auto_shows_place_and_redetect`, `radio_manual_writes_location`, `region_resets_when_country_changes`).
- [ ] **Step 2: Run** `npm test` -> FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** `npm test`, `npm run check` -> PASS.
- [ ] **Step 5: Commit** `PRISM-4: Stations in favorites, Home and search, and the radio location settings`.

### Task 12: Robustness checks and the live smoke test

**Files:**
- Modify: `src-tauri/src/audio/live.rs` (tests), `src-tauri/src/radio/directory.rs` (tests), `src-tauri/src/radio/mod.rs` (tests)

**Interfaces:**
- Consumes: Tasks 2, 4, 7.
- Produces tests only:
  - `live_one_hour_simulated` (`#[ignore]`, `--release`): a fake 320 kbps station served at 50x speed for the equivalent of one hour through `RadioSource`; asserts the ring never grew, no file was written, no thread leaked (thread count before and after), and the decoded frame count matches.
  - `reconnect_storm` (`#[ignore]`): the fake station cuts every 2 s for a minute; the source never errors and reconnects stay within the backoff.
  - `near_with_10k_stations_under_200ms` (`#[cfg_attr(debug_assertions, ignore)]`): `near::rank` over 10 000 fake stations.
  - `live_directory_smoke` (`#[ignore]`): reads `PRISM_LIVE_RADIO=1` (skips otherwise); read-only against the real directory: `/json/servers`, one near search around the fake place "Testhausen" (lat 50.0, lon 10.0, a point in rural Germany, never the owner's place), one `byuuid`, the first 256 KiB of the most clicked station's stream (checks `icy-metaint` and that a title arrives within 30 s or prints "no title"). Never clicks, never votes. Prints counts and timings only, never names or URLs.

- [ ] **Step 1: Write the tests.**
- [ ] **Step 2: Run** `cargo test --manifest-path src-tauri/Cargo.toml --release --lib -- --exact --ignored --test-threads=1 --nocapture audio::live::tests::live_one_hour_simulated audio::live::tests::reconnect_storm radio::near::tests::near_with_10k_stations_under_200ms` -> PASS (a miss is a finding: fix the code, never raise the budget).
- [ ] **Step 3: Run** the full `cargo test`, `npm test`, `npm run check`, clippy, fmt -> PASS.
- [ ] **Step 4: Commit** `PRISM-4: Endurance and reconnect checks for live streams, and an opt-in live smoke test`.

### Task 13: Owner hand check

- [ ] **Step 1:** Build a dev bundle the owner starts himself (agents never start it against the real data dir or identifier).
- [ ] **Step 2:** Ask the owner to check on Windows and on the Mac:
  - "Radio near you" on first open: the place is right (roughly), the list makes sense for Kochel / Bavaria, favicons load, scrolling is smooth.
  - Search by name, by country and region, by genre.
  - Playing a station: starts within about 2 s, the stream title appears and changes with the song, the mini player, media keys and the OS media overlay show it; next and previous do nothing.
  - Pause for a minute, then Play: it plays live, not from where it stopped.
  - Pull the network for 5 s mid-station: no notice, it carries on. Pull it for 30 s: one notice, paused. Reconnect and Play: live again.
  - Favorite three stations, restart Prism: they are in Favorites, on Home and in Ctrl+K, and play while the directory is blocked (hosts file or firewall).
  - Manual location: Germany / another region changes the near list; Automatic again detects the place.
  - Flow while a station plays replaces it; the "All" views, playlists and Flow show no stations.
  - Windows close-to-mini and the tray still behave as in M3.
  - Optional: the live smoke test (`PRISM_LIVE_RADIO=1`, command in the M4 ledger).
- [ ] **Step 3:** Collect feedback, fix in worktrees, merge, and record the outcome in the M4 ledger. Then write the M5 plan (YouTube Music) against the real code.

---

## Carried in from M3

Folded in: player errors of `kind: 'track'` had no UI (M2 gap) -> Task 10; `classify_reqwest` style error scrubbing -> Tasks 2 and 4 use the same approach for hosts; the remote cover purge per prefix -> Task 7.

Left out (not touched by M4, still open): cover fetches and media-controls cover loading can block up to 20 s while a hung Navidrome server still reads as online (Task 6 note of M3); `PRAGMA cache_size = -32768` in `Db::open` (worth a one-line follow-up on its own); the AppManifest command list for main-only commands (M6 hardening, now also for the radio commands); every deferred minor of the M3 ledger.

## Owner answers (2026-10-21)

1. Location service: GeoJS with ipinfo as fallback, asked lazily when the radio view first needs it, at most once a week. Accepted.
2. Near you radius 75 km, then region, then country. Accepted.
3. Pause on a station closes the stream and Play joins live (no time shift). Accepted.
4. Favicons from station hosts are fetched, and logos may also be taken from the station's homepage when the favicon is missing or broken (see Decisions, Favicons). Initials only when neither works.
5. The sidebar entry is labeled just "Radio".
6. Branch `PRISM-4` is cut now from `PRISM-3`, before the M3 hand check; hand-check fixes on `PRISM-3` are merged into `PRISM-4` afterwards.

## Execution notes

- Order and waves (each wave from the merged result of the previous one): A = 1, 2, 3, 4 (independent; Task 1 is the only migration); B = 5, 7 (Task 7 can start on 1-3 and plugs in Task 5's opener at merge); C = 6, 8; D = 9, 10, 11 (UI, in parallel); E = 12; then 13. Merge in task number order inside a wave.
- Tasks 6, 7 and 8 all touch `lib.rs` and `commands.rs`; the controller resolves those merges by keeping both sides. Tasks 9, 10 and 11 all touch `i18n.ts`, `views.spec.ts` and `App.svelte`; hand merge them as in M3 Wave F.
- `Source::Radio` (Task 1) widens every exhaustive match in Rust and TypeScript: Task 1 must leave `cargo test`, clippy and `npm run check` green before Wave A merges further.
