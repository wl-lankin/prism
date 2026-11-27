# M8 Raspberry Pi touch player

Branch `PRISM-8`, cut from `main` at `a015139` (v1.1.1). Target release: v1.2.0.

## Goal

Prism runs on the owner's Raspberry Pi with a touchscreen as a full-screen player that starts at boot (kiosk). Same sources as on the desktop: local music, Navidrome, radio and YouTube Music. Sound goes out of the Pi's 3.5 mm jack. The Windows and macOS apps stay as they are.

## Owner answers (2026-11-27)

- Use: full-screen player (kiosk) that starts at boot, no desktop visible.
- Sources: local music, Navidrome, radio, YouTube Music. All four.
- Sound: the Pi's 3.5 mm jack.
- The Pi may be wiped, renamed and set up from scratch. Branches `PRISM-*` may be pushed at any time.

## The device

| What | Value |
|---|---|
| Board | Raspberry Pi 3 Model B Rev 1.2, 1 GB RAM, 4x Cortex-A53 |
| Boot | SanDisk Ultra Fit USB stick (no SD card) |
| OS | Raspberry Pi OS Lite 64-bit (Trixie, 2026-10-06), set up through cloud-init: host `prism`, user `pi` with the owner's SSH key, Wi-Fi |
| Display | HDMI touchscreen 1920x1200, touch over USB (`TSTP MTouch`, 0416:c168) |
| GPU | VideoCore IV (vc4-kms-v3d), OpenGL ES 2.0 only |

1 GB RAM and the VideoCore IV are the tight spots: WebKitGTK, the Svelte UI at 1920x1200 and a second web view for YouTube Music all share them.

## Decisions pinned here

### Build and package

- Target `aarch64-unknown-linux-gnu`, built on GitHub's `ubuntu-22.04-arm` runner (`.github/workflows/linux.yml`). glibc 2.35 and WebKitGTK 4.1, so the package runs on Raspberry Pi OS Bookworm and Trixie.
- Package: `.deb` only (`src-tauri/tauri.linux.conf.json`). It depends on the distribution's `ffmpeg` and the GStreamer plugins instead of bundling an FFmpeg sidecar; `tools.rs` already finds `ffmpeg` and `ffprobe` on `PATH`.
- Updates: `tauri-plugin-updater` installs a `.deb` with `pkexec`, a graphical sudo or `sudo`. The Pi's user has passwordless sudo, so the kiosk updates without a prompt.

### Navidrome password on Linux

`navidrome::keychain::platform_store`: the Secret Service (gnome-keyring, KWallet) when the session answers on it, otherwise `navidrome-password.json` in the data folder, mode 0600, replaced atomically. A kiosk session has no keyring daemon, so the Pi uses the file. The choice is made once per start.

### Kiosk session

- Compositor: `cage` (a Wayland kiosk compositor) on tty1 after autologin, restarted when Prism exits.
- Prism detects the kiosk and opens its main window full screen: no transparent strip, no window shape, no mini player, no tray, no global shortcut, no autostart entry.
- Audio: ALSA `default` points at the `Headphones` card (`/etc/asound.conf`).

## Tasks

1. Linux build green in CI: compile, clippy, tests, `.deb` artifact.
2. Linux platform pieces: password store (done), media controls over MPRIS, tray, shortcut and autostart skipped where the session has none, window setup without the Windows and macOS specifics.
3. Smoke test on the Pi: install the `.deb`, start it in `cage`, measure memory and frame rate, check touch, audio on the jack, local files, radio, Navidrome.
4. YouTube Music on WebKitGTK: page, sign-in route, playback, memory on 1 GB. If it is not usable on the Pi 3, the owner decides before the release.
5. Touch kiosk view: full screen, large touch targets, no hover-only controls, lighter effects (no `backdrop-filter`) for the VideoCore IV. Mockup first, owner approves it.
6. Text input on the touchscreen (search, Navidrome sign-in): on-screen keyboard or setup over SSH. Owner decides with the mockup.
7. Pi setup script in the repo: packages, autologin, `cage`, audio, screen blanking off, Prism update path.
8. Release workflow: Linux arm64 job, signed updater artifact, `latest.json` entry for `linux-aarch64`; README, CHANGELOG, version 1.2.0.

## Open

- Performance of WebKitGTK on the Pi 3 at 1920x1200 (task 3 decides how much the kiosk view has to cut).
- YouTube Music on 1 GB RAM (task 4).
