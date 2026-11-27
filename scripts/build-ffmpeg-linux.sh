#!/usr/bin/env bash
# Builds Prism's minimal LGPL ffmpeg and ffprobe for Linux, natively on the machine it runs on
# (CI: the arm64 runner, Ubuntu 22.04). Same pinned release and component list as the macOS and
# Windows builds (scripts/ffmpeg-config.sh). Needs a C toolchain, make, xz, pkg-config and the
# OpenSSL headers (Ubuntu: `apt-get install build-essential pkg-config xz-utils libssl-dev`).
#
#   scripts/build-ffmpeg-linux.sh [--no-stage]
#
# Output: $FFMPEG_BUILD_DIR/out/<triple>/{ffmpeg,ffprobe} (default build dir: ffmpeg-build/ in
# the repo), <triple> being e.g. aarch64-unknown-linux-gnu. Unless --no-stage, they are also
# copied to src-tauri/binaries/ as prism-ffmpeg-<triple> and prism-ffprobe-<triple>
# (src-tauri/tauri.sidecar-linux.conf.json). FFmpeg's own libraries are linked in statically;
# the binaries need only glibc and OpenSSL 3 (for https), which this script checks.
set -euo pipefail

repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=ffmpeg-config.sh
. "$repo/scripts/ffmpeg-config.sh"

stage=1
[ "${1:-}" = "--no-stage" ] && stage=0

[ "$(uname -s)" = "Linux" ] || { echo "build-ffmpeg-linux.sh runs on Linux only" >&2; exit 1; }

case "$(uname -m)" in
  aarch64 | arm64) arch=aarch64 ;;
  x86_64) arch=x86_64 ;;
  *) echo "unsupported architecture: $(uname -m)" >&2; exit 1 ;;
esac
triple="$arch-unknown-linux-gnu"

build_dir="${FFMPEG_BUILD_DIR:-$repo/ffmpeg-build}"
src_dir="$build_dir/src/ffmpeg-${FFMPEG_VERSION}"
work="$build_dir/work/linux-$arch"
out_dir="$build_dir/out/$triple"

ffmpeg_fetch_source "$build_dir/dl" "$build_dir/src"
ffmpeg_validate_components "$src_dir"

rm -rf "$work"
mkdir -p "$work"
# OpenSSL for https (HLS stations); the system's libssl.so.3 is linked dynamically. OpenSSL 3 is
# Apache 2.0, which the LGPL build may use (configure refuses it only with --enable-gpl).
# shellcheck disable=SC2046
(cd "$work" && "$src_dir/configure" \
  $(ffmpeg_common_flags) \
  --enable-pthreads --enable-openssl)
make -C "$work" -j"$(ffmpeg_jobs)" ffmpeg ffprobe

mkdir -p "$out_dir"
for tool in ffmpeg ffprobe; do
  cp "$work/$tool" "$out_dir/$tool"
  strip "$out_dir/$tool"
  # Anything beyond glibc and OpenSSL 3 might be missing on the Pi.
  libs="$(readelf -d "$out_dir/$tool" | sed -n 's/.*(NEEDED).*\[\(.*\)\]/\1/p' | sort -u)"
  echo "$tool needs: $(echo "$libs" | tr '\n' ' ')"
  for lib in $libs; do
    case "$lib" in
      libc.so.* | libm.so.* | libpthread.so.* | libdl.so.* | ld-linux-*.so.* | \
        libssl.so.3 | libcrypto.so.3) ;;
      *)
        echo "$tool depends on $lib, which the package does not declare" >&2
        exit 1
        ;;
    esac
  done
done

if [ "$stage" = 1 ]; then
  "$repo/scripts/stage-ffmpeg.sh" "$out_dir"
fi

for tool in ffmpeg ffprobe; do
  echo "$tool: $(wc -c <"$out_dir/$tool" | tr -d ' ') bytes"
done
