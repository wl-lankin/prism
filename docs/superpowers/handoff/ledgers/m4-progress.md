# M4 Radio - SDD ledger

Plan: docs/superpowers/plans/2026-10-21-prism-m4-radio.md (a23ae1c). Branch PRISM-4 cut from PRISM-3 at b5599b0 (M3 Tasks 1-19 merged; M3 Task 20 hand check still open, its fixes get merged into PRISM-4 later).

Machine: the owner's Mac (10 cores, 24 GB). No CDP visual checks on macOS.

## Waves (adjusted for parallelism)
- A: Tasks 1, 3, 4 in parallel. Task 2 starts as soon as Task 1 is merged (it needs `Station` and `clean_name`).
- B: Task 5 (after 1, 4), Task 7 (after 1, 2, 3; plugs in Task 5 at merge).
- C: Task 6 (after 5), Task 8 (after 7).
- D: Tasks 9, 10, 11 in parallel.
- E: Task 12. Then 13 (owner hand check).

## Owner answers carried in (2026-10-21)
- Logos: favicon first, then the station homepage (apple-touch-icon, icon, og:image, /favicon.ico), initials last.
- Sidebar label: just "Radio".
- Questions 1-3 of the plan accepted as proposed.

## Progress
- [x] Task 1: approved, fix round 9820c5b (derived cover id always, format chars dropped, cut-end trimmed, tags trimmed first, stronger never-touch test). Merged 5009d06. Combined: cargo 673 passed, clippy and fmt clean, npm 276 passed, check clean.
- [x] Task 2: approved after fix round 1 (0d9d7c4: HugeUnsized test for the read cap, real host in "not a list" errors, fake country names). Merged c8bab07. Combined: cargo 695 passed, clippy and fmt clean.
- Controller fix c798e2a: `opening_makes_one_request` was not a load flake but a test assumption: on a fast loopback the whole 5 MB file is downloaded before the seek, so no Range request is needed. The test now allows 1 or 2 requests (any second one ranged). This M3 test fix should also go to PRISM-3.
- [x] Task 4: approved after fix round 1 (1e77753), re-review approved. Merged fbe0b73. Combined: cargo 728 passed, clippy and fmt clean.
- [ ] Task 5: m4-t05 / PRISM-405 (base PRISM-4 fbe0b73), running (Opus). Must cancel through `LiveReader::canceller()`.
- [ ] Task 5: 8236082. Review FIX NEEDED, fix round 1 running. Must: Ogg chain restarts handled inside SymphoniaSource (`rechain()` on ResetRequired; a fresh reader cannot work because the old reader consumed the new chain's headers); every AAC station to FFmpeg (symphonia is LC only, AAC+ plays muffled); any non-offline open_media error falls back to FFmpeg; a failed reopen or the cap after play goes up as Offline. Plus: opener takes a title sink factory per station id.
- [x] Task 6: approved, fix round ebf612f (live check by queue entry, unresolvable station live by prefix, `set_live_title(station_id, title)`, [n1, STATION] wrap test). Merged 74b5bc4. Combined: cargo 752 passed, clippy and fmt clean, npm 276, check clean.
- [x] Task 7: approved after fix round 1 (35ce630: private-network refusal for favicons via a public-only resolver plus literal and redirect checks; `title_sink_for`/`title_sinks`; no resurrection; lazy locale; cfg(test) loopback guards; bomb tests; Drop; near-key cleanup). Merged 0a24179.
- [x] Task 8: approved, fix round edae677. Merged a2d31ba. Combined: cargo 805 passed, clippy and fmt clean, npm 291, check clean.
- Controller a92e9e3: `radio::TitleSink` is now `audio::live::TitleSink`; `on_title` became `OnTitle(station_id, title)` and lib.rs wires it to `MediaBridge::set_live_title`. Remaining wiring after Task 5: the opener chain.
- [x] Task 5: approved after fix round 1 (46a8d4a), re-review approved. Merged 292d222. Non-blocking for Task 12: a fresh FFmpeg decoder on the same stream opens synchronously without silence (slow on very low bitrates); the reconnect wait bound can report Offline slightly early.
- Controller 08cfb3c: opener chain `radio_opener(radio.lookup(), LiveConfig::new()?, radio.title_sinks(), navidrome_opener(..))`. cargo 831 passed, clippy clean.
- [x] Task 9: approved after fix round 1 (8341bae: capability reverted, backend `radio_open_homepage` with a bounded map of recent near/search homepages, loading note, fixes). Merged 4b16d6e (hand merge: i18n blocks, duplicate `radio.menu.homepage` dropped from the Task 9 block, store hunks; Heart unified on a `station` prop instead of Task 9's `onflip`). cargo 835 passed, clippy/fmt clean, npm 333, check clean.
- [ ] Integration pass: m4-tint / PRISM-499, running (Opus): one heart, StationTile everywhere, startStation only, TrackMenu homepage through the backend, Settings deep link, StationList in Favorites, i18n cleanup, serve_for_ui with a FakeRadio stream.
- [x] Task 12: e13b029, checked by the controller (tests only; the loopback guard opt-out is a process flag set only by `live_directory_smoke` with PRISM_LIVE_RADIO=1). Merged 21931a5. Release numbers: one simulated hour = 144 MB through a 256 KiB ring (peak 80 KiB), 3600 s decoded in 1.17 s, threads 5 -> 5; reconnect storm 29 cuts in 60 s, one reconnect per cut, never an error, longest read gap 605 ms; near::rank over 10 200 stations 1.9 ms (budget 200).
  - Live smoke (owner runs it, or the controller with the owner's OK): `PRISM_LIVE_RADIO=1 cargo test --manifest-path src-tauri/Cargo.toml --lib -- --exact --ignored --nocapture radio::directory::tests::live_directory_smoke`
- [x] Task 10: approved after a short fix round (443079c). Merged 04c7dd9. Its TrackMenu "Open homepage" still uses the webview opener: switch to Task 9's command in the integration pass.
- [x] Task 11: approved after fix round 1 (e0ef7eb: Home row on an empty library, tabs hidden under the Radio chip, localized country names, calmer effects). Merged 13d07b6 (hand merge of i18n blocks and radio.svelte.ts imports; radio.spec `favorite_toggle_reloads_saved` adjusted because initRadio now loads saved once). npm 315 passed, check clean, cargo 831.
- UI integration pass needed after merging 9-11 (as in M3): three different station-heart approaches (Task 9 Heart `onflip`, Task 10 Heart `kind="station"`, Task 11 local button) -> one; StationTile replaces Task 10's Cover `initials` and Task 11's StationArt; `startStation` (9) vs `stationUi.ts` (11) play-with-notice -> one; "Open homepage" in StationMenu (9) and TrackMenu (10) through one backend command; duplicate i18n keys (`radio.menu.homepage` in 9 and 10); smoke.spec.ts and views.spec.ts edits.
- [ ] Tasks 9, 10, 11: m4-t09/t10/t11 (base PRISM-4 74b5bc4 + PRISM-408 at 343f1ed), running in parallel (Sonnet). Merge aids: own i18n blocks per task, own spec files (radio.spec.ts, live.spec.ts, radioExtras.spec.ts), App.svelte touched only at their own spot. Tasks 10/11 build local tile fallbacks; the controller swaps in Task 9's StationTile/StationList at merge.
- [ ] Task 7: Review FIX NEEDED, fix round 1 running. Must: favicon/homepage fetches refuse private, loopback and link-local addresses (custom DNS resolver plus literal and redirect checks; streams unrestricted); `title_sink_for(station_id)`. Minors: no resurrection in refresh, lazy locale, cfg(test) loopback guards, bomb tests, Drop, near-key cleanup.
- [ ] Task 8: m4-t08 / PRISM-408 (base PRISM-4 + PRISM-407 at 121e448), running (Sonnet).
- Wiring at merge (controller): `radio_opener(radio.lookup(), Arc::new(LiveConfig::new()?), radio.title_sink(), navidrome_opener(..))`; unify `radio::TitleSink` with `audio::live::TitleSink`; replace the no-op `on_title` with `MediaBridge::set_live_title`.
- Was Task 4 review round 1: Must: reconnect bridges 404/500/empty while a playing stream is down (plain failures immediate only at open); Shoutcast `ICY 200 OK` raw-socket fallback for http:// (reqwest classifies it as offline today). Minor: Canceller bound to one reader, icy-name Latin-1, redirect and repeated-title tests, context string.
- [ ] Task 7: m4-t07 / PRISM-407, running (Opus). Base: PRISM-4 + PRISM-402 at b477d67. Seams: no radio/stream.rs, no opener chain; exposes lookup_station and title_sink; on_title hook for the media bridge until Task 6.
- Controller survey 2026-10-21: of the 120 most clicked German stations, after redirects 81 answer HTTP/1.1 200, 26 HTTP/2 200, 11 HTTP/1.0 200, none `ICY 200`. The ICY fallback is for small stations.
- [x] Task 3: approved, fix round 1ebb292 (body cap via take(), no redirects, control chars stripped). Merged 49da357. Combined: cargo 651 passed, clippy and fmt clean, npm 276 passed, check clean.

- [x] Integration pass: 2c93f1e approved. Merged 4decbf2. Controller 867cf47: station heart tooltips say "Save station" / "Remove station".

## State at end of the build (2026-10-21)
- Tasks 1-12 and the integration pass are merged on PRISM-4 (867cf47). Task branches and worktrees are deleted.
- Full checks: cargo 836 passed, 17 ignored; clippy and fmt clean; npm 340 passed; check clean; `npm run build` and `cargo build` fine. No em or en dashes in src, src-tauri/src or the plan.
- Open: Task 13 (owner hand check on Windows and the Mac), the optional live smoke test, merging the M3 hand-check fixes from PRISM-3 into PRISM-4, and the M3 test fix c798e2a back to PRISM-3.
- Dev run against fake data: `cargo test --manifest-path src-tauri/Cargo.toml --lib radio::testserver::serve_for_ui -- --exact --ignored --nocapture` prints PRISM_RADIO_DIRECTORY and PRISM_RADIO_GEO; start the dev app with those plus PRISM_DATA_DIR and an identifier override.
- Deferred minors:
  - Task 5: a fresh FFmpeg decoder on the same stream opens synchronously without silence (slow only on very low bitrates); the reconnect wait bound can report Offline slightly early.
  - Integration: the palette and StationList build a station's second line differently; the Settings deep-link scroll can shift when library folders load later.
  - Task 4: one failed reqwest request before each raw ICY request on Shoutcast v1 stations; `raw_get` DNS lookup has no own timeout.
  - Task 6: souvlaki cannot grey out next/prev/seek; they are ignored for stations instead.
  - M6 hardening: AppManifest command list now also for the radio commands.

## Rulings
- Plan commands that list several cargo test filters do not work (cargo takes one filter). Run each filter on its own.
- Task 3: `SettingsStore::update` refuses an invalid `radioLocation` patch (like `navidromeQuality`); the file reader stays lenient. Accepted.
- rustfmt was added to the Mac toolchain (`rustup component add rustfmt`).

- Flaky under load: `navidrome::stream::tests::opening_makes_one_request` fails sometimes in a full run while several worktrees build at once (also on the untouched base), passes alone. Pre-existing; not a blocker. Rerun it alone before calling a run red.

## Binding for later tasks
- Task 8 (from Task 1): `radio_favorite` must write through `set_with_source(.., Source::Radio, ..)`; plain `favorites_set` would store source 'local'. TS `FavKind` gets `'station'` in Task 8/11 (Task 1 left it out, FavoritesView indexes by kind).
- Task 7 (from Task 3):
  - Call `resolve_place` off the DB lock and off the async runtime (blocking reqwest; up to about 10 s).
  - Store the place with `place_to_json` in `radio_state` key `place` only when the returned bool is true.
  - Never log a `Place` with `{:?}` (its Debug prints coordinates).
- Task 2: `radio/testserver.rs` already holds `FakeGeo`; add the fake directory as its own section.
- Task 7 (from Task 2 review): never cache mirrors from `MirrorSource::Fixed` (the dev override); refresh mirrors against `MIRROR_TTL`.
- Task 9 (from Task 2 review): `serve_for_ui` serves no ICY stream yet; station URLs point at `PRISM_FAKE_STREAM_BASE` (default 127.0.0.1:8099). Task 9 (or the controller) starts Task 4's FakeRadio inside `serve_for_ui`.
- Task 5 (from Task 4): fake HLS playlist names seg1.ts/seg2.ts but serves none; Task 5 adds real TS segments. Tests live in `audio/live/tests.rs`. Sending `None` to the title sink on open is Task 5's job.

## Deferred minors

## Incidents
