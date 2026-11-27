#!/usr/bin/env bash
# Sets up a Raspberry Pi with Raspberry Pi OS Lite (64-bit, Bookworm or Trixie) and a
# touchscreen as a Prism touch player: Prism starts full screen at boot (cage on tty1, see
# prism-kiosk.service), restarts when it exits, and plays through the 3.5 mm jack.
#
#   sudo scripts/pi/setup-kiosk.sh [prism_<version>_arm64.deb]
#
# The kiosk runs as the user who called sudo (or PRISM_KIOSK_USER). Without a .deb only the
# session is set up; install Prism later with `sudo apt install ./prism_<version>_arm64.deb`.
# Safe to run again: every step replaces its own files.
#
# Settings for the session go in /etc/default/prism-kiosk (KEY=value lines, read by the
# service). On a Pi 3 or older (VideoCore IV, OpenGL ES 2 only) the script writes the two that
# make WebKitGTK draw correctly there, and turns animations off for the kiosk user: Prism then
# follows its reduced motion rules (no endless loops), because every frame costs that GPU a
# full screen copy.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
deb="${1:-}"
user="${PRISM_KIOSK_USER:-${SUDO_USER:-}}"

[ "$(id -u)" = 0 ] || { echo "run with sudo" >&2; exit 1; }
[ -n "$user" ] && [ "$user" != root ] || { echo "run with sudo as the kiosk user, or set PRISM_KIOSK_USER" >&2; exit 1; }
id "$user" >/dev/null 2>&1 || { echo "no such user: $user" >&2; exit 1; }
[ "$(dpkg --print-architecture)" = arm64 ] || { echo "Prism's Pi package is for 64-bit Raspberry Pi OS (arm64)" >&2; exit 1; }

export DEBIAN_FRONTEND=noninteractive

echo "== locale"
# The language from /etc/default/locale must exist, or every program warns and falls back.
lang="$(sed -n 's/^LANG=//p' /etc/default/locale 2>/dev/null | tr -d '"')"
if [ -n "$lang" ] && ! locale -a 2>/dev/null | grep -qix "$(echo "$lang" | sed 's/UTF-8/utf8/I')"; then
  sed -i "s/^# *\($lang .*\)/\1/" /etc/locale.gen
  locale-gen
fi

echo "== packages"
apt-get update
apt-get install -y --no-install-recommends cage dbus-user-session alsa-utils wlr-randr \
  fonts-dejavu-core libglib2.0-bin gsettings-desktop-schemas dconf-gsettings-backend
if [ -n "$deb" ]; then
  case "$deb" in
    /*) ;;
    *) deb="$PWD/$deb" ;;
  esac
  apt-get install -y "$deb"
fi
command -v prism >/dev/null || echo "note: Prism is not installed yet (/usr/bin/prism missing)"

echo "== groups"
# Display (video, render), touch (input) and sound (audio) in the kiosk session.
usermod -aG video,render,input,audio "$user"

echo "== sound on the 3.5 mm jack"
if aplay -l 2>/dev/null | grep -q 'Headphones'; then
  cat >/etc/asound.conf <<'EOF'
# Prism kiosk: the default device is the Pi's 3.5 mm jack, shared through dmix so Prism and the
# YouTube Music page can play at the same time (written by setup-kiosk.sh).
pcm.!default {
    type plug
    slave.pcm "prismjack"
}
pcm.prismjack {
    type dmix
    ipc_key 4517
    ipc_perm 0666
    slave {
        pcm "hw:CARD=Headphones,DEV=0"
        rate 48000
    }
}
ctl.!default {
    type hw
    card "Headphones"
}
EOF
  # Full level at the jack; Prism's own volume does the rest.
  amixer -q -c Headphones sset PCM 100% unmute 2>/dev/null || amixer -q -c Headphones sset Headphone 100% unmute 2>/dev/null || true
  alsactl store 2>/dev/null || true
else
  echo "no 'Headphones' card found; ALSA's default device stays as it is"
fi

echo "== graphics"
model="$(tr -d '\0' </proc/device-tree/model 2>/dev/null || true)"
echo "$model"
case "$model" in
  "Raspberry Pi 3"* | "Raspberry Pi 2"* | "Raspberry Pi Zero"* | "Raspberry Pi Model"*)
    # GTK only gets a GL context with GLES; WebKit's GPU buffers come out with the wrong
    # layout on this GPU, so they go through shared memory.
    cat >/etc/default/prism-kiosk <<'EOF'
# Prism kiosk on a Pi 3 or older (written by setup-kiosk.sh).
GDK_GL=gles
WEBKIT_DMABUF_RENDERER_FORCE_SHM=1
EOF
    # Every frame is a software copy here, so a big panel runs at 1280x800 (less than half the
    # pixels of 1920x1200) and Prism zooms its page to touch size (960x600 points).
    preferred="$(head -n 1 /sys/class/drm/card*-HDMI-A-1/modes 2>/dev/null | head -n 1)"
    if [ "${preferred%%x*}" -ge 1600 ] 2>/dev/null && cat /sys/class/drm/card*-HDMI-A-1/modes 2>/dev/null | grep -qx 1280x800; then
      printf 'PRISM_KIOSK_MODE=1280x800\nPRISM_KIOSK_SCALE=1\nPRISM_KIOSK_ZOOM=1.333\n' >>/etc/default/prism-kiosk
    fi
    # Reduced motion for the page (WebKitGTK follows GTK's enable-animations).
    runuser -u "$user" -- dbus-run-session -- gsettings set org.gnome.desktop.interface enable-animations false
    ;;
  *)
    touch /etc/default/prism-kiosk
    ;;
esac

echo "== session"
install -m 0755 "$here/prism-kiosk-run" /usr/local/bin/prism-kiosk-run
sed "s/@USER@/$user/" "$here/prism-kiosk.service" >/etc/systemd/system/prism-kiosk.service
systemctl daemon-reload
systemctl set-default graphical.target
systemctl disable getty@tty1.service 2>/dev/null || true
systemctl enable prism-kiosk.service

echo "== done"
echo "Prism starts on the screen at the next boot, or now with: sudo systemctl start prism-kiosk"
echo "Logs: journalctl -u prism-kiosk -f"
