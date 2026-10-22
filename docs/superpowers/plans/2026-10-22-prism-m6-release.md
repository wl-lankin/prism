# Prism M6 (Release v1.0.0) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Prism v1.0.0 ships: installers for Windows (NSIS, x64) and macOS (universal DMG) built by GitHub Actions from a tag, every asset also under a versionless name, a signed auto-updater that downloads in the background and installs on quit or on "Restart to update", FFmpeg bundled as a minimal LGPL build, a proper macOS mini player panel, a README with screenshots from demo data, the open-source licenses in the About dialog, and the repo made public.

**Architecture:** No new subsystems. The updater is `tauri-plugin-updater` plus `tauri-plugin-process` (relaunch), with a small `update` service in Rust that checks after start and every 6 hours, downloads in the background and emits `update://state`; the frontend shows a pill. `createUpdaterArtifacts` lives in `src-tauri/tauri.updater.conf.json`, passed only by the release workflow (source builds have no key). FFmpeg and ffprobe become Tauri sidecars (`externalBin`), built in CI from source with an audio-only LGPL configure line and cached per FFmpeg version; `tools.rs` already prefers a sidecar next to the executable. The release workflow follows the owner's Session Buddy workflow (draft release, `latest.json`, CHANGELOG body), plus a step that uploads versionless copies.

**Spec:** `docs/superpowers/specs/2026-10-20-prism-design.md` (2 Goals, 3 Constraints, 5.10 macOS, 5.11 About, 5.14 Updates, 7.2 README, 7.3 License, 7.4 Release pipeline). Roadmap row M6. Previous plans and ledgers in `docs/superpowers/`.

## Global Constraints

