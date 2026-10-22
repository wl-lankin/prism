# Prism M5 (YouTube Music) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** YouTube Music lives inside Prism's main window. Selecting "YouTube Music" in the sidebar shows music.youtube.com as a child web view in the content area; navigating away hides it without stopping playback. The user signs in through Google's own sign-in page in a separate window that shares the session. Prism reads what YouTube Music plays and drives play/pause, next, previous, seek and volume, so the transport strip, Now Playing, the mini player, media keys, the OS media overlay and the Flow ring control it. Only one thing plays at a time: Prism's engine pauses when YouTube Music starts, and the other way round.

**Architecture:** A new `src-tauri/src/ytm/` module owns the child web view (`Window::add_child`, Tauri feature `unstable`), the sign-in window, the bridge and the state. The bridge is a small script (`ytm/bridge.js`) injected with `initialization_script` into music.youtube.com only. It watches `navigator.mediaSession`, the `<video>` element and the player bar, and reports through one app command, `ytm_report`, the only IPC the remote page gets (a capability with `remote.urls` for `https://music.youtube.com/*` and one permission). Commands into the page go through `Webview::eval` with fixed, parameterless snippets. A new `shell::transport` layer decides which player the transport targets (`Active::Prism` or `Active::Ytm`, the last one that started) and enforces exclusive playback. Because a remote page gets IPC for the first time, the app gets an `AppManifest` command list (the M6 hardening item, done now): main and mini capabilities list the app commands they use, the YouTube Music capability lists `ytm_report` only.

**Tech Stack:** M4 stack. Tauri gains the `unstable` feature (child web views). No new crates.

**Spec:** `docs/superpowers/specs/2026-10-20-prism-design.md` (5.3, 5.6, 5.9, 5.10, 5.12, 5.13). Spike: `docs/spikes/ytm-signin.md` (Windows: direct sign-in inside the view fails with "Bei deiner Anmeldung in YouTube ist ein Problem aufgetreten"; signing in on accounts.google.com in its own window with the same data directory works and the session carries over; mediaSession metadata, `#play-pause-button`, `.next-button` and `<video>` work). Previous plan and constraints: `docs/superpowers/plans/2026-10-21-prism-m4-radio.md`. Ledgers: `docs/superpowers/handoff/ledgers/`.

## Global Constraints

