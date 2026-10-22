# Prism M2 (Everyday player) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Prism becomes the player that runs all day: a sturdier library and engine (stable track ids, read-ahead decoding, cover resize limits, safe startup), then favorites, Flow, crossfade, Prism playlists, a Now Playing view, the cover-colored ring, online cover lookup, the mini player with tray / taskbar / global shortcut, "Open with", and the remaining settings.

**Architecture:** Same split as M1: Rust owns every piece of state that two windows share (library DB, favorites, playlists, queue, Flow session, settings, window shapes) and pushes it as events; each web view (`main`, and the new `mini`) is a view over IPC commands plus those events. M2 opens with five foundation tasks (ids, read-ahead, cover gate, startup safety, indexes) because later features key on track ids and rely on a non-blocking engine. The OS shell (tray, taskbar, shortcut, single instance, window regions) lives in a new `src-tauri/src/shell/` module so `lib.rs` stays a wiring file.

**Tech Stack:** M1 stack plus: `tauri` feature `tray-icon`, `tauri-plugin-global-shortcut`, `tauri-plugin-single-instance`, `tauri-plugin-autostart`, `rand` (seeded Flow), `reqwest` (blocking, rustls; same major as already in Cargo.lock) for MusicBrainz, `rfd` (already in Cargo.lock via the dialog plugin) for the startup failure dialog, `windows` 0.62 (COM: `ITaskbarList3`) next to `windows-sys` 0.61. The dead `window-vibrancy` dependency is removed.

**Spec:** `docs/superpowers/specs/2026-10-20-prism-design.md` (sections 5.2, 5.3, 5.7 local only, 5.8, 5.9, 5.10, 5.12). Roadmap and M1 constraints: `docs/superpowers/plans/2026-10-20-prism-m1-foundation.md`. M1 ledger with rulings and deferred items: `.superpowers/sdd/2026-10-20-prism-m1-foundation/progress.md`.

## Global Constraints

- Platforms: Windows 11 and macOS 11+ (universal). Developed and checked on Windows; anything Windows-only is behind `cfg(target_os = "windows")` with a macOS counterpart or a no-op, and every such pair is named in its task.
- Repo `C:\Projects\private\Prism`, work on the current branch `PRISM-1`, repo-local identity `Wolfgang Linz <27097016+wl-lankin@users.noreply.github.com>`. Commit, never push.
- App identifier `de.wolfganglinz.prism`, productName `Prism`, version `1.0.0`, MIT, publisher "Wolfgang Linz", homepage `https://wolfgang-linz.de`.
- No em dash or en dash anywhere (code, UI strings, docs, commits). No AI attribution or session links anywhere. No sparkle glyphs.
- Palette: background `#0d0d10`, surface `#141418`, raised `#1f1f25`, text `#f3f3f5`; spectrum `#ff4d6d`, `#ffc94d`, `#4dffb2`, `#4da6ff`. Source colors: navidrome `#4dffb2`, apple `#ff4d6d`, youtube `#ffc94d`, radio `#4da6ff`, local `#cccccc`.
- Reduced motion: only large motion is switched off; hover and fade transitions keep running at 120 ms (`--dur`).
- Window look (owner decision): no Mica, no vibrancy, no OS shadow, no DWM border or rounded system corners. The main window's Flow ring is 96 px and hangs half out of the card's bottom edge over a transparent strip; the mini player's ring (76 px, `mini-v2.html`) hangs half out of its left edge.
- The mini player never steals focus and never appears without a user action (ring/tray/taskbar click, shortcut, "Open with").
- Dev server ports 1440 (Vite) and 1441 (HMR). Never kill processes by image name (`taskkill /IM node.exe` is forbidden); stop only PIDs you started.
- Write files with Write/Edit, never Bash heredocs. Frontend tests are `src/**/*.spec.ts`.
- Commands: `npm test`, `npm run check`, `cargo test --manifest-path src-tauri/Cargo.toml [filter]`. The local `ffmpeg`/`ffprobe` on PATH are fine for development.
- Migrations are append-only from now on: the owner has a real library DB. Never edit `0001_init.sql` or any migration an earlier task added.
- The only network calls in M2 are MusicBrainz and the Cover Art Archive (Task 18), with User-Agent `Prism/<version> ( https://wolfgang-linz.de )`, at most 1 request per second, and only while the setting is on.
- Never use the owner's real library, accounts or credentials in tests, fixtures or screenshots.
- Every new UI string goes into both `en` and `de` in `src/lib/i18n.ts` (the existing `i18n.spec.ts` checks keys, placeholders and dashes).

## Review Focus

1. **A NAS library folder whose mount point exists but is empty** (share not mounted yet at login): the scan must mark the folder offline and keep its tracks, favorites and playlist entries; nothing is deleted. Pinned in Task 2 (`empty_mount_point_keeps_tracks`) and Task 9 (`favorites_survive_an_offline_folder`).
2. **A slow NAS read or an FFmpeg stall mid-track:** audio keeps playing from the decoded buffer, and pause / next / seek answer within 100 ms instead of waiting for the read. Pinned in Task 5 (`slow_source_does_not_block_pump`, `seek_does_not_block_on_slow_source`).
3. **Flow with a tiny or lopsided pool** (no favorites, one favorite, every favorite by one artist): Flow still starts, never loops forever, pads small pools, and falls back to the whole library when there are no favorites. Pinned in Task 11 (`single_artist_pool_still_plays`, `small_pool_is_padded`) and Task 12 (`empty_favorites_fall_back_to_library`).
4. **Display scaling (125 %, 150 %) and a taskbar not at the bottom:** the click-through region and the mini player's docked position must use physical pixels and the monitor work area, not the screen size. Pinned in Task 19 (`region_scales_with_dpi`) and Task 20 (`docks_inside_work_area`).
5. **"Open with" on a file from a network share or a path with spaces, while Prism is already running with the main window closed:** the running instance plays it (single instance), shows the mini player without focus, and does not refuse the UNC path. Pinned in Task 23 (`args_keep_unc_and_spaced_paths`, `second_instance_plays_files`).

---

## File Structure

```
src-tauri/src/db/mod.rs                    Migration enum (SQL or Rust step), runner
src-tauri/src/db/migrations/0002_indexes.sql, 0004_favorites.sql, 0005_track_stats.sql,
                         0006_playlists.sql, 0007_cover_lookups.sql
src-tauri/src/db/rekey.rs                  0003 Rust step: track ids by folder + relative path; TRACK_ID_REFS
src-tauri/src/startup.rs                   DB quarantine (library.db.bad) + startup failure dialog
src-tauri/src/audio/readahead.rs           decode thread with a 2-4 s buffer (ReadAhead)
src-tauri/src/audio/mix.rs                 crossfade curves, level meter
src-tauri/src/thumbs.rs                    cover resize (moved from protocol.rs) + ResizeGate
src-tauri/src/library/favorites.rs         local favorites
src-tauri/src/library/history.rs           play history (track_stats) + recorder sink
src-tauri/src/library/playlists.rs         Prism playlists
src-tauri/src/library/lookup.rs            MusicBrainz / Cover Art Archive lookup worker
src-tauri/src/flow/{mod.rs, deck.rs}       Flow service (refill) and the pure deck (rules)
src-tauri/src/shell/{mod.rs, shape.rs, mini.rs, tray.rs, taskbar_win.rs, shortcut.rs, open_with.rs}
src-tauri/windows/hooks.nsh                NSIS hooks: "Open with" registration (Windows)
src-tauri/Info.plist                       CFBundleDocumentTypes, rank Alternate (macOS)
src/lib/window-shape.json                  window geometry shared by CSS and the Rust regions
src/lib/palette.ts                         cover -> ring colors
src/lib/stores/{favorites, playlists, ring, window}.svelte.ts
src/lib/components/{Heart, FavoritesView, PlaylistsView, PlaylistView, TrackMenu, FlowMenu,
                    NowPlaying, MiniPlayer, ShortcutInput}.svelte
```

---

### Task 1: Migration runner with Rust steps, migration 0002 (indexes)

**Files:**
- Modify: `src-tauri/src/db/mod.rs`
- Create: `src-tauri/src/db/migrations/0002_indexes.sql`

**Interfaces:**
- Produces:
  - `pub enum Migration { Sql(&'static str), Rust(fn(&rusqlite::Transaction) -> anyhow::Result<()>) }` and `const MIGRATIONS: &[Migration]`. Each step runs in its own transaction and sets `user_version` to its 1-based index, exactly like today.
  - `0002_indexes.sql`: `idx_albums_artist ON albums (artist)`, `idx_tracks_folder ON tracks (folder)`, `idx_tracks_order ON tracks (artist COLLATE NOCASE, album COLLATE NOCASE, disc_no IS NULL, disc_no, track_no IS NULL, track_no, title COLLATE NOCASE, id)` (the exact `TRACK_ORDER` of `library/query.rs`), `idx_albums_order ON albums (artist COLLATE NOCASE, year IS NULL, year, title COLLATE NOCASE, id)` (`ALBUM_ORDER`).

- [ ] **Step 1: Write the failing tests** in `db/mod.rs`: `migrates_fresh_db_to_latest` asserts `user_version == MIGRATIONS.len()` (not a literal); `rust_step_runs_once` (a test-only list with a Rust step that inserts a row: open twice, one row); `rejects_negative_user_version` (`PRAGMA user_version = -1` -> `migrate` errs; closes the M1 gap); `tracks_range_uses_order_index`: `EXPLAIN QUERY PLAN` of `SELECT id FROM tracks t ORDER BY <TRACK_ORDER> LIMIT 200 OFFSET 0` contains `idx_tracks_order` and no `USE TEMP B-TREE`; the same for albums with `ALBUM_ORDER` and `idx_albums_order`; `folder_lookup_uses_index` for `WHERE folder = ?1`. Make `TRACK_ORDER` and `ALBUM_ORDER` `pub(crate)` so the test reuses them.
- [ ] **Step 2: Run** `cargo test --manifest-path src-tauri/Cargo.toml db::` -> FAIL (index tests, version count).
- [ ] **Step 3: Implement** the enum and runner; add `0002_indexes.sql`. If SQLite does not use the expression index for the `t.`-aliased query, align the index expressions with the query rather than changing the sort order.
- [ ] **Step 4: Run** the same command -> PASS; run the full `cargo test` -> PASS.
- [ ] **Step 5: Commit** `feat: migration runner with Rust steps, list indexes`.

