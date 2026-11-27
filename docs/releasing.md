# Releasing Prism

A release is made by GitHub Actions from a tag. Nothing is published automatically: the workflow
only fills a draft, and the owner publishes it.

## Before the tag

1. `CHANGELOG.md` has a `## <version>` section for the version in `src-tauri/tauri.conf.json`.
   That section is the release body and the notes the app shows, so it ends with the install
   notes. The workflow adds nothing to it.
2. The version in `package.json` (and its lock file), `src-tauri/Cargo.toml` and
   `src-tauri/tauri.conf.json` is exactly the tag without the `v`. A beta sets the full version
   first: tag `v1.0.0-beta.1` needs `1.0.0-beta.1` in all three, and its files are named
   `Prism_1.0.0-beta.1_x64-setup.exe`. The NSIS installer accepts a prerelease version (only MSI
   would not, and none is built). The beta takes its notes from the `1.0.0` section when there
   is no `1.0.0-beta.1` section. Going to 1.0.0 means setting the version back.
   The Windows installer is code-signed (`docs/CODE-SIGNING.md`), so the install notes of a new
   version no longer need the "not code-signed" line for Windows; macOS stays unsigned.
3. CI is green on the commit (`.github/workflows/ci.yml` for macOS and Windows,
   `.github/workflows/linux.yml` for the Raspberry Pi package).

## Running it

The owner pushes the tag (`git tag v1.0.0 && git push origin v1.0.0`), or starts the Release
workflow by hand from the Actions tab and types the tag. The workflow:

1. **prepare** checks the tag shape, that it equals the app version in `tauri.conf.json`,
   `package.json` and `Cargo.toml`, that an existing tag on origin points at this commit, and
   that `CHANGELOG.md` has text for it. It fails before any build otherwise. Runs for the same
   tag never overlap.
2. **ffmpeg** (`ffmpeg.yml`) builds the minimal LGPL FFmpeg for macOS (universal), Windows
   (x64, cross-built on Linux) and Linux arm64 (natively on the arm64 runner), cached by the
   hash of the build scripts. `check-ffmpeg.sh` runs on a Mac, on `windows-latest` and on the
   arm64 runner and rejects GPL or non-free flags, missing or extra decoders and a wrong
   version. See `docs/ffmpeg.md`.
3. **release** builds one platform after the other (macOS universal DMG and app, the Windows
   NSIS installer, then the Raspberry Pi `.deb` on `ubuntu-22.04-arm`), bundles the FFmpeg
   sidecars, signs the updater artifacts and adds everything to one draft release. A tag with
   a `-` in it is marked as a prerelease.
4. **assets** copies the installers to `Prism-setup.exe`, `Prism.dmg` and `Prism-arm64.deb`,
   adds the Pi's kiosk setup (`scripts/pi` as `prism-pi-kiosk.tar.gz`) and then fails the run
   unless the draft has all of these (`<version>` is the full app version, beta suffix included):

   | Asset | Purpose |
   |---|---|
   | `Prism_<version>_x64-setup.exe` | Windows installer |
   | `Prism_<version>_x64-setup.exe.sig` | updater signature for Windows |
   | `Prism_<version>_universal.dmg` | macOS installer |
   | `Prism_universal.app.tar.gz` | macOS updater payload |
   | `Prism_universal.app.tar.gz.sig` | updater signature for macOS |
   | `Prism_<version>_arm64.deb` | Raspberry Pi package, also the updater payload |
   | `Prism_<version>_arm64.deb.sig` | updater signature for the Raspberry Pi |
   | `Prism-setup.exe` | versionless copy, same size as the versioned installer |
   | `Prism.dmg` | versionless copy, same size as the versioned installer |
   | `Prism-arm64.deb` | versionless copy, same size as the versioned package |
   | `prism-pi-kiosk.tar.gz` | the Raspberry Pi kiosk setup (`scripts/pi`) |
   | `latest.json` | updater manifest |
   | `ffmpeg-<FFmpeg version>.tar.xz` | the pinned FFmpeg source (LGPL), checked by its SHA-256 |
   | `prism-ffmpeg-build-scripts.tar.gz` | the scripts that build the shipped FFmpeg, and `docs/ffmpeg.md` |

   It also checks `latest.json`: the version, and for `darwin-aarch64`, `darwin-x86_64`,
   `windows-x86_64` and `linux-aarch64-deb` (or `linux-aarch64`, which the Pi's updater tries
   second) a download URL of this repository that points at an asset of the release, with a
   signature identical to that asset's `.sig`.

## After the run

1. Open the draft on GitHub and check the notes, the assets and that the installers start.
2. Publish the draft by hand. Only then do the updater (`releases/latest/download/latest.json`)
   and the versionless links (`releases/latest/download/Prism.dmg`, `Prism-setup.exe`,
   `Prism-arm64.deb`, `prism-pi-kiosk.tar.gz`) resolve.
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