- Everything in the M4 Global Constraints holds (platforms, repo and branches, identity, commit format `PRISM-5: <Description>`, no em or en dash, no AI attribution, no sparkle glyphs, palette and source colors (youtube `#ffc94d`), reduced motion, window look and close-to-mini, the mini player never steals focus, dev runs with `PRISM_DATA_DIR` and an identifier override, no OS input injection, ports, Write/Edit only, append-only migrations, EN and DE strings, fake data only).
- Branch `PRISM-5`, cut from `PRISM-4`. Task branches `PRISM-5NN` in worktrees under `~/Projects/Prism-wt/`.
- M5 adds no migration (YouTube Music keeps no rows in Prism's DB; spec 5.6: its tracks never enter the queue, playlists, Home, search, favorites or history).
- Network: M5 adds music.youtube.com and the hosts it loads itself (inside its own web view), accounts.google.com in the sign-in window, and artwork from `*.googleusercontent.com`, `*.ggpht.com` and `i.ytimg.com` (fetched by Prism's cover gate, https only, the M4 private-address refusal applies). Nothing else.
- Credentials: Prism never sees or stores the Google password. The session lives in the web view's own cookie store: on Windows a separate WebView2 data directory `<data>/ytm-webview`; on macOS 14+ a WKWebsiteDataStore of its own (`view::YTM_STORE`), below macOS 14 the app's default WKWebsiteDataStore. Prism never reads cookies.
- The remote page is untrusted: it gets exactly one command, `ytm_report`, whose payload is size-capped (16 KiB; a queue report 512 KiB, Task 13), schema-checked and stripped of control characters; strings are capped (title, artist, album 256 chars; artwork URLs 2 048 bytes and https only). It never gets core, window, event, dialog or opener permissions. Script sent into the page is a fixed set of constant snippets; no value from the page is ever interpolated into script.
- Navigation inside the YouTube Music view stays on `music.youtube.com`, `consent.youtube.com`, `www.youtube.com` (only paths YouTube Music itself redirects through) and `accounts.google.com`/`accounts.youtube.com`; any other link opens in the system browser through the Rust opener (http(s) only) and the view does not navigate. `window.open` is handled the same way.
- Agents never sign in to Google, never type credentials, and never open music.youtube.com in tests. Tests use a local fake page (`127.0.0.1:0`) that mimics the player DOM and mediaSession.
- On macOS, WKWebView has no CDP: every visual and sign-in check goes to the owner hand check (Task 12). On Windows the owner checks too.

## Review Focus

1. **The remote page can do nothing but report.** A hostile or compromised music.youtube.com script must not reach any other Prism command, read files, open windows, or inject script back. Pinned in Task 1 (`ytm_capability_has_only_ytm_report`, `every_command_is_in_the_manifest`) and Task 4 (`report_payload_is_capped_and_validated`, `eval_snippets_are_constants`).
2. **Exclusive playback without fights.** Prism starts a track while YouTube Music plays: YouTube Music pauses, once. YouTube Music starts while Prism plays: Prism pauses (radio included), once. No ping-pong when both report at the same moment, no pause of a player that is already paused. Pinned in Task 3 (`ytm_start_pauses_prism`, `prism_start_pauses_ytm`, `simultaneous_starts_settle_on_the_last`).
3. **Hidden, not destroyed.** Leaving the YouTube Music view, closing the main window to the mini player, or opening an overlay hides the child view; playback goes on and controls keep working. Re-entering shows the same page at the same place. Pinned in Task 5 (`hide_keeps_the_webview`, `main_close_keeps_ytm_playing`) and Task 9.
4. **Overlays above a native view.** The child view is a native view on top of the main page: the search palette, context menus, dialogs, the About overlay and notices that overlap the content area would be hidden under it. Every such overlay hides the child view while open (and shows a calm placeholder in its place). Pinned in Task 9 (`overlay_hides_the_ytm_view`, `last_overlay_closing_shows_it_again`).
5. **Selectors break.** YouTube Music changes its DOM. A missing element or a changed mediaSession must leave Prism in a clear "controls unavailable" state, never a crash, stuck "playing", or a loop of eval calls. Pinned in Task 4 (`missing_buttons_report_unavailable`) and Task 2 (`bridge_survives_a_dom_without_the_player_bar`).

---

## File Structure

```
src-tauri/build.rs                      AppManifest with the app command list
src-tauri/capabilities/ytm.json         remote music.youtube.com: ytm_report only
src-tauri/src/ytm/mod.rs                YtmService: state, report handling, commands into the page
src-tauri/src/ytm/view.rs               child web view: create, bounds, show, hide, navigation policy
src-tauri/src/ytm/signin.rs             sign-in window (accounts.google.com), session carry-over
src-tauri/src/ytm/bridge.js             the injected script (watch and report, act on commands)
src-tauri/src/ytm/report.rs             report schema, validation, caps
src-tauri/src/ytm/covers.rs             artwork for the resize gate (yt- covers)
src-tauri/src/ytm/state.rs              YtmState, YtmAction, YtmControl (as built, Task 3)
src-tauri/src/ytm/policy.rs             navigation policy (as built, Task 5)
src-tauri/src/ytm/dev.rs                debug-only fake page override and its capability (as built, Task 10)
src-tauri/src/ytm/testpage/             #[cfg(test)] fake YouTube Music page (HTML + JS) and server
src-tauri/src/shell/transport.rs        Active player, routing, exclusive playback
src/lib/stores/ytm.svelte.ts            state store, view rect reporting, overlay registry hook
src/lib/stores/overlays.svelte.ts       one registry of open overlays (palette, menus, dialogs)
src/lib/components/YouTubeView.svelte   the placeholder the child view sits on, sign-in states
src/lib/ytm-bridge.spec.ts              bridge tests in jsdom (fake DOM and mediaSession)
```

Modified: `Cargo.toml` (tauri `unstable`), `lib.rs`, `commands.rs`, `player/{engine,mod}.rs` (`Pause`), `media_controls.rs`, `thumbs.rs` (no change expected beyond a new prefix), `shell/{mod,tray,mini}.rs`, `capabilities/{default,mini}.json`, `settings.rs`; frontend `types.ts`, `ipc.ts`, `i18n.ts`, `stores/{nav,player}`, `Sidebar`, `App.svelte`, `TransportStrip`, `NowPlaying`, `MiniPlayer`, `FlowRing`, `QueuePanel`, `SearchPalette`, `TrackMenu`, `About`, `Settings`.

## Decisions pinned here

- **One view, created lazily.** The child web view (label `ytm`) is created the first time the user opens the YouTube Music view, never at app start. It loads `https://music.youtube.com/`. It is hidden (not destroyed) when the user navigates away, when the main window closes to the mini player, and while an overlay is open. It is destroyed only on sign-out and on quit. `background_throttling(Disabled)` so playback and reports go on while hidden.
- **Bounds from the page.** The YouTube Music view renders an empty placeholder element where the page goes (the content area right of the sidebar, below the title bar, above the transport strip). A ResizeObserver plus window resize sends its rect in logical pixels (`ytm_show({ x, y, width, height })`), throttled to one per animation frame; Rust sets the child's bounds (clamped to the main window's inner size). The placeholder fills the content area edge to edge (no padding, radius or inset; only the dismissible sign-in hint row sits above it). The main page keeps its own layout; the child never covers the sidebar, the title bar, the transport strip or the Flow ring.
- **Data store.** Windows: the child view uses its own data directory `<data>/ytm-webview` with `additional_browser_args("--disable-features=HardwareMediaKeyHandling")`, so WebView2 neither grabs the media keys nor shows its own SMTC entry (Prism's bridge shows YouTube Music in SMTC instead); a separate directory is required because browser args must match per data directory. macOS 14+: a WKWebsiteDataStore of its own (`data_store_identifier(YTM_STORE)`, a fixed UUID) for the view and the sign-in window, so sign-out never touches the main page's storage; sign-out clears it and then removes it (`remove_data_store`). Below macOS 14 the identifier is ignored and both use the default store, shared with the main page (sign-out then clears that store). A store that cannot be removed at sign-out (tried twice, 500 ms apart) leaves `<data>/ytm-webview.remove`; the store (Windows: the directory) is then removed under the view's creation lock before the view or the sign-in window is created (at the next start in the background, or on their creation, whichever comes first, and only while neither exists), and the marker goes with it, even if that removal fails again, so it never outlives a new session (controller ruling in the Task 5 review; built in Tasks 5 and 11).
- **Sign-in.** The view shows YouTube Music's own "Sign in" button; Prism intercepts navigations from the child to `accounts.google.com` and opens them in a separate window `ytm-signin` (900x700, same data store, normal decorations, the stock user agent), never inside the child (the spike's direct route fails). When the sign-in window navigates to `music.youtube.com`, Prism closes it and reloads the child view. Settings offers "Sign in" (opens the window directly at `https://accounts.google.com/ServiceLogin?service=youtube&continue=https://music.youtube.com/`) and "Sign out" (`clear_all_browsing_data` on the child, or on the sign-in window when it is the only one open; close the sign-in window and destroy the child; reset the state to idle so the transport falls back to Prism; then remove the session's store, see Data store). Not every accounts.google.com URL goes to the sign-in window: WKWebView asks for every frame, so background paths (`/RotateCookiesPage`, `/CheckCookie`, `/ListAccounts`, `/Logout`, `/o/oauth2/`) stay in the view, and a hand-off (sign-in window or system browser) happens only while the view is visible, at most once per second. The sign-in window finishes on page-load Started of `music.youtube.com`.
- **The bridge.** `bridge.js` runs only when `location.host === 'music.youtube.com'` and only in the top frame. It reports `{ v: 1, kind: 'state', playing, title, artist, album, artwork, positionMs, durationMs, canNext, canPrev, available }` on change and every 1 s while playing, plus `{ kind: 'ended' }`. It never reports on every timeupdate. It listens for commands on a `window` CustomEvent `prism-ytm` dispatched by fixed eval snippets: `toggle`, `play`, `pause`, `next`, `prev`, `seek` (position from a separate fixed snippet that reads `window.__prismSeekMs`, set by Rust with a number literal formatted by Rust, never a string), `volume` (same pattern, 0..=1). Selectors live in one table at the top of the file: `#play-pause-button`, `.next-button`, `.previous-button`, `video`, `ytmusic-player-bar`; failing ones set `available: false`. Task 13 adds the queue: `{ v: 1, kind: 'queue', items: [{ title, byline, thumb, current }], hasQueue }` read only inside `ytmusic-player-queue` (at most 200 entries, the tail left out past 511 KiB), sent on start and on change, at most one per 500 ms (MutationObserver on the queue element, re-attached when replaced); the fixed `queue` action clicks entry `window.__prismQueueIndex` (an integer literal set by Rust).
- **IPC for the page.** `capabilities/ytm.json`: `webviews: ["ytm"]`, `remote: { urls: ["https://music.youtube.com/*"] }`, `permissions: ["allow-ytm-report"]`. `build.rs` declares every app command in an `AppManifest`; `default.json` (main) and `mini.json` list the commands each uses (the mini player loses the Navidrome, radio-management and settings commands it never calls). A test compares the `generate_handler!` list with the manifest and the capabilities. Both capabilities are bound by `webviews` (not `windows`), so the `ytm` child inside window `main` never gets main's. The mini player gets `ytm_state` and `transport_active` but no view command (`ytm_show`, `ytm_hide`, `ytm_sign_in`, `ytm_sign_out`). There is no `ytm_control`: every transport action reaches the page through `Transport` (Task 11 removed the direct command). Debug dev runs add a runtime capability `ytm-dev` for the fake page's exact origin with the same permissions as `ytm.json` (Task 10).
- **Active player and exclusivity.** `shell::transport::Transport` holds `Active::{Prism, Ytm}`. Prism becomes active when its engine reports `Loading` or `Playing` from a user action; YouTube Music becomes active when the bridge reports `playing: true` after `playing: false`. Becoming active pauses the other one if it plays (Prism through the new `PlayerHandle::pause()`, YouTube Music through the `pause` snippet). Reports caused by Prism's own pause are recognized by a 2 s "expected" window and never flip the active player back. Transport commands (`player_toggle`, `player_next`, `player_prev`, `player_seek`, `player_set_volume`, media keys, tray, thumbnail buttons, mini player, Flow ring play/pause) go to the active player. Media keys and the volume read the active player once (`Transport::route_with`, `set_volume` answers the player it reached), so a flip in between cannot mis-route a key or save the page's volume as Prism's. An Idle or Unavailable page hands the transport back to Prism. `transport_active` and the `transport://active` event mirror `Active` to the main and the mini page. An idle click on the Flow ring always starts Flow (Prism), which pauses YouTube Music.
- **What Prism shows for YouTube Music.** A `YtmState` (`status: idle|playing|paused|unavailable`, title, artist, album, artwork cover id `yt-<sha1(url) 20 hex>`, position, duration, `canNext`, `canPrev`) is emitted as `ytm://state`; next and previous are enabled from `canNext`/`canPrev`. While YouTube Music is active, the transport strip, Now Playing and the mini player show it with the source label "YouTube Music" (amber), a seek bar when the duration is known, next and previous, no heart, YouTube Music's own queue (Task 13: `YtmQueue` as `ytm://queue` and `ytm_queue`, rows in the queue panel and Now Playing, a click or Enter plays an entry through `ytm_queue_play`, main only; without the page's queue element a note asks to open the player once), no "Add to playlist". Volume changes go to the page's `<video>`. The Flow ring shows play/pause and progress with a steady glow (no level meter for web audio).
- **Media controls.** While YouTube Music is active, MediaBridge shows its metadata and position and routes the OS buttons through `Transport`. On macOS, if WKWebView publishes its own Now Playing entry as well, the owner hand check notes it; there is no switch for it in WKWebView's public API.
- **Artwork.** The `yt-` prefix gets a `YtmCovers` remote in the resize gate: the URL behind the id comes from the last report (an in-memory map, last 512 since Task 13, room for a full queue's thumbnails), https only, hosts `*.googleusercontent.com`, `*.ggpht.com`, `i.ytimg.com`; caps as M4 favicons (1 MiB, 10 s, decode check).
- **Overlays.** `stores/overlays.svelte.ts` keeps a set of open overlay ids. The search palette, every context menu, the About overlay, confirm dialogs, the Flow menu, the queue panel, the drag-and-drop `.drop` overlay and the notice above the Flow ring (it sits inside the content area, 104 px above its bottom edge) register while open. While the set is not empty and the YouTube Music view is shown, the child is hidden and the placeholder shows a blurred, dimmed still (the last artwork or a neutral amber gradient). The registry tells the view through a listener that `YouTubeView` sets while mounted, so the mini page never reaches the view commands.
- **Sidebar and naming.** The sidebar gets "YouTube Music" under Sources with the amber dot, between Navidrome and Radio. The view title is "YouTube Music". Settings gets a "YouTube Music" section (signed-in state as far as the bridge can tell from the page's avatar button, Sign in, Sign out). As built, the bridge does not report the sign-in state, so the status line always reads "Sign-in status unknown" (open for M6).

---

### Task 1: AppManifest, capabilities and the ytm capability

**Files:**
- Modify: `src-tauri/build.rs`, `src-tauri/capabilities/default.json`, `src-tauri/capabilities/mini.json`, `src-tauri/src/window.rs` (tests), `src-tauri/Cargo.toml`
- Create: `src-tauri/capabilities/ytm.json`

**Interfaces:**
- Produces:
  - `build.rs`: `tauri_build::try_build(Attributes::new().app_manifest(AppManifest::new().commands(&[ ...every command in lib.rs generate_handler!..., "ytm_report", the M5 ytm_* commands ])))`.
  - `default.json` (main): its current permissions plus `allow-<cmd>` for every app command the main page calls.
  - `mini.json`: `core:default` plus the app commands the mini page calls (player, favorites read and set, radio_now_playing, radio_favorite, ytm_state, ~~ytm_control~~, main_show, settings read, the ones `MiniPlayer.svelte` and its stores import; find them by reading the mini entry's imports). As built: the mini player also keeps `settings_set` (repeat and shuffle), `navidrome_status`, `radio_saved`, `library_genres` and `transport_active`; `ytm_control` was removed in Task 11.
  - `ytm.json` as in Decisions (`webviews: ["ytm"]`, `remote.urls`, `permissions: ["allow-ytm-report"]`).
  - Cargo: tauri features add `unstable`.
  - Placeholder commands `ytm_report`, `ytm_show`, `ytm_hide`, `ytm_control`, `ytm_state`, `ytm_sign_in`, `ytm_sign_out` exist in `commands.rs` returning `Err("not yet")` so the manifest and handler list are complete; Tasks 4 and 5 fill them.
- [ ] **Step 1: Write the failing tests** (`window.rs` tests, reading the files with `include_str!` and parsing JSON): `every_command_is_in_the_manifest` (the command names in `lib.rs`'s `generate_handler!` block equal the `build.rs` list), `main_capability_covers_the_main_page` (every `invoke('<name>'` in `src/` files reachable from the main entry is allowed in `default.json`), `mini_capability_covers_the_mini_page` (same for the mini entry), `mini_has_no_settings_or_navidrome_commands`, `ytm_capability_has_only_ytm_report` (remote urls exactly `https://music.youtube.com/*`, webviews exactly `ytm`, permissions exactly `allow-ytm-report`).
- [ ] **Step 2: Run** `cargo test --manifest-path src-tauri/Cargo.toml window::` -> FAIL.
- [ ] **Step 3: Implement.** Then build the app (`cargo build`) to prove the capabilities resolve.
- [ ] **Step 4: Run** full `cargo test`, clippy, fmt, `npm test`, `npm run check` -> PASS.
- [ ] **Step 5: Commit** `PRISM-5: App command manifest, per-window capabilities and the YouTube Music capability`.

### Task 2: The bridge script

**Files:**
- Create: `src-tauri/src/ytm/bridge.js`, `src/lib/ytm-bridge.spec.ts`

**Interfaces:**
- Produces `bridge.js` per Decisions: an IIFE guarded by `location.host === 'music.youtube.com' && window.top === window && !window.__prismBridge`; a selector table; `report(payload)` through `window.__TAURI_INTERNALS__.invoke('ytm_report', { report })` (absent -> no-op); state diffing so unchanged states are not resent except the 1 s heartbeat while playing; a MutationObserver on `ytmusic-player-bar` (re-attached when the bar is replaced) plus `play`, `pause`, `ended`, `durationchange`, `loadedmetadata` listeners on the current `<video>` (re-attached when YouTube swaps it); the `prism-ytm` command listener; `available: false` when the bar or the buttons are missing. No network requests of its own, no globals besides `__prismBridge`.
- The spec loads the file text (Vite `?raw` import) and runs it in jsdom with a fake `location` (`music.youtube.com`), a fake `__TAURI_INTERNALS__.invoke` recorder, a fake `navigator.mediaSession`, and a fake player DOM.
- [ ] **Step 1: Write the failing tests:** `reports_metadata_and_play_state`, `heartbeat_only_while_playing`, `unchanged_state_is_not_resent`, `toggle_clicks_play_pause`, `next_and_prev_click_their_buttons`, `seek_and_volume_read_the_numbers_rust_set`, `does_nothing_on_other_hosts`, `does_nothing_in_frames`, `runs_once`, `bridge_survives_a_dom_without_the_player_bar` (available false, no throw), `video_swap_is_followed`, `strings_are_capped`.
- [ ] **Step 2: Run** `npm test -- ytm-bridge` -> FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** `npm test`, `npm run check` -> PASS.
- [ ] **Step 5: Commit** `PRISM-5: YouTube Music bridge script that watches the player and takes commands`.

### Task 3: Transport routing and exclusive playback

**Files:**
- Create: `src-tauri/src/shell/transport.rs`
- Modify: `src-tauri/src/player/engine.rs`, `src-tauri/src/player/mod.rs` (`Pause`), `src-tauri/src/shell/mod.rs`

**Interfaces:**
- Produces:
  - `PlayerHandle::pause()` / `Player::pause()`: pauses when Playing (a live station is held as in M4), no-op otherwise; `Command::Pause`.
  - `pub trait YtmControl: Send + Sync { fn send(&self, action: YtmAction); }` with `enum YtmAction { Toggle, Play, Pause, Next, Prev, Seek(u64), Volume(f32) }`.
  - `pub struct Transport` with `new(player: PlayerHandle, ytm: Arc<dyn YtmControl>, clock)`, `active() -> Active`, `on_player_event(&PlayerEvent)`, `on_ytm_state(&YtmState)`, and `toggle()`, `next()`, `prev()`, `seek(ms)`, `set_volume(v)` routed to the active player. Exclusivity and the 2 s expected-pause window per Decisions. Flow start always targets Prism.
- [ ] **Step 1: Write the failing tests** (fake player handle and fake YtmControl, fake clock): `engine_pause_pauses_only_when_playing`, `pause_on_a_station_holds_it`, `ytm_start_pauses_prism`, `prism_start_pauses_ytm`, `own_pause_report_does_not_flip_active`, `simultaneous_starts_settle_on_the_last`, `commands_follow_the_active_player`, `flow_start_targets_prism`, `nothing_is_paused_twice`.
- [ ] **Step 2: Run** `cargo test --manifest-path src-tauri/Cargo.toml shell::transport`, then `player::` -> FAIL.
- [ ] **Step 3: Implement.** All M4 engine tests stay green.
- [ ] **Step 4: Run** `player::` 3x, full `cargo test`, clippy, fmt -> PASS.
- [ ] **Step 5: Commit** `PRISM-5: One transport for Prism and YouTube Music, with exclusive playback`.

### Task 4: YtmService, reports and commands into the page

**Files:**
- Create: `src-tauri/src/ytm/mod.rs`, `src-tauri/src/ytm/report.rs`, `src-tauri/src/ytm/covers.rs`
- Modify: `src-tauri/src/commands.rs` (`ytm_report`, `ytm_state`, `ytm_control`; Task 11 removed `ytm_control` again, the strip's seek goes through `player_seek`), `src-tauri/src/lib.rs`

**Interfaces:**
- Consumes: Task 2's report shape, Task 3's `YtmControl` and `Transport`.
- Produces:
  - `report::parse(value: serde_json::Value) -> Result<Report, ReportError>`: version 1 only, byte cap 16 KiB (checked on the serialized size), strings cleaned (control and format characters dropped, capped), artwork https only with an allowed host, numbers finite and clamped (position <= duration + 5 s).
  - `YtmService` with `state() -> YtmState`, `handle(report)`, `sink(EventSink-like)` for `ytm://state`, artwork URL map for `yt-` covers, and `impl YtmControl` that evals one of the fixed snippets (`const SNIPPETS: [(&str, &str)]`; seek and volume first eval `window.__prismSeekMs = <u64>` / `window.__prismVolume = <f32 formatted with {:.3}>`, then the fixed dispatch).
  - `ytm_report(webview: Webview, report: Value)`: refuses unless `webview.label() == "ytm"` and its URL host is `music.youtube.com`.
  - `YtmCovers` remote for the gate under `yt-`.
- [ ] **Step 1: Write the failing tests:** `report_payload_is_capped_and_validated` (17 KiB, wrong version, NaN position, `javascript:` artwork, an artwork on another host, control characters), `state_follows_reports`, `artwork_gets_a_stable_cover_id`, `eval_snippets_are_constants` (every snippet is a `const`, and seek/volume format only numbers: a test with `f32::NAN` and `u64::MAX` produces valid JS literals or nothing), `report_from_another_webview_is_refused`, `covers_only_from_allowed_hosts`, `missing_buttons_report_unavailable`.
- [ ] **Step 2: Run** `cargo test --manifest-path src-tauri/Cargo.toml ytm::` -> FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** filter 3x, full `cargo test`, clippy, fmt -> PASS.
- [ ] **Step 5: Commit** `PRISM-5: YouTube Music service: validated reports, state, commands and artwork`.

### Task 5: The child view and the sign-in window

**Files:**
- Create: `src-tauri/src/ytm/view.rs`, `src-tauri/src/ytm/signin.rs`, `src-tauri/src/ytm/testpage/` (fake page server for tests)
- Modify: `src-tauri/src/commands.rs` (`ytm_show`, `ytm_hide`, `ytm_sign_in`, `ytm_sign_out`), `src-tauri/src/shell/mod.rs` (hide on close-to-mini, destroy on quit), `src-tauri/src/lib.rs`

**Interfaces:**
- Produces:
  - `view::ensure(app, rect) -> Result<Webview>`: creates the `ytm` child on `main` with the bridge as `initialization_script`, `background_throttling(Disabled)`, the Windows data directory and browser args, `on_navigation` per the navigation policy (allowed hosts stay; accounts.google.com from the child opens the sign-in window and returns false; anything else opens in the system browser and returns false), `on_new_window` the same way.
  - `view::show(rect)`, `view::hide()`, `view::destroy()`; bounds in logical pixels clamped to the main window's inner size.
  - `signin::open(app)`: the `ytm-signin` window per Decisions; when it reaches `music.youtube.com` it closes and the child reloads. A second open focuses the existing window.
  - `sign_out`: clear browsing data, destroy the child, remove the Windows data directory after the view is gone (retry once after 500 ms; log on failure).
  - The navigation policy as a pure function `policy::decide(url: &Url, from: From) -> Decision` with `From::{View, SignIn}` and `Decision::{Stay, SignIn, External, Block}`.
- [ ] **Step 1: Write the failing tests:** `policy_keeps_youtube_music_hosts`, `policy_sends_google_accounts_to_the_sign_in_window`, `policy_opens_other_links_outside`, `policy_blocks_non_web_schemes`, `bounds_are_clamped`, `hide_keeps_the_webview` (state test on the service: hide marks hidden, no destroy), `main_close_keeps_ytm_playing` (close-to-mini hides the view and leaves the service active), `sign_out_forgets_the_view`. Window creation itself is not unit-tested (no app instance in tests); it goes to the owner check.
- [ ] **Step 2: Run** `cargo test --manifest-path src-tauri/Cargo.toml ytm::` -> FAIL.
- [ ] **Step 3: Implement.** Build the app (`cargo build`) to check the unstable APIs compile on macOS; Windows-only calls (`data_directory`, `additional_browser_args`) are behind `cfg(windows)`.
- [ ] **Step 4: Run** full `cargo test`, clippy, fmt -> PASS.
- [ ] **Step 5: Commit** `PRISM-5: YouTube Music view inside the main window and a Google sign-in window`.

### Task 6: Media controls, tray, mini player shell and the Flow ring backend

**Files:**
- Modify: `src-tauri/src/media_controls.rs`, `src-tauri/src/shell/{tray,mini,mod,taskbar_win}.rs`, `src-tauri/src/commands.rs` (player_* commands route through `Transport`), `src-tauri/src/lib.rs`

**Interfaces:**
- Consumes: Tasks 3, 4.
- Produces: every transport entry point (commands, tray menu, Windows thumbnail buttons, global shortcut actions, media keys) calls `Transport`; MediaBridge shows `YtmState` while YouTube Music is active (title, artist, album, artwork from the `yt-` cache, position and duration) and Prism's track otherwise; `ytm://state` reaches the mini window.
- [ ] **Step 1: Write the failing tests:** `media_metadata_follows_the_active_player` (pure mapping), `tray_toggle_goes_to_ytm_when_active` (through a fake transport), `thumb_buttons_follow_the_active_player`.
- [ ] **Step 2: Run** the filters -> FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** full `cargo test`, clippy, fmt -> PASS.
- [ ] **Step 5: Commit** `PRISM-5: Media keys, tray and taskbar buttons follow whichever player is active`.

### Task 7: Frontend store, IPC and the overlay registry

**Files:**
- Create: `src/lib/stores/ytm.svelte.ts`, `src/lib/stores/ytm.spec.ts`, `src/lib/stores/overlays.svelte.ts`, `src/lib/stores/overlays.spec.ts`
- Modify: `src/lib/ipc.ts`, `src/lib/types.ts`, `src/lib/stores/player.svelte.ts` (an `active` field mirrored from the backend)

**Interfaces:**
- Produces: `ytm` store (`state`, `active`, `shown`), `reportRect(rect)` (rAF-throttled `ytm_show`), `hideView()`, `control(action)`, `signIn()`, `signOut()`; `overlays` registry with `openOverlay(id)`, `closeOverlay(id)`, `anyOverlay` derived, and an effect that hides the child while any overlay is open and the view is shown.
- [ ] **Step 1: Write the failing tests:** `rect_reports_are_throttled_to_frames`, `overlay_hides_the_ytm_view`, `last_overlay_closing_shows_it_again`, `active_player_mirrors_the_backend`.
- [ ] **Step 2: Run** `npm test` -> FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** `npm test`, `npm run check` -> PASS.
- [ ] **Step 5: Commit** `PRISM-5: YouTube Music store, view rect reporting and one overlay registry`.

### Task 8: Sidebar entry, YouTube Music view and Settings section

**Files:**
- Create: `src/lib/components/YouTubeView.svelte`, `src/lib/components/youtube.spec.ts`
- Modify: `src/lib/components/Sidebar.svelte`, `src/lib/stores/nav.svelte.ts` (`'youtube'`), `src/App.svelte` (route), `src/lib/components/Settings.svelte`, `src/lib/components/settings.spec.ts`, `src/lib/i18n.ts`

**Interfaces:**
- Produces: the sidebar entry per Decisions; YouTubeView renders the placeholder (full content area, rounded like the cards, a subtle amber hairline while loading), reports its rect while mounted and hides the view on unmount; a first-open hint above it ("Sign in with your Google account to see your library") with a Sign in button that calls `signIn()`; the overlay still (blurred last artwork) while hidden for an overlay. Settings > YouTube Music: status line, Sign in, Sign out (with a confirm dialog that registers as an overlay).
- i18n keys (EN and DE): `source.youtube` (exists), `ytm.title`, `ytm.signInHint`, `ytm.signIn`, `ytm.signOut`, `ytm.signOutConfirm`, `ytm.unavailable`, `ytm.loading`, `settings.ytm`, `settings.ytm.status.signedIn`, `settings.ytm.status.signedOut`, `settings.ytm.status.unknown`.
- [ ] **Step 1: Write the failing tests:** `sidebar_shows_youtube_music`, `view_reports_its_rect_and_hides_on_leave`, `sign_in_button_calls_the_backend`, `settings_sign_out_asks_first`.
- [ ] **Step 2: Run** `npm test` -> FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** `npm test`, `npm run check` -> PASS.
- [ ] **Step 5: Commit** `PRISM-5: YouTube Music in the sidebar, its view and its settings`.

### Task 9: Overlays register, player surfaces show YouTube Music

**Files:**
- Modify: `SearchPalette`, `TrackMenu`, `StationMenu`, `FlowMenu`, `About`, confirm dialogs, `TransportStrip`, `NowPlaying`, `MiniPlayer`, `FlowRing`, `QueuePanel`, `format.ts`, `i18n.ts`, specs

**Interfaces:**
- Consumes: Task 7.
- Produces: every overlay calls `openOverlay`/`closeOverlay`; the player surfaces per Decisions (YouTube Music label in amber, seek bar when the duration is known, no heart, no queue entries, a queue panel note); the Flow ring progress for YouTube Music with a steady glow.
- i18n keys: `ytm.source`, `ytm.queueNote`, `ytm.noHeart`.
- [ ] **Step 1: Write the failing tests:** `each_overlay_registers_while_open`, `strip_shows_ytm_when_active`, `mini_shows_ytm_when_active`, `queue_panel_explains_ytm`, `ring_shows_ytm_progress`.
- [ ] **Step 2: Run** `npm test` -> FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** `npm test`, `npm run check` -> PASS.
- [ ] **Step 5: Commit** `PRISM-5: Overlays make room above YouTube Music, and the player shows what it plays`.

### Task 10: Fake YouTube Music page for dev runs

**Files:**
- Create: `src-tauri/src/ytm/testpage/{index.html, player.js, server.rs}`
- Modify: `src-tauri/src/ytm/view.rs` (debug-only `PRISM_YTM_URL` override that accepts `http://127.0.0.1:<port>/` only)

**Interfaces:**
- Produces: an `#[ignore]` test `ytm::testpage::serve_for_ui` serving a page with a fake player bar, `#play-pause-button`, `.next-button`, `.previous-button`, a `<video>` playing a local tone file, and mediaSession metadata for three fake songs; debug builds can point the child at it so the whole loop (reports, exclusivity, overlays) runs without Google. The bridge's host guard accepts `127.0.0.1` only in debug builds (the injected script gets a `const ALLOW_DEV_HOST = true|false` prefix from Rust). As built: the override lives in `ytm/dev.rs`, the fake page gets a runtime capability for its exact origin, and `ytm_report` accepts it only on that origin (`report_allowed(.., dev::page())`, Task 11).
- [ ] **Step 1: Write the failing tests:** `dev_url_override_accepts_loopback_only` (debug), `release_never_reads_the_override`, `bridge_prefix_matches_the_build`.
- [ ] **Step 2: Run** -> FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** full checks -> PASS.
- [ ] **Step 5: Commit** `PRISM-5: A fake YouTube Music page for dev runs`.

### Task 11: Integration pass

Unify what parallel UI tasks built twice, re-check the capability lists against the final command set, and run every check. Commit `PRISM-5: Integration pass for YouTube Music`.

### Task 12: Owner hand check (Mac and Windows)

- [ ] On the Mac, with a dev build the owner starts:
  - Open YouTube Music: the page sits exactly in the content area, resizes with the window, never covers the sidebar, strip or ring.
  - Sign in through the window: does Google allow it in WKWebView? Does the session carry into the view? (If Google refuses on the Mac, stop and decide before M6.)
  - Play a song: Prism's strip, Now Playing, the mini player, the media keys and Control Center show and control it. Note whether macOS shows a second Now Playing entry from the web view.
  - Start a local or radio track: YouTube Music pauses. Start YouTube Music again: Prism pauses.
  - Open Ctrl+K, a context menu, About: the view makes room and comes back.
  - Leave the view, close the main window to the mini player: music goes on, controls work. Come back: same page.
  - Sign out: the view forgets the account.
- [ ] On Windows the same list, plus: no second SMTC entry from WebView2, and the taskbar thumbnail buttons control YouTube Music.
- [ ] Record the outcome in the M5 ledger; fix in worktrees; then the M6 plan.

---

## Execution notes

- Waves: A = 1, 2, 3 (independent); B = 4, 7 (4 needs 2 and 3; 7 needs only the IPC names from Task 1); C = 5, 6, 8 (5 needs 4; 6 needs 3 and 4; 8 needs 7); D = 9, 10; E = 11; then 12.
- Task 1 changes what every webview may call. Merge it first and run the whole app once (the controller starts a dev build on the Mac and opens every view, Settings, the mini player) before Wave B merges, because a missing permission only shows at runtime.
- The bridge (Task 2) is plain JavaScript with no build step: it is included with `include_str!` and tested in jsdom from the frontend test suite.