- Everything in the M5 Global Constraints holds (identity, commit format now `PRISM-6: <Description>`, no em or en dash, no AI attribution, fake data only, dev runs with `PRISM_DATA_DIR` and an identifier override, Write/Edit only, one cargo test filter per run).
- Branch `PRISM-6`, cut from `PRISM-5`. Task branches `PRISM-6NN` in worktrees under `~/Projects/Prism-wt/`.
- **Nothing is pushed to a tag, published, merged into `main` or made public without the owner's explicit yes at that moment.** CI may run on pushes to `PRISM-6`.
- No secret in the repo: the updater private key and its password live only on the owner's machine and in GitHub secrets (`TAURI_SIGNING_PRIVATE_KEY`, `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`). Only the public key goes into `tauri.conf.json`.
- Versionless assets exactly as the spec names them: `Prism-setup.exe`, `Prism.dmg`; links `https://github.com/wl-lankin/prism/releases/latest/download/<name>`.
- Installers are unsigned in v1.0.0 (spec 7.4). Windows code signing via Azure Artifact Signing (the owner's chosen route for his apps, identity validation pending) and macOS notarization come in v1.1.
- FFmpeg: LGPL only (no `--enable-gpl`, no `--enable-nonfree`), the configure line and the source version are recorded in the repo and shown in About, with a link to the source. No GPL component is bundled.
- README screenshots come from the running app with the demo library (`node scripts/make-fixtures.mjs --demo <n>`) and fake accounts only: never the owner's library, server, Google account or location.

## Review Focus

1. **The updater can never install something unsigned or from elsewhere.** The endpoint is the repo's `latest.json` only; the signature is checked by the plugin with the pinned public key; a failed check leaves the installed version untouched and shows a calm notice. Pinned in Task 2 (`update_endpoint_is_the_repo`, `pubkey_is_pinned`).
2. **An update never interrupts listening.** Downloads run in the background; installing happens on quit or when the user presses "Restart to update"; a relaunch restores the main window or the mini player as it was, not the playback position (v1.0.0). Pinned in Task 2 and the hand check.
3. **The bundled FFmpeg is LGPL and minimal.** CI checks the built binary's `-version` output has no `--enable-gpl` / `--enable-nonfree` and that only the configured decoders are present. Pinned in Task 3.
4. **Release assets are complete and stable.** Each release has the versioned installers, the versionless copies, `latest.json` and the `.sig` files; the versionless links resolve. Pinned in Task 5 (a workflow step that fails the run when an asset is missing).

---

## Decisions pinned here

- **Updater flow.** First check 60 s after start, then every 6 hours, only when online. A found update downloads in the background (no prompt). When it is ready, the transport strip and Settings > Updates show a small pill "Restart to update"; quitting installs it (Windows: NSIS `/S` via the plugin's passive mode; macOS: the app bundle is replaced). Settings > Updates: "Check now", the current version, the last check time, and "Install updates automatically" (on by default; off = only notify). Release notes from `latest.json` show in Settings.
- **Key.** One updater key pair `~/.tauri/prism-updater.key` (+ `.pass`, `.pub`) on the owner's machine (Owner answer 1). The public key goes into `plugins.updater.pubkey`.
- **FFmpeg build.** A `scripts/build-ffmpeg.sh` (macOS, universal via two builds and `lipo`) and `scripts/build-ffmpeg.ps1` or an MSYS2 step (Windows x64), both from the same pinned FFmpeg release tarball (checksum in the script), configure: `--disable-everything --disable-autodetect --disable-programs --enable-ffmpeg --enable-ffprobe --disable-doc --enable-protocol=file,pipe --enable-demuxer=<audio list> --enable-decoder=<audio list> --enable-parser=<audio list> --enable-encoder=pcm_f32le,pcm_s16le --enable-muxer=f32le,s16le,wav --enable-filter=aresample,aformat,anull,atrim --enable-swresample --disable-swscale --disable-avdevice --disable-postproc`, LGPL. HLS radio (M4) opens its URL in FFmpeg, so the build also enables `--enable-demuxer=hls --enable-protocol=http,https,tcp,tls,crypto` with the platform TLS (`--enable-securetransport` on macOS, `--enable-schannel` on Windows); that stays LGPL, and `--disable-network` is not set. The exact demuxer/decoder list is derived from `audio/ffmpeg.rs` and the formats in spec 5.2 (WMA, APE, WavPack, Musepack, TTA, DSD, MKA/WebM, Opus, AAC/HE-AAC, AC-3/E-AC-3 for some radio). Built binaries are cached in CI by FFmpeg version + configure hash and attached as sidecars `ffmpeg-<target-triple>` / `ffprobe-<target-triple>`.
- **macOS mini player.** The mini window becomes a non-activating panel: `NSWindowStyleMaskNonactivatingPanel` behaviour through `objc2-app-kit` (`NSPanel` subclass is not possible on an existing NSWindow, so set `setStyleMask` with `.nonactivatingPanel` where Tauri allows, else keep `orderFrontRegardless` + `setLevel(.floating)` when "Keep on top" and `setCollectionBehavior(.canJoinAllSpaces | .fullScreenAuxiliary)`). Docked top right under the menu bar (owner decision 2026-10-21). Clicking the Dock icon opens the main window. Pinned in Task 4.
- **About and licenses.** About lists: the app version, "Made by Wolfgang Linz" with the link, the GitHub repo link, and "Open-source licenses" opening a generated list (Rust crates via `cargo about` or `cargo-license` output committed as `src/lib/licenses.json`, npm packages via `license-checker`-style output), plus FFmpeg (LGPL 2.1, configure line, source link), the Navidrome logo (GPL-3.0, Navidrome project), and data from Radio Browser (free API, attribution) and GeoJS.
- **README.** Logo and tagline, two screenshots (main window with demo albums, the mini player), download buttons to the versionless links, features, supported formats table, Navidrome setup note, YouTube Music note (sign-in in its own window; Prism never sees the password), radio note (Radio Browser, rough location from GeoJS, can be manual), macOS first launch note (right-click > Open or `xattr -dr com.apple.quarantine /Applications/Prism.app`), Windows SmartScreen note, credits line, MIT license.
- **Branches into main.** At release, `PRISM-6` (which contains M1-M5) merges into `main` with `--no-ff`; the tag `v1.0.0` is made on that merge commit.

## Owner answers (2026-10-22)

1. No key existed on Windows. As for Session Buddy, Klecks and Orbo: a new pair `~/.tauri/prism-updater.key` (+ `.pub`, `.pass`, random password) on the Mac, and the secrets `TAURI_SIGNING_PRIVATE_KEY` / `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` set in `wl-lankin/prism` (done 2026-10-22).
2. The open hand checks (M3 Navidrome, M5 Windows) come after the beta, before v1.0.0.
3. A `v1.0.0-beta.1` first through the same pipeline, then 1.0.0.

## Owner questions as first asked (defaults in brackets)

1. Updater key: does `prism-updater.key` already exist on the Windows PC (the spec names `%USERPROFILE%\.tauri\prism-updater.key`)? [If not: generate a new pair on the Mac with `npm run tauri signer generate -- -w ~/.tauri/prism-updater.key`, the owner sets the two GitHub secrets.]
2. The open hand checks before release: M3 Task 20 (Navidrome against the real server), M5 on Windows. [Both before tagging v1.0.0.]
3. Version: start at v1.0.0 as the spec says, or a v1.0.0-beta.1 first through the same pipeline? [beta.1 first, then 1.0.0.]

---

### Task 1: Version, CHANGELOG, release notes, repo hygiene

**Files:** `package.json`, `package-lock.json`, `src-tauri/Cargo.toml`, `src-tauri/tauri.conf.json` (stay 1.0.0), create `CHANGELOG.md` (one `## 1.0.0` section in plain language: what Prism does), `docs/releases/v1.0.0.md`, `.gitignore` (sidecar build outputs), remove leftovers (`docs/spikes` stays as history; the `spike-ytm` branch is not merged).
- [ ] Steps: write, `npm test`, `npm run check`, commit `PRISM-6: Changelog and release notes for 1.0.0`.

### Task 2: Updater service and UI

**Files:** `src-tauri/Cargo.toml` (`tauri-plugin-updater`, `tauri-plugin-process`), `src-tauri/tauri.conf.json` (`plugins.updater.endpoints` = `["https://github.com/wl-lankin/prism/releases/latest/download/latest.json"]`, `pubkey`, `windows.installMode = "passive"`), create `src-tauri/tauri.updater.conf.json` (`bundle.createUpdaterArtifacts: true`), create `src-tauri/src/update.rs`, `lib.rs`, `commands.rs` (`update_state`, `update_check`, `update_restart`), `build.rs` and capabilities (main only), `settings.rs` (`autoUpdate`), frontend `ipc.ts`, `stores/update.svelte.ts`, `TransportStrip` pill, `Settings` section, `i18n.ts`.
- Interfaces: `UpdateState { status: idle|checking|downloading|ready|failed|upToDate, version: Option<String>, notes: Option<String>, progress: Option<f32>, lastCheck: Option<i64> }`; the service never installs while playing unless on quit; on quit with `ready` it installs.
- Tests: `update_endpoint_is_the_repo`, `pubkey_is_pinned` (config parse), state machine tests with a fake checker (found -> downloading -> ready; failure -> failed and a retry later; offline skips), frontend pill and Settings tests.
- [ ] Commit `PRISM-6: Signed auto-update that installs on quit or on Restart to update`.

### Task 3: Minimal LGPL FFmpeg as sidecars

**Files:** create `scripts/build-ffmpeg.sh`, `scripts/build-ffmpeg-windows.sh` (MSYS2) or `.ps1`, `src-tauri/tauri.conf.json` (`bundle.externalBin: ["binaries/ffmpeg", "binaries/ffprobe"]` in a separate `tauri.sidecar.conf.json` used by release builds, so dev builds keep using PATH), `src-tauri/src/tools.rs` (sidecar lookup also under `Contents/MacOS` / next to the exe with the triple-less name Tauri installs), `docs/ffmpeg.md` (version, configure line, source link, license), tests for the lookup.
- CI job `ffmpeg` builds per target, caches, runs `ffmpeg -version` and `ffmpeg -decoders` checks (LGPL, expected decoders present, no GPL flags), uploads as an artifact the release job downloads.
- [ ] Commit `PRISM-6: A minimal LGPL FFmpeg built in CI and bundled as sidecars`.

### Task 4: macOS mini panel and Dock behaviour

**Files:** `src-tauri/src/shell/mini.rs`, `shell/mod.rs`, `lib.rs` (`RunEvent::Reopen` opens the main window), `Cargo.toml` (`objc2-app-kit` features as needed).
- The mini window never activates Prism, floats above other apps only with "Keep on top", appears on every Space and over full-screen apps, and stays out of Cmd+Tab and Mission Control. A Dock click opens the main window. Tested: pure config functions; the native calls go to the owner hand check.
- [ ] Commit `PRISM-6: Mac mini player as a non-activating panel, Dock click opens Prism`.

### Task 5: CI and the release workflow

**Files:** create `.github/workflows/ci.yml` (on pushes and PRs: `npm ci`, `npm test`, `npm run check`, `cargo fmt --check`, `cargo clippy -D warnings`, `cargo test` on macOS and Windows), `.github/workflows/release.yml` (tag `v*` or manual: the FFmpeg artifacts, tauri-action with `--config src-tauri/tauri.updater.conf.json --config src-tauri/tauri.sidecar.conf.json`, macOS `--target universal-apple-darwin`, draft release, `uploadUpdaterJson`, `updaterJsonPreferNsis`, CHANGELOG body; then a job that downloads the versioned `Prism_<v>_x64-setup.exe` and `Prism_<v>_universal.dmg`, uploads copies as `Prism-setup.exe` and `Prism.dmg`, and fails if any of the six expected assets is missing).
- [ ] Commit `PRISM-6: CI on both platforms and a tag-driven release with versionless assets`.

### Task 6: About, licenses, README

**Files:** `src/lib/components/About.svelte`, `src/lib/licenses.json` (generated by `scripts/licenses.mjs` from `cargo metadata` and `package-lock.json`), `README.md`, `docs/screenshots/*.png` (taken by the controller from a demo-data dev run with `screencapture -l <window id>` of Prism's own windows only), `i18n.ts`.
- [ ] Commit `PRISM-6: About with open-source licenses, and the README`.

### Task 7: Integration pass and a dry-run release

- Run the release workflow by hand (`workflow_dispatch`) with tag `v1.0.0-beta.1` into a **draft** (never published by an agent); check all assets and `latest.json`; the owner installs the beta on both machines.
- [ ] Commit fixes `PRISM-6: Release dry run fixes`.

### Task 8: Owner hand check and release

- [ ] Owner installs the beta on Windows and the Mac: first-launch notes, every source, mini player, tray/menu bar, autostart off by default, then an update from beta.1 to beta.2 (a second dry run) installs on quit and on "Restart to update".
- [ ] With the owner's explicit yes: merge `PRISM-6` into `main`, tag `v1.0.0`, run the release, publish the draft, make the repo public, check the versionless links.

---

## Execution notes

- Waves: A = 1, 3, 4 (independent); B = 2, 5 (2 needs the owner's key answer; 5 needs 3's artifact names); C = 6; D = 7; then 8.
- Agents never push tags, publish releases, change repo visibility or set secrets. The controller asks the owner before each of those.
