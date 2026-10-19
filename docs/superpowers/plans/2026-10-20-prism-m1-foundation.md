# Prism M1 (Foundation) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A running Prism app on Windows that indexes local music folders, shows albums, artists and songs with covers in the Prism look, and plays every local format through a native Rust engine, controlled by the half-out ring and the OS media keys.

**Architecture:** Tauri 2 shell. Rust owns all state that more than one window will need (library index in SQLite, queue, player engine, OS media controls) and pushes it to the web views as events; the Svelte 5 frontend is a view over IPC commands plus those events. Audio never touches the web view: Symphonia decodes the common formats, a spawned FFmpeg process decodes everything else, and cpal plays the result.

**Tech Stack:** Tauri 2, Svelte 5 + TypeScript + Vite, Vitest; Rust: rusqlite (bundled), symphonia, cpal, rubato, lofty, image, souvlaki, walkdir, notify, window-vibrancy, tempfile (dev).

**Spec:** `docs/superpowers/specs/2026-10-20-prism-design.md`

## Roadmap (one plan per milestone, each written against the real code of the previous one)

| Milestone | Content | Spec sections |
|---|---|---|
| **M1 Foundation** (this plan) | scaffold, brand + icons, theme, DB, local scan + covers, audio engine, queue/player, media keys, main window shell, library views, local search, settings (folders), About, EN/DE, YTM sign-in spike (Windows) | 5.1-5.3, 5.9 (partly), 5.11-5.13, 7.1 |
| M2 Everyday player | mini player window, Flow (ring idle mode, rules, variants), favorites (local), tray / taskbar behaviour, global shortcut, Now Playing view, cover-colored ring + loudness glow, Prism playlists, crossfade, online cover lookup (MusicBrainz / Cover Art Archive, switchable), "Open with" registration, remaining settings | 5.2, 5.3, 5.7 (local), 5.8, 5.9, 5.10, 5.12 |
| M3 Navidrome | Subsonic client, keychain, background index sync, covers, HTTP streaming into the engine, two-way stars, scrobble, Navidrome playlists | 5.4, 5.7 |
| M4 Radio | Radio Browser client, near-you location + override, search, station favorites, ICY metadata | 5.5 |
| M5 YouTube Music | embedded view, sign-in pop-up fallback, transport bridge, exclusive playback | 5.6 |
| M6 Release v1.0.0 | updater, minimal LGPL FFmpeg bundling, CI, release workflow with versionless assets, README with screenshots, macOS checks | 5.14, 7.2-7.4 |
| v1.1 | Apple Music (MusicKit + token service), macOS notarization | 4 |

## Global Constraints

- Platforms: Windows 11 and macOS 11+ (universal). M1 is developed and checked on Windows; nothing may be Windows-only unless gated by `cfg(target_os)` with a macOS counterpart or a no-op.
- Repo `wl-lankin/prism` (public later in M6), local `C:\Projects\private\Prism`, branch `main`, repo-local git identity `Wolfgang Linz <27097016+wl-lankin@users.noreply.github.com>`. Commit, never push without the owner asking.
- App identifier `de.wolfganglinz.prism`, productName `Prism`, version `1.0.0`, license MIT, publisher "Wolfgang Linz", homepage `https://wolfgang-linz.de`.
- No em dash or en dash anywhere (code, UI strings, docs, commits). No AI attribution or session links anywhere.
- Palette: background `#0d0d10`, surface `#141418`, raised `#1f1f25`, text `#f3f3f5`; spectrum `#ff4d6d`, `#ffc94d`, `#4dffb2`, `#4da6ff`. Source colors: navidrome `#4dffb2`, apple `#ff4d6d`, youtube `#ffc94d`, radio `#4da6ff`, local `#cccccc`.
- No sparkle glyphs anywhere.
- Reduced motion: only large motion is switched off; hover and fade transitions keep running at 120 ms.
- Write files with Write/Edit, never Bash heredocs (they mangle backslashes on this machine).
- Commands always: `npm test`, `npm run check`, `cargo test --manifest-path src-tauri/Cargo.toml`. The local `ffmpeg` on PATH is fine for development; bundling comes in M6.
- Never use the owner's real library, accounts or credentials in tests, fixtures or screenshots.

## Review Focus

