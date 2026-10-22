#!/usr/bin/env bash
# Prints the release notes for a tag: the CHANGELOG.md section of that version. A prerelease tag
# (v1.0.0-beta.1) first looks for its own section and falls back to the plain version (1.0.0),
# because the app version of a beta is already the final one. Fails when no section has text,
# so a release never goes out with an empty body. The section already ends with the install
# notes; nothing is appended here.
#
#   .github/release-notes.sh v1.0.0-beta.1      (CHANGELOG_FILE overrides CHANGELOG.md)
set -euo pipefail

tag="${1:?usage: release-notes.sh <tag>}"
file="${CHANGELOG_FILE:-CHANGELOG.md}"
version="${tag#v}"
base="${version%%-*}"

section() {
  awk -v v="$1" '
    /^## / {
      if (found) exit
      h = $0
      gsub(/[\[\]]/, "", h)
      split(h, a, " ")
      if (a[2] == v) { found = 1; next }
    }
    found' "$file"
}

for candidate in "$version" "$base"; do
  notes="$(section "$candidate")"
  if [ -n "$(printf '%s' "$notes" | tr -d '[:space:]')" ]; then
    # Trim blank lines at both ends.
    printf '%s\n' "$notes" | awk 'NF { started = 1 } started { lines[++n] = $0 } END {
      while (n > 0 && lines[n] !~ /[^[:space:]]/) n--
      for (i = 1; i <= n; i++) print lines[i]
    }'
    exit 0
  fi
done

echo "No text under a '## $version' or '## $base' heading in $file" >&2
exit 1
