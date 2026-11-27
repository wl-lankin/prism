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
# service), e.g. WEBKIT_DISABLE_DMABUF_RENDERER=1.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
deb="${1:-}"
user="${PRISM_KIOSK_USER:-${SUDO_USER:-}}"

[ "$(id -u)" = 0 ] || { echo "run with sudo" >&2; exit 1; }
[ -n "$user" ] && [ "$user" != root ] || { echo "run with sudo as the kiosk user, or set PRISM_KIOSK_USER" >&2; exit 1; }
id "$user" >/dev/null 2>&1 || { echo "no such user: $user" >&2; exit 1; }
[ "$(dpkg --print-architecture)" = arm64 ] || { echo "Prism's Pi package is for 64-bit Raspberry Pi OS (arm64)" >&2; exit 1; }

echo "== packages"
apt-get update
apt-get install -y --no-install-recommends cage dbus-user-session alsa-utils wlr-randr fonts-dejavu-core
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
# Prism kiosk: ALSA's default device is the Pi's 3.5 mm jack (written by setup-kiosk.sh).
defaults.pcm.card Headphones
defaults.ctl.card Headphones
EOF
  # Full level at the jack; Prism's own volume does the rest.
  amixer -q -c Headphones sset PCM 100% unmute 2>/dev/null || amixer -q -c Headphones sset Headphone 100% unmute 2>/dev/null || true
  alsactl store 2>/dev/null || true
else
  echo "no 'Headphones' card found; ALSA's default device stays as it is"
fi

echo "== session"
touch /etc/default/prism-kiosk
sed "s/@USER@/$user/" "$here/prism-kiosk.service" >/etc/systemd/system/prism-kiosk.service
systemctl daemon-reload
systemctl set-default graphical.target
systemctl disable getty@tty1.service 2>/dev/null || true
systemctl enable prism-kiosk.service

echo "== done"
echo "Prism starts on the screen at the next boot, or now with: sudo systemctl start prism-kiosk"
echo "Logs: journalctl -u prism-kiosk -f"
