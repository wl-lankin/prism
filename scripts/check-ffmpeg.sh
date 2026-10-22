#!/usr/bin/env bash
# Checks a built ffmpeg/ffprobe pair before it is bundled:
#   - LGPL: the configuration has no --enable-gpl, --enable-nonfree or --enable-version3, and
#     `ffmpeg -L` names the LGPL 2.1;
#   - every required demuxer, decoder, protocol, muxer, encoder and filter is present
#     (scripts/ffmpeg-config.sh), and no decoder beyond the configured list;
#   - on macOS, both binaries hold arm64 and x86_64;
#   - docs/ffmpeg.md names the pinned version and checksum.
# Prints the sizes. Exits non-zero on the first failed group, after listing what failed.
#
#   scripts/check-ffmpeg.sh <dir with ffmpeg[.exe] and ffprobe[.exe]>
set -euo pipefail

repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=ffmpeg-config.sh
. "$repo/scripts/ffmpeg-config.sh"

dir="${1:?usage: check-ffmpeg.sh <dir>}"
suffix=""
[ -f "$dir/ffmpeg.exe" ] && suffix=".exe"
ffmpeg="$dir/ffmpeg$suffix"
ffprobe="$dir/ffprobe$suffix"
for tool in "$ffmpeg" "$ffprobe"; do
  [ -f "$tool" ] || { echo "missing: $tool" >&2; exit 1; }
done

failed=0
fail() {
  echo "FAIL: $*" >&2
  failed=1
}

# Strips the Windows line ends, so names compare on both platforms.
run() {
  "$@" 2>&1 | tr -d '\r'
}

for tool in "$ffmpeg" "$ffprobe"; do
  version="$(run "$tool" -hide_banner -version)"
  config="$(echo "$version" | sed -n 's/^configuration: //p')"
  [ -n "$config" ] || fail "$(basename "$tool") -version shows no configuration"
  for flag in --enable-gpl --enable-nonfree --enable-version3; do
    case " $config " in
      *" $flag "*) fail "$(basename "$tool") is configured with $flag" ;;
    esac
  done
  case " $config " in
    *" --enable-lib"*) fail "$(basename "$tool") links an external library" ;;
  esac
  # A here-string, not a pipe: with pipefail, grep -q quitting early fails the echo (SIGPIPE).
  grep -q "version ${FFMPEG_VERSION}" <<<"$version" ||
    fail "$(basename "$tool") is not version $FFMPEG_VERSION"
done
for pinned in "FFmpeg ${FFMPEG_VERSION}" "$FFMPEG_SHA256"; do
  grep -qF "$pinned" "$repo/docs/ffmpeg.md" || fail "docs/ffmpeg.md does not name $pinned"
done
license="$(run "$ffmpeg" -hide_banner -L | tr '\n' ' ' | tr -s ' ')"
case "$license" in
  *"GNU Lesser General Public License"*"version 2.1"*) ;;
  *) fail "ffmpeg -L does not name the LGPL 2.1: $license" ;;
esac

# Names from a component listing: the word after the flag column.
#   decoders/encoders: " A....D name  description" after the "------" line
#   demuxers/muxers:   " D  name  description" after the "--" line
#   filters:           " ... name  A->A  description" after the legend
names_after_rule() {
  awk 'found { print $2 } /^ *-+ *$/ { found = 1 }'
}
decoders="$(run "$ffmpeg" -hide_banner -decoders | names_after_rule)"
encoders="$(run "$ffmpeg" -hide_banner -encoders | names_after_rule)"
demuxers="$(run "$ffmpeg" -hide_banner -demuxers | names_after_rule | tr ',' '\n')"
muxers="$(run "$ffmpeg" -hide_banner -muxers | names_after_rule)"
filters="$(run "$ffmpeg" -hide_banner -filters | awk 'NF >= 3 && $3 ~ /->/ { print $2 }')"
# Input protocols: the lines between "Input:" and "Output:".
protocols="$(run "$ffmpeg" -hide_banner -protocols | awk '/^Output:/ { exit } found { print $1 } /^Input:/ { found = 1 }')"

require() {
  local kind="$1" have="$2" want="$3" name
  for name in $want; do
    grep -qx "$name" <<<"$have" || fail "missing $kind: $name"
  done
}
require demuxer "$demuxers" "$FFMPEG_REQUIRED_DEMUXERS"
require decoder "$decoders" "$FFMPEG_REQUIRED_DECODERS"
require protocol "$protocols" "$FFMPEG_REQUIRED_PROTOCOLS"
require muxer "$muxers" "$FFMPEG_REQUIRED_MUXERS"
require encoder "$encoders" "$FFMPEG_REQUIRED_ENCODERS"
require filter "$filters" "$FFMPEG_REQUIRED_FILTERS"

for name in $decoders; do
  case " $FFMPEG_DECODERS " in
    *" $name "*) ;;
    *) fail "decoder not in the configured list: $name" ;;
  esac
done

if [ "$(uname -s)" = "Darwin" ] && [ -z "$suffix" ]; then
  for tool in "$ffmpeg" "$ffprobe"; do
    archs="$(lipo -archs "$tool")"
    case " $archs " in
      *" arm64 "*) ;;
      *) fail "$(basename "$tool") has no arm64 slice ($archs)" ;;
    esac
    case " $archs " in
      *" x86_64 "*) ;;
      *) fail "$(basename "$tool") has no x86_64 slice ($archs)" ;;
    esac
  done
fi

for tool in "$ffmpeg" "$ffprobe"; do
  echo "$(basename "$tool"): $(wc -c <"$tool" | tr -d ' ') bytes"
done
echo "decoders: $(echo "$decoders" | wc -l | tr -d ' '), demuxers: $(echo "$demuxers" | wc -l | tr -d ' ')"

if [ "$failed" = 1 ]; then
  echo "ffmpeg check failed" >&2
  exit 1
fi
echo "ffmpeg check passed: LGPL, $FFMPEG_VERSION, all required components present"