1. **Bad tags.** Files with no tags, only a title, or non-UTF-8 tags must still appear: title falls back to the file name without extension, album to "Unknown album", artist to "Unknown artist". Pinned in Task 5.
2. **A file that fails to decode mid-queue** (corrupt, truncated, unknown) must be skipped with an error event, and playback continues with the next track. The player must never hang or stop silently. Pinned in Task 8.
3. **A library folder that is offline** (unplugged drive, NAS not mounted) must not wipe its tracks from the index during a rescan. The folder is marked offline instead. Pinned in Task 5.
4. **The output device disappears** (headphones unplugged) during playback: the engine reopens on the new default device and continues. Pinned in Task 7.
5. **Rescanning a large, unchanged library** must skip unchanged files by size and mtime and must not block IPC. Pinned in Task 5 (timing test on 2,000 generated entries).

---

## File Structure

```
package.json, vite.config.ts, svelte.config.js, tsconfig.json, index.html, LICENSE, .gitignore
assets/logo.svg                      brand source (Task 3)
scripts/make-icons.mjs               logo -> src-tauri/icons + docs/logo.png (Task 3)
scripts/make-fixtures.mjs            regenerates audio fixtures with ffmpeg (Task 5)
src/main.ts, src/App.svelte          entry + top-level layout
src/lib/theme.css                    tokens, fonts, motion rules
src/lib/i18n.ts, src/lib/i18n.test.ts
src/lib/ipc.ts                       typed wrappers around invoke/listen (single place for command names)
src/lib/types.ts                     Track, Album, Artist, PlayerState, ... (mirrors Rust serde types)
src/lib/stores/player.svelte.ts      player state from events
src/lib/stores/nav.svelte.ts         current view
src/lib/format.ts, format.test.ts    durations, quality labels
src/lib/virtual.ts, virtual.test.ts  virtual list window math
src/lib/components/TitleBar.svelte, Sidebar.svelte, FlowRing.svelte, TransportStrip.svelte,
                   Cover.svelte, AlbumGrid.svelte, AlbumView.svelte, SongList.svelte,
                   ArtistList.svelte, SearchPalette.svelte, Settings.svelte, About.svelte
src-tauri/src/lib.rs                 builder, plugins, state, commands registration
src-tauri/src/db/mod.rs, db/migrations/0001_init.sql
src-tauri/src/library/{mod.rs, model.rs, scan.rs, tags.rs, covers.rs, watch.rs, search.rs}
src-tauri/src/audio/{mod.rs, source.rs, symph.rs, ffmpeg.rs, output.rs, resample.rs}
src-tauri/src/player/{mod.rs, queue.rs, engine.rs}
src-tauri/src/media_controls.rs
src-tauri/src/protocol.rs            prism:// cover protocol
src-tauri/src/commands.rs            all #[tauri::command] fns (thin; logic lives in modules)
src-tauri/tests/fixtures/            tiny generated audio files
docs/spikes/ytm-signin.md            spike result (Task 2)
```

---

### Task 1: Scaffold

**Files:** everything at the root listed above except components; `src-tauri/{Cargo.toml, tauri.conf.json, build.rs, capabilities/default.json, src/main.rs, src/lib.rs}`; move the spec and mockups into the new repo (they are already in `docs/superpowers/`).

**Interfaces:**
- Produces: `npm run dev` (Vite on port 1420), `npm test`, `npm run check` (`svelte-check` + `tsc --noEmit`), `npm run tauri`, `npm run icons`; the crate `prism_lib` with `pub fn run()`.

- [ ] **Step 1:** `git init` in `C:\Projects\private\Prism`, set the repo-local identity, add `.gitignore` (node_modules, dist, src-tauri/target, `.superpowers/`, `demo/`).
- [ ] **Step 2:** Create the Vite + Svelte 5 + TS project by hand (no template junk): `svelte`, `@sveltejs/vite-plugin-svelte`, `svelte-check`, `typescript`, `vite`, `vitest`, `jsdom`, `@tauri-apps/api`, `@tauri-apps/cli`, `@tauri-apps/plugin-dialog`, `@tauri-apps/plugin-opener`. Vitest config uses `resolve.conditions: ['browser']` under `process.env.VITEST` (Svelte runes otherwise run the server build, see Kern).
- [ ] **Step 3:** `tauri.conf.json` modeled on Glint (`C:\Projects\private\Glint\src-tauri\tauri.conf.json`): productName `Prism`, identifier `de.wolfganglinz.prism`, version `1.0.0`, one window `main` 1280x820, min 900x600, `decorations: false`, `transparent: true`, `backgroundColor: "#0d0d10"`, CSP allowing `img-src 'self' data: prism: http://prism.localhost`, `dangerousDisableAssetCspModification: ["style-src"]`. No updater yet (M6). `index.html` sets `background:#0d0d10` inline so no white frame appears.
- [ ] **Step 4:** `Cargo.toml` (edition 2021, `[lib] name = "prism_lib"`, release profile copied from Glint, `[profile.dev.package."*"] opt-level = 3` because decoders are hot loops). Add a trivial `#[test] fn it_builds()` and a trivial Vitest test.
- [ ] **Step 5:** Run `npm test`, `npm run check`, `npx vite build`, `cargo test --manifest-path src-tauri/Cargo.toml`. Expected: all pass. Run `npm run tauri dev` once: a dark frameless window opens with no white flash.
- [ ] **Step 6:** Add `LICENSE` (MIT, "Copyright 2026 Wolfgang Linz"). Commit `chore: scaffold Prism (Tauri 2 + Svelte 5)` including the spec, mockups and this plan.

