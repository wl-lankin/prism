#!/usr/bin/env bash
# Fails when a Prism release is missing something the installers or the updater need.
#
#   .github/verify-release-assets.sh <version> <repo owner/name> <dir>
#
# <dir> holds assets.tsv (one "name<TAB>size" line per release asset), latest.json and every
# *.sig of the release. <version> is the app version (tauri.conf.json), which is also what the
# file names carry, even for a beta tag.
#
# What tauri 2 (createUpdaterArtifacts) and tauri-action produce for productName "Prism":
#   Windows (NSIS):  Prism_<v>_x64-setup.exe, Prism_<v>_x64-setup.exe.sig (the installer itself
#                    is the updater artifact)
#   macOS universal: Prism_<v>_universal.dmg, Prism_universal.app.tar.gz (no version in the
#                    name) and Prism_universal.app.tar.gz.sig
#   both:            latest.json
# plus what the release workflow adds: the versionless copies Prism-setup.exe and Prism.dmg, and
# the LGPL source: ffmpeg-<FFMPEG_VERSION>.tar.xz and prism-ffmpeg-build-scripts.tar.gz.
set -euo pipefail

version="${1:?usage: verify-release-assets.sh <version> <repo> <dir>}"
repo="${2:?repo missing}"
dir="${3:?dir missing}"

# shellcheck source=/dev/null
. "$(dirname "${BASH_SOURCE[0]}")/../scripts/ffmpeg-config.sh"

setup="Prism_${version}_x64-setup.exe"
dmg="Prism_${version}_universal.dmg"
tarball="Prism_universal.app.tar.gz"

failed=0
fail() {
  echo "FAIL: $*" >&2
  failed=1
}

size_of() {
  awk -F '\t' -v n="$1" '$1 == n { print $2; exit }' "$dir/assets.tsv"
}

expected=(
  "$setup" "$setup.sig"
  "$dmg"
  "$tarball" "$tarball.sig"
  "Prism-setup.exe" "Prism.dmg"
  "latest.json"
  "ffmpeg-${FFMPEG_VERSION}.tar.xz" "prism-ffmpeg-build-scripts.tar.gz"
)
for name in "${expected[@]}"; do
  size="$(size_of "$name")"
  if [ -z "$size" ]; then
    fail "missing asset: $name"
  elif [ "$size" -le 0 ]; then
    fail "empty asset: $name"
  fi
done

# The versionless links must serve the same bytes as the versioned installers.
[ "$(size_of Prism-setup.exe)" = "$(size_of "$setup")" ] || fail "Prism-setup.exe differs in size from $setup"
[ "$(size_of Prism.dmg)" = "$(size_of "$dmg")" ] || fail "Prism.dmg differs in size from $dmg"

if [ ! -f "$dir/latest.json" ]; then
  fail "latest.json was not downloaded"
else
  json="$dir/latest.json"
  [ "$(jq -r '.version // empty' "$json")" = "$version" ] ||
    fail "latest.json version is '$(jq -r '.version // empty' "$json")', expected $version"

  # platform -> the asset its url must point at
  check_platform() {
    local platform="$1" want="$2" url sig name sigfile
    url="$(jq -r --arg p "$platform" '.platforms[$p].url // empty' "$json")"
    sig="$(jq -r --arg p "$platform" '.platforms[$p].signature // empty' "$json")"
    if [ -z "$url" ] || [ -z "$sig" ]; then
      fail "latest.json has no url or signature for $platform"
      return
    fi
    name="${url##*/}"
    case "$url" in
      "https://github.com/$repo/releases/download/"*) ;;
      *) fail "$platform url is not a release download of $repo: $url" ;;
    esac
    [ "$name" = "$want" ] || fail "$platform url points at $name, expected $want"
    [ -n "$(size_of "$name")" ] || fail "$platform url points at $name, which is not a release asset"
    sigfile="$dir/$name.sig"
    if [ ! -f "$sigfile" ]; then
      fail "no $name.sig to compare with the $platform signature"
    elif [ "$(tr -d '[:space:]' <"$sigfile")" != "$(printf '%s' "$sig" | tr -d '[:space:]')" ]; then
      fail "$platform signature in latest.json differs from $name.sig"
    fi
  }
  check_platform darwin-aarch64 "$tarball"
  check_platform darwin-x86_64 "$tarball"
  check_platform windows-x86_64 "$setup"
fi

if [ "$failed" = 1 ]; then
  echo "release assets incomplete" >&2
  exit 1
fi
echo "release assets complete: ${expected[*]}"
