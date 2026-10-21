# SDD ledger — plan: docs/superpowers/plans/2026-10-20-prism-m1-foundation.md

Spec: docs/superpowers/specs/2026-10-20-prism-design.md
Branch: PRISM-1 (from main 9770a13, docs only)

## Preflight scan

| Rows | Produces / consumes | Finding |
|---|---|---|
| T1 self | creates .gitignore | already exists from setup commit -> extend it |
| T1 self | scaffold + tests | consistent |
| T2 self | spike needs owner sign-in | cannot run unattended |
| T3 self | "show owner before moving on" | conflicts with continuous execution |
| T3 -> T10 | theme tokens -> components | consistent names |
| T4 -> T5 | Track model incl. replayGain fields -> scanner | T5 text does not say to fill RG fields |
| T4 -> T8 | Db, models -> library commands | consistent |
| T5 -> T6 | CoverStore -> protocol | consistent |
| T6 -> T8 | T6 writes coverUrl into src/lib/ipc.ts, T8 "creates" ipc.ts | file created twice |
| T7 -> T8 | AudioSource/open_source/OutputFactory -> engine | consistent (OutputFactory introduced in T7) |
| T8 -> T12 | player_set_volume "persisted" vs settings in T12 | persistence store does not exist yet in T8 |
| T8 -> T9 | player events/commands -> media controls | consistent |
| T8 -> T11 | player_play, library_* -> views; T11 adds player_play_files | consistent |
| T10 self | screenshot for owner | conflicts with continuous execution |
| T12 self | RG gain in engine (T7/T8 code) | consistent |
| T13 self | needs owner hand check | final step, expected stop |
| others self | tests vs code | consistent |

Ruling: .gitignore exists -> Task 1 extends it and adds .gitattributes (eol=lf) -> cost if wrong: none.
Ruling: Task 2 (YTM spike) moves to just before Task 13: a subagent builds the dev-only spike window on branch spike-ytm, the owner signs in during the Task 13 session -> needs a human; nothing else depends on it -> cost if wrong: the YTM risk is known later in the day (still before M2).
Ruling: Task 3 / Task 10 owner reviews (logo, screenshots) are collected and shown at Task 13 instead of pausing -> logo and styling are cheap to change -> cost if wrong: some rework of icons/CSS.
Ruling: Task 5 also fills replayGain* fields from tags (RG tags read by lofty) -> T12 needs them and T4 defines them -> cost if wrong: none.
Ruling: Task 6 creates src/lib/ipc.ts with coverUrl only; Task 8 extends the same file -> avoid double-create -> cost if wrong: none.
Ruling: Task 8 keeps volume in memory; Task 12 persists it via settings -> no store in T8 yet -> cost if wrong: none.

