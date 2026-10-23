# M7 Lyrics - SDD ledger

Plan: docs/superpowers/plans/2026-10-22-prism-m7-lyrics.md. Branch PRISM-7 cut from PRISM-6 at ccd41a3, controller worktree ~/Projects/Prism-wt/m7. Model commit 84113b5.

## Owner answers (2026-10-22)
- Sources: files and .lrc, Navidrome, LRCLIB, YouTube Music.
- Display: Now Playing and a side panel. Not the mini player.
- LRCLIB on by default, switchable.

## Waves
- A: Tasks 1-5 in parallel. B: Task 6. C: Task 7, then Task 8 (owner).

## Progress
- Task 2: 42e95a7 done (cargo 989, lyrics:: 30 x3). Python edit in a test module (rule slip, noted). In review.
- Task 3: 8b044c6 done (cargo 978, lyrics_ 13 x3). Code before tests, a Python edit reverted and redone. Offset: start - offset (spec: positive = sooner). In review.

- Task 1: 030a22f done (cargo 990, lyrics 31 x3). Reuses fixtures; SYLT only for MPEG. Python edit in one test. In review.
- Task 5: a2bce5f done (npm 504, check 0). Toggle in its own "Lyrics" settings section under Covers. In review.
- Task 2 review: approve with minors (duration 1..3600, blank artist/title, suffix whole words, tests, live gate, key escaping); fixes sent back to the implementer.
- Task 3 review: changes needed (extensions error cached as no songLyrics; mutex across network), plus minors; fixes sent back.
- Task 1 review: changes needed (memory blow-up with many tags per line), minors; controller ruled Windows-1252 fallback for sidecars; fixes sent back.
- Merged into PRISM-7: Task 2 (9bf3061), Task 1 (621060f), Task 3 (6c85188), Task 5 (a5d1cf2). Controller: bidi overrides stripped in Lyrics clean() (from the Task 4 report). cargo 1046 + 2 expected failures (window manifest tests: lyrics_* commands come in Task 6), npm 512, check 0. One unreproduced flaky failure in a full run during parallel load.
- Task 4: 3831e76 done (cargo 976, ytm 62 x3, npm 486). Deviations: two snippets, lyrics() -> Result<(id, Option<Lyrics>), PageLyricsError>, `failed` flag, Changed error, later answer serves earlier waiter; bridge now fetches (credentials omit). In security review.
- Task 4 review: approve with minors (format drift reported as failed, fixture, surrogate cap, spec asserts); fixes sent back. Security rule change accepted: the bridge now makes two same-origin, credential-less innertube requests (M5 had ruled bridge network requests out).
- Task 4 fixes e598b94; merged 7b457d3. cargo 1057 + 2 expected manifest failures, npm 527, check 0. Task 6 started in m7-t06 (PRISM-706).
- Task 6: c8fc688 (cargo 1096 all green, npm 527). Review: approve with minors (setting-off refetch of lrclib answers, tests, signed-out none, clear navidrome rows on disconnect); fixes sent back.
- Task 6 fixes 43cd471; merged 96904e5. Task 7: cargo 1101 green, clippy, fmt, Windows check, npm 528, check 0; CHANGELOG Unreleased + README (d2fc848), LRCLIB credit (ae1b569). Hand-check instance dev8 (prism-data8) running from m7 with demo/ (60 tracks; Night Swim Revisited 01 .lrc, 02 embedded synced, 03 embedded plain, invented text). Task 8 owner hand check open.
- Task 8: owner 2026-10-23 "Mir passts", then "dann release". Task worktrees m7-t01..t06 and branches removed. Version 1.1.0 (0740f38), merged into main 222e2c0, CI on main red once (clippy 1.99 chunks_exact in lyrics/local.rs), fixed; main df0c937, tag v1.1.0, release run 39925991835 green, all 10 assets, sigs version:1.1.0, published as latest.

## Rulings
- Recurring: implementers edit with Python despite the Write/Edit rule (Tasks 1, 2, 3). Reviewers check the result, not the tool.
- One cargo test filter per run.
- Agents never push, publish, change visibility or touch secrets/keys.

## Binding for later tasks
- Task 6 (from Task 4 review): `youtube:` found entries expire after 30 days (a page answer must not stick forever).
- Task 6 (from Task 4): `YtmService::lyrics(timeout)` blocks up to 8 s: blocking thread, no lock; call only when the ytm state is not Idle/Unavailable; cache Ok((id, None)) as a miss, never an error. `videoId` is in ytm://state but not in the TS YtmState type.
- Task 6 (from Task 5 review): `file:` track ids (dropped files) answer none, never error.
- Task 6 (from Task 5): Rust setting `lyricsLookup`; UI calls `lyrics_for_track { trackId }` and `lyrics_for_youtube` (no args); a rejected command shows the error state.
- Task 6 (from Task 1): `local::read(path)` is blocking file IO (and a second MPEG open for SYLT): spawn_blocking, no lock.
- Task 6 (from Task 3): `SubsonicClient::lyrics(raw_song_id, artist, title)` returns (synced, plain); strip the prefix with `library::model::navidrome_raw`. `Ok(None)` is a cacheable miss, `Err` never. Blocking.
- Task 6 (from Task 2): `Query.album` is `Option<String>`, `duration_ms` 0 = unknown. `Lrclib::find` is blocking, up to 4 calls of 10 s: call it off the UI thread and outside every lock.

## Deferred minors
