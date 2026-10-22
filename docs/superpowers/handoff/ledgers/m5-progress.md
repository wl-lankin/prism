# M5 YouTube Music - SDD ledger

Plan: docs/superpowers/plans/2026-10-21-prism-m5-youtube-music.md. Branch PRISM-5 cut from PRISM-4 at f63ebdb, worked in the controller worktree ~/Projects/Prism-wt/m5 (the main checkout stays on PRISM-4 while the owner tests radio in a running dev app).

Machine: the owner's Mac. No CDP visual checks on macOS. Agents never open music.youtube.com or Google sign-in.

## Waves
- A: Tasks 1, 2, 3 in parallel.
- B: Task 4 (after 2, 3), Task 7 (after 1).
- C: Tasks 5, 6, 8.
- D: Tasks 9, 10. E: Task 11 (integration). Then 12 (owner hand check, Mac and Windows).
- After Task 1 merges, the controller runs the app once and opens every view, Settings and the mini player (a missing permission only shows at runtime).

## Progress
- [x] Task 1: approved, hardening 184c373 (capabilities bound by `webviews`, stricter scan). Merged 9f8ad4e, PRISM-4 synced 6acb6eb. cargo 862, npm 366, all clean. Runtime click-through (main 70 / mini 20 commands) not yet done: the owner's running dev app is PRISM-4; the M5 hand check covers it (checklist in the Task 1 review: every view, Settings, mini player, plus "not allowed by ACL" negatives in the Web Inspector).
- [ ] Task 4: m5-t04 / PRISM-504, running (Opus).
- [x] Task 7: approved, fix round 9a94bcc (`player.active` is the only active-player source; no ytm_hide before the first show). Merged 5970c32. npm 388.
- [x] Task 8: approved after fix round 1 (3fd781c: honest still label, edge-to-edge view, hint dismissal on Sign in, focus after sign-out). Merged 3e07f32. npm 397.
- [x] Task 4: approved, round 2 (7c5fb23 merge with the TS YtmState fix; 2073061 token bucket 20/s burst 10 that never drops a playing change, artwork length after parse, ordered emits). Merged 98e10a3. cargo 874, npm 397.
- [x] Task 6: approved (638922a; Windows code checked with `cargo check --target x86_64-pc-windows-gnu`). Merged b75a549.
- Controller 7731e93: overlays no longer import the ytm store (a listener YouTubeView sets while mounted), so the mini page never reaches ytm_show/ytm_hide; the window:: mini scan is green again.
- [x] Task 5: approved after fix round 1 (4e7bc47) and re-review. Merged 79f07c3; controller a99db80 (sign-out clears through the sign-in window when it is the only one). PRISM-4 synced 4c94786. cargo 899, npm 426.
- [x] Task 10: approved (f3742d9: fake page and server, debug-only override read only under #[cfg(debug_assertions)], runtime capability via add_capability scoped to the exact dev origin, decide_view). Merged 7d596ea.
- Owner check 2026-10-22 on the Mac against the fake page (PRISM-5 at 7d596ea, dev5): view placement and resize, play and Prism's strip, controls (strip, media keys, mini), exclusive playback with radio, overlays (Cmd+K, menus, About), close to mini and back, every view, Settings and the mini player (capability split): "geht alles".
- [x] Task 11: approved after fix round 1 (cb989f5). Merged 04f5ef7.
- Owner hand check on the Mac against the real music.youtube.com (2026-10-22), with fixes:
  - YouTube Music refused WKWebView ("nicht für deinen Browser optimiert"): c679de2 sends the installed Safari's user agent on macOS.
  - Sign-in ended on "Etwas ist schiefgelaufen" until a second try: Google sets cookies on country domains (accounts.google.de); 4f433ae lets Google country domains stay in the sign-in window. Sign-in then worked.
  - Position stuttered (1 s reports): 4f433ae interpolates the drawn position every 100 ms.
  - YouTube's own player bar duplicated Prism's: hidden by bridge CSS (4f433ae), buttons still used invisibly.
  - The view covered the seek line: f2f9ca7/4954ac8 (view ends 3 px above the transport).
  - Owner asked for the YouTube queue in Prism: 3a73072 + 7e6e3e1 (bridge reads ytmusic-player-queue, ytm_queue / ytm_queue_play), reviewed and approved, merged afd5055. Not yet confirmed on the real site.
  - Dropdowns inside YouTube are cut at the native view's edge: a platform limit, accepted.
  - The queue panel over the YouTube view shows the blurred still (overlay rule): owner accepted it as uniform.
  - App-wide fixes found on the way (PRISM-4, synced): the card scrolled by script on first view open (overflow: clip on html, body, .card, .content, c7a767f); sidebar scrollbar gutter; backdrop paint containment; illustration family; mini player top right on the Mac.
- Still open for M5: the Windows hand check (WebView2 sign-in, no second SMTC entry, data dir removal, taskbar buttons), the YouTube queue on the real site, the macOS second Now Playing entry question.
- [ ] Task 11 (old line): m5-t11 / PRISM-511. Run the fake page: `PRISM_FAKE_SERVER_MINUTES=30 cargo test --manifest-path src-tauri/Cargo.toml ytm::testpage::serve_for_ui -- --exact --ignored --nocapture`, then the dev app with `PRISM_YTM_URL=http://127.0.0.1:<port>/`.
- Was Task 5 review round 1: ba65f19. FIX NEEDED: (1) after add_child `get_webview_window("main")` is None (main_show, on_main_event, open_with break) -> use Window; (2) `prism://` protocol answered the remote page -> main/mini labels only; (3) own WKWebsiteDataStore for YouTube Music on macOS 14+ (controller decision, amends the plan's Data store line). Plus autostart hand-offs, Windows data-dir marker, `local: false`, consent.google.com, LAN names blocked. Fix round 1 running after merging PRISM-5.
- Windows checks on this Mac: `cargo check --target x86_64-pc-windows-gnu --all-targets` works (mingw installed), target dir in the scratchpad `wintarget`.
- [x] Task 9: approved after fix round 1 (6965487: notice overlay tracks a boolean, so a replaced notice does not flicker the view; queue heading; alt). Merged ee16fcc. npm 415. It also wired Transport into lib.rs (AppState.ytm/.transport, late_sink); Task 6 still routes player_*, tray, media keys and the ring. YtmState gained canNext/canPrev: the TS type from Task 7 must get them at merge.
- [x] Task 3: approved after fix round 1 (7053aa7: pause re-armed per Prism start, dead page hands control back, start status seeded, `on_active_changed`). Merged into PRISM-5 with PRISM-4 synced (ef8741c). cargo 857 passed.
- [x] Task 2: approved after fix round 1 (7e7a423: controls only inside `ytmusic-player-bar`, video `ytmusic-player video` with `#movie_player video` fallback, guarded callbacks, block-wrapper spec). Merged 81da454. npm 366 passed.
- [ ] Task 3: a75583d. Review FIX NEEDED: re-arm the page pause per Prism start; Idle/Unavailable hands active back to Prism; plus an active-changed notification. Fix round 1 running. YtmState/YtmAction/YtmControl live in `ytm/state.rs` (`ytm/mod.rs` = `pub mod state;`).
- Synced PRISM-4 (radio hand-check fixes up to 4505bf1) into PRISM-5 at 0917847.

## Rulings
- One cargo test filter per run (M4 ruling).

## Binding for later tasks
- Tasks 3/4 (from Task 2 review): the page can bypass or spoof the bridge (call `ytm_report` directly, fake fields). Rust trusts no report field; Transport takes over only on a real false-to-true playing edge, with a short debounce, so a flapping page cannot pause Prism over and over.
- Task 5 (from Task 1 review), mandatory: in `on_navigation` and `on_new_window` of the `ytm` child, block every scheme except https (tauri:, ipc:, asset:, custom protocols, http: incl. localhost:1440 / tauri.localhost / ipc.localhost, file:, data:, blob:, javascript:) and any `*.localhost` / `tauri.localhost` https host; never open those externally. Child label exactly `ytm`; sign-in window `ytm-signin`. Test `policy_blocks_app_origins` (tauri://localhost/, http://localhost:1440/, https://tauri.localhost/, ipc://localhost/).
- Task 6 (from Task 7): add a `transport_active` command (initial active player for windows that load later) and emit `transport://active`; the frontend stores start at 'prism' otherwise.
- Tasks 8/9 (from Task 7): call `initYtm()` in the main window and in the mini player (main done in Task 8; mini still open).
- Task 9 (from Task 8 review): the plan was wrong that notices sit below the content edge: `.notice` sits 104 px above the card's bottom edge, inside the content area, so it registers as an overlay while its text shows (or moves below the content edge while the YouTube view is active). The QueuePanel (absolute, over the content) and the drag-and-drop `.drop` overlay register as overlays too. Overlays registered from an effect wrap open/close in `untrack`.
- Task 7: IPC argument names `rect` (ytm_show) and `action` (ytm_control).
- Task 10: the fake page on 127.0.0.1 is a remote origin; it needs its own debug-only capability.
- Task 4 (from Task 3): call `Transport::on_ytm_state` for every report, heartbeats included (the debounce needs them); `YtmControl::send` must not block and must not call back into Transport.
- Task 5 (from Task 3 review): sign-out and view destroy feed `on_ytm_state(&YtmState::default())` so control falls back to Prism.
- Task 6: wiring sketch in task3-report.md (OnceLock slot plus `late_sink` in the player's fan_out); mirror `active` through the new notification as `transport://active`.
- Task 4/10: Rust wraps the bridge as `{ const ALLOW_DEV_HOST = <bool>; <bridge.js> }`.
- Task 10 / Task 12: the fake page wraps its <video> in `ytmusic-player` and its buttons in `ytmusic-player-bar`; the hand check confirms the scoped selectors on the real site.

## Deferred minors

## Incidents
