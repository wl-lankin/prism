# Prism handoff

Start here to continue Prism on another machine or in a new session. It says where the project stands, what was decided and why, and how work on it is run.

## Where things are

| What | Where |
|---|---|
| Design spec (the source of truth for features and look) | `docs/superpowers/specs/2026-10-20-prism-design.md` |
| Roadmap and the M1 plan | `docs/superpowers/plans/2026-10-20-prism-m1-foundation.md` |
| M2 plan (Everyday player) | `docs/superpowers/plans/2026-10-20-prism-m2-everyday-player.md` |
| M3 plan (Navidrome) | `docs/superpowers/plans/2026-10-21-prism-m3-navidrome.md` |
| M4 plan (Radio) | `docs/superpowers/plans/2026-10-21-prism-m4-radio.md` |
| Rules for every M4 implementer agent | `docs/superpowers/handoff/ledgers/m4-implementer-rules.md` |
| Per-milestone ledgers (progress, rulings, deferred minors, incidents) | `docs/superpowers/handoff/ledgers/` |
| Rules every implementer agent follows | `docs/superpowers/handoff/ledgers/m3-implementer-rules.md` |
| Mockups from brainstorming | `docs/superpowers/mockups/` |
| YouTube Music sign-in spike | `docs/spikes/ytm-signin.md` (code on branch `spike-ytm`) |

The ledgers are copies of the working ledgers in `.superpowers/sdd/` (gitignored, along with review diffs and screenshots). When work continues, keep the working ledger in `.superpowers/sdd/<plan>/progress.md` and refresh the copy here at the end of a milestone.

## Branches

Each milestone branch is cut from the previous one. `main` holds the released state (v1.0.0 is the merge of `PRISM-6`). The GitHub default branch is still `PRISM-3`.

| Branch | Content | State |
|---|---|---|
| `PRISM-1` | M1 Foundation | done |
| `PRISM-2` | M2 Everyday player, plus the up next panel, repeat and shuffle | done, owner hand check mostly confirmed |
| `PRISM-3` | M3 Navidrome | Tasks 1-19 done; Task 20 (owner hand check) open |
| `PRISM-4` | M4 Radio, cut from `PRISM-3` before the M3 hand check | done; checked by the owner in daily use on the Mac, plus UI fixes from that |
| `PRISM-5` | M5 YouTube Music, cut from `PRISM-4` | done; Mac hand check done (sign-in, playback, controls), Windows hand check open |
| `PRISM-6` | M6 Release v1.0.0, cut from `PRISM-5` | released as v1.0.0 from `main` 6284b56; `PRISM-6` is one fix ahead (cb93132) |
| `main` | released versions | v1.0.0 |
| `spike-ytm` | YouTube Music sign-in spike | reference only |

Parallel work uses short-lived branches `PRISM-<milestone><task>` (for example `PRISM-312`) in worktrees under `../Prism-wt/`. They are merged with `--no-ff` into the milestone branch and then deleted.

## Status

### Milestones

1. **M1 Foundation**: done.
2. **M2 Everyday player**: done. It covers:
   - favorites, Flow, crossfade and playlists
   - Now Playing, with the cover-colored ring and seek bar
   - MusicBrainz cover lookup
   - the mini player, tray, close-to-mini and global shortcut
   - autostart and "Open with"
   - the up next panel, repeat and shuffle
3. **M3 Navidrome**: built and reviewed, merged on `PRISM-3`. It covers:
   - Subsonic client
   - keychain password
   - background sync of 30k+ tracks
   - covers
   - streaming with seeking, gapless and crossfade
   - offline pause instead of skipping
   - two-way stars
   - scrobbling
   - playlists (read-only)
   - Flow with Navidrome
   - Settings, sidebar, source views, hearts and quality labels

   Open: Task 20, the owner hand check against the real server (see below).