## Tasks
Task 1: fix round 1/5 (port moved off Glint 1420, HMR CSP, fileURLToPath; commits c0c374b..0a8b8bd)
Ruling: Prism dev port 1440 / HMR 1441 (Glint uses 1420, Kern 1430) - owner runs several apps' dev servers - cost if wrong: none.
Task 1: fix round 2/5 (3 addressed, 0 open; commits 0a8b8bd..3e41074)
Task 1: minor (deferred): CSP has no dev-only split; capabilities grant extra window perms (set-title, is-maximized, show, set-focus); make-icons uses npx via shell.
Task 1: complete (commits 9770a13..3e41074, review clean)
Task order note: Task 2 (YTM spike) runs after Task 12, before Task 13 (see preflight ruling).
Task 3: minor (deferred): scrollbar-width overrides ::-webkit-scrollbar rules (dead hover); macOS icon has no ~10% padding; theme.css also holds a global reset.
Task 3: owner check at Task 13: logo resembles Pink Floyd "Dark Side of the Moon" prism - ask owner if OK.
Task 3: complete (commits 3e41074..9fb35b6, review clean)
Task 4: fix round 1/5 (3 addressed, 0 open - FTS keyed on explicit row_id + upsert/delete test, newer-schema guard, camelCase test; commits 1e68642..f25f740)
Task 4: minor (deferred): negative user_version branch untested; foreign_keys=ON unused; UNC canonical path strip gives "UNC\..."; lock().unwrap() panics on poison.
Ruling: migration 0001 edited in place instead of 0002 - nothing shipped, only throwaway test DBs exist - cost if wrong: delete a stale local dev DB.
Task 4: complete (commits 9fb35b6..f25f740, review clean)
Ruling: Task 5 unreadable formats (WMA/TTA/DSF/DFF) get an ffprobe fallback now (tools.rs ffprobe_path/ffmpeg_path), rows with duration 0 re-read when ffprobe missing - spec "every format" + covers are core - cost if wrong: small extra module.
Task 5: minor (deferred): cover stat/hash work under DB lock; no mass-deletion guard for empty mount points; watcher has no extension filter (temp files trigger rescans); failed files re-read each scan; nested library folders should be refused in Task 8/12 add_folder; offline folder not re-watched when it returns; Task 4 files not cargo-fmt clean.
Task 5: fix round 1/5 (7 addressed, 0 open - per-chunk album rebuild + reconcile, unregistered folders not rescanned, canonical watcher roots, ffprobe fallback via tools.rs, metadata() failure keeps row, normalized remove, debouncer Err rescans all; commits 992413b..e47292c)
Task 5: minor (deferred): without ffprobe, zero-duration rows emit Changed every rescan; tools.rs test sets env var; Task 8 should unwatch on remove_folder and refuse nested folders.
Task 5: complete (commits f25f740..e47292c, review clean)
Ruling: frontend tests are named *.spec.ts (Task 1 vitest include) instead of the plan's *.test.ts - consistency with the scaffold - cost if wrong: none.
Ruling: release profile drops panic = "abort" so decoder/protocol panics can be caught (catch_unwind -> 500) - app parses untrusted media - cost if wrong: slightly larger binary.
Ruling: undecodable cover -> 422 accepted (brief named only 400/404) - distinguishes bad image from bad request - cost if wrong: none.
Task 6: fix round 1/5 (7 addressed, 0 open; commits 37c5eef..9785216)
Ruling: encodeURIComponent in coverUrl without a server-side decode stands - cover ids are sha1 hex from CoverStore::store_bytes (Task 5), so the encoding is a no-op - cost if wrong: 404 for non-hex ids, none exist.
Task 6: minor (deferred): catch_unwind path untested; a missing original file now yields 500 instead of 404; rustfmt churn on db/model bundled into the fix commit.
Task 6: complete (commits e47292c..9785216, review clean)
Task 7: minor (deferred): 5.1 downmix normalized by weight sum (~7.6 dB quieter); AAC priming trim fixed 1024 frames (no edit list/iTunSMPB, ~23 ms seek offset); FfmpegSource duration None (use DB).
Task 7: fix round 1/5 (8 addressed, 0 open - count-based ring clear w/ ramp, NoDevice + NullSink + 1 s backoff, primed ffmpeg seek, deferred error after Advanced, typed DecoderPanic; commits dd8ca2b..514e536)
Task 7: minor (deferred): device lost late drops up to ~200 ms tail; first reopen after NullSink start waits 1 s; NullSink spec 48 kHz during NoDevice; smoke test needs an active output device (owner hand check).
Task 7: complete (commits 9785216..514e536, review clean)
Task 8: minor (deferred): seek blocks engine thread during ffmpeg restart; prev during final tail goes to second-to-last; failing pre-opened track cuts old tail; error flood for big dead queue; add_folder overlap check not atomic; LIKE case-folding ASCII only; player://state after every command; initPlayer() must be called by Task 10 shell.
Task 8: fix round 1/5 (6 addressed - resolve on opener thread, idle skips ignored, paused hand-over, store track identity, snapshot only when needed, watch in blocking; commits 45e7e4f..43163b4)
Task 8: fix round 2/5 (2 addressed, 0 open - shown track kept while loading, no event for ignored skips; commits 43163b4..8bed4b2)
Task 8: minor (deferred): opener is serial (blocked preload resolve delays manual Next); paused hand-over deadline not shifted on resume; no-op seeks still emit state.
Task 8: complete (commits 514e536..8bed4b2, review clean)
Ruling: future dispatches must kill only their own PIDs (Task 9 ran taskkill /IM node.exe, may have killed owner's other dev servers) - tell owner at hand check.
Task 9: minor (deferred): media worker detached thread without explicit shutdown; push/fan_out untested; souvlaki pulls old windows/cocoa crates.
Task 9: fix round 1/5 (6 addressed, 0 open - placeholder cover, encoded file URLs off Windows, seek clamp, metadata retry, catch_unwind, explicit cache dir; commits d6018fa..5182b98; compile confirmed by implementer's 105-pass cargo run)
Task 9: minor (deferred): placeholder cached forever; non-UTF-8 unix paths lossy in cover URL; live SMTC re-check after fix pending (hand check).
Task 9: complete (commits 8bed4b2..5182b98, review clean)
Task 10: minor (deferred): logo/About button dead until Task 12; Cover reset via $effect.pre instead of {#key}; macOSPrivateApi needed for transparency (note in README, no App Store); Mica over wallpaper + white flash not eyeballed (hand check).
Task 10: fix round 1/5 (8 addressed, 0 open; commits 1a27acd..f344b7f)
Task 10: minor (deferred): unused @keyframes fade in App.svelte.
Task 10: complete (commits 5182b98..f344b7f, review clean)
Task 11: fix round 1/5 (11 addressed, 0 open; commits c4f90a1..2ebab38)
Task 11: minor (deferred): failed song page not retried until range changes; album backdrop 64:34 aspect may not cover on narrow windows; mapped network drive letters not detected; drop race restore; song double-click queues ~1000 rows around click.
Task 11: complete (commits f344b7f..2ebab38, review clean)
Owner is iterating the window layout directly with the Task 11 agent (resumed from its transcript): commits 593d109, 8ad4498 (app fills window, ring moved). Not a controller ruling; owner-driven. Final review must treat the ring position as owner-decided.
Task 12: implementer done (c36ae59, 0f4a17d); concurrent owner-driven edits in App.svelte/window.rs in working tree.
Task 12: minor (deferred): device switch during gapless tail drops old tail; ffmpeg seek on device switch can stall engine up to 5 s / fail track; ReplayGain mode change unramped; Select aria-selected follows committed value, Home/End don't open, single-char typeahead; Vitest specs written after components.
Task 12: fix round 1/5 (7 addressed, 0 open - settings changes applied under store lock, Select clip box, opener scope narrowed, About blocks global shortcuts, keyed store apply/revert, .bad on read errors, volume flush on close; commit a4f1671)
Task 12: minor (deferred): player send errors in settings_set only logged; close waits on settings IPC; opener reveal-item-in-dir unscoped; blocked settings branch untested.
Task 12: complete (commits 2ebab38..a4f1671 excluding owner layout commits 593d109 8ad4498 d16a04d 2e2e089 28e7bf7, review clean)
Ruling: Task 2 spike runs in a separate git worktree (C:\Projects\private\Prism-wt\spike-ytm) - the owner is editing the shared working copy with another agent, branch switching there would disrupt it - cost if wrong: one npm ci in the worktree.
Owner-driven layout work (Task 11 agent, resumed by owner): 593d109, 8ad4498 (rejected tries), d16a04d, 2e2e089 (transparent strip, no OS shadow, no Mica), 28e7bf7 (ring 96 px), 77ba1d8 (empty-state prism illustration), b498b16 (demo chime). Owner decisions, not rulings. Final review: check that the transparent strip is see-through on a real desktop and macOS.
Owner-driven: b701ad1 prism illustration glass fill.
Owner-driven: f71a70c no DWM border/corners/backdrop (windows-sys 0.61). Found: gapless_no_drain_between_same_spec flaky in full runs since c36ae59 -> sent back to Task 12 implementer.
Owner-driven: e32ed9e 48 px transparent strip below ring. Follow-up for M2: transparent strip swallows clicks to the desktop behind it (consider hit-test region / SetWindowRgn).
Task 12 follow-up: flaky gapless test fixed in test fake (d239647), review Approve.
Deferred for M2: preload skipped for !seekable (FFmpeg) sources and duration 0 -> Opus/WMA/etc never gapless; RING_MS in tests duplicates BUFFER_MS; wall-clock tests (corrupt_file_errors_cleanly 2 s, rescan_2000 1 s) can flake under load.
Owner asked (via Task 11 agent) about YouTube with Prism's own controls; that agent advised official IFrame player (visible video, no gapless/RG) and against stream extraction. Spec 5.6 (embedded music.youtube.com view) may change - revisit with owner before M5 plan.
Task 2: complete (spike prepared signed-out: renders, mediaSession readable, buttons clickable, no "not secure" block at identifier step; remote IPC removed; docs e83afc6; spike-ytm worktree bef1a6f..8e5b6b4). Owner sign-in test open -> Task 13.
Owner decision: YTM view inside the main window content area (child webview), no extra window; spec updated.
Final review: Ready for hand check; 0 critical, 5 important.
Ruling: fix now - I1 ReplayGain clamp (gain -24..+12 dB, no-peak positive gain capped at 1.0, samples clamped to [-1,1]), remove unused opener reveal permission, ffprobe/ffmpeg `file:` prefix + absolute-path check in player_play_files, scan-progress final event on error, stale Task-number comments - cheap and user-facing safety - cost if wrong: none.
Ruling: defer to M2 - I2 cover resize concurrency limit, I3 track id without per-file canonicalize (MUST land before favorites), I4 read-ahead/decode thread (NAS dropouts), I5 startup failure dialog/DB quarantine; missing indexes migration 0002; dead window-vibrancy dep; maximize keeps transparent band; ACAO * tightened in M5; macOS traffic lights in M6 - larger changes, M2 opens with them - cost if wrong: dropouts on slow NAS reads and slower first paint until M2.
Final fix wave: 9dbcc24, re-review all 5 resolved. Deferred: scan error before first progress emits 0/0 (check UI treats as done); ffmpeg/ffprobe tests skip without tools (CI must install them in M6).
Tasks 1-12 + final review complete. Task 13 (owner hand check) open: YTM sign-in, own music folder, real audio device, SMTC/media keys, logo (Dark Side resemblance), transparent strip on desktop, device switch, white flash.
Task 13: YTM sign-in verified by owner via ytm-signin route; probe OK (spike doc updated).
Task 13 hand-check fixes: 05c6bb6 coming-soon pill outside scroll, 525468f window backgroundColor #00000000 (tauri-runtime-wry repaints config bg on WM_PAINT), 6bf7323 settings horizontal scrollbar. M2 plan b6891cf awaiting owner review.
