# Releasing Prism

A release is made by GitHub Actions from a tag. Nothing is published automatically: the workflow
only fills a draft, and the owner publishes it.

## Before the tag

1. `CHANGELOG.md` has a `## <version>` section for the version in `src-tauri/tauri.conf.json`.
   That section is the release body and the notes the app shows, so it ends with the install
   notes. The workflow adds nothing to it.
2. The version in `package.json`, `src-tauri/Cargo.toml` and `src-tauri/tauri.conf.json` is the
   one being released. A beta keeps the final number: tag `v1.0.0-beta.1` ships app version
   `1.0.0`, because the Windows installer needs a numeric version. The beta falls back to the
   `1.0.0` section when there is no `1.0.0-beta.1` section.
3. CI is green on the commit (`.github/workflows/ci.yml`).

## Running it

The owner pushes the tag (`git tag v1.0.0 && git push origin v1.0.0`), or starts the Release
workflow by hand from the Actions tab and types the tag. The workflow:

1. **prepare** checks the tag shape, that it matches the app version, and that `CHANGELOG.md`
   has text for it. It fails before any build otherwise.
2. **ffmpeg** (`ffmpeg.yml`) builds the minimal LGPL FFmpeg for macOS (universal) and Windows
   (x64, cross-built on Linux), cached by the hash of the build scripts. `check-ffmpeg.sh` runs
   on a Mac and on `windows-latest` and rejects GPL or non-free flags, missing or extra
   decoders and a wrong version. See `docs/ffmpeg.md`.
3. **release** builds one platform after the other (macOS universal DMG and app, then the
   Windows NSIS installer), bundles the FFmpeg sidecars, signs the updater artifacts and adds
   everything to one draft release. A tag with a `-` in it is marked as a prerelease.
4. **assets** copies the installers to `Prism-setup.exe` and `Prism.dmg` and then fails the run
   unless the draft has all of these:

   | Asset | Purpose |
   |---|---|
   | `Prism_<version>_x64-setup.exe` | Windows installer |
   | `Prism_<version>_x64-setup.exe.sig` | updater signature for Windows |
   | `Prism_<version>_universal.dmg` | macOS installer |
   | `Prism_universal.app.tar.gz` | macOS updater payload |
   | `Prism_universal.app.tar.gz.sig` | updater signature for macOS |
   | `Prism-setup.exe` | versionless copy, same size as the versioned installer |
   | `Prism.dmg` | versionless copy, same size as the versioned installer |
   | `latest.json` | updater manifest |

   It also checks `latest.json`: the version, and for `darwin-aarch64`, `darwin-x86_64` and
   `windows-x86_64` a download URL of this repository that points at an asset of the release,
   with a signature identical to that asset's `.sig`.

## After the run

1. Open the draft on GitHub and check the notes, the assets and that the installers start.
2. Publish the draft by hand. Only then do the updater (`releases/latest/download/latest.json`)
   and the versionless links (`releases/latest/download/Prism.dmg`, `Prism-setup.exe`) resolve.
   A prerelease is never "latest", so a beta does not reach installed apps through the updater.
3. To redo a run, delete the draft (and the tag if it was pushed) first, or the second run adds
   to the same draft.

## Secrets

The workflow reads `TAURI_SIGNING_PRIVATE_KEY` and `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` from the
repository secrets. They are never written to a file in the repository. The public key is in
`src-tauri/tauri.conf.json`.

## Scripts

- `.github/release-notes.sh <tag>` prints the CHANGELOG section of a tag.
- `.github/verify-release-assets.sh <version> <owner/repo> <dir>` is the asset check of step 4.