4. **M4 Radio** (Radio Browser): built and reviewed, merged on `PRISM-4`. It covers:
   - Radio Browser client with mirror rotation
   - coarse location (geojs.io, ipinfo fallback) or a manual country and region
   - "Near you" in three bands (75 km, region, country), worldwide search, saved and recent stations
   - live streams with a bounded buffer, ICY titles, playlists, reconnect, Shoutcast `ICY 200 OK`, HLS through FFmpeg, Ogg chains in place, AAC+ through FFmpeg
   - engine live mode (no seek, no next, pause rejoins live, one offline notice per source)
   - station logos from the favicon or the station homepage, never from private networks
   - the Radio view, live player surfaces, stations in Favorites, Home, Ctrl+K and Settings

   Open: Task 13, the owner hand check (see `ledgers/m4-progress.md`), and merging the M3 hand-check fixes from `PRISM-3` into `PRISM-4`.
5. **M5 YouTube Music**: built on `PRISM-5` (plan `docs/superpowers/plans/2026-10-21-prism-m5-youtube-music.md`, ledger `ledgers/m5-progress.md`). An embedded music.youtube.com child view in the main window, Google sign-in in its own window, a bridge that reads and drives the page through one IPC command, one transport with exclusive playback, media keys and tray following the active player, the YouTube queue in Prism's queue panel, a per-window app command manifest. Checked on the Mac by the owner; the Windows hand check is open.
6. **M6 Release v1.0.0**: released and published on 2026-10-22 (plan `docs/superpowers/plans/2026-10-22-prism-m6-release.md`, ledger `ledgers/m6-progress.md`, release flow `docs/releasing.md`). Updater, CI and release workflow, bundled LGPL FFmpeg, mac mini panel, About with licenses, README. The repo is public. The owner skipped the beta and dropped the M3 Navidrome and M5 Windows hand checks before 1.0.0. Open: bring cb93132 (FFmpeg check fix, `requireSignedVersion`) to `main` with the next release.
7. **v1.1**: Apple Music (needs the owner's Apple developer account; no dev key in the public app) and macOS notarization.

### M3 Task 20: the owner hand check

Agents never run this. The owner starts a dev build himself with his own data and his own server. Checks:

- **Sync and browsing:**
  - first sync time (budget 60 s on the LAN)
  - scrolling the Navidrome Songs list and the album grid
  - search speed
  - an album opening instantly
- **Playback:**
  - streaming at Original and at 192
  - seeking far into a FLAC and into a transcoded track
  - gapless album playback
  - a Flow crossfade with Navidrome tracks
- **Stars, scrobbles and playlists:**
  - starring in Prism appears in the Navidrome web UI
  - the reverse appears after "Sync now"
  - a scrobble shows in Navidrome
  - Navidrome playlists show up
- **Offline:** stop the server or unplug the network mid-track. Expect one notice, playback paused, and the library still browsable. After reconnecting, play resumes.
- **Settings and window:**
  - the password is not in `settings.json`
  - closing the main window still hides it with its taskbar button and opens the mini player
- **Live smoke test (Task 19):** read-only, prints counts only. The command is in `ledgers/m3-progress.md` under Task 19.

Feedback from the hand check is fixed in worktrees, merged into `PRISM-3`, and recorded in the M3 ledger. Then the M4 plan is written.

## Owner decisions to keep

### Identity
- **Name:** Prism.
- **Look:** graphite glass with a spectral ring. Spectrum colors `#ff4d6d #ffc94d #4dffb2 #4da6ff`.
- **Logo:** a hand-made SVG prism. No sparkle glyphs.
- **Source colors:**

  | Source | Color |
  |---|---|
  | navidrome | `#4dffb2` |
  | apple | `#ff4d6d` |
  | youtube | `#ffc94d` |
  | radio | `#4da6ff` |
  | local | `#cccccc` |

### Window and ring
- **The Flow ring** (CleanMyMac idea) hangs half out of the window's bottom edge, 96 px:
  - idle, it shows FLOW and starts an endless mix of favorites
  - while playing, it is play/pause with a progress arc
- **Mini player:** sits bottom right. The same ring, 76 px, hangs half out of its left edge. It never steals focus and never appears without a user action.
- **Window look:** a transparent strip below the app. No Mica, no vibrancy, no OS shadow, no DWM border, no rounded system corners.
- **Closing the main window:**
  - It hides the window together with its taskbar button and opens the mini player.
  - The mini player does not open on quit or on an autostart launch.

### UI details
- Every song row has a play button.
- The settings segmented marker slides softly.
- The seek bar takes the cover colors.
- The cover backdrop runs up behind the title bar.
- **Reduced motion:** only large motion is switched off. Hover and fade transitions keep running at 120 ms (`--dur`). Endless loops (breathing dots, the ring glow) stop.
- **About dialog:** names Wolfgang Linz and links https://wolfgang-linz.de.

### Services and sources
- **Services:**
  - Spotify is dropped.
  - Apple Music moves to v1.1.
  - YouTube Music lives inside the main window, with no sidebar entry until M5.
- **Library:** local files and Navidrome never overlap, so the "All" views are a plain union. The sort is by artist, not by source.
- **Navidrome:**
  - It sits behind a reverse proxy with a public CA certificate, so there is no self-signed option.
  - The client follows same-host redirects only and refuses https to http.
  - Proxy 502/503/504 counts as offline, never as wrong credentials.
  - Base paths are supported.
- **Releases:**
  - Start at v1.0.0.
  - Every asset also exists under a versionless file name.
  - The GitHub repo `wl-lankin/prism` is public since the v1.0.0 release (2026-10-22).

## How the work is run

- **Process:** subagent-driven development, using the `superpowers` skills.
  1. Plan the milestone, then run its tasks in parallel waves, one worktree per task.
  2. After each task, a task reviewer checks it, and fix rounds follow until it is approved.
  3. The controller merges the work and runs the full checks after every merge.
- **Commits:**
  - Identity: `Wolfgang Linz <27097016+wl-lankin@users.noreply.github.com>`.
  - Message format: `PRISM-<n>: <description>`.
  - Branch names match `^[A-Z]+-\d+$`.
  - Commit freely, push only when the owner asks.
- **Writing:**
  - Never an em dash or en dash, anywhere.
  - No AI attribution or session links.
  - Every UI string goes into both EN and DE in `src/lib/i18n.ts`.
- **Data safety:**
  - Every dev run sets `PRISM_DATA_DIR` to a fresh temp folder.
  - Every dev run also passes a Tauri `--config` override with its own identifier (`de.wolfganglinz.prism.dev<n>`).
  - Never touch the real `%APPDATA%` / `%LOCALAPPDATA%\de.wolfganglinz.prism`. An agent once migrated the owner's real database.
  - Keychain entries are keyed by the identifier, so dev runs never see the real password.
- **The owner uses the machine while agents work:**
  - Agents never inject OS mouse or keyboard input and never move or focus the owner's windows. Visual checks use CDP screenshots of the agent's own instance.
  - Agents stop only the PIDs they started, never kill by image name.
  - Never enable autostart or register shortcuts for real, and never run the built installer.
- **Ports:**
  - The main dev copy uses Vite 1440 and HMR 1441.
  - Parallel agents use 1500-1599.
  - Glint uses 1420 and Kern 1430.
- **Test data:**
  - Tests use fake data only: the fake Subsonic server binds `127.0.0.1:0`, with account `demo` / `demo-pass`.
  - Never use the owner's real server, library or credentials in tests, fixtures or screenshots.
- **Migrations:** append-only; the owner has a real library database.

## Setup on a new machine

- **Tools:** Node with npm, Rust stable with cargo, and FFmpeg / ffprobe on PATH (used for formats symphonia cannot decode).
- **Install:** `npm ci`, then `npm run tauri dev` with `PRISM_DATA_DIR` and an identifier override, as above.
- **Checks:**
  - `npm test`
  - `npm run check`
  - `cargo test --manifest-path src-tauri/Cargo.toml`
  - `cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings`
  - `cargo fmt --check`
- **Windows:** WebView2 and the MSVC build tools. On macOS, the Xcode command line tools.
