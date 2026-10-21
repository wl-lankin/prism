# M3 Navidrome - SDD ledger

Plan: docs/superpowers/plans/2026-10-21-prism-m3-navidrome.md (a19847e). Branch PRISM-3 cut from PRISM-2 at 6489c99 (M2 + up next panel, repeat, shuffle).

## Waves
A = 1, 2, 3 | B = 4, 6, 7 | C = 5, 8 | D = 9, 10, 11, 12, 13 | E = 14 | F = 15, 16, 17, 18 | G = 19 | then 20 (owner hand check)

## Owner requirements carried in
- Navidrome sits behind a reverse proxy (Tasks 2, 7, 8):
  - follow redirects
  - map proxy 502/503/504 (and connection errors) to offline, never to wrong credentials
  - don't rely on Range: fall back to timeOffset reopen, or restart + skip
  - tolerate proxy response buffering (slow first byte) within the stall timeout
  - accept gzip JSON
  - support a base path in the server URL (https://host/navidrome)
- The server has a public CA certificate, so no self-signed option is needed.
- The owner's server URL never goes in tests, fixtures, screenshots or commits.
- The queue work (repeat, shuffle, up next) landed after the plan was written. Engine tasks (8, 9, 13) must keep RepeatMode/shuffle/uid-based entries working: a Flow ignores repeat/shuffle, and an offline pause must not count toward the failure cap.

## Progress
- [x] Task 1: approved, merged cf976e4
- [x] Task 2: fixed in two rounds (stream client built once, same-host redirects, proxy 401, a body cut stays offline and only InvalidData counts as a decode error), merged 3d570aa. Combined result: cargo 433 passed, clippy clean.
- [x] Task 3: approved and merged (keyring 3.6.3 backends verified for Windows and macOS, no mock fallback)
  - Task 5 requirements from the Task 3 review:
    - Emit `settings://changed` with the returned Settings after connect/disconnect, because `set_navidrome_account` emits nothing.
    - Pass `normalize_url` output and a trimmed, non-empty user.
    - On startup with a half-set account, and on disconnect, delete any orphan keychain entry.
  - Task 15: show a clear message for keychain TooLong (Windows 513-char username cap).
- [x] Task 4: approved and merged 875a8e7. Combined Tasks 1-4: cargo 458 passed, clippy clean.
  - Binding for Task 5:
    - **Wipe vs. a running sync:** serialize the wipe (connect to another server, disconnect) with the running cycle. Run the wipes on the scheduler thread, or cancel and join the in-flight cycle first. Otherwise an old pass keeps writing old-server rows into the wiped index and stores old `last_modified`.
    - **Single caller:** `sync_library` has exactly one caller at a time, because its temp tables live on the single `Mutex<Connection>`.
    - **Refused passes:** surface `SyncError::Refused` as a visible error state, not as offline and not silently.
    - **Short-page guard (optional, cheap):** refuse the pass when the songs seen are far below the album-pass total.
  - Binding for Task 6:
    - Purge `nd-` covers (rows and files) on a server switch or disconnect, or key covers by server.
    - Filter the MusicBrainz cover-lookup candidates (`library/lookup.rs:251`) to `source = 'local'`.
    - Add `DELETE FROM cover_lookups WHERE album_id GLOB 'navidrome:*'` to the wipe.
  - Tasks 12/18: local playlist entries pointing at `navidrome:` ids are kept (user data). Show them as unavailable while no server is connected.
  - Optional test gaps: assert FTS is empty after the wipe; add an interrupted-first-sync variant.
- [x] Task 7: approved after fix round 1, merged d36abdc. Combined result: cargo 483 passed, clippy clean.
  - Notes:
    - `StreamConfig.stall` must stay below `STREAM_READ_TIMEOUT` (30 s).
    - The API client timeout is per operation, so a trickling JSON response has no total bound (minor).
  - Fix round 1 covered:
  - open() race: a body failure right after the headers lost the buffered bytes.
  - The stream client's timeout is per operation in reqwest blocking, so it is set to Some(stall + 10 s).
  - Binding for Task 8 (from the Task 7 review):
    - `is_seekable` is false for non-ranged streams. Symphonia then refuses backward seeks (ForwardOnly from FLAC, MP3 and WAV) and its trailing-metadata probe waits for the whole download. Handle backward seeks on non-seekable `original` streams in the opener itself, since every byte from 0 is held in the spill. Check whether Navidrome honors `timeOffset` for `format=raw` (likely only when transcoding).
    - Only one reader may be active per HttpStream at a time, or two distant cursors will ping-pong download restarts. This includes the FFmpeg feeder.
    - Optional: avoid the 3-request open (the trailing probe restarts the download at the tail) by fetching small tails as a separate range.
  - Binding for Task 9: a disk-full spill error and an auth-proxy 401 are plain errors that skip toward the failure cap. Consider a clearer notice (Task 15).
- Task 6 consumes NavidromeService (Task 5), so it moves after Task 5 despite being listed in wave B.
- [x] Task 5: approved after fix round 1, merged f787df6.
  - Fix round 1:
    - case-insensitive keychain target
    - server_key lowercases the user
    - online_flag set by reachability
    - a saved account starts in Syncing
    - panic guard
  - Combined Tasks 1-5 and 7: cargo 504 passed, clippy clean.
- [x] Task 6: approved and merged.
  - For Task 9 / Task 19: while the server hangs but still reads as online (up to 30 min between cycles), cover fetches and the media-controls thread can block up to 20 s.
    - Suggested fix: fail fast in ServerCovers for about 30-60 s after a network-class Unreachable, or nudge the service to ping.
    - Move the media-controls cover fetch off its thread.
  - Deferred: `immutable` max-age on nd- covers. If ever needed, add a server-epoch query param on the frontend cover URL (Task 14/16). Task 20 should confirm that covers refresh after a server switch.
  - Deferred: on Windows a purge can leave a file behind when it is held without FILE_SHARE_DELETE (logged only).
- [x] Task 8: approved after fix round 1 (StreamFailure marker; only stream errors fail the probe). Merged 8cf2a1d. Combined: cargo 546 passed, clippy clean.
- [ ] Task 9: m3-t307 / PRISM-309, fix round 1.
  - Issues being fixed:
    - flaky test (pre-open behind a failed stream)
    - resume re-announced TrackChanged
    - offline pre-open retry
    - SetDevice while held
  - Ruling: `TrackChanged` always means a new play. A resume after an offline hold does not emit it. Task 11's scrobbler must rely on this rather than on a heuristic.
  - Found: before Task 9, the engine `resolve()` refused path-less Navidrome tracks. Task 9 fixes this.
  - Hand check: a crossfade into a next track that drops before its first frame cuts the outgoing tail by up to the fade length.
- [x] Task 12: approved after fix round 1, merged 8499e68.
  - Summaries are on par with the N+1 (about 90 ms at 100x1000).
  - The cycle order is now: flush stars, library, import stars, playlists.
  - Combined: cargo 594 passed, clippy and fmt clean.
  - The m3-t312 worktree is free (detached at PRISM-3, warm).
- Task 12 fix round 1:
  - The single-query summaries were 3-4x slower than the N+1 they replaced, so they now fold from one ordered scan.
  - A failing playlist is skipped instead of halting the scheduler.
  - Merge with Task 10: keep both sides in `mod.rs` and `commands.rs`. In `cycle()`: `import_stars`, then playlists.
  - Task 19 live check: a server whose `getPlaylists` songCount/changed differ from `getPlaylist` refetches every cycle.
- Ruling, applies to every cycle step (Tasks 10, 11, 12):
  - Credential-class errors (wrongCredentials, tls, tokenAuthUnsupported, invalidUrl) and offline errors end the cycle.
  - Other non-offline API errors in a secondary step (star import, one playlist, scrobble flush) are logged by code and skipped, so they never halt the scheduler.
  - Merge note: lib.rs conflicts with Task 6. Drop the old NavidromeService::spawn block and keep Task 6's `let thumbs` and `covers::attach` lines after the flow spawn.
  - Finding: Navidrome ignores timeOffset for format=raw. Raw and length-known streams seek through the buffer; only live transcodes use timeOffset.
  - For Task 9:
    - Confirm the engine clamps seeks to duration_ms (a past-end timeOffset gives an open error).
    - Decode errors carry Offline on both decoder paths.
- [x] Task 13: approved, merged 649dbd7.
  - The lib.rs conflict was resolved so that lib.rs has exactly one `NavidromeService::spawn`.
  - Notes:
    - An offline Favorites Flow with only Navidrome favorites becomes Library at start for good. A fallback at refill keeps the Favorites label and returns once the server answers.
    - Pre-existing M2 race: jumping near the end of a very short queue can let the queue run out before the refill. Deferred under the engine timing items.
- [x] Task 10: approved after fix round 1, merged 71014db, plus 1 test fix: stream tests need `..ServiceOptions::default()`. Combined Tasks 1-8, 10, 13: cargo 577 passed, clippy and fmt clean.
- [x] Task 11: approved after fix round 1 (R1, cumulative cap, no sends after shutdown), merged 018b4bf. Combined: cargo 627 passed, clippy and fmt clean. The m3-t304 worktree is free (detached).
- [x] Task 9: approved after fix round 1. Merged d23a758.
  - Deferred minors:
    - A `seek` can still settle a hand-over that has a deferred error in its one-tick window.
    - The SetDevice-while-held guard has no test that reaches it.
- [x] Task 14: approved after fix round 1, merged bce6006. It adds a `keychainTooLong` code and a single NOT_CONNECTED constant.
- Backend complete. Combined at bce6006:
  - cargo: 645 passed, clippy and fmt clean
  - npm: 221 passed, check clean
- Wave F running in parallel:
  - Task 15: m3-t304 / PRISM-315
  - Task 16: m3-t307 / PRISM-316
  - Task 17: m3-t312 / PRISM-317
  - Task 18: m3-t318 / PRISM-318
- Merge plan: `i18n.ts`, `views.spec.ts` and `App.svelte` (Tasks 16 and 17) will need hand merges. Unify Task 18's source dot with Task 16's `SourceBadge`.
- Task 15: fix round 1 is running.
  - stale connect assign
  - relogin keeps the account
  - focus
  - plus minors
- Task 18: approved. Polish is done at 532dde8. It waits to merge after Tasks 15, 16 and 17.
  - At merge:
    - Replace the `.source-dot` in `PlaylistsView.svelte:112`/`:304` and `PlaylistView.svelte:477`/`:837` with `SourceBadge`. Keep `data-source` for the spec.
    - Move the lock SVG to `Icon.svelte`.
  - Follow-up: move `playEntries` into `play.ts`.
- Task 16: in fix round 1.
  - Fixes:
    - reduced-motion guard on the syncing dot
    - short source-hint chip that fits at 900 px
    - quieter live regions
    - badges only for non-local sources
  - Sort order verified fine: there is no source in ALBUM_ORDER. The fake artist names caused the apparent grouping. This concern is closed.
- [x] Task 15: approved after fix round 1, merged 9f64471. npm 232 passed, check clean.
- Task 17: fix round 1.
  - Ruling: the quality label follows the real stream. Navidrome transcodes unless the file is MP3 at or below the cap.
  - Fixes in this round:
    - a `loaded` flag, so hearts don't flash disabled at startup
    - German offline wording
  - At merge: on Favorites artist cards, Task 16's SourceBadge (bottom right) sits under Task 17's heart pin. Move the badge to bottom left there.
  - For Task 19: a Navidrome album without artistId gets the plain name as its artist id, so the heart goes the local path.
- M2 gap, noted for later: no UI at all for `kind: 'track'` player errors (a local file that fails is silent).
- Task 11 earlier notes:
  - R1: every Navidrome `TrackChanged` is a new play. The heuristic is removed.
  - R2 (binding on Task 9):
    - A resume after a hold of an already-announced track emits no `TrackChanged`.
    - A resume after a held-at-open track emits exactly one `TrackChanged`.
  - R3: submits without a client are dropped.
  - R4: `notSubsonic` keeps the scrobble queue and only logs.
  - Merge into PRISM-3: in `mod.rs`, keep both module lines. The `cycle()` tail is `import_stars`, then playlists, then `flush_scrobbles`, then `Ok`.
- [ ] Task 14: approved; short fix round in progress:
  - backend `keychainTooLong` code
  - unknown-code fallback
  - NOT_CONNECTED pinned
  - `trackCounts` from 2 calls
  - source-aware scroll keys
- For Task 16: bind `albumsOf`/`artistsOf` once per view via `$derived`.
- For Task 17: reconcile the wording of `navidrome.offlineNotice`, and show player errors by `kind`.
- Task 10 fix round 1:
  - Fixes:
    - strict starred2 envelope
    - a failed star import does not halt the cycle for non-account errors
    - `..ServiceOptions::default()` literal
    - wipe deletes favorites with `GLOB 'navidrome:*'`
  - Rulings:
    - The service owns the heart path.
    - Account-level errors keep the star queue. NotAuthorized and Server drop the row, per the plan.
    - Every `ServiceOptions` literal ends with `..ServiceOptions::default()`.
  - Task 17: handle the "Navidrome is not connected" heart error, or hide hearts on Navidrome items while Off.
  - Merge with Task 12: resolve the cycle() tail as `sync_library_cancellable`, then `import_stars`, then playlists if changed and current, then `Ok(report)`. Keep `flush_stars` at the top.
- [ ] Task 12: m3-t312 / PRISM-312, running
- [ ] Task 13: m3-t313 / PRISM-313, running

## State at handoff (2026-10-21)
- Tasks 1-19 are merged on PRISM-3, plus a UI integration pass.
- Full checks pass:
  - cargo: 646 passed, 11 ignored
  - clippy and fmt: clean
  - npm: 276 passed, check clean
- Task 20 (owner hand check) is open. The owner's server was down for Proxmox repairs.
- Task 19 numbers (release):

  | Check | Measured |
  |---|---|
  | First sync, 30k songs | 4.5-8 s |
  | Unchanged full pass | about 3 s |
  | Search | 1-26 ms |
  | Songs page | about 2 ms |
  | Artists | about 8 ms |
  | Playlist summaries, 100x1000 | 41-138 ms (budget 150, the tightest) |

- Task 19 fix: playlist summaries above 5000 entries read the tracks once into a map. With the default 2 MB page cache, the old row-by-row join was 450 ms+.
- Recommended follow-up: `PRAGMA cache_size = -32768` in `Db::open`. It adds margin to every query on a large library.
- Live smoke test, which the owner runs (PowerShell, repo root):
  ```powershell
  $env:PRISM_LIVE_SUBSONIC_URL = '<server URL>'
  $env:PRISM_LIVE_SUBSONIC_USER = '<user>'
  $env:PRISM_LIVE_SUBSONIC_PASSWORD = '<password>'
  cargo test --manifest-path src-tauri/Cargo.toml --lib navidrome::client::tests::live_server_smoke -- --exact --ignored --nocapture
  Remove-Item Env:PRISM_LIVE_SUBSONIC_URL, Env:PRISM_LIVE_SUBSONIC_USER, Env:PRISM_LIVE_SUBSONIC_PASSWORD
  ```
  It is read-only and prints counts only. It reports:
  - albums without artistId
  - playlist count/changed mismatches
  - what a `format=mp3&maxBitRate=320` request of a non-MP3 file really streams (this checks the quality-label rule)
- Release scale tests: `cargo test --manifest-path src-tauri/Cargo.toml --release --lib -- --include-ignored --test-threads=1 --nocapture sync_30k_synthetic search_with_30k songs_page_with_30k artists_with_30k playlist_summaries`
- Incident: a broad `smoke -- --include-ignored` filter once played a short tone through `audio::tests::smoke_plays_tone_on_default_device`. Use exact test names.
- Still open:
  - The Task 6 note: cover fetches and media controls can block for up to 20 s while a hung server still reads as online.
  - The M2 gap: `kind: 'track'` player errors have no UI.

## Owner request (2026-10-21)
When the M3 agent work is finished:
- Push everything to GitHub as a PRIVATE repo, wl-lankin/prism, with all branches. Make it public at the v1.0.0 release (M6).
- Include the complete plan and status, so the owner can continue on another PC.
- The `.superpowers/` folder is gitignored and 44 MB (diffs and screenshots). So:
  - Commit a curated handoff under `docs/superpowers/handoff/`.
  - Leave the owner's Navidrome URL out of everything committed.

## Rulings
- Task 2: fake server is a hand-rolled TcpListener server (not tiny_http) so it can stall/cut sockets; flate2 is a dev-dependency. Accepted.
- Task 2: `PlaylistWithSongs` flattens `summary: PlaylistSummary`; later tasks read `playlist.summary.name`.
- Task 2: redirects follow same host only, http->https allowed, https->http refused (token+salt are replayable credentials).
- Task 2: a 401 with a non-Subsonic body (the proxy's own auth) maps to NotSubsonic("HTTP 401"), not WrongCredentials.
- Task 2: stream errors must go through `classify_reqwest` (strips the URL, scrubs, classifies). Tasks 7 and 8 must use it.
- Task 4 requirement: the search3 paging loop must stop when a page adds no new ids, and have a hard page cap (a server ignoring songOffset must not loop forever). Add Fault::IgnoreOffset to the fake server.
- Task 4 requirement (from Task 1): the sync must fill `albums.artist_id`. Otherwise Navidrome artist ids fall back to names and can collide with local favorites.

- Task 5: `online_flag` means "the server answers with these credentials". It is true in Syncing, Online, Error{syncRefused} and Error{database}, and false otherwise. A saved account starts in Syncing.
- Task 5: the mini window can reach the navidrome commands, because build.rs has no AppManifest. Accepted for M3, since it is the M2 trust model.
- Tasks 10-13 lock rules:
  - New cycle steps check `is_current(epoch)` under the DB lock.
  - Never take the DB lock while holding `Core::inner`.

## Deferred minors
- M6 hardening: add an AppManifest command list so main-only commands (navidrome and others) can be restricted from the mini window.
- Task 1 (approved, merged cf976e4):
  - FavoritesView.svelte:65 has a stale comment "Artists carry no source yet", and artists skip keepSource. -> Task 17.
  - ArtistView.svelte:78 hearts with id={name}, so a Navidrome-only artist favorite never resolves. Task 17 decides what the union-page heart writes.
  - Each artist name is listed twice (once per source) until the source badges land. -> Task 16.
  - stores/favorites.svelte.ts:7 doc comment still says "artist names". -> Task 17.
  - Optional: a test that applies 0008 on top of a populated v7 DB.
  - Task 4 must pin with a test that artist_id is always written when the server provides one.
- Task 2 deferred minors: flaky-port races in restart()/dead_port(), detached stall threads in the fake server. Watch these in Task 7.

## Incidents