### Task 2: YouTube Music sign-in spike (throwaway, Windows)

The riskiest part of the spec, so it runs first. Nothing from this task is kept except the findings document.

**Files:** Create `docs/spikes/ytm-signin.md`. Spike code lives on a branch `spike-ytm` that is deleted afterwards.

- [ ] **Step 1:** On `spike-ytm`, add a dev-only command that opens a second `WebviewWindow` on `https://music.youtube.com` with a desktop Chrome-like user agent left at the WebView2 default.
- [ ] **Step 2:** Ask the owner to try signing in there with their own account (Claude never types credentials). Record whether Google allows it.
- [ ] **Step 3:** If it is refused: try a second window opened on `https://accounts.google.com/ServiceLogin?continue=https://music.youtube.com` that shares the WebView2 data directory, then reload the embedded view. Record whether the session carries over.
- [ ] **Step 4:** Check from the injected script that `navigator.mediaSession.metadata` exposes title, artist and artwork, and that clicking `#play-pause-button` / `.next-button` works. Record the selectors used.
- [ ] **Step 5:** Write `docs/spikes/ytm-signin.md` (outcome per step, selectors, the macOS part marked "open until a macOS build exists in M6"). Commit it on `main`, delete the spike branch. If neither sign-in path works on Windows, stop and tell the owner before continuing past M1.

### Task 3: Brand, icons and theme

**Files:** Create `assets/logo.svg`, `scripts/make-icons.mjs`, `src/lib/theme.css`. Modify `package.json` (script `icons`, devDependency `@resvg/resvg-js`, dependencies `@fontsource-variable/inter`, `@fontsource-variable/space-grotesk`).

**Interfaces:**
- Produces: CSS custom properties used by every component: `--bg`, `--surface`, `--raised`, `--text`, `--text-dim`, `--line`, `--spec-1..4`, `--src-local|navidrome|apple|youtube|radio`, `--radius-s|m|l` (8/12/16 px), `--dur` (120ms), `--dur-l` (320ms, 0ms under reduced motion), `--font-ui` (Inter Variable), `--font-display` (Space Grotesk Variable).

- [ ] **Step 1:** Draw `assets/logo.svg` (1024 viewBox): graphite rounded square `#141418` (radius 22%), a white outline prism, one white incoming beam from the left, and three curved sound waves leaving the right face in a red-amber-mint-blue gradient. It must read at 16 px. Compare against the mockup `docs/superpowers/mockups/identity.html` (option C).
- [ ] **Step 2:** Port Glint's `scripts/make-icons.mjs` (no file-association icon): render the logo at 1024, run `tauri icon` into `src-tauri/icons`, and write `docs/logo.png` (512).
- [ ] **Step 3:** Run `npm run icons`. Expected: `src-tauri/icons/icon.ico` and `icon.icns` exist. View `docs/logo.png` and a 32 px render, and show both to the owner before moving on.
- [ ] **Step 4:** Write `theme.css` with the tokens above, `color-scheme: dark`, scrollbars styled thin, and `@media (prefers-reduced-motion: reduce) { :root { --dur-l: 0ms } }` (keep `--dur`).
- [ ] **Step 5:** Commit `feat: Prism logo, app icons and theme tokens`.

### Task 4: Database

**Files:** Create `src-tauri/src/db/mod.rs`, `src-tauri/src/db/migrations/0001_init.sql`, `src-tauri/src/library/model.rs`.