### Task 2: Track ids from folder key + relative path, rekey migration, empty mount guard

**Files:**
- Modify: `src-tauri/src/library/model.rs`, `src-tauri/src/library/scan.rs`, `src-tauri/src/library/tags.rs:99`, `src-tauri/src/db/mod.rs`
- Create: `src-tauri/src/db/rekey.rs`

**Interfaces:**
- Consumes: `Migration::Rust` (Task 1).
- Produces:
  - `pub fn track_id(folder_key: &str, relative: &Path) -> String` = `"local:" + hex(sha1(fold(folder_key) + "\u{1f}" + fold(rel)))`, where `rel` joins the components with `/`, and `fold` lowercases on Windows and macOS (as `library::comparable` does) and is the identity elsewhere. No `canonicalize` or any other file system call. `file_track_id(path)` keeps its `file:` prefix but also drops `canonicalize` (absolute + the same Windows normalization).
  - `tags::read` no longer sets an id (`id: String::new()`); `scan_folder` and `DroppedFiles::read` set it.
  - `rekey.rs`: `pub struct TrackIdRef { pub table: &'static str, pub column: &'static str, pub filter: Option<&'static str> }`, `pub const TRACK_ID_REFS: &[TrackIdRef]` (empty now; Tasks 9, 11 and 14 add their tables), `pub fn rekey_tracks(tx: &Transaction, map: &[(String, String)]) -> anyhow::Result<()>` (updates `tracks.id` and every ref whose table exists), and `pub fn migrate_0003(tx: &Transaction) -> anyhow::Result<()>` (registered as migration 3).
- Rules: the migration computes the new id of every `tracks` row from `folder` and `path` (`path` minus the `folder` prefix). A row whose path does not start with its folder keeps its id and is logged. When two old rows map to one new id, the first by `row_id` wins and the others are deleted (their refs are rewritten to the winner). FTS rows follow through the existing update/delete triggers.
- Mass-deletion guard in `scan_folder`: when the walk finds zero audio files but the folder has indexed tracks, the folder is set `online = 0`, nothing is removed, and `failed` gets one entry `(folder, "no audio files found; treated as offline")`.

