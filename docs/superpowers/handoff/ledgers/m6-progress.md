# M6 Release v1.0.0 - SDD ledger

Plan: docs/superpowers/plans/2026-10-22-prism-m6-release.md. Branch PRISM-6 cut from PRISM-5 at 9d5ea4d, controller worktree ~/Projects/Prism-wt/m6 (PRISM-5 synced in later as fixes land).

Machine: the owner's Mac. Windows checks: `cargo check --target x86_64-pc-windows-gnu --all-targets` (mingw installed, target dir in the scratchpad `wintarget`).

## Owner answers (2026-10-22)
- Updater key: generated on the Mac like the other apps: `~/.tauri/prism-updater.key` (+ .pub, .pass). Secrets set in wl-lankin/prism. Public key: `~/.tauri/prism-updater.key.pub` (the controller pastes it into tauri.conf.json in Task 2).
- Hand checks M3 (Navidrome) and M5 (Windows) after the beta, before v1.0.0.
- v1.0.0-beta.1 first.

## Waves
- A: Tasks 1, 3, 4 in parallel.
- B: Task 2 (updater), Task 5 (CI and release; needs Task 3's sidecar names).
- C: Task 6. D: Task 7 (dry-run draft release, controller only triggers with the owner's yes). Then 8.

## Progress
- [x] Task 1: af6a59c, controller corrections amended (Flow only favorites from library and Navidrome; the global shortcut only toggles the mini player; full format list from spec 5.2; artists in favorites; stars both ways and scrobbles). Merged c4caf4b.
- [x] Task 3: f9246d6 + controller follow-up 4686f2a (review minors: dst, adpcm_ima_qt, wmavoice decoders; check fails on --enable-lib; https-only curl; docs wording). Reviewer approved with minors. Merged abc38ba, npm 452, cargo 930. LGPL source assets moved to Task 5.
- [x] Task 2: 6fdc009 + 7da9e96 (15 min retry, probe_online, notify-only on quit) + 3ee3493 (installsOnQuit in state). Controller read the quit path. Merged, pushed; npm 464, cargo 959, clippy and Windows clean. Open for Task 7: check the .sig trusted comment carries version, then requireSignedVersion true.
- [x] Task 5: c8d448b + fixes 8920c6d (exact versions incl. beta, version match across three files, tag sha check, concurrency, LGPL source tarball + build scripts as assets, 10 expected assets, CI only PRISM-6/main, tauri-action 0.6.2). Controller read the fix diff, actionlint clean. Merged, pushed; first CI run 39758153313.
- [x] Task 4: 6b169c7 (panel_behavior, apply_panel: not hiding on deactivate, all Spaces, full-screen auxiliary, stationary, out of window cycling, floating only with keep-on-top; no nonactivatingPanel mask on an NSWindow; Dock Reopen already called main_show, now pinned). Controller read the diff. Merged, PRISM-5 synced e506449, cargo 925. Hand check: whether the panel shows over another app's full-screen Space (else a real NSPanel later).
- [x] Task 6: b4f9b0d (scripts/licenses.mjs, 445 crates, 24 npm, 4 bundled; About list view; README). Controller checked README claims against CHANGELOG. Merged. Screenshots e649d26 from a demo-library run (dev7, PRISM_DATA_DIR prism-data7, built dist, no dev server), owner started playback and the mini player. CI fix c65b1c6 (clippy 1.99 chunks_exact_to_as_chunks; CI uses floating stable, local is 1.97).

## Binding for Task 5 (from Task 1)
- CHANGELOG.md's `## 1.0.0` section already ends with `---` and the first-install notes: the release workflow uses the section as the body and does NOT append its own install hints.

## Rulings
- One cargo test filter per run.
- Agents never push tags, publish, change visibility or touch secrets/keys.

## Binding for later tasks

## Deferred minors

## Owner decisions open (from the Task 5 review)
- The updater reads releases/latest/download/latest.json: it skips prereleases and drafts and needs the repo public. So a beta.1 -> beta.2 updater test needs the repo public and beta.2 published as a normal release (or a test endpoint).
- workflow_dispatch only works from the default branch (PRISM-3 today). Dry run in Task 7 needs a tag push or a default branch change.
- Owner 2026-10-22: no updater test, no beta. Go straight to v1.0.0 (Task 7 dry run replaced by the real v1.0.0 draft). Publishing the draft and making the repo public still need the owner's yes.
- v1.0.0: main 6284b56 (merge of PRISM-6), tag pushed by the owner. Release run 39768759532 green, draft with all 10 assets, latest.json urls on v1.0.0, sigs carry version:1.0.0, notes = CHANGELOG section. CI on main failed once in check-ffmpeg.sh (SIGPIPE under pipefail, flaky); fixed on PRISM-6 cb93132 together with requireSignedVersion true (applies from the next release). Open: owner yes to publish the draft and make the repo public.
- Owner 2026-10-22: yes. v1.0.0 published (latest, not prerelease), repo wl-lankin/prism now PUBLIC. latest.json and versionless links answer 200. Default branch still PRISM-3. Open: cb93132 (check-ffmpeg SIGPIPE, requireSignedVersion) to main with the next release; cleanup worktrees.