**Interfaces:**
- Produces:
  - `pub struct Db(Mutex<rusqlite::Connection>)`, with `Db::open(path: &Path) -> anyhow::Result<Db>` and `Db::open_in_memory() -> anyhow::Result<Db>`. Migrations run by `PRAGMA user_version`; WAL mode on.
  - Serde models (camelCase, mirrored in `src/lib/types.ts`):
    - `Track { id: String, source: Source, title, artist, albumArtist, album, albumId: String, trackNo: Option<u32>, discNo: Option<u32>, durationMs: u64, year: Option<i32>, genre: Option<String>, format: String, sampleRate: Option<u32>, bitDepth: Option<u8>, bitrateKbps: Option<u32>, coverId: Option<String>, path: Option<String>, replayGainTrackDb: Option<f32>, replayGainAlbumDb: Option<f32>, replayGainTrackPeak: Option<f32>, replayGainAlbumPeak: Option<f32> }`
    - `Album { id, source, title, artist, year, coverId, trackCount: u32, durationMs: u64 }`
    - `Artist { name, albumCount: u32, trackCount: u32, coverId }`
    - `enum Source { Local, Navidrome }` (serialized lowercase)
  - Id rules: track id = `"local:" + hex(sha1(normalized absolute path))`. Album id = `"local:" + hex(sha1(lowercase(albumArtist) + "\u{1f}" + lowercase(album)))`.
- Schema `0001_init.sql`: `folders(path TEXT PRIMARY KEY, online INTEGER NOT NULL DEFAULT 1)`; `tracks(... columns of Track ..., folder TEXT, mtime INTEGER, size INTEGER)` with indexes on `album_id`, `artist`, `album_artist`; `albums(...)`; `tracks_fts` (FTS5, title/artist/album, content=tracks, with triggers); `covers(id TEXT PRIMARY KEY, file TEXT NOT NULL)`.

- [ ] **Step 1:** Write tests in `db/mod.rs`: `migrates_fresh_db_to_latest` (user_version == 1), `migration_is_idempotent` (opening twice does not fail), `fts_finds_inserted_track` (insert a track titled "Teardrop", `MATCH 'tear*'` returns it).
- [ ] **Step 2:** Run `cargo test db::` and see them fail. Implement. Run again: PASS.
- [ ] **Step 3:** Commit `feat: SQLite library schema`.

### Task 5: Local scanner, tags, covers on disk, watcher

**Files:** Create `src-tauri/src/library/{mod.rs, tags.rs, scan.rs, covers.rs, watch.rs}`, `scripts/make-fixtures.mjs`, `src-tauri/tests/fixtures/*`.

**Interfaces:**
- Consumes: `Db`, models from Task 4.
- Produces:
  - `tags::read(path: &Path) -> anyhow::Result<TrackMeta>`. `TrackMeta` holds the Track fields plus `embedded_cover: Option<Vec<u8>>`. Uses lofty. Falls back to symphonia probing for duration/format when lofty cannot read the file.
  - `scan::scan_folder(db: &Db, covers: &CoverStore, folder: &Path, progress: impl Fn(ScanProgress)) -> anyhow::Result<ScanReport>`, with `ScanReport { added, updated, removed, unchanged, failed: Vec<(String, String)> }` and `ScanProgress { done: u32, total: u32 }`.
  - `AUDIO_EXTENSIONS: &[&str]`: mp3, m4a, m4b, aac, alac, flac, wav, aif, aiff, aifc, ogg, oga, opus, wma, ape, wv, mpc, tta, dsf, dff, mka, webm, caf.
  - `covers::CoverStore::new(dir: PathBuf)`, `store_bytes(&self, bytes: &[u8]) -> anyhow::Result<String>` (id = sha1 of the bytes, writes `<id>.<ext>`, dedupes), `path(&self, id: &str) -> Option<PathBuf>`, `find_folder_cover(dir: &Path) -> Option<PathBuf>` (case-insensitive `cover|folder|front|album` + `jpg|jpeg|png|webp`).
  - `watch::start(app: AppHandle, folders: Vec<PathBuf>)` debounces notify events by 2 s and rescans only the affected folder.
  - Library service on `AppState`: `add_folder`, `remove_folder`, `rescan_all`; emits `library://changed` and `library://scan-progress`.
- Rules: cover order is embedded art, then a folder cover file. An album's cover = the cover of its first track that has one. Online lookup is M2+ (left out of M1). A file is unchanged when size and mtime match the indexed values. A folder that does not exist or cannot be read at scan time is set `online = 0` and its tracks are kept.

