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

1. Linux build green in CI: compile, clippy, tests, `.deb` artifact. **Done.** Ubuntu 22.04's FFmpeg 4.4 failed the HLS tests, so Linux bundles Prism's own FFmpeg too (`scripts/build-ffmpeg-linux.sh`, with OpenSSL and the `udp` protocol FFmpeg 8's TLS glue needs), as `/usr/bin/prism-ffmpeg`.
2. Linux platform pieces. **Done.** Password store (Secret Service or a 0600 file), kiosk mode (`shell/kiosk.rs`), media controls over MPRIS through souvlaki as before. The YouTube Music view floats over the page in a `GtkOverlay` (`ytm/overlay_linux.rs`): tauri-runtime-wry packs child web views into the window's `GtkBox` on Linux.
3. Smoke test on the Pi. **Done.** See "Findings on the Pi 3" below.
4. YouTube Music on WebKitGTK. **Done.** Sign-in, playback and the view work; on the Pi 3 the page takes about a core and memory goes into zram swap. Usable, but a Pi 4 or 5 is the machine for it.
5. Touch kiosk view. **Done as tuning, not a new layout:** the maximized layout at 960x600 points (page zoom), no window controls, no tooltips, no frosted glass, reduced motion on the Pi 3. The owner checked it on the device.
6. Text input on the touchscreen. **Done:** Prism's own keyboard (`TouchKeyboard.svelte`) and `ytm/osk.js` for Google's sign-in (with a Cancel bar that navigates to `https://prism.invalid/close-sign-in`) and the YouTube Music view.
7. Pi setup script in the repo. **Done:** `scripts/pi/` (cage on tty1 as a systemd service, ALSA dmix to the jack, Pi 3 graphics, screen mode and zoom, the sudoers rule for the power buttons).
8. Release workflow. **Done:** a third leg on `ubuntu-22.04-arm`, `Prism-arm64.deb`, `prism-pi-kiosk.tar.gz`, the asset check; CI checks the `.deb.sig` with a throwaway key. Version 1.2.0.
9. Power buttons (owner request). **Done:** Settings, This device, kiosk only.

## Findings on the Pi 3

- GTK gets no GL context on the VideoCore IV unless `GDK_GL=gles`. With GL, WebKit's DMA-BUF buffers come out with the wrong layout (garbled rows); `WEBKIT_DMABUF_RENDERER_FORCE_SHM=1` fixes that. Without GL only the animated layer (the Flow ring) was ever repainted.
- Every frame is then a software copy of the whole screen. The idle Flow ring's endless spin cost a full core; reduced motion stops it. WebKitGTK takes reduced motion from GSettings `org.gnome.desktop.interface enable-animations` (not from `settings.ini` under Wayland).
- 1920x1200 at scale 2 worked but was sluggish; 1280x800 with page zoom 1.333 copies less than half the pixels for the same 960x600 points.
- `defaults.pcm.card Headphones` broke ALSA on Trixie ("card is not a string"), and cpal panicked with it. A `pcm.!default` plug over dmix on `hw:CARD=Headphones,DEV=0` works.
- Under cage the window state reported between resizes is not reliable: the kiosk layout is pinned to full screen.
- The owner's Wi-Fi is hidden: NetworkManager needs `802-11-wireless.hidden yes` (cloud-init's netplan profile lacks it).

## Open

- macOS and Windows CI on the PR, then the release (owner's go).