- [ ] **Step 1: Write the failing tests:** `model.rs`: `track_id_uses_folder_and_relative_path` (same file, two spellings of the folder that differ only in case on Windows give one id; a different relative path gives another), `track_id_does_not_touch_the_file_system` (a path under a nonexistent UNC root `\\nas\music` gives a stable id with no `UNC\` text in the hashed key, checked via a `pub(crate) fn id_key(..) -> String` helper). `rekey.rs`: `rekey_rewrites_ids_and_keeps_fts` (insert two rows with old-style ids, run `migrate_0003`, ids equal `track_id(folder, rel)`, `tracks_fts MATCH` still finds both), `rekey_merges_collisions` (two rows that map to one id leave one row), `rekey_updates_registered_refs` (a test-only ref table with a `filter` is rewritten only where the filter matches). `scan.rs`: `empty_mount_point_keeps_tracks` (scan fixtures, empty the directory but keep it, rescan: track count unchanged, folder `online == 0`, `failed` has the entry), `rescan_after_rekey_is_unchanged` (scan, rekey, rescan: `unchanged == total`, `added == 0`).
- [ ] **Step 2: Run** `cargo test --manifest-path src-tauri/Cargo.toml library:: db::` -> FAIL.
- [ ] **Step 3: Implement** the id function, the scan change (compute `rel` from `path.strip_prefix(folder)`), the guard, `rekey.rs`, and register migration 3. Update existing tests that call `track_id(path)`.
- [ ] **Step 4: Run** the full `cargo test` -> PASS.
- [ ] **Step 5: Commit** `feat: track ids from folder and relative path, rekey migration, empty mount guard`.

### Task 3: Scanner and watcher hardening (grouped M1 minors)

**Files:**
- Modify: `src-tauri/src/library/scan.rs`, `src-tauri/src/library/watch.rs`, `src-tauri/src/library/mod.rs`, `src-tauri/src/commands.rs`, `src/lib/stores/library.svelte.ts`

**Interfaces:**
- Consumes: `Library`, `FolderWatcher`, `tools::ffprobe_path()`.
- Produces: `watch::is_relevant(path: &Path, is_dir: bool) -> bool` (audio extension, a cover file name from `find_folder_cover`, or a directory; temp and other files are ignored); `Library::failed_stamps` (in-memory `(path -> (mtime, size))`, so unchanged failing files are not re-read every scan); `watch::start` also re-checks offline folders every 60 s and, once a folder is readable again, watches and rescans it.
- Rules: rows from `UNPARSED_EXTENSIONS` are re-read only when ffprobe is available, decided once per `Library` (`Library::new(db, covers, LibraryOptions { reread_unparsed: bool })`, set from `tools::ffprobe_path().is_some()` in `lib.rs`; `ffprobe_path` caches in a `OnceLock`, so tests pass the option instead of an env var). No `Changed` event every rescan without ffprobe; cover stat and hashing happen before the DB lock is taken (register only the id under the lock); `add_folder` holds `scan_lock` across the overlap check and the insert (atomic). The frontend treats a `0/0` progress as done.

- [ ] **Step 1: Write the failing tests:** `watcher_ignores_temp_files` (`is_relevant("a.flac.tmp", false) == false`, `"song.FLAC"` true, `"folder.jpg"` true, a directory true); `failing_file_not_reread_when_unchanged` (scan with `corrupt.flac` twice; the second report has it in `unchanged`, not `failed`, until its mtime changes); `unparsed_rows_quiet_without_ffprobe` (a `Library` with `reread_unparsed: false` and a zero-duration `wma` row: two rescans emit no `Changed`); `rescan_2000_unchanged_under_1s` becomes `#[ignore]` in debug and is run with `--release` (ledger: wall-clock flake); `library.spec.ts`: a `{done:0,total:0}` progress clears the busy state.
- [ ] **Step 2: Run** `cargo test --manifest-path src-tauri/Cargo.toml library::` and `npm test -- library` -> FAIL.
- [ ] **Step 3: Implement.** For the tools test that sets `PRISM_TEST_TOOL_OVERRIDE` (ledger: racy), route `resolve` through a pure `fn resolve_from(env_value: Option<OsString>, name: &str) -> Option<PathBuf>` and test that instead.
- [ ] **Step 4: Run** both commands -> PASS.
- [ ] **Step 5: Commit** `fix: quieter rescans, watcher filter, offline folders come back`.

### Task 4: Startup failure dialog and DB quarantine

**Files:**
- Create: `src-tauri/src/startup.rs`
- Modify: `src-tauri/src/lib.rs`, `src-tauri/src/db/mod.rs`, `src-tauri/Cargo.toml` (`rfd = "0.16"`), `src/lib/ipc.ts`, `src/App.svelte`, `src/lib/i18n.ts`

**Interfaces:**
- Consumes: `Db::open`.
- Produces:
  - `pub enum OpenOutcome { Opened(Db), Quarantined { db: Db, bad: PathBuf } }`, `pub fn open_or_quarantine(path: &Path) -> anyhow::Result<OpenOutcome>`: on `SQLITE_CORRUPT` / `SQLITE_NOTADB` (or a failing `PRAGMA quick_check`), it moves `library.db`, `-wal` and `-shm` to `library.db.bad` (`library.db.bad.2`, `.3` when taken) and creates a fresh DB. A schema from a newer build is not quarantined; it is an error.
  - `pub fn fail(title: &str, body: &str) -> !` shows a native error dialog with `rfd::MessageDialog` (synchronous, safe on the main thread during setup) and exits with code 1. `run()` calls it when `setup` returns `Err`, instead of the `expect` panic.
  - `pub struct StartupNotice(pub Mutex<Option<String>>)` managed state; command `startup_notice() -> Option<String>` returns and clears it; the main window shows it via `notify(t('startup.quarantined', { file }))`.
- Rules: the dialog text is English and German by the system locale only for these two strings, both kept in `startup.rs` (the web view is not available yet). They name the data folder and say a newer Prism wrote the DB, or that the folder cannot be created.

- [ ] **Step 1: Write the failing tests** in `startup.rs`: `corrupt_db_is_quarantined` (write 4 KiB of garbage to `library.db`, open: `Quarantined`, `library.db.bad` holds the garbage, the new DB has `user_version == MIGRATIONS.len()`), `second_quarantine_does_not_overwrite` (`.bad` exists -> `.bad.2`), `newer_schema_is_an_error_not_quarantine` (user_version 99 -> `Err`, file untouched), `healthy_db_opens`.
- [ ] **Step 2: Run** `cargo test --manifest-path src-tauri/Cargo.toml startup::` -> FAIL.
- [ ] **Step 3: Implement** and wire into `setup` and `run`.
- [ ] **Step 4: Run** -> PASS. Manual: corrupt a copy of a dev DB under a temp `APPDATA` and start `npm run tauri dev`: the notice appears; make the data dir a file: the dialog appears and the process exits.
- [ ] **Step 5: Commit** `feat: quarantine a corrupt library DB, show startup failures`.

### Task 5: Read-ahead decode thread, non-blocking seek, gapless for FFmpeg sources

**Files:**
- Create: `src-tauri/src/audio/readahead.rs`
- Modify: `src-tauri/src/audio/mod.rs` (`Playback`), `src-tauri/src/player/engine.rs` (`maybe_preload`), `src-tauri/src/audio/output.rs`, `src-tauri/src/audio/ring.rs` (tests)

**Interfaces:**
- Consumes: `AudioSource`, `guarded`.
- Produces:
  - `pub const READ_AHEAD_MS: u32 = 3_000;` (inside the 2-4 s window).
  - `pub struct ReadAhead` with `ReadAhead::spawn(source: Box<dyn AudioSource>, target_ms: u32) -> ReadAhead`, `spec()`, `duration_ms()`, `try_read(&mut self, out: &mut Vec<f32>) -> anyhow::Result<ReadOutcome>`, `seek(&mut self, ms: u64)` (never blocks: bumps an epoch, drops buffered chunks, the thread seeks), `exhausted(&self) -> bool` (the decoder reached the end), `buffered_frames(&self) -> u64`, `set_target_ms(&mut self, ms: u32)`.
  - `pub enum ReadOutcome { Frames(usize), Starved, End }`. Errors from the decode thread are delivered after the frames decoded before them. Dropping a `ReadAhead` stops its thread (an FFmpeg child is killed there).
  - `Playback` keeps `current` and `next` as `ReadAhead` (`set_source` / `queue_next` still take `Box<dyn AudioSource>` and wrap them). `Starved` ends the fill loop with `PumpStatus::Playing`. `Playback::seek` no longer blocks. New `Playback::current_exhausted(&self) -> bool`.
  - Engine rule: `maybe_preload` also fires when `playback.current_exhausted()` (so tracks with `duration_ms == 0`, typically FFmpeg-decoded ones, get gapless), and no longer depends on the duration being known.
- Grouped minors here: `RING_MS` in tests reuses `output::BUFFER_MS` (make it `pub(crate)`); a device switch during a gapless tail keeps the queued next source (`set_device` resumes `next` too); `corrupt_file_errors_cleanly` uses a 5 s bound measured with a generous margin instead of 2 s (ledger flake).

- [ ] **Step 1: Write the failing tests** in `readahead.rs` and `audio/mod.rs` with a `SlowSource` fake (sleeps a configurable time per `read` and per `seek`): `buffers_up_to_target` (after 200 ms the buffer holds about `target_ms` of frames and no more than target + one chunk), `slow_source_does_not_block_pump` (source read sleeps 500 ms once mid-stream: every `pump()` call returns within 20 ms, and audio continues from the buffer), `seek_does_not_block_on_slow_source` (seek sleeps 1 s: `Playback::seek` returns within 20 ms; after the seek, the first frames read come from the target position), `error_after_frames_is_delivered_last`, `drop_stops_the_thread` (a counter in the fake's `Drop` reaches 1 within 500 ms). In `engine.rs`: `unknown_duration_tracks_are_gapless` (two wav fixtures with `duration_ms: 0` in their `Track`, the fake output sees one continuous run, like `gapless_no_drain_between_same_spec`).
- [ ] **Step 2: Run** `cargo test --manifest-path src-tauri/Cargo.toml audio:: player::` -> FAIL.
- [ ] **Step 3: Implement** `ReadAhead` (one thread per source, `Mutex<VecDeque<Chunk>>` + `Condvar`, chunks of 4096 frames, each tagged with the epoch) and switch `Playback` over. Keep the `guarded` wrapping around the source calls inside the thread.
- [ ] **Step 4: Run** the full `cargo test` -> PASS (all M1 engine tests unchanged, including the gapless and device-lost ones).
- [ ] **Step 5: Commit** `feat: read-ahead decoding, non-blocking seek, gapless for FFmpeg sources`.

### Task 6: Cover resize gate (concurrency limit + coalescing)

**Files:**
- Create: `src-tauri/src/thumbs.rs` (move `resize`, `flatten`, `write_atomic`, `cached_cover`, `placeholder_cover` out of `protocol.rs`)
- Modify: `src-tauri/src/protocol.rs`, `src-tauri/src/media_controls.rs`, `src-tauri/src/lib.rs`

**Interfaces:**
- Produces: `pub struct ResizeGate` (managed in `AppState` as `thumbs: Arc<ResizeGate>`), `ResizeGate::new(limit: usize)`, `pub fn get(&self, store: &CoverStore, cache: &Path, id: &str, size: u32) -> Result<PathBuf, ResizeError>`. At most `limit` resizes run at once (`limit = min(4, available_parallelism)`), and concurrent requests for the same `(id, size)` wait for the one in flight and share its result. `protocol::handle` and `media_controls` go through it.
- Grouped minor: the placeholder file name carries a short hash of the icon bytes (`_placeholder-<8 hex>.jpg`), so a new icon is not hidden by an old cache file.

- [ ] **Step 1: Write the failing tests** in `thumbs.rs` with a counting resize function injected through a `pub(crate) fn get_with(.., resize: impl Fn(&Path, u32) -> Result<Vec<u8>, ResizeError>)`: `same_cover_is_resized_once` (16 threads ask for one `(id, 300)` at once: resize ran once, all get the same path), `limit_caps_parallel_resizes` (32 different ids, the resize fn tracks max concurrency: never above the limit), `error_is_shared_then_retried` (an `Undecodable` result reaches every waiter; a later call tries again). Existing `protocol` tests stay green.
- [ ] **Step 2: Run** `cargo test --manifest-path src-tauri/Cargo.toml thumbs:: protocol::` -> FAIL.
- [ ] **Step 3: Implement** with `Mutex<HashMap<(String, u32), Arc<InFlight>>>` and a counting semaphore (`Mutex<usize>` + `Condvar`).
- [ ] **Step 4: Run** -> PASS.
- [ ] **Step 5: Commit** `perf: limit and coalesce cover resizes`.

### Task 7: Queue operations for M2 and engine minors

**Files:**
- Modify: `src-tauri/src/player/{engine.rs, queue.rs, mod.rs}`, `src-tauri/src/commands.rs`, `src-tauri/src/lib.rs`, `src/lib/{types.ts, ipc.ts}`, `src/lib/stores/player.svelte.ts`

**Interfaces:**
- Produces:
  - `Queue::extend(&mut self, ids: Vec<String>)`, `Queue::jump(&mut self, index: usize) -> bool`, `Queue::ids(&self) -> &[String]`.
  - `#[derive(Clone)] pub struct PlayerHandle` from `Player::handle()`: `play`, `append(ids)`, `jump(index)`, `state()`, `queue_ids() -> Vec<String>`. Tasks 12, 21, 22 and 23 use it from other threads.
  - `PlayerState.queue_version: u64` (`queueVersion`), bumped whenever the queue's ids change.
  - Commands: `player_append(trackIds: Vec<String>)`, `player_jump(index: u32)`, `player_queue_range(offset: u32, limit: u32) -> Vec<QueueItem>` with `QueueItem { index: u32, track: Option<Track> }` (`None` when the id no longer resolves). TS: `playerAppend`, `playerJump`, `playerQueueRange`, types `QueueItem`.
- Rules (grouped engine minors from the ledger): the opener runs one worker per slot (`Current`, `Next`), so a slow preload never delays a manual Next; the paused hand-over deadline is shifted by the paused time on resume; a no-op seek (not seekable, or same position) emits no state; `player://state` is only emitted when the snapshot differs from the last one sent (the 250 ms tick still sends while playing); `prev` during the final tail of the last track restarts the last track; a failing pre-opened track no longer cuts the previous track's tail; after 10 failures in a row the player goes idle with one error whose message is `"<n> tracks in a row could not be played"` (no error flood on a dead queue).

- [ ] **Step 1: Write the failing tests** (`cargo test player::`): `queue_extend_and_jump` (pure), `append_keeps_playing_and_bumps_version`, `jump_plays_that_index`, `queue_range_marks_missing_tracks`, `slow_preload_does_not_delay_next` (opener sleeps 2 s for the next slot only: a manual `next` reaches `Playing` of the following track within 500 ms), `dead_queue_reports_once` (50 missing ids: at most 10 `Error` events, then idle; the last message mentions `10`), `noop_seek_is_silent`.
- [ ] **Step 2: Run** -> FAIL. **Step 3: Implement.** **Step 4: Run** the full `cargo test` and `npm test` (`player.spec.ts` gains a `queueVersion` default) -> PASS.
- [ ] **Step 5: Commit** `feat: queue append, jump and range; quieter engine`.

### Task 8: Settings for M2 (schema, apply hooks, volume persisted by Rust)

**Files:**
- Create: `src-tauri/src/shell/mod.rs`
- Modify: `src-tauri/src/settings.rs`, `src-tauri/src/commands.rs`, `src-tauri/src/lib.rs`, `src/lib/types.ts`, `src/lib/stores/settings.svelte.ts`, `src/lib/components/TransportStrip.svelte`

**Interfaces:**
- Produces:
  - New `Settings` fields (camelCase in JSON) with defaults: `crossfade: Crossfade` (`"off" | "flow" | "always"`, default `flow`), `crossfadeMs: u32` (default `4000`, clamped to `1000..=8000`), `coverLookup: bool` (default `true`), `miniOnTop: bool` (default `false`), `miniShortcut: String` (default `"CommandOrControl+Alt+P"`, which is Ctrl+Alt+P on Windows and Cmd+Option+P on macOS), `startWithSystem: bool` (default `false`).
  - `Change` gains `Crossfade(Crossfade, u32)`, `CoverLookup(bool)`, `MiniOnTop(bool)`, `MiniShortcut(String)`, `StartWithSystem(bool)`. `commands::settings_set` sends player changes to the player and every other change to `shell::apply_setting(app: &AppHandle, change: &Change) -> Result<(), String>` (Task 8 adds `shell/mod.rs` with this function logging only; Tasks 18, 20 and 22 fill it).
  - `SettingsStore::remember_volume(&self, v: f32)` (memory, marks dirty, a 400 ms debounced write on a helper thread) and `SettingsStore::flush(&self)`. `player_set_volume` calls `remember_volume`. `run()` switches to `.build(..)?.run(|app, event| ..)` and calls `flush()` on `RunEvent::Exit`.
  - The frontend drops `saveVolume`, `flushVolume` and the `onCloseRequested` hook (they would block the close-to-hide of Task 21).
  - `Player::set_crossfade(mode: Crossfade, ms: u32)` is added as a stored no-op here; Task 13 makes it work.

- [ ] **Step 1: Write the failing tests:** `settings_roundtrip_and_defaults` extended with every new default; `crossfade_ms_is_clamped`; `old_file_without_new_keys_loads` (an M1 settings.json keeps its values and gets the new defaults); `remembered_volume_is_flushed` (remember 0.3, `flush`, reload from disk: 0.3); `settings.spec.ts`: `DEFAULT_SETTINGS` mirrors the Rust defaults.
- [ ] **Step 2: Run** `cargo test --manifest-path src-tauri/Cargo.toml settings::` and `npm test -- settings` -> FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** -> PASS. Manual: change the volume, quit via the window close (still the M1 close until Task 21), restart: the volume is kept.
- [ ] **Step 5: Commit** `feat: M2 settings, volume saved by the backend`.

### Task 9: Favorites backend (local)

**Files:**
- Create: `src-tauri/src/library/favorites.rs`, `src-tauri/src/db/migrations/0004_favorites.sql`
- Modify: `src-tauri/src/db/{mod.rs, rekey.rs}`, `src-tauri/src/commands.rs`, `src-tauri/src/lib.rs`, `src/lib/{types.ts, ipc.ts}`

**Interfaces:**
- Consumes: `query::track/album/artist` column helpers, `TRACK_ID_REFS`.
- Produces:
  - Schema: `favorites(kind TEXT NOT NULL, item_id TEXT NOT NULL, source TEXT NOT NULL DEFAULT 'local', added_at INTEGER NOT NULL, PRIMARY KEY (kind, item_id))`. `kind` is `track | album | artist`; tracks are keyed by track id, albums by album id, artists by name. `TRACK_ID_REFS` gains `{ table: "favorites", column: "item_id", filter: Some("kind = 'track'") }`.
  - `#[serde(rename_all = "lowercase")] pub enum FavKind { Track, Album, Artist }`; `pub struct FavoriteIds { tracks: Vec<String>, albums: Vec<String>, artists: Vec<String> }`.
  - `favorites::set(conn, kind, id, on: bool) -> anyhow::Result<bool>` (idempotent, returns the new state), `ids(conn) -> FavoriteIds`, `tracks(conn) -> Vec<Track>`, `albums(conn) -> Vec<Album>`, `artists(conn) -> Vec<Artist>` (only rows that resolve; newest first).
  - Commands: `favorites_set(kind, id, on) -> bool`, `favorites_ids() -> FavoriteIds`, `favorites_tracks()`, `favorites_albums()`, `favorites_artists()`. Event `favorites://changed` (no payload) after every effective change. TS wrappers `favoritesSet`, `favoritesIds`, `favoritesTracks`, `favoritesAlbums`, `favoritesArtists`, `onFavoritesChanged`.
- Rules: favorites whose item is not in the index (folder offline, file removed) are kept, not shown, and come back when the item returns.

- [ ] **Step 1: Write the failing tests:** `set_is_idempotent`, `ids_group_by_kind`, `lists_resolve_and_skip_missing`, `favorites_survive_an_offline_folder` (scan fixtures, favorite a track and its album, make the folder unreadable, rescan, restore, rescan: both still favorites and listed), `rekey_updates_track_favorites` (old-style id favorite is rewritten by `rekey_tracks`; an album favorite with the same text is untouched).
- [ ] **Step 2: Run** `cargo test --manifest-path src-tauri/Cargo.toml favorites` -> FAIL. **Step 3: Implement.** **Step 4: Run** -> PASS.
- [ ] **Step 5: Commit** `feat: local favorites for tracks, albums and artists`.

### Task 10: Favorites UI (hearts, Favorites view)

**Files:**
- Create: `src/lib/stores/favorites.svelte.ts`, `src/lib/components/Heart.svelte`, `src/lib/components/FavoritesView.svelte`, `src/lib/stores/favorites.spec.ts`
- Modify: `src/App.svelte`, `src/lib/components/{TransportStrip, AlbumView, SongList, ArtistView, Sidebar}.svelte`, `src/lib/i18n.ts`

**Interfaces:**
- Consumes: favorites commands (Task 9).
- Produces: `favorites` store (`SvelteSet`s `tracks`, `albums`, `artists`), `initFavorites(): Promise<() => void>` (loads, then follows `favorites://changed`), `isFavorite(kind, id): boolean`, `setFavorite(kind, id, on): Promise<void>` (optimistic, reverted with a notice on failure). `Heart.svelte` props `{ kind: FavKind; id: string; size?: number; label?: string }`, an accessible toggle button (`aria-pressed`). `FavoritesView` has tabs Songs / Albums / Artists and source chips (All, Local files; the Navidrome chip appears in M3 when that source has favorites).
- Rules: the heart is visible in the transport strip (current track), the album header, the artist header, and on song rows on hover or when set. Sidebar "Favorites" is enabled. Playing from the Favorites Songs tab queues the listed tracks.

- [ ] **Step 1: Write the failing tests:** `favorites.spec.ts` (mock IPC): `setFavorite` updates the set before the IPC resolves and reverts on rejection; a `favorites://changed` event reloads the ids. `views.spec.ts`: `Heart` renders `aria-pressed="true"` for a favorite and toggles on click.
- [ ] **Step 2: Run** `npm test` -> FAIL. **Step 3: Implement.** **Step 4: Run** `npm test` and `npm run check` -> PASS. Manual: heart a song, an album and an artist; the Favorites view lists them.
- [ ] **Step 5: Commit** `feat: hearts and the Favorites view`.

### Task 11: Play history and the Flow deck (rules)

**Files:**
- Create: `src-tauri/src/library/history.rs`, `src-tauri/src/db/migrations/0005_track_stats.sql`, `src-tauri/src/flow/mod.rs`, `src-tauri/src/flow/deck.rs`
- Modify: `src-tauri/src/db/{mod.rs, rekey.rs}`, `src-tauri/src/lib.rs`, `src-tauri/Cargo.toml` (`rand = "0.9"`)

**Interfaces:**
- Consumes: favorites (Task 9), `PlayerEvent::TrackChanged`.
- Produces:
  - Schema: `track_stats(track_id TEXT PRIMARY KEY, play_count INTEGER NOT NULL DEFAULT 0, last_played INTEGER)` and `idx_tracks_genre ON tracks (genre COLLATE NOCASE)`. `TRACK_ID_REFS` gains `track_stats.track_id`.
  - `history::record_play(conn, track_id: &str, at_ms: i64)`, `history::spawn_recorder(db: Arc<Db>) -> EventSink` (a thread fed by a channel; records on `TrackChanged`; added to `fan_out` in `lib.rs`).
  - `#[serde(tag = "kind", rename_all = "lowercase")] pub enum FlowVariant { Favorites, Library, Genre { genre: String } }` in `flow/mod.rs`.
  - `pub struct Candidate { pub id: String, pub artist: String, pub last_played: Option<i64> }`; `flow::pool(conn, variant: &FlowVariant) -> anyhow::Result<Vec<Candidate>>`; `pub const PAD_TARGET: usize = 100;`.
  - `pub struct Deck`, `Deck::new(pool: Vec<Candidate>, now_ms: i64, rng: rand::rngs::StdRng)`, `next_batch(&mut self, n: usize) -> Vec<String>`, `is_empty()`.
- Rules (spec 5.8):
  - Favorites pool = favorite tracks + every track of favorite albums + every track whose `artist` or `album_artist` is a favorite artist, without duplicates. Library pool = all tracks. Genre pool = tracks whose genre matches case-insensitively.
  - A pool under `PAD_TARGET` is padded from the library: first tracks by the pool's artists, then tracks of the pool's genres, until `PAD_TARGET` or the library runs out.
  - No repeats until the whole pool was dealt; then a new cycle starts, whose first track is not by the last dealt artist when avoidable.
  - Weighting toward less recently played: each cycle orders the pool by the key below, then spaces artists.

```
weight(c) = 1 + min(days_since(c.last_played), 60)    // never played: 61
key(c)    = u ^ (1 / weight(c)),  u uniform in (0, 1)  // sort by key, descending
spacing:  take the first remaining candidate whose artist != previous artist;
          if none, take the first remaining one
```

- [ ] **Step 1: Write the failing tests** (`cargo test flow:: history::`, seeded `StdRng::seed_from_u64(7)`): `no_repeats_until_pool_used` (pool of 30, three batches of 10 hold 30 distinct ids; a fourth batch starts a new cycle), `no_adjacent_same_artist_when_avoidable` (10 tracks by A, 10 by B: no two neighbors share an artist), `single_artist_pool_still_plays` (5 tracks by one artist: a batch of 5 returns all 5 and the call ends), `least_recently_played_tend_first` (50 never played, 50 played today: at least 35 of the first 50 dealt are never-played), `favorite_album_contributes_all_tracks`, `favorite_artist_contributes_all_tracks`, `small_pool_is_padded` (library of 150 tracks over 5 artists, one favorite track: pool has 100, the first padded ones share the favorite's artist), `genre_pool_only_that_genre`, `history_records_track_changes` (the recorder sink gets two `TrackChanged`: `play_count == 2`, `last_played` set).
- [ ] **Step 2: Run** -> FAIL. **Step 3: Implement.** **Step 4: Run** -> PASS.
- [ ] **Step 5: Commit** `feat: play history and the Flow deck`.

### Task 12: Flow service, ring idle mode and variants menu

**Files:**
- Modify: `src-tauri/src/flow/mod.rs`, `src-tauri/src/player/engine.rs`, `src-tauri/src/commands.rs`, `src-tauri/src/lib.rs`, `src-tauri/src/library/query.rs`, `src/lib/{types.ts, ipc.ts, flow.ts, flow.spec.ts, i18n.ts}`, `src/App.svelte`
- Create: `src/lib/components/FlowMenu.svelte`

**Interfaces:**
- Consumes: `Deck`, `flow::pool`, `PlayerHandle` (Task 7).
- Produces:
  - Engine: `PlayerHandle::play_with(ids, start, flow: Option<FlowVariant>)`; `PlayerState.flow: Option<FlowVariant>` (`flow` in TS, `null` outside Flow). `play` (no Flow) clears it; `append` keeps it.
  - `pub struct FlowService` with `FlowService::spawn(db: Arc<Db>, player: PlayerHandle) -> (Arc<FlowService>, EventSink)`; `start(&self, variant: FlowVariant) -> anyhow::Result<FlowOutcome>`. It deals a first batch of `FLOW_BATCH = 50`, and refills with another 50 when `queue_length - queue_index <= FLOW_REFILL_AT (10)` while `state.flow` is set. A state without `flow` drops the session. The refill runs on the service thread, never on the engine thread.
  - `#[serde(tag = "outcome", rename_all = "lowercase")] pub enum FlowOutcome { Started { variant: FlowVariant }, Empty }`. Favorites with an empty pool fall back to `Library` (reported in `variant`); an empty library gives `Empty`.
  - Commands `flow_start(variant: FlowVariant) -> FlowOutcome`, `library_genres() -> Vec<String>` (distinct non-empty genres, NOCASE order). TS `flowStart`, `libraryGenres`.
  - `FlowMenu.svelte` props `{ x: number; y: number; onclose: () => void }`: "Flow from favorites", "Flow from the whole library", "Flow from a genre" (a scrollable genre list with type-to-filter). Opened by right-click on the ring and by a 500 ms press. Used by the main window and (Task 20) the mini player.
- Rules: an idle ring click calls `flowStart({ kind: 'favorites' })`; the fallback shows the notice `flow.noFavorites` ("No favorites yet, so Flow plays your whole library."); `Empty` goes to Settings as in M1. `playAllShuffled` and `FLOW_CAP` are deleted; `shuffle` stays in `flow.ts`.

- [ ] **Step 1: Write the failing tests:** Rust `flow_refills_near_the_end` (fake output at high speed, pool of 120: the queue grows past 50 without a gap, no id repeats within the first 120), `playing_an_album_ends_the_flow_session` (after `play`, no more appends), `empty_favorites_fall_back_to_library`, `empty_library_is_empty`, `genres_are_distinct_and_sorted`; `flow.spec.ts`: an idle activation calls `flow_start` with `{kind:'favorites'}` and shows the notice when the outcome's variant is `library`.
- [ ] **Step 2: Run** `cargo test --manifest-path src-tauri/Cargo.toml flow::` and `npm test` -> FAIL. **Step 3: Implement.** **Step 4: Run** -> PASS. Manual: click the idle ring; right-click it and start a genre Flow.
- [ ] **Step 5: Commit** `feat: Flow - endless mixes from favorites, library or a genre`.

### Task 13: Crossfade

**Files:**
- Create: `src-tauri/src/audio/mix.rs`
- Modify: `src-tauri/src/audio/mod.rs`, `src-tauri/src/audio/readahead.rs`, `src-tauri/src/player/engine.rs`, `src-tauri/src/commands.rs`

**Interfaces:**
- Consumes: `ReadAhead::exhausted/buffered_frames/set_target_ms` (Task 5), `Crossfade` setting and `Change::Crossfade` (Task 8), `PlayerState.flow` (Task 12).
- Produces:
  - `mix::equal_power(t: f32) -> (f32, f32)` (`(cos(t*pi/2), sin(t*pi/2))`, `t` in 0..=1).
  - `pub struct NextOptions { pub gain: f32, pub crossfade_ms: u32 }`; `Playback::queue_next_with(source, NextOptions)` (`queue_next` = `crossfade_ms: 0`).
  - `pub fn crossfade_for(mode: Crossfade, ms: u32, in_flow: bool, current: &Track, next: &Track) -> u32` in `engine.rs`: `Off` -> 0; `Flow` -> `ms` only in Flow; `Always` -> `ms`, except 0 when `next` is the following track of the same album (`album_id` equal and `track_no == current.track_no + 1`), so albums stay gapless.
- Rules: the crossfade starts when the current source is exhausted and its buffered frames are at most the crossfade length; the effective length is `min(crossfade_ms, remaining)`. While a crossfade is configured, the read-ahead target is `max(READ_AHEAD_MS, crossfade_ms + 1000)` and the preload lead is `max(PRELOAD_MS, crossfade_ms + 3000)`. Both sources are resampled to the sink spec separately and mixed with `equal_power`; each keeps its ReplayGain factor. `Advanced` is reported when the fade starts. Manual next / prev / jump cut without a fade. Grouped minor: a ReplayGain mode change ramps over 20 ms (reuse the ramp from `ring.rs`).

- [ ] **Step 1: Write the failing tests:** `equal_power_is_constant_power` (`a^2 + b^2 == 1` within 1e-6 at 11 points); `crossfade_overlaps_tracks` (two DC fake sources, 1.0 for 3 s and 0.5 for 3 s at 48 kHz, crossfade 1000 ms: total output frames = 5 s +- 1 block; samples inside the window move from 1.0 toward 0.5 with no sample above 1.0); `short_track_shortens_the_fade` (a 0.5 s first track with a 4 s crossfade: no panic, output length = sum - 0.5 s); `crossfade_zero_is_gapless` (existing gapless assertion); `crossfade_for_rules` (table test over the three modes, in/out of Flow, same-album consecutive); `manual_next_does_not_fade`.
- [ ] **Step 2: Run** `cargo test --manifest-path src-tauri/Cargo.toml audio:: player::` -> FAIL. **Step 3: Implement.** **Step 4: Run** the full `cargo test` -> PASS.
- [ ] **Step 5: Commit** `feat: crossfade, on by default inside Flow`.

### Task 14: Prism playlists backend

**Files:**
- Create: `src-tauri/src/library/playlists.rs`, `src-tauri/src/db/migrations/0006_playlists.sql`
- Modify: `src-tauri/src/db/{mod.rs, rekey.rs}`, `src-tauri/src/commands.rs`, `src-tauri/src/lib.rs`, `src/lib/{types.ts, ipc.ts}`

**Interfaces:**
- Produces:
  - Schema: `playlists(id INTEGER PRIMARY KEY, name TEXT NOT NULL, created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL)`, `playlist_tracks(playlist_id INTEGER NOT NULL REFERENCES playlists(id) ON DELETE CASCADE, position INTEGER NOT NULL, track_id TEXT NOT NULL, PRIMARY KEY (playlist_id, position))`, `idx_playlist_tracks_track ON playlist_tracks (track_id)`. `TRACK_ID_REFS` gains `playlist_tracks.track_id`. (`foreign_keys = ON` from M1 is now used.)
  - `Playlist { id: i64, name: String, trackCount: u32, durationMs: u64, coverIds: Vec<String> /* up to 4 distinct, for a mosaic */, updatedAt: i64 }`, `PlaylistEntry { position: u32, trackId: String, track: Option<Track> }`.
  - Functions and commands of the same names: `playlists_list() -> Vec<Playlist>` (by `updated_at` desc), `playlist_create(name) -> Playlist`, `playlist_rename(id, name)`, `playlist_delete(id)`, `playlist_entries(id) -> Vec<PlaylistEntry>`, `playlist_add(id, trackIds: Vec<String>)` (appends; duplicates allowed), `playlist_remove(id, positions: Vec<u32>)` (renumbers 0..n), `playlist_move(id, from: u32, to: u32)`. Event `playlists://changed`. TS wrappers in camelCase (`playlistsList`, `playlistCreate`, ...).
- Rules: names are trimmed, 1..=200 characters, duplicates allowed. Entries whose track is gone stay with `track: None` (shown dimmed, skipped when playing). Track ids from any source (`local:`, later `navidrome:`) are accepted.

- [ ] **Step 1: Write the failing tests:** `create_add_and_list` (counts, duration, cover mosaic ids), `remove_renumbers`, `move_entry_up_and_down`, `delete_cascades_entries`, `blank_name_is_refused`, `missing_track_entry_is_kept`, `rekey_updates_playlist_entries`.
- [ ] **Step 2: Run** `cargo test --manifest-path src-tauri/Cargo.toml playlists` -> FAIL. **Step 3: Implement.** **Step 4: Run** -> PASS.
- [ ] **Step 5: Commit** `feat: Prism playlists backend`.

### Task 15: Playlists UI and the track context menu

**Files:**
- Create: `src/lib/stores/playlists.svelte.ts`, `src/lib/components/{PlaylistsView, PlaylistView, TrackMenu}.svelte`, `src/lib/stores/playlists.spec.ts`
- Modify: `src/lib/stores/nav.svelte.ts` (`'playlist'` view with the id as `param`), `src/App.svelte`, `src/lib/components/{Sidebar, SongList, AlbumView, FavoritesView}.svelte`, `src/lib/play.ts`, `src/lib/i18n.ts`

**Interfaces:**
- Consumes: playlist commands (Task 14), `setFavorite` (Task 10).
- Produces: `playlists` store (`list: Playlist[] | null`), `initPlaylists()`; `playPlaylist(id, fromPosition?)` in `play.ts`; `TrackMenu.svelte` props `{ trackIds: string[]; x: number; y: number; albumId?: string; artist?: string; onclose: () => void }` with "Add to playlist" (the list plus "New playlist..."), "Add to favorites" / "Remove from favorites" (single track), "Go to album", "Go to artist". Opened by right-click on song rows (SongList, AlbumView, FavoritesView, PlaylistView) and on album cards ("Add album to playlist").
- Rules: PlaylistView has an editable title (Enter saves, Esc cancels), play and shuffle, drag-to-reorder rows (keyboard: Alt+Up / Alt+Down), remove via Delete or the menu, and dimmed missing entries. Grouped minors: SongList retries a failed page when it is next in view; the album backdrop covers the header at any width (`background-size: cover` on a square source).

- [ ] **Step 1: Write the failing tests:** `playlists.spec.ts`: `playPlaylist` skips entries without a track and starts at the right index; `views.spec.ts`: `TrackMenu` lists playlists and calls `playlist_add` with the given ids; "New playlist..." creates then adds.
- [ ] **Step 2: Run** `npm test` -> FAIL. **Step 3: Implement.** **Step 4: Run** `npm test`, `npm run check` -> PASS. Manual: build a playlist from three albums, reorder, rename, play.
- [ ] **Step 5: Commit** `feat: playlists view and track menu`.

### Task 16: Now Playing view

**Files:**
- Create: `src/lib/components/NowPlaying.svelte`
- Modify: `src/lib/stores/nav.svelte.ts` (`'nowplaying'`), `src/App.svelte`, `src/lib/components/TransportStrip.svelte`, `src/lib/format.ts` + `format.spec.ts`, `src/lib/theme.css`, `src/lib/i18n.ts`

**Interfaces:**
- Consumes: `playerQueueRange`, `playerJump`, `queueVersion` (Task 7), `visibleRange`.
- Produces: `sourceQualityLabel(track: Track, sourceName: (s: Source) => string): string` in `format.ts` ("Local files · FLAC 24/96"; without quality just the source name).
- Rules: opened by clicking the cover or title in the transport strip (and from the mini player, Task 20). Big 600 px cover with the blurred backdrop of AlbumView, title, artist and album as links, the source + quality line, a heart, and the queue as a virtualized list (rows of 52 px, pages of 200 via `player_queue_range`, refetched when `queueVersion` changes). The current entry is highlighted and scrolled into view on track change unless the user scrolled in the last 5 s; a click jumps there. In Flow the header shows "Flow · <variant>". Grouped minor: `theme.css` scrollbars use either `scrollbar-width` or the `::-webkit-scrollbar` rules, not both (the hover style works again).

- [ ] **Step 1: Write the failing tests:** `format.spec.ts`: `sourceQualityLabel` for flac 24/96, mp3 320, and a track without quality; `views.spec.ts`: NowPlaying requests the page containing `queueIndex` and calls `player_jump(5)` when row 5 is clicked.
- [ ] **Step 2: Run** `npm test` -> FAIL. **Step 3: Implement.** **Step 4: Run** -> PASS.
- [ ] **Step 5: Commit** `feat: Now Playing view with the queue`.

### Task 17: Cover-colored ring and loudness glow

**Files:**
- Create: `src/lib/palette.ts` + `src/lib/palette.spec.ts`, `src/lib/stores/ring.svelte.ts`
- Modify: `src-tauri/src/audio/{mix.rs, mod.rs}`, `src-tauri/src/player/{engine.rs, mod.rs}`, `src/lib/components/{FlowRing, Cover}.svelte`, `src/lib/ipc.ts`, `src/App.svelte`

**Interfaces:**
- Produces:
  - Rust: `mix::LevelMeter` (`push(&mut self, samples: &[f32])`, `level(&self) -> f32` in 0..=1 = `clamp((20*log10(rms) + 50) / 50, 0, 1)`, smoothed with 150 ms release); `PlayerEvent::Level(f32)` sent every 50 ms while playing; `tauri_sink` emits it as `player://level` (payload: number); the media bridge and the history and Flow sinks ignore it.
  - TS: `paletteFromPixels(data: Uint8ClampedArray): RingColors | null`, `ringColorsFor(coverId: string | null): Promise<RingColors>` (draws the 96 px cover into a canvas; cached per id; `SPECTRUM` when null); `ring` store `{ colors: RingColors; level: number }`, `initRing()`; `onPlayerLevel(cb)`. `FlowRing` gains `level?: number` (glow opacity 0.35..0.75 and scale 1..1.12, 120 ms transitions); `--c1..--c4` are registered with `@property` so a color change fades over 600 ms.
- Rules (palette): skip pixels with saturation < 0.2 or lightness outside 0.12..0.92; bucket hues into 12 bins weighted by saturation; take up to 4 strongest bins (fill missing ones by rotating the strongest hue by +-40 degrees); order by hue starting at the strongest; output at saturation 0.9, lightness 0.64. Fewer than 3 % chromatic pixels -> `null`. Grouped minor: `Cover.svelte` resets on id change with `{#key}` instead of `$effect.pre`.

- [ ] **Step 1: Write the failing tests:** Rust `level_tracks_rms` (sine of amplitude 0.5 -> level within 0.02 of `(20*log10(0.3536)+50)/50`; silence -> 0); `palette.spec.ts`: a half red / half blue image gives 4 colors including one red-ish (hue within 20 degrees of 350) and one blue-ish; a grayscale image gives `null`; output strings are `#rrggbb`.
- [ ] **Step 2: Run** `cargo test --manifest-path src-tauri/Cargo.toml mix::` and `npm test -- palette` -> FAIL. **Step 3: Implement.** **Step 4: Run** -> PASS. Manual: the ring takes the cover's colors and the glow breathes subtly with the music.
- [ ] **Step 5: Commit** `feat: ring colors from the cover, glow from the loudness`.

### Task 18: Online cover lookup (MusicBrainz + Cover Art Archive)

**Files:**
- Create: `src-tauri/src/library/lookup.rs`, `src-tauri/src/db/migrations/0007_cover_lookups.sql`
- Modify: `src-tauri/src/library/{scan.rs (rebuild_albums), query.rs (TRACK_COLUMNS), mod.rs}`, `src-tauri/src/shell/mod.rs` (`apply_setting`), `src-tauri/src/lib.rs`, `src-tauri/Cargo.toml` (`reqwest`, blocking + rustls + json)

**Interfaces:**
- Consumes: `CoverStore::store_bytes`, `Settings.coverLookup`, `Change::CoverLookup`.
- Produces:
  - Schema: `cover_lookups(album_id TEXT PRIMARY KEY, status TEXT NOT NULL /* found | notfound */, cover_id TEXT, checked_at INTEGER NOT NULL)`.
  - `pub trait CoverFetcher: Send + Sync { fn fetch(&self, artist: &str, album: &str) -> anyhow::Result<Option<Vec<u8>>>; }`; `pub struct MusicBrainz` (rate limit 1 request/s across calls, UA from Global Constraints, 10 s timeouts) implementing it via `https://musicbrainz.org/ws/2/release/?query=<q>&fmt=json&limit=5` then `https://coverartarchive.org/release/<mbid>/front-500`.
  - Pure helpers: `mb_query(artist, album) -> String` (Lucene-escaped `release:"..." AND artist:"..."`), `pick_release(json: &serde_json::Value) -> Option<String>` (first release with `score >= 90`).
  - `pub struct LookupWorker` with `LookupWorker::spawn(db, covers, fetcher: Arc<dyn CoverFetcher>, enabled: Arc<AtomicBool>, emit) -> LookupWorker` and `kick(&self)`; it is kicked after the startup rescan, after every `library://changed`, and when the setting turns on.
- Rules: candidates are albums with `cover_id IS NULL`, not titled "Unknown album" and not by "Unknown artist", with no `found` row and no `notfound` row younger than 30 days. Network errors are not recorded (retried on the next kick). The flag is checked before each album, so switching off stops within one request. `rebuild_albums` uses the lookup cover when no track has one; `TRACK_COLUMNS` returns `COALESCE(t.cover_id, (SELECT cover_id FROM cover_lookups l WHERE l.album_id = t.album_id))`, so the transport strip, SMTC and Now Playing show it too. `library://changed` is emitted after each found cover (debounced to once per 2 s).

- [ ] **Step 1: Write the failing tests** with a `FakeFetcher`: `found_cover_sets_album_and_tracks`, `not_found_waits_30_days` (a `notfound` row 10 days old is skipped, 31 days old is retried), `network_error_is_retried_next_kick`, `disabled_fetches_nothing`, `unknown_album_is_skipped`, `rescan_keeps_lookup_cover`, `mb_query_escapes_quotes`, `pick_release_requires_score_90`. One `#[ignore]` live test `live_lookup_finds_a_known_album` (a well-known public album, not from the owner's library).
- [ ] **Step 2: Run** `cargo test --manifest-path src-tauri/Cargo.toml lookup` -> FAIL. **Step 3: Implement.** **Step 4: Run** -> PASS.
- [ ] **Step 5: Commit** `feat: online cover lookup via MusicBrainz and the Cover Art Archive`.

### Task 19: Main window shape: click-through strip, no strip when maximized

**Files:**
- Create: `src/lib/window-shape.json`, `src-tauri/src/shell/shape.rs`, `src/lib/stores/window.svelte.ts` + `window.spec.ts`
- Modify: `src-tauri/src/shell/mod.rs`, `src-tauri/src/window.rs`, `src-tauri/src/lib.rs`, `src-tauri/Cargo.toml` (remove `window-vibrancy`; `windows-sys` features `Win32_Graphics_Gdi`), `src/App.svelte`, `src/lib/components/{TitleBar, TransportStrip}.svelte`

**Interfaces:**
- Produces:
  - `window-shape.json`: `{ "main": { "strip": 96, "ring": 96, "ringRoom": 48, "corner": 16, "glow": 22, "ringMaximized": 72 }, "mini": { "padLeft": 66, "padY": 22, "width": 310, "height": 92, "ring": 76, "corner": 18, "glow": 22 } }`. CSS reads it through `App.svelte` style props; Rust reads it with `include_str!` (one source of truth).
  - `shape.rs`: plain `RectF { x, y, w, h }` and `Circle { cx, cy, r }` (f64), `pub struct Region { pub card: RectF, pub corner: f64, pub ring: Circle }` (logical px), `pub fn main_region(width: f64, height: f64) -> Region` (card = `0,0,width,height - strip`; ring centre `(width/2, height - strip)`; radius `ring/2 + 6 + glow`), `pub fn mini_region(..)`, `Region::contains(&self, x: f64, y: f64) -> bool`, `Region::to_physical(&self, scale: f64) -> Region`.
  - `pub fn apply(window: &WebviewWindow, region: Option<&Region>)`: Windows `SetWindowRgn` with `CreateRoundRectRgn` OR `CreateEllipticRgn` (`CombineRgn(RGN_OR)`), `None` removes the region. macOS: a per-window tracker thread polls `app.cursor_position()` every 30 ms while the window is visible and calls `set_ignore_cursor_events(!region.contains(..))`; it stops when the window hides.
  - Rust re-applies on `WindowEvent::Resized`, `ScaleFactorChanged` and at setup; a maximized or fullscreen main window gets `None`.
  - `window` store `{ maximized: boolean }` (moved out of `TitleBar`), and `App.svelte` drops the strip when maximized: no bottom padding, ring `ringMaximized` px centred in the transport strip.
- Grouped minors: the `window-vibrancy` dependency is removed (the `window_vibrancy` command stays and returns false); `TitleBar` reads the shared store.

- [ ] **Step 1: Write the failing tests:** `shape.rs`: `card_and_ring_are_inside` (a point in the card, a point in the ring's lower half below the card), `strip_corners_are_outside` (`(5, height - 5)` is outside), `region_scales_with_dpi` (`to_physical(1.5)` multiplies every coordinate; containment of a scaled point matches), `shared_json_matches_constants` (parses the JSON); `window.spec.ts`: the store follows `onResized` + `isMaximized`.
- [ ] **Step 2: Run** `cargo test --manifest-path src-tauri/Cargo.toml shape::` and `npm test` -> FAIL. **Step 3: Implement.** **Step 4: Run** -> PASS. Manual (Windows): a click on the desktop through the strip beside the ring reaches the desktop; the ring and its glow are not clipped; maximize: no strip, the ring sits inside the transport strip; restore: the strip returns.
- [ ] **Step 5: Commit** `feat: click-through strip below the card, no strip when maximized`.

### Task 20: Mini player window

**Files:**
- Create: `src-tauri/src/shell/mini.rs`, `src/lib/components/MiniPlayer.svelte`, `src/lib/components/mini.spec.ts`
- Modify: `src/main.ts`, `src-tauri/src/{lib.rs, shell/mod.rs, commands.rs}`, `src-tauri/capabilities/default.json`, `src-tauri/tauri.conf.json`, `src/lib/{ipc.ts, i18n.ts}`

**Interfaces:**
- Consumes: shape (Task 19), favorites, ring, FlowMenu, player store, `Settings.miniOnTop`.
- Produces:
  - `pub const MINI_LABEL: &str = "mini";` `mini::create(app: &AppHandle, on_top: bool) -> tauri::Result<WebviewWindow>` at setup: hidden, `transparent(true)`, `decorations(false)`, `shadow(false)`, `resizable(false)`, `skip_taskbar(true)`, `focused(false)`, `focusable(false)`, `always_on_top(on_top)`, size from `window-shape.json` (`padLeft + width + 16` by `height + 2 * padY`), same URL as main.
  - `pub enum Anchor { WorkAreaBottomRight, BelowRect(tauri::PhysicalRect<i32, u32>) }`; `pub fn dock_position(work_area: RectI, size: SizeI, anchor: &Anchor, margin: i32) -> PosI` (pure; clamps into the work area; `RectI { x, y, w, h }`, `SizeI { w, h }`, `PosI { x, y }` are plain i32 physical-pixel structs in `mini.rs`); `mini::show(app, anchor)`, `mini::hide(app)`, `mini::toggle(app, anchor)`, `mini::is_visible(app) -> bool`.
  - Showing never activates: Windows `ShowWindow(hwnd, SW_SHOWNOACTIVATE)`; macOS `orderFrontRegardless` on the `NSWindow` (objc2, same version as Tauri's); the work area is `Monitor::work_area()` of the primary monitor on Windows and of the tray icon's monitor on macOS.
  - Commands `mini_toggle()`, `main_show()` (shows, unminimizes and focuses the main window; Task 21 extends it). `apply_setting` handles `MiniOnTop` (`set_always_on_top`).
  - `main.ts` mounts `MiniPlayer` when `getCurrentWindow().label === 'mini'`, else `App`.
  - `MiniPlayer.svelte` per `mini-v2.html`: card 310 px, cover 46 px, title, "artist · album", source dot + quality, heart, previous, next, volume, an "Open Prism" button (`main_show`), and the 76 px `FlowRing` half out of the left edge (with `FlowMenu`, colors and glow). Clicking the cover opens Now Playing in the main window (`main_show` + a `nav://go` event the main window follows).
- Grouped minors: capabilities list both windows and drop unused permissions (`set-title`, `show`, `set-focus` when unused); the production CSP drops `ws://localhost:1441`, which moves to `app.security.devCsp`.

- [ ] **Step 1: Write the failing tests:** Rust `docks_inside_work_area` (work area `0,0,1920,1032` (taskbar at the bottom), size `392x136`, margin 12 -> `(1516, 884)`; a taskbar on the left (`work area x = 48`) still docks bottom-right; `BelowRect` centres under the rect and clamps at the right edge), `mini_window_is_unfocusable_and_hidden` (builder config helper `mini::builder_config(on_top) -> MiniConfig` asserts `focusable == false`, `visible == false`, `skip_taskbar == true`); `mini.spec.ts`: renders title, artist, the source + quality line, and the open button calls `main_show`.
- [ ] **Step 2: Run** `cargo test --manifest-path src-tauri/Cargo.toml mini::` and `npm test` -> FAIL. **Step 3: Implement.** **Step 4: Run** -> PASS. Manual: from a typing editor, `mini_toggle` (temporary dev button) shows the mini player and the editor keeps the caret and keystrokes; clicking prev/next in the mini player does not take focus.
- [ ] **Step 5: Commit** `feat: mini player window`.

### Task 21: Tray, close-to-mini, taskbar button, macOS menu bar

**Files:**
- Create: `src-tauri/src/shell/tray.rs`, `src-tauri/src/shell/taskbar_win.rs`
- Modify: `src-tauri/Cargo.toml` (`tauri` feature `tray-icon`; `windows = { version = "0.62", features = ["Win32_UI_Shell", "Win32_UI_WindowsAndMessaging", "Win32_System_Com", "Win32_Foundation"] }` under `cfg(windows)`; `windows-sys` feature `Win32_UI_Shell`), `src-tauri/src/{lib.rs, shell/mod.rs, shell/mini.rs, commands.rs}`, `scripts/make-icons.mjs` (tray template PNG and four 16/32 px thumb-bar ICOs), `src/lib/{ipc.ts, i18n.ts}`, `src/App.svelte`

**Interfaces:**
- Consumes: `mini::toggle/show/hide`, `PlayerHandle`, `SettingsStore::flush`.
- Produces:
  - `pub struct ShellState { pub main_closed: AtomicBool, pub last_tray_rect: Mutex<Option<PhysicalRect<i32, u32>>> }` (managed).
  - Pure policy functions in `shell/mod.rs`: `pub enum CloseAction { MinimizeAndMarkClosed, Hide }`, `pub fn on_main_close(windows: bool) -> CloseAction`; `pub enum RestoreAction { RestoreMain, ToggleMini }`, `pub fn on_taskbar_restore(main_closed: bool) -> RestoreAction`.
  - Main window close (`WindowEvent::CloseRequested`, `prevent_close`): Windows minimizes the main window and sets `main_closed`, so the app keeps its single taskbar button; macOS hides the main window. `taskbar_win.rs` subclasses the main HWND (`SetWindowSubclass`): `WM_SYSCOMMAND` + `SC_RESTORE` while `main_closed` is swallowed and toggles the mini player at `Anchor::WorkAreaBottomRight`; this is how a taskbar click toggles the mini player. `main_show` clears `main_closed` and hides the mini player.
  - Thumbnail toolbar (Windows, `ITaskbarList3::ThumbBarAddButtons` after the `TaskbarButtonCreated` message): previous, play/pause, next; `WM_COMMAND`/`THBN_CLICKED` maps to the player; the play/pause icon follows `player://state` via `ThumbBarUpdateButtons`.
  - Tray (`TrayIconBuilder`, `show_menu_on_left_click(false)`): left click toggles the mini player (macOS: `Anchor::BelowRect(rect)` from the click event, remembered in `last_tray_rect`); the menu has Open Prism, Play/Pause, Next, Previous, Quit Prism. macOS uses a template icon in the menu bar. `RunEvent::Reopen` (Dock click) calls `main_show`.
  - Command `tray_set_labels(labels: TrayLabels)` with `TrayLabels { open, playPause, next, previous, quit }`, sent by the main window whenever the language resolves or changes, so the strings live in `i18n.ts`.
  - Quit (tray menu) calls `app.exit(0)`; `RunEvent::Exit` flushes the settings (Task 8).
- Rules: nothing here shows the mini player on its own; only the tray click, the taskbar click (while closed) and Task 22's shortcut do.

- [ ] **Step 1: Write the failing tests:** `close_policy_per_platform` (`on_main_close(true) == MinimizeAndMarkClosed`, `on_main_close(false) == Hide`), `taskbar_restore_toggles_only_when_closed`, `tray_labels_default_to_english` (Rust fallback labels before the frontend sends any), and `i18n.spec.ts` keeps passing with the new `tray.*` keys.
- [ ] **Step 2: Run** `cargo test --manifest-path src-tauri/Cargo.toml shell::` -> FAIL. **Step 3: Implement.** **Step 4: Run** -> PASS. Manual (Windows): close the main window: it disappears, one Prism taskbar button remains; click it: the mini player toggles bottom-right without stealing focus from the editor; tray left-click toggles it too; "Open Prism" in the mini player restores the main window; the main window's taskbar thumbnail shows working prev/play/next; Quit exits and keeps the volume.
- [ ] **Step 5: Commit** `feat: tray, close to mini player, taskbar toggle and thumbnail buttons`.

### Task 22: Global shortcut, start with system, remaining settings UI

**Files:**
- Create: `src-tauri/src/shell/shortcut.rs`, `src/lib/components/ShortcutInput.svelte` + `ShortcutInput.spec.ts`
- Modify: `src-tauri/Cargo.toml` (`tauri-plugin-global-shortcut = "2"`, `tauri-plugin-autostart = "2"`), `package.json` (the matching JS packages only if the frontend imports them; it does not need to), `src-tauri/src/{lib.rs, shell/mod.rs, commands.rs}`, `src-tauri/tauri.conf.json` (main window `visible: false`, shown in setup), `src-tauri/capabilities/default.json`, `src/lib/components/{Settings, Select}.svelte`, `src/lib/components/settings.spec.ts`, `src/lib/i18n.ts`

**Interfaces:**
- Consumes: `Settings.miniShortcut`, `Settings.startWithSystem`, `Settings.crossfade*`, `Settings.coverLookup`, `Settings.miniOnTop`, `mini::toggle`, `ShellState.last_tray_rect`.
- Produces:
  - `shortcut::apply(app: &AppHandle, accelerator: &str) -> Result<(), String>` (unregisters the previous one, parses with the plugin's `Shortcut::from_str`, registers; on failure the previous shortcut stays registered). Its handler toggles the mini player (Windows: `WorkAreaBottomRight`; macOS: below the last tray rect, else top-right of the work area). Applied at setup from the settings.
  - `settings_set` with a `miniShortcut` in the patch registers first and only saves when that worked; the error text (invalid or taken) reaches the UI.
  - Autostart: `tauri_plugin_autostart::init(MacosLauncher::LaunchAgent, Some(vec!["--autostart"]))`; `apply_setting(StartWithSystem)` enables or disables it. Started with `--autostart`, the main window stays hidden and `main_closed` is set (tray only; nothing pops up). Otherwise setup shows the main window after the first frame is themed.
  - `ShortcutInput.svelte` props `{ value: string; onchange: (accelerator: string) => void }`: records a key combination (needs at least one of Ctrl/Alt/Cmd plus a non-modifier key), shows it in platform words (Ctrl+Alt+P, Cmd+Option+P), Esc cancels, Backspace resets to the default.
  - Settings sections (spec 5.12): Playback gains crossfade mode (Off / In Flow / Always) and length (1-8 s slider); "Mini player" (keep on top, shortcut); "Covers" (online lookup, with the note that it asks MusicBrainz and the Cover Art Archive); "System" (start with system). Navidrome, streaming quality, radio location and updates stay out (M3, M4, M6).
- Grouped minors: `Select.svelte` sets `aria-selected` on the active option, Home/End open and move, and type-ahead matches multi-character prefixes typed within 500 ms.

- [ ] **Step 1: Write the failing tests:** Rust `shortcut_rejects_invalid_and_keeps_previous` (via a `trait ShortcutRegistry` fake), `autostart_flag_detected` (`is_autostart(args)`); `ShortcutInput.spec.ts`: Ctrl+Alt+P -> `"CommandOrControl+Alt+P"`, a lone modifier does nothing, Esc restores; `settings.spec.ts`: changing crossfade calls `settings_set({ crossfade: 'always' })`; Select: Home/End and "ro" type-ahead.
- [ ] **Step 2: Run** `cargo test --manifest-path src-tauri/Cargo.toml shortcut` and `npm test` -> FAIL. **Step 3: Implement.** **Step 4: Run** -> PASS. Manual: Ctrl+Alt+P toggles the mini player from inside the editor without moving focus; change it to Ctrl+Alt+M; enable start with system, sign out and in: only the tray icon appears.
- [ ] **Step 5: Commit** `feat: global shortcut, start with system, remaining settings`.

### Task 23: Single instance and "Open with"

**Files:**
- Create: `src-tauri/src/shell/open_with.rs`, `src-tauri/windows/hooks.nsh`, `src-tauri/Info.plist`
- Modify: `src-tauri/Cargo.toml` (`tauri-plugin-single-instance = "2"`), `src-tauri/src/{lib.rs, commands.rs}`, `src-tauri/src/player/dropped.rs`, `src-tauri/tauri.conf.json` (`bundle.windows.nsis.installerHooks: "windows/hooks.nsh"`)

**Interfaces:**
- Consumes: `DroppedFiles`, `PlayerHandle`, `mini::show`, `AUDIO_EXTENSIONS`.
- Produces:
  - `open_with::paths_from_args(args: &[String], cwd: &Path) -> Vec<String>` (skips `args[0]` and `--flags`, resolves relative paths against `cwd`, keeps only `is_audio_file`; no file system access, so a slow share cannot hang it), `open_with::paths_from_urls(urls: &[tauri::Url]) -> Vec<String>` (macOS `file://` URLs).
  - `pub fn play_paths(app: &AppHandle, paths: Vec<String>)`: reads them on a blocking thread with `DroppedFiles::read_with(paths, ReadOptions { allow_network: true })`, plays them, and, when the main window is closed or hidden, shows the mini player without focus. `DroppedFiles::read` (drag and drop) keeps `allow_network: false`.
  - The single-instance plugin is registered first in the builder; its callback calls `play_paths` with the forwarded argv, or `main_show` when there are no files. The first instance reads `std::env::args()` in setup; macOS handles `RunEvent::Opened { urls }`.
  - `hooks.nsh`: `NSIS_HOOK_POSTINSTALL` writes, for every extension in `AUDIO_EXTENSIONS`, `HKCU\Software\Classes\.<ext>\OpenWithProgids` value `Prism.Audio`, the ProgID `HKCU\Software\Classes\Prism.Audio` (`shell\open\command` = `"$INSTDIR\${MAINBINARYNAME}.exe" "%1"`, icon), and `HKCU\Software\Classes\Applications\${MAINBINARYNAME}.exe\SupportedTypes\.<ext>`; `NSIS_HOOK_PREUNINSTALL` removes exactly those. It never writes an extension's default value, so Prism never becomes the default player on its own.
  - `Info.plist`: one `CFBundleDocumentTypes` entry, role Viewer, `LSHandlerRank` `Alternate`, the audio UTIs plus `CFBundleTypeExtensions` for the same list.
- Grouped minor: `is_network_path` also treats mapped network drive letters as network (`GetDriveTypeW == DRIVE_REMOTE` on Windows), so drag and drop does not hang on them.

- [ ] **Step 1: Write the failing tests:** `args_keep_unc_and_spaced_paths` (`["prism.exe", "--autostart", "\\\\nas\\music\\a b.flac", "rel\\x.mp3", "notes.txt"]` with cwd `C:\Users\me` -> the UNC path and `C:\Users\me\rel\x.mp3`), `file_urls_decode` (`file:///Users/me/A%20B.flac`), `hooks_cover_every_audio_extension` (`include_str!` the nsh and the plist; every `AUDIO_EXTENSIONS` entry appears in both), `hooks_never_set_a_default` (no `WriteRegStr` targets `Software\Classes\.<ext>` with an empty value name; the plist has no `Owner` or `Default` rank), `second_instance_plays_files` (calls the callback body with a fixture path against a test `Player`: the queue holds that file).
- [ ] **Step 2: Run** `cargo test --manifest-path src-tauri/Cargo.toml open_with` -> FAIL. **Step 3: Implement.** **Step 4: Run** -> PASS. Manual: `npm run tauri build`, install, right-click a FLAC in Explorer: "Open with > Prism" plays it in the running instance; the default app for .flac is unchanged.
- [ ] **Step 5: Commit** `feat: single instance and "Open with" registration`.

### Task 24: M2 hand check

- [ ] **Step 1:** Run `npm test`, `npm run check`, `cargo test --manifest-path src-tauri/Cargo.toml`, and `cargo test --release --manifest-path src-tauri/Cargo.toml -- --ignored rescan_2000`. Expected: all green. Paste the summary lines.
- [ ] **Step 2:** `npm run tauri dev` with the demo library (`scripts/make-fixtures.mjs --demo 3000`): Flow from favorites / library / a genre with crossfade, a playlist, Now Playing, the mini player via tray, taskbar and Ctrl+Alt+P while typing in an editor, the click-through strip at 100 % and 150 % scaling, maximize, a cover found online for an album without art. Take screenshots of the main window, Now Playing and the mini player (demo data only).
- [ ] **Step 3:** Ask the owner for a hand check with their own library (they open the app themselves): the M1 DB migrates (ids rekeyed, nothing rescanned as new), NAS playback without dropouts, the taskbar toggle feels right, "Open with" from Explorer. Collect feedback, fix, commit. Update the Prism memory entry. Then write the M3 plan against the real code.