- [ ] **Step 1:** Write `scripts/make-fixtures.mjs`: with the local ffmpeg, generate 1-second sine files into `src-tauri/tests/fixtures/`: `tagged.mp3` (title "Alpha", artist "Fixture Artist", album "Fixture Album", track 1), `tagged.flac` (track 2 same album, with an embedded 64x64 PNG cover), `notags.ogg` (no tags), `latin1.mp3` (ID3v2.3 title in ISO-8859-1 "Café"), `opus.opus`, `wma.wma`, `wv.wv`, `tta.tta`, `aac.m4a`, `corrupt.flac` (the first 200 bytes of a flac then zeros), `cover/folder.jpg` + `cover/track.mp3`. Run it and commit the fixtures (they must stay under 300 KB total).
- [ ] **Step 2:** Write the failing tests (`cargo test library::`):
  - `reads_basic_tags`: `tagged.mp3` gives title "Alpha", artist "Fixture Artist", trackNo 1, durationMs between 900 and 1100.
  - `missing_tags_fall_back`: `notags.ogg` gives title "notags", album "Unknown album", artist "Unknown artist".
  - `latin1_tag_decodes`: title == "Café".
  - `embedded_cover_preferred`: after scanning `fixtures/`, the album "Fixture Album" has a coverId and `CoverStore::path` exists.
  - `folder_cover_used`: the track in `fixtures/cover/` has the coverId of `folder.jpg`.
  - `corrupt_file_reported_not_fatal`: the scan returns Ok, `corrupt.flac` is in `failed` or indexed with durationMs 0, and the other files are indexed.
  - `offline_folder_keeps_tracks`: scan a temp copy of the fixtures, delete the directory, rescan: the tracks are still in the DB and the folder has `online == 0`.
  - `rescan_skips_unchanged`: the second scan reports `unchanged == total` and `added == 0`.
  - `rescan_2000_unchanged_under_1s`: insert 2,000 synthetic track rows pointing at a temp dir of 2,000 empty `.mp3` files whose size/mtime match, rescan, and assert under 1 s in release-like settings (mark it `#[ignore]` in debug if it flakes and run it with `--release` in CI).
- [ ] **Step 3:** Run, see them fail. Implement `tags.rs`, `covers.rs`, `scan.rs` (walkdir, parallel tag reading with rayon, one DB transaction per 500 rows, album table rebuilt for the touched albums), then `watch.rs`.
- [ ] **Step 4:** Run `cargo test library::`. Expected: PASS.
- [ ] **Step 5:** Commit `feat: local library scanner with covers and folder watching`.

### Task 6: Cover protocol

**Files:** Create `src-tauri/src/protocol.rs`. Modify `lib.rs` (register `prism` URI scheme).

**Interfaces:**
- Produces: URLs `prism://localhost/cover/<coverId>/<size>` (on Windows the WebView sees `http://prism.localhost/cover/<coverId>/<size>`); TS helper `coverUrl(id: string | null, size: 96 | 300 | 600): string | null` in `src/lib/ipc.ts`, which uses `convertFileSrc`-style platform detection like Glint.
- Rules: resize with the `image` crate (Lanczos3), encode as JPEG q85, cache under `<appCache>/covers/<size>/<id>.jpg`. Unknown id: 404. Response header `Cache-Control: max-age=31536000, immutable`.

- [ ] **Step 1:** Write the failing tests: `serves_resized_cover` (store the fixture PNG, request size 96, decode the response: the dimensions are at most 96), `unknown_cover_is_404`, `second_request_hits_disk_cache` (the cache file exists after the first call).
- [ ] **Step 2:** Implement a pure function `fn handle(store: &CoverStore, cache: &Path, path: &str) -> http::Response<Vec<u8>>` (tested directly), plus the registration in `lib.rs`. Run the tests: PASS.
- [ ] **Step 3:** Commit `feat: prism:// cover protocol with size cache`.

### Task 7: Audio engine

**Files:** Create `src-tauri/src/audio/{mod.rs, source.rs, symph.rs, ffmpeg.rs, resample.rs, output.rs}`.

**Interfaces:**
- Produces:
  - `pub trait AudioSource: Send { fn spec(&self) -> StreamSpec; fn read(&mut self, out: &mut Vec<f32>) -> anyhow::Result<usize>; fn seek(&mut self, ms: u64) -> anyhow::Result<()>; fn duration_ms(&self) -> Option<u64>; }`. `StreamSpec { sample_rate: u32, channels: u16 }`. `read` appends interleaved f32 and returns frames read, with 0 = end of stream.
  - `pub fn open_source(input: &Input) -> anyhow::Result<Box<dyn AudioSource>>`, with `enum Input { File(PathBuf) }` (M3 adds `Url`). It tries symphonia first and falls back to FFmpeg when symphonia has no decoder for the codec or the probe fails.
  - `ffmpeg::FfmpegSource::open(input, target: StreamSpec)`: spawns `ffmpeg -nostdin -hide_banner -loglevel error [-ss <s>] -i <input> -vn -f f32le -ac 2 -ar <rate> pipe:1`. Seeking = restart with `-ss`. The child is killed on drop. FFmpeg resolution: `PRISM_FFMPEG` env, then the bundled sidecar next to the exe, then PATH.
  - `resample::Resampler` (rubato, sinc) converting any rate/channels to the device spec (mono is duplicated, >2 channels are downmixed).
  - `output::Output::open(device: Option<String>) -> anyhow::Result<Output>`, which plays from an `rtrb` ring buffer fed by the engine thread. It reports `DeviceLost` through a channel when the cpal stream errors; the engine then calls `Output::open(None)` again and continues from the same position. `list_devices() -> Vec<String>`.
