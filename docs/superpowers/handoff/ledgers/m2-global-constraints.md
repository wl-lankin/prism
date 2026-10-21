## Global Constraints

- Platforms: Windows 11 and macOS 11+ (universal). Developed and checked on Windows; anything Windows-only is behind `cfg(target_os = "windows")` with a macOS counterpart or a no-op, and every such pair is named in its task.
- Repo `C:\Projects\private\Prism`, work on branch `PRISM-2`, repo-local identity `Wolfgang Linz <27097016+wl-lankin@users.noreply.github.com>`. Commit, never push.
- App identifier `de.wolfganglinz.prism`, productName `Prism`, version `1.0.0`, MIT, publisher "Wolfgang Linz", homepage `https://wolfgang-linz.de`.
- No em dash or en dash anywhere (code, UI strings, docs, commits). No AI attribution or session links anywhere. No sparkle glyphs.
- Palette: background `#0d0d10`, surface `#141418`, raised `#1f1f25`, text `#f3f3f5`; spectrum `#ff4d6d`, `#ffc94d`, `#4dffb2`, `#4da6ff`. Source colors: navidrome `#4dffb2`, apple `#ff4d6d`, youtube `#ffc94d`, radio `#4da6ff`, local `#cccccc`.
- Reduced motion: only large motion is switched off; hover and fade transitions keep running at 120 ms (`--dur`).
- Window look (owner decision): no Mica, no vibrancy, no OS shadow, no DWM border or rounded system corners. The main window's Flow ring is 96 px and hangs half out of the card's bottom edge over a transparent strip; the mini player's ring (76 px, `mini-v2.html`) hangs half out of its left edge.
- The mini player never steals focus and never appears without a user action (ring/tray/taskbar click, shortcut, "Open with").
- Dev server ports 1440 (Vite) and 1441 (HMR). Never kill processes by image name (`taskkill /IM node.exe` is forbidden); stop only PIDs you started.
- Write files with Write/Edit, never Bash heredocs. Frontend tests are `src/**/*.spec.ts`.
- Commands: `npm test`, `npm run check`, `cargo test --manifest-path src-tauri/Cargo.toml [filter]`. The local `ffmpeg`/`ffprobe` on PATH are fine for development.
- Migrations are append-only from now on: the owner has a real library DB. Never edit `0001_init.sql` or any migration an earlier task added.
- The only network calls in M2 are MusicBrainz and the Cover Art Archive (Task 18), with User-Agent `Prism/<version> ( https://wolfgang-linz.de )`, at most 1 request per second, and only while the setting is on.
- Never use the owner's real library, accounts or credentials in tests, fixtures or screenshots.
- Every new UI string goes into both `en` and `de` in `src/lib/i18n.ts` (the existing `i18n.spec.ts` checks keys, placeholders and dashes).

