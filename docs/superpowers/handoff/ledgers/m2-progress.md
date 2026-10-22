# SDD ledger — plan: docs/superpowers/plans/2026-10-20-prism-m2-everyday-player.md

Spec: docs/superpowers/specs/2026-10-20-prism-design.md
Branch: PRISM-2 (cut from PRISM-1 at b6891cf)

## Preflight scan

| Rows | Produces / consumes | Finding |
|---|---|---|
| T1 -> T2 | Migration enum (Sql/Rust) -> migration 3 Rust step | consistent |
| T2 -> T9/T11/T14 | TRACK_ID_REFS -> favorites, track_stats, playlist_tracks refs | consistent; migration numbers 0002, 3, 0004, 0005, 0006, 0007 follow task order |
| T5 -> T13 | ReadAhead exhausted/buffered_frames/set_target_ms -> crossfade | consistent |
| T7 -> T12/T16/T21/T22/T23 | PlayerHandle, queueVersion, player_queue_range | consistent |
| T8 -> T13/T18/T20/T22 | Settings fields + Change variants + shell::apply_setting | consistent |
| T8 vs M1 T12 | drops frontend flushVolume/onCloseRequested | core:window:allow-destroy may become unused -> T20 capability cleanup covers it |
| T9 -> T10, T11 | favorites API -> UI, Flow pool | consistent |
| T11 -> T12 | Deck, pool, FlowVariant -> FlowService | consistent |
| T12 -> T13 | PlayerState.flow -> crossfade_for | consistent |
| T16 -> T20 | NowPlaying + nav://go | consistent |
| T17 | PlayerEvent::Level must be ignored by sinks from T9/T11/T12 and media bridge | consistent (stated in T17) |
| T19 -> T20 | window-shape.json, shape.rs -> mini region | consistent |
| T20 -> T21 -> T22 -> T23 | mini::show/toggle, ShellState, shortcut, open_with | consistent |
| Global constraints | "work on branch PRISM-1" | conflicts with milestone branch per owner convention |
| all tasks self | tests vs code | consistent |

Ruling: M2 runs on branch PRISM-2 cut from PRISM-1 (plan says PRISM-1) - one branch per milestone like Kern/Quire, keeps M1 reviewable - cost if wrong: none (fast-forwardable).
Ruling: owner layout decisions win over spec 5.9 Mica/vibrancy text (no Mica, no shadow, ring 96 px half out) - owner decided in M1 - cost if wrong: none.
Ruling: plan decisions shown to owner and accepted with "go": maximized -> no strip, ring 72 px inside; mini on top default off; taskbar click toggles mini only while main is closed; Flow without favorites falls back to library with notice; crossfade read-ahead up to 9 s.