- Volume: applied in the output callback with a 20 ms ramp (no clicks). Gapless: the engine (Task 8) keeps the next source pre-opened, and the sample boundary is continuous (no buffer drain between tracks with the same spec).

- [ ] **Step 1:** Write the failing tests (no audio device needed: they test sources and the resampler only):
  - `symphonia_decodes_common_formats`: mp3, flac, ogg, m4a, wav fixtures each yield about 1 s of frames (+-5%).
  - `ffmpeg_fallback_decodes_rest`: opus, wma, wv, tta come through `open_source` and yield about 1 s. Skip with a printed note when ffmpeg is not found.
  - `seek_lands_near_target`: seek to 500 ms on flac, read: the remaining frames are about 0.5 s.
  - `corrupt_file_errors_cleanly`: `open_source(corrupt.flac)` or its first `read` returns Err within 2 s (no hang).
  - `resampler_converts_rate`: 44,100 Hz to 48,000 Hz, 1 s in gives 48,000 +- 64 frames out.
  - `device_lost_triggers_reopen`: with a fake `OutputFactory` trait object, simulate a stream error and assert the engine requested a reopen with `None`. Introduce `trait OutputFactory { fn open(&self, device: Option<String>) -> anyhow::Result<Box<dyn OutputSink>> }` for this.
- [ ] **Step 2:** Run, see them fail. Implement in the order source -> symph -> ffmpeg -> resample -> output.
- [ ] **Step 3:** Run `cargo test audio::`. Expected: PASS.
- [ ] **Step 4:** Commit `feat: native audio engine (symphonia + ffmpeg fallback, cpal output)`.

### Task 8: Queue, player service, IPC

**Files:** Create `src-tauri/src/player/{mod.rs, queue.rs, engine.rs}`, `src-tauri/src/commands.rs`, `src/lib/ipc.ts`, `src/lib/types.ts`, `src/lib/stores/player.svelte.ts`.

**Interfaces:**
- Consumes: `open_source`, `Output`/`OutputFactory`, `Db` (to resolve track ids to paths).
- Produces (commands; names are fixed because M2 windows reuse them):
  - `player_play(trackIds: Vec<String>, startIndex: u32)`, `player_toggle()`, `player_next()`, `player_prev()` (restarts the track when position > 3 s), `player_seek(ms: u64)`, `player_set_volume(v: f32)` (0..1, persisted), `player_state() -> PlayerState`
  - `PlayerState { status: "idle"|"loading"|"playing"|"paused", track: Option<Track>, positionMs: u64, durationMs: u64, volume: f32, queueIndex: u32, queueLength: u32 }`
  - Events: `player://state` (on every change, plus every 250 ms while playing) and `player://error { trackId, message }`.
  - `library_add_folder(path)`, `library_remove_folder(path)`, `library_folders() -> Vec<Folder>`, `library_rescan()`, `library_albums() -> Vec<Album>` (sorted by artist, year, title), `library_album_tracks(albumId) -> Vec<Track>`, `library_artists() -> Vec<Artist>`, `library_artist_albums(name) -> Vec<Album>`, `library_track_count() -> u32`, `library_tracks_range(offset: u32, limit: u32) -> Vec<Track>` (sorted by artist, album, disc, track), `library_search(q: String, limit: u32) -> SearchResult { albums, artists, tracks }`
  - TS: `ipc.ts` exports one typed function per command and `onPlayerState(cb)`, `onLibraryChanged(cb)`, `onPlayerError(cb)`; `player.svelte.ts` exports a `$state` object `player` kept in sync.
- Rules: the engine runs on its own thread; commands talk to it over a crossbeam channel. When a track fails to open or decode, emit `player://error`, then advance to the next track. When every track in the queue has failed in a row, stop with status `idle` (no infinite loop). The next track is pre-opened 5 s before the end for gapless.

- [ ] **Step 1:** Write the failing tests (`cargo test player::`) with a fake `OutputFactory` that consumes samples as fast as possible:
  - `plays_queue_in_order`: queue [tagged.mp3, tagged.flac], collect `TrackChanged` events: order mp3 then flac, then idle.
  - `skips_corrupt_and_continues`: queue [corrupt.flac, tagged.mp3]: one error event, then mp3 plays.
  - `all_failing_queue_goes_idle`: queue [corrupt.flac, corrupt.flac]: two errors, status idle, the test finishes within 5 s.
  - `prev_restarts_after_3s`: position 4 s, prev: same index, position 0.
  - `gapless_no_drain_between_same_spec`: two wav fixtures with the same spec, the fake output sees one continuous run with no zero-padding block between them.
