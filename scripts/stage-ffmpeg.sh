#!/usr/bin/env bash
# Copies a built ffmpeg/ffprobe pair into src-tauri/binaries/ under the names the release build
# (src-tauri/tauri.sidecar.conf.json) looks up.
#
#   scripts/stage-ffmpeg.sh <dir with ffmpeg[.exe] and ffprobe[.exe]>
#
# Windows (ffmpeg.exe): <tool>-x86_64-pc-windows-msvc.exe.
# macOS (universal binaries): <tool>-universal-apple-darwin for the bundler, and the same file as
# <tool>-aarch64-apple-darwin and <tool>-x86_64-apple-darwin for the two per-architecture builds
# of `tauri build --target universal-apple-darwin`.
set -euo pipefail

repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
dir="${1:?usage: stage-ffmpeg.sh <dir>}"
dest="$repo/src-tauri/binaries"
mkdir -p "$dest"

if [ -f "$dir/ffmpeg.exe" ]; then
  for tool in ffmpeg ffprobe; do
    cp "$dir/$tool.exe" "$dest/$tool-x86_64-pc-windows-msvc.exe"
  done
else
  for tool in ffmpeg ffprobe; do
    for triple in universal-apple-darwin aarch64-apple-darwin x86_64-apple-darwin; do
      cp "$dir/$tool" "$dest/$tool-$triple"
      chmod +x "$dest/$tool-$triple"
    done
  done
fi
ls -l "$dest"
