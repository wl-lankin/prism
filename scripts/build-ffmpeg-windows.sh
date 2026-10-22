#!/usr/bin/env bash
# Builds Prism's minimal LGPL ffmpeg.exe and ffprobe.exe for Windows x64 by cross-compiling
# with mingw-w64 (Ubuntu: `apt-get install mingw-w64 make xz-utils`; macOS: `brew install
# mingw-w64`). Same pinned release and component list as the macOS build
# (scripts/ffmpeg-config.sh).
#
#   scripts/build-ffmpeg-windows.sh [--no-stage]
#
# Output: $FFMPEG_BUILD_DIR/out/x86_64-pc-windows-msvc/{ffmpeg,ffprobe}.exe (default build dir:
# ffmpeg-build/ in the repo). Unless --no-stage, they are also copied to src-tauri/binaries/ as
# ffmpeg-x86_64-pc-windows-msvc.exe and ffprobe-x86_64-pc-windows-msvc.exe, the names a Tauri
# build for the MSVC target looks up. The binaries are fully static: they import Windows system
# DLLs only, which this script checks.
set -euo pipefail

repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=ffmpeg-config.sh
. "$repo/scripts/ffmpeg-config.sh"

stage=1
[ "${1:-}" = "--no-stage" ] && stage=0

prefix="x86_64-w64-mingw32-"
command -v "${prefix}gcc" >/dev/null || { echo "${prefix}gcc not found (install mingw-w64)" >&2; exit 1; }

build_dir="${FFMPEG_BUILD_DIR:-$repo/ffmpeg-build}"
src_dir="$build_dir/src/ffmpeg-${FFMPEG_VERSION}"
work="$build_dir/work/windows-x86_64"
triple="x86_64-pc-windows-msvc"
out_dir="$build_dir/out/$triple"

ffmpeg_fetch_source "$build_dir/dl" "$build_dir/src"
ffmpeg_validate_components "$src_dir"

rm -rf "$work"
mkdir -p "$work"
# shellcheck disable=SC2046
(cd "$work" && "$src_dir/configure" \
  $(ffmpeg_common_flags) \
  --enable-cross-compile --cross-prefix="$prefix" --arch=x86_64 --target-os=mingw32 \
  --extra-ldflags=-static \
  --enable-w32threads --enable-schannel)
make -C "$work" -j"$(ffmpeg_jobs)" ffmpeg.exe ffprobe.exe

mkdir -p "$out_dir"
for tool in ffmpeg ffprobe; do
  cp "$work/$tool.exe" "$out_dir/$tool.exe"
  # Anything but a system DLL (libgcc, libwinpthread, ...) would be missing on the user's PC.
  dlls="$("${prefix}objdump" -p "$out_dir/$tool.exe" | sed -n 's/^[[:space:]]*DLL Name: //p' | tr 'A-Z' 'a-z' | sort -u)"
  echo "$tool.exe imports: $(echo "$dlls" | tr '\n' ' ')"
  for dll in $dlls; do
    case "$dll" in
      kernel32.dll | user32.dll | advapi32.dll | bcrypt.dll | crypt32.dll | ncrypt.dll | \
        secur32.dll | ws2_32.dll | shell32.dll | ole32.dll | psapi.dll | shlwapi.dll | \
        msvcrt.dll | api-ms-win-*.dll) ;;
      *)
        echo "$tool.exe depends on $dll, which Windows does not ship" >&2
        exit 1
        ;;
    esac
  done
done

if [ "$stage" = 1 ]; then
  "$repo/scripts/stage-ffmpeg.sh" "$out_dir"
fi

for tool in ffmpeg ffprobe; do
  echo "$tool.exe: $(wc -c <"$out_dir/$tool.exe" | tr -d ' ') bytes"
done