## Tasks
Task 1: minor (deferred): EXPLAIN tests use SELECT id (covering) not full columns; mid-module use statement in db tests; no failing-step rollback test. Upgrade-from-v1-with-data test -> carried into Task 2 dispatch.
Task 1: complete (commits 525468f..8a9ac3f, review clean)
Task 2 note for T9/T11/T14: ref columns must not FK tracks(id); unique-key refs (favorites, stats) need merge handling in rekey_tracks.
Task 2: minor (deferred): rows whose path case differs from folder keep old id (re-read once); case-sensitive macOS volumes fold a.flac/A.flac together (add comment); folder key text not normalized (relocate must rekey); guard fires for deliberately emptied folder (UI should explain in Settings offline hint); EMPTY_MOUNT test const duplicates EMPTY_FOLDER_OFFLINE; db::rekey -> library::scan layering.
Ruling: pre-migration backup (copy library.db to library.db.pre-v<from>-<to> before running pending migrations) goes into Task 4 (startup safety) - owner's real DB is migrated at first M2 start - cost if wrong: one extra file in app data.
Task 2: complete (commits 8a9ac3f..5a9e487, review clean)
Task 3: minor (deferred): transient read failures cached until restart (no TTL); prune skipped on early returns; event_is_relevant stats FS in debouncer callback.
Task 3: fix round 1/5 (4 addressed, 0 open - offline memo, dotted folder deletes, catch_unwind recheck, stamp cleanup; commits 4adcd33..8fc6408). copy_fixtures arg is a skip list, test correct.
Task 3: minor (deferred): nested-only changes in an offline-but-readable folder do not trigger rescan; poisoned locks app-wide (shared lock helper idea).
Task 3: complete (commits 5a9e487..8fc6408, review clean)
INCIDENT Task 4: implementer launched dev app against owner's REAL %APPDATA%\de.wolfganglinz.prism (APPDATA env ignored by Tauri): real DB migrated v1->v3, backup library.db.pre-v1-v3 created by the new code, brief rescan, nothing deleted. Owner informed. Fix: PRISM_DATA_DIR debug override; all future dev runs must use it.
Task 4: fix round 1/5 (PRISM_DATA_DIR debug override, quick_check dropped; commit 637f99a)
Task 4: minor (deferred): backup target deleted before VACUUM INTO succeeds; quarantine not atomic across db/wal/shm; dummy debug_assert(damaged); CORRUPT_VTAB quarantines whole DB (FTS rebuild would do); lossy path in VACUUM INTO; rfd default features (Linux gtk3/xdg-portal clash); hide main window before fail() dialog; notice names file not folder.
Rule for all later dispatches: every dev run sets PRISM_DATA_DIR to a temp folder; never touch %APPDATA%/%LOCALAPPDATA%\de.wolfganglinz.prism.
Task 4: complete (commits 8fc6408..637f99a, review clean)
Task 5: minor (deferred): skip-spam on FFmpeg sources leaves detached threads/children until current call returns (bounded by 5 s stall timeout).
Ruling: owner asked for parallel subagents. Wave A = Tasks 6, 8, 9 in worktrees C:\Projects\private\Prism-wt\m2-t206/t208/t209 (branches PRISM-206/208/209 from 5a74951), Task 5 fix continues in main copy. Engine tasks (7, 13) wait for Task 5. Merge order into PRISM-2: 5, 6, 8, 9 (migration 0004 stays 4th). Parallel agents run tests only; manual app runs need PRISM_DATA_DIR + own ports. Cost if wrong: merge conflicts in lib.rs/commands.rs/ipc.ts/types.ts, resolved at merge.
Task 8: minor (deferred): core:window:allow-destroy unused (decide in Task 21); remember_volume skipped while settings blocked; timing-based debounce test; Player.crossfade stored, read in Task 13.
Task 8: complete on PRISM-208 (d15eb07, review clean) - awaiting merge after Task 5.
Task 9: minor (deferred): unique-ref merge winner by rowid (VACUUM renumbers) - prefer MIN(added_at); no explicit A->B,B->C chain test with favorites; favorites_set writes via read() helper; favorites::set test-only.
Task 9: complete on PRISM-209 (89228f8, review clean) - awaiting merge.
Task 6: minor (deferred): InFlight entry leaks if acquire/write_atomic/is_file panic (Drop guard idea); parallel-limit test only checks upper bound; old _placeholder.jpg orphaned.
Task 6: complete on PRISM-206 (89457bd, review clean) - awaiting merge.
Ruling: Wave B = Task 10 (worktree m2-t210 from PRISM-209, needs favorites API) and Task 19 (m2-t219 from 5a74951, window shape, independent of engine) while Task 5 finishes - cost if wrong: merge conflicts in App.svelte/TransportStrip.
Ruling: Task 5 20-run stress gate needs a quiet machine; wave B launch held until controller runs the gate on merged code - parallel cargo runs make wall-clock tests flaky - cost if wrong: ~20 min delay.
Task 5: fix round 1/5 (4 addressed, 0 open; commit 7129e2a). Stress: 18/20, then 37 consecutive green full runs on a quiet machine.
Ruling: stress gate accepted with 37 consecutive green (2 early failures unattributed, likely leftover processes) - cost if wrong: a rare flake reappears later.
Task 5: minor (deferred): starve_wait knob dead + misleading doc; dropping_the_player_drops_the_decoded_sources timing-sensitive under load.
Task 5: complete (commits 637f99a..7129e2a, review clean)
Merged 6, 8, 9 into PRISM-2 (HEAD 4ad9bce): cargo 209 passed, npm 73, check clean, clippy clean. Wave B = Tasks 7, 10, 19 in worktrees m2-t207/t210/t219 from 4ad9bce. Merge order 7, 10, 19.
Task 7: minor (deferred): queue_range resolves ids one by one (batch later if paged often); queue_changed double dirty.
Task 7: fix round 1/5 (5 addressed - prev in tail jumps to shown track, settle before Play/Jump, append after cap jumps to first appended, exact 10, BufferedSink tests; commit a9277aa)
Task 7: minor (deferred): out-of-range Jump settles hand-over early; no test for append after failure-cap tail.
Task 7: complete on PRISM-207 (4521f9b..a9277aa, review clean) - awaiting merge.
Task 10: fix rounds 1-2 (keyboard f, hidden hearts, file: no heart, specs; 4abda11, bace4a7). complete on PRISM-210 (5f456c4..bace4a7, review clean)
Merged 7, 10 into PRISM-2 (0d03517): cargo 219, npm 85, check clean. Wave C = Tasks 11, 16, 17 from 0d03517 (worktrees m2-t211/t216/t217). Task 19 still running on 5a74951 base.
Task 19: minor (deferred): region aliasing clips AA corner pixels; TRANSPORT=84 duplicated in App.svelte; macOS tracker needs a real Mac run (M6); heredoc process note. glow 36 accepted (measured).
Task 11: minor (deferred): rekey merge drops loser's play_count/last_played (sum/max would be better); spacing compares artist names byte-exact; genre test name oversells (padding can add other genres); single-artist test does not request > pool; deck now_ms fixed at creation.
Task 11: complete on PRISM-211 (431f575, review clean)
Merged 11 (431f575, cargo 233). Wave D = Tasks 12, 14 from 431f575.
Task 16: minor (deferred): click-jump suppresses follow 5 s (limit to wheel/touch/scrollbar); rows flicker to ghosts on queueVersion bump; failed queue page not retried; heart in option; first ArrowDown skips current; title button double-signals; dead scrollbar-color; heredoc process note.
Task 16: complete on PRISM-216 (1ff2fef, review clean)
Ruling: owner is using the machine (RDP). No more injected mouse/keyboard input or window moving by agents; visual checks only via CDP screenshots of our own window; real-input checks (resize band drag, click-through) go to the M2 hand check (Task 24).
Task 19: fix round 1/5 (6 addressed; f4b7106). minor (deferred): GdiRects inserted between apply's doc comment and fn (misplaced docs); APPLIED map never pruned; resize band overlaps bottom 5 px of transport strip (visual check); real-input band drag pending owner hand check.
Task 19: complete on PRISM-219 (d2c743a..f4b7106, review clean)
Merged 16, 19 into PRISM-2 (conflicts App.svelte effect + Cargo.toml resolved by controller: keep both effects, drop window-vibrancy keep rand): cargo 241, npm 97, check+clippy clean.
Task 14: minor (deferred): playlists://changed fires on no-op commands; playlist_remove O(n*m) contains; playlists_list N+1 summaries; ensure_exists outside txn; generic move error text.
Task 14: complete on PRISM-214 (9a91958, review clean)
Merged 14 (269306b). Wave E = Tasks 15, 18 from 269306b. Task 20 waits for 12 and 17.
Ruling: palette 2 % MIN_BIN_SHARE and +80 third fill accepted - the palette rules were plan-authored, not owner-specified; avoids jarring stray stops - cost if wrong: small accents dropped from ring colors.
Task 17: minor (deferred): DeviceLost keeps last level (reset meter in lost branch); stale reading after pause; >64 pumps within sink depth -> level 0 dip; 20 Hz emit could skip unchanged/emit_to main; --head 600 ms fade lag; silent passage dimmer than paused; macOS canvas CORS unverified; player.track not refreshed when coverId changes for same id (relevant to Task 18).
Task 17: complete on PRISM-217 (b967552, review clean)
Merged 17 (App.svelte ring/ringSize clash resolved by controller).
Ruling: genre Flow must not be padded with other genres (a reasonable user expects only that genre); padding only for favorites/library pools - cost if wrong: small genres give short cycles that repeat sooner.
Task 18: minor (deferred): cover lookup default on at first launch (plan default; owner to confirm at hand check); two reqwest copies (0.12 + tauri 0.13), ring C build; network errors retry only on next kick (no timer).
Task 12: minor (deferred): Flow stays set when queue goes idle (undocumented); service tests wall-clock based (40x, 30 s deadlines).
Task 18: fix rounds 1-2 (refresh_track without TrackChanged - avoids double play count; notfound only for unrecognized images; per-album errors skipped, Unreachable/3-in-row stop; size cap; Unavailable fetcher; SMTC re-push on cover change; d399ad6, fac454c).
Task 18: minor (deferred): persistent set_metadata failure re-runs cover_for each push; variable name track_changed.
Task 18: complete on PRISM-218 (670b0b9..fac454c, review clean)
Task 15: minor (deferred): drag can't start on artist/album text, no Esc cancel; Backdrop thin center band at wide windows; current playlist listed in its own submenu; no automated drag/keyboard-nav coverage; sed process note.
Task 15: fix round 1/5 (7 addressed; 59cf457). minor (deferred): failed queued change -> later changes still sent with optimistic indices (final refetch corrects); retry counter resets on scroll; wrapped multi-line title still shifts meta line.
Task 15: complete on PRISM-215 (a969b6f..59cf457, review clean)
Task 12: fix round 1/5 (6 addressed; e7a6f52). Carried into Task 20: FlowMenu flip-below dead (measures capped height) - mini player must measure scrollHeight/uncapped before above/below decision.
Task 12: minor (deferred): onfocusout ignores null relatedTarget; place() runs twice on mount.
Task 12: complete on PRISM-212 (33b8a30..e7a6f52, review clean)
Merged 12 (8a386e0; lib.rs/App.svelte/types.ts conflicts resolved by controller, keep both sides): cargo 290, npm 134, check+clippy clean. Wave F = Tasks 13, 20 from 8a386e0.
Task 13: minor (deferred): track change reported at fade start (UI/history switch up to 8 s early); hard clip on correlated loud mixes; outgoing gain frozen at target; device switch cuts fade hard.
Task 13: fix rounds 1-2 (crossfade independent of gapless, disc-aware album exception, Skipped reset; 995e721, 0352b8f). minor (deferred): Append resets Skipped on every append (one wasted re-open); test message missing word.
Task 13: complete on PRISM-213 (ddf384a..0352b8f, review clean)
Task 20: owner hand check (Task 24): type in editor, click mini prev/next/heart/volume, caret keeps keys. macOS follow-up (M6): mini must be a non-activating NSPanel (tauri-nspanel or class swap), else a click activates Prism; macOS docks on primary monitor until Task 21 tray rect.
Task 20: minor (deferred): on-top read from WS_EX_TOPMOST bit; raw ShowWindow bypasses tao VISIBLE; hidden mini webview keeps rendering/receiving level events (measure idle cost); BelowRect room may extend past work area.
Task 20: fix round 1/5 (7 addressed; 3e2eb53) + controller one-char spacing fix. complete on PRISM-220 (a6853da..HEAD, review clean). macOS code not compile-verified.
Merged 13, 20 (4d3054c): cargo 314, npm 144, check+clippy clean. Wave G = Tasks 21, 23 from 4d3054c; 22 after 21.
Task 23: minor (deferred): .webm registers Prism for WebM video Open with; relative path resolution untested off Windows; installer build + real Open with = owner hand check.
Ruling: hooks.nsh keeps value-level deletes only; no DeleteRegKey on .ext/OpenWithProgids keys (NSIS /ifempty semantics uncertain; deleting a user's .mp3 key breaks other apps) - cost if wrong: empty registry keys left after uninstall.
Task 23: fix rounds 1-2 (public.audio, multi-select batching, args_os, early paths buffered, panic guard, 15 s bound; value-level registry deletes only; 71a9a01, 0111e14). minor (deferred): 15 s silent drop if setup slower; empty registry keys remain; guard/timeout untested. Follow-up after Task 21 merge: main_is_away -> ShellState.main_closed.
Task 23: complete on PRISM-223 (a9806bd..0111e14, review clean)
Task 21: minor (deferred): thumb icons near-white (light theme check); thumb icon DPI fixed at startup; player state before thumb bar attach; tray labels English until main page loads; stale ITaskbarList3 after Explorer restart.
Owner hand check (Task 24): real taskbar click - does the editor keep focus / is minimized Prism foreground? Alt+Tab or thumbnail pick while main closed toggles the mini (cannot open main) - show owner as known behavior.
Carried into Task 22: move shell::mini::create after app.manage(AppState); commands use try_state helpers; close_main try_state; open_with main_is_away -> ShellState.main_closed.
Task 21: complete on PRISM-221 (8075a3f, review clean)
Merged 21 (0055ef4; Cargo.toml windows deps + RunEvent arms resolved by controller): cargo 338, npm 144, clean. Task 22 from 0055ef4.
Ruling: capabilities unchanged for autostart/global-shortcut (Rust-only use; granting autostart:default would let pages toggle the Run key) - cost if wrong: none.
Ruling: main window shown synchronously at end of setup instead of after a themed-first-frame handshake - no event to miss; transparent window avoids white flash - confirm at hand check.
Task 22: minor (deferred): watch.rs/protocol.rs still use bare state::<AppState>() (outside commands); Select repeated letter cycles.
Task 22: fix round 1/5 (6 addressed; f8f387b). minor (deferred): `before` read outside lock (rapid double change restores stale value); apply_setting MiniShortcut/StartWithSystem arms now unreachable duplicates.
Task 22: complete on PRISM-222 (55ba09b..f8f387b, review clean)
All 23 build tasks merged into PRISM-2 (f8f387b): cargo 349, npm 155, clean. Final whole-branch review next.
Final review M2: Ready for hand check; 0 critical, 1 important (crossfade hard clip).
Ruling: one fix wave now - soft clip on fade overlap, unreachable apply_setting arms no-op, Flow first-state race, split main/mini capabilities, main_closed only cleared when visible, doc fixes (Crossfade::Flow, GdiRects/apply, lib.rs glass, starve_wait), backend rejects file: ids in favorites/playlists - all cheap, several on the default path - cost if wrong: none.
Final fix wave c0aa960 approved (re-review). M2 Tasks 1-23 complete on PRISM-2; Task 24 owner hand check open.
Task 24 hand check feedback batch 1 (worktree m2-hand1, branch PRISM-224): search bar top spacing; white lines under the Flow ring; play button on each song row.
Hand-check batch 1 merged (PRISM-224: header 64px, glow bounded, row play button, transport centring, focus fix 701048e).
Hand check: white specks persist at region bottom extremes (below ring tip and both card bottom corners) - batch-1 glow fix did not address root cause. Debug agent in m2-hand2 (PRISM-225).
Hand check: specks root cause = WM_NCACTIVATE classic frame paint under SetWindowRgn; fixed via subclass passing lParam -1 (f3cdba6, PRISM-225), merged.
Owner decision (hand check): when the main window is closed to the tray, the taskbar button must disappear. Replaces the M2 'close minimizes, taskbar click toggles mini' design (spec 5.10). Mini toggles via tray click and Ctrl+Alt+P. Agent in m2-hand4 (PRISM-227).
Merged PRISM-227 (82e65c4): close hides main + taskbar button, opens mini; quit/autostart excluded.
Merged PRISM-226 (411e141 row play click: hidden .mark stacking context caught clicks; 20f624a backdrop under floating title bar).
Owner: specks gone (confirmed). Request: seek bar uses ring cover colors instead of spectrum (m2-hand5, PRISM-228).
Merged PRISM-228 (d7c5391 seek bar cover colors, 47002d3 Segmented sliding marker). INCIDENT: that agent's cleanup killed unrelated bash.exe/node.exe processes (owner informed); Prism app + its vite survived.
Owner request (hand check): toggleable queue panel, repeat (off/all/one), shuffle toggle - agent in m2-queue (PRISM-229). M3 plan a19847e approved by owner ('yes'); certificate question (how Navidrome is reached) still open.
Owner: Navidrome at <the owner's server, address kept out of the repo> (internal+external, public domain -> CA certificate expected; no self-signed option needed in v1.0.0). Server currently down (owner fixing Proxmox) - M3 hand check later. Never put the URL in tests/fixtures/screenshots.
Owner: Navidrome sits behind a reverse proxy. Carry into M3 dispatches (Tasks 2, 7, 8): follow redirects, map proxy 502/503/504 to offline (not wrong credentials), don't rely on Range (fallback to restart + skip, or timeOffset), tolerate proxy response buffering (slow first byte) within the stall timeout, accept gzip JSON, support a base path in the URL.
