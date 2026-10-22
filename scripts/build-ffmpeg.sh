#!/usr/bin/env bash
# Builds Prism's minimal LGPL ffmpeg and ffprobe for macOS: one build per architecture from the
# pinned release (scripts/ffmpeg-config.sh), joined with lipo into universal binaries.
#
#   scripts/build-ffmpeg.sh [--no-stage]
#
# Output: $FFMPEG_BUILD_DIR/out/universal-apple-darwin/{ffmpeg,ffprobe} (default build dir:
# ffmpeg-build/ in the repo). Unless --no-stage, they are also copied to src-tauri/binaries/
# under the three names a universal Tauri build looks up: the per-architecture builds want
# ffmpeg-aarch64-apple-darwin and ffmpeg-x86_64-apple-darwin, the bundler wants
# ffmpeg-universal-apple-darwin.
set -euo pipefail

repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=ffmpeg-config.sh
. "$repo/scripts/ffmpeg-config.sh"

stage=1
[ "${1:-}" = "--no-stage" ] && stage=0

[ "$(uname -s)" = "Darwin" ] || { echo "build-ffmpeg.sh runs on macOS only" >&2; exit 1; }

build_dir="${FFMPEG_BUILD_DIR:-$repo/ffmpeg-build}"
src_dir="$build_dir/src/ffmpeg-${FFMPEG_VERSION}"
out_dir="$build_dir/out/universal-apple-darwin"
# Prism's bundle.macOS.minimumSystemVersion.
min_macos="11.0"
host_arch="$(uname -m)"

ffmpeg_fetch_source "$build_dir/dl" "$build_dir/src"
ffmpeg_validate_components "$src_dir"

for arch in arm64 x86_64; do
  work="$build_dir/work/darwin-$arch"
  rm -rf "$work"
  mkdir -p "$work"
  cross=()
  if [ "$arch" != "$host_arch" ]; then
    cross=(--enable-cross-compile)
  fi
  flags="-arch $arch -mmacosx-version-min=$min_macos"
  # shellcheck disable=SC2046
  (cd "$work" && "$src_dir/configure" \
    $(ffmpeg_common_flags) \
    --cc=clang --arch="$arch" --target-os=darwin ${cross[@]+"${cross[@]}"} \
    --extra-cflags="$flags" --extra-ldflags="$flags" \
    --enable-pthreads --enable-securetransport)
  make -C "$work" -j"$(ffmpeg_jobs)" ffmpeg ffprobe
done

mkdir -p "$out_dir"
for tool in ffmpeg ffprobe; do
  lipo -create -output "$out_dir/$tool" \
    "$build_dir/work/darwin-arm64/$tool" "$build_dir/work/darwin-x86_64/$tool"
done

if [ "$stage" = 1 ]; then
  "$repo/scripts/stage-ffmpeg.sh" "$out_dir"
fi

for tool in ffmpeg ffprobe; do
  echo "$tool: $(lipo -archs "$out_dir/$tool"), $(wc -c <"$out_dir/$tool" | tr -d ' ') bytes"
done