- [ ] **Step 2:** Implement `queue.rs` (pure), `engine.rs`, `commands.rs`, and register everything in `lib.rs`. Run the tests: PASS.
- [ ] **Step 3:** Write `src/lib/types.ts` + `ipc.ts` + `player.svelte.ts`. Test `player.test.ts` (mock `@tauri-apps/api/event`): a `player://state` event updates `player.status`. Run `npm test`: PASS.
- [ ] **Step 4:** Commit `feat: queue, player service and IPC`.

### Task 9: OS media controls

**Files:** Create `src-tauri/src/media_controls.rs`. Modify `lib.rs`.

**Interfaces:**
- Consumes: player state events and commands from Task 8.
- Produces: souvlaki `MediaControls` attached to the main window HWND on Windows (SMTC) and Now Playing on macOS. Metadata is title, artist, album, duration and cover URL (`file://` path of the cached 600 px cover). Play/Pause/Toggle/Next/Previous/Seek events map to the player commands.

- [ ] **Step 1:** Implement a pure mapping `fn to_metadata(track: &Track, cover_file: Option<&Path>) -> OwnedMetadata` with a unit test `maps_track_to_metadata`, then wire it up.
- [ ] **Step 2:** Run `cargo test media_controls::`: PASS. Manual check with `npm run tauri dev`: play a fixture, the Windows media overlay shows title + cover, and the keyboard media keys toggle and skip.
- [ ] **Step 3:** Commit `feat: media keys and system now-playing`.

### Task 10: Main window shell, i18n, Flow ring (play/pause mode)

**Files:** Create `src/App.svelte`, `src/lib/i18n.ts` + test, `src/lib/stores/nav.svelte.ts`, `src/lib/format.ts` + test, `src/lib/components/{TitleBar, Sidebar, FlowRing, TransportStrip, Cover}.svelte`. Modify `lib.rs` (window-vibrancy: Mica on Windows 11, `NSVisualEffectMaterial::HudWindow` on macOS; fall back to the opaque `--bg` when unavailable).

**Interfaces:**
- Produces:
  - `t(key: string, vars?: Record<string, string | number>): string`, `setLang('en' | 'de' | 'system')`. The language follows `navigator.languages` (`de*` = German). The string tables live in `i18n.ts`.
  - `nav` state: `{ view: 'home' | 'favorites' | 'playlists' | 'albums' | 'artists' | 'songs' | 'album' | 'artist' | 'settings', param?: string }`. M1 shows `home` as Albums and keeps the Favorites/Playlists/Navidrome/YouTube Music/Radio entries visible but disabled with a "coming soon" tooltip.
  - `FlowRing.svelte` props: `{ mode: 'flow' | 'play' | 'pause', progress: number /* 0..1 */, colors?: [string, string, string, string], size?: number }`; events `onactivate`, `oncontextmenu`. M1 uses `mode` play/pause only. In M1, an idle click plays all albums shuffled (Flow replaces this in M2).
  - `format.ts`: `formatDuration(ms): string` ("3:07", "1:02:03"), `qualityLabel(track): string` ("FLAC 24/96", "MP3 320", "OPUS").
- Layout per `docs/superpowers/mockups/layout.html`: custom title bar (drag region, Windows min/max/close drawn in the Prism style, native traffic lights kept on macOS), sidebar 220 px, content area, a bottom transport strip (cover thumb, title/artist, quality, seek bar in the spectral gradient, prev/next, volume, heart placeholder hidden in M1), and the ring 84 px centered on the bottom edge of the window, half outside the content card. The content card has 16 px margins, so the ring's lower half sits over the window frame area, which is transparent with a drop shadow.

- [ ] **Step 1:** Tests: `i18n.test.ts`: every key exists in both `en` and `de`, the placeholders match between languages, and no string contains U+2013 or U+2014. `format.test.ts`: `formatDuration(187000) === "3:07"`, `formatDuration(3723000) === "1:02:03"`, `qualityLabel({format:'flac', bitDepth:24, sampleRate:96000}) === "FLAC 24/96"`, `qualityLabel({format:'mp3', bitrateKbps:320}) === "MP3 320"`.
- [ ] **Step 2:** Run `npm test`, see them fail. Implement. Run: PASS.
- [ ] **Step 3:** Build the components. Run `npm run tauri dev`. Check by hand, and take a screenshot to show the owner: no white flash, the ring sits half outside, hover transitions are smooth with reduced motion on.
- [ ] **Step 4:** Commit `feat: main window shell, Flow ring and EN/DE strings`.

### Task 11: Library views and search

**Files:** Create `src/lib/virtual.ts` + test, `src/lib/components/{AlbumGrid, AlbumView, SongList, ArtistList, SearchPalette}.svelte`.

**Interfaces:**
- Consumes: the library commands and `coverUrl` from Tasks 6/8.
- Produces: `visibleRange(scrollTop: number, viewport: number, rowHeight: number, total: number, overscan: number): { start: number; end: number }` used by SongList (rows of 44 px) and AlbumGrid (rows of cards, computed per row).
- Rules: SongList fetches pages of 200 via `library_tracks_range` on demand and caches them in a Map. AlbumView shows a large cover, a backdrop blurred from it (CSS `filter: blur(60px)` on a 96 px cover, scaled), the track list, and play / shuffle buttons. Double-click on a track = `player_play(albumTrackIds, index)`. SearchPalette opens on Ctrl+K / Cmd+K, debounces 120 ms, calls `library_search(q, 8)` and is keyboard-navigable (arrows, Enter, Esc). Empty library state: a friendly panel with an "Add music folder" button.

- [ ] **Step 1:** Test `virtual.test.ts`: `visibleRange(0, 440, 44, 30000, 5)` gives `{start:0,end:15}`; `visibleRange(44*1000, 440, 44, 30000, 5)` gives `{start:995,end:1015}`; the end is clamped at `total`.
- [ ] **Step 2:** Run, fail, implement, PASS.
- [ ] **Step 3:** Build the views, plus drop-to-play: files dropped onto the window are played via a command `player_play_files(paths: Vec<String>)`, which reads tags on the fly without indexing. Manual check with a demo library in a gitignored `demo/` folder, generated with `scripts/make-fixtures.mjs --demo 3000` (synthetic tagged files, 3,000 tracks / 300 albums). Scrolling Songs and Albums stays smooth, search answers instantly, double-click plays.
- [ ] **Step 4:** Commit `feat: albums, artists, songs, album view and Ctrl+K search`.

### Task 12: Settings (folders, playback) and About dialog

**Files:** Create `src/lib/components/{Settings, About}.svelte`, `src-tauri/src/settings.rs`.

**Interfaces:**
- Produces: `settings_get() -> Settings`, `settings_set(patch: serde_json::Value) -> Settings`. Settings are stored in `<appConfig>/settings.json`: `{ language: 'system'|'en'|'de', volume: f32, outputDevice: Option<String>, gapless: bool, replayGain: 'off'|'track'|'album' }`. `audio_devices() -> Vec<String>`.
- ReplayGain: read the RG tags in Task 5's `tags::read` (`replayGainTrackDb`, `replayGainAlbumDb` on Track) and apply them in the engine as a linear gain, clamped to avoid clipping (peak-aware when the peak tag exists).
- About (opened from the sidebar logo and Settings): logo, "Prism", version from `getVersion()`, the tagline "One player, every color of sound.", "Made by Wolfgang Linz" linking to `https://wolfgang-linz.de` via `@tauri-apps/plugin-opener`, a GitHub link `https://github.com/wl-lankin/prism`, and a "Licenses" section (a static list for now; M6 generates it).

- [ ] **Step 1:** Rust test `settings_roundtrip_and_defaults` (a missing file gives defaults, a patch is merged and saved) and `replaygain_gain_clamps_to_peak` (gain -6 dB with peak 0.99 gives a factor of about 0.501; +6 dB with peak 0.9 is clamped so that peak*gain <= 1.0).
- [ ] **Step 2:** Run, fail, implement, PASS. Build the UI. Manual check: add a folder via the dialog, the scan progress shows, the views fill; switching the output device works; the About link opens the browser.
- [ ] **Step 3:** Commit `feat: settings, ReplayGain and About dialog`.

### Task 13: M1 hand check

- [ ] **Step 1:** Run `npm test`, `npm run check`, `cargo test --manifest-path src-tauri/Cargo.toml`. Expected: all green. Paste the summary lines.
- [ ] **Step 2:** `npm run tauri dev` with the demo library. Play one fixture of each format in the fixtures list, including the FFmpeg ones. Unplug or switch the output device during playback. Take screenshots of the main window, an album view, search and About.
- [ ] **Step 3:** Ask the owner for a hand check on their machine with their own music folder (they add it themselves). Collect feedback, fix, commit. Update the memory entry for Prism. Then write the M2 plan against the real code.
