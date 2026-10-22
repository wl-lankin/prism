# Rules for every M3 implementer

- **Where to work:** your own worktree and branch only. Commit with `git -c user.name="Wolfgang Linz" -c user.email="27097016+wl-lankin@users.noreply.github.com"`, using the commit message from the plan. Never push.
- **What to read:** the plan at `docs/superpowers/plans/2026-10-21-prism-m3-navidrome.md` (Global Constraints, Review Focus, Decisions, your task, Execution notes) and the ledger at `C:\Projects\private\Prism\.superpowers\sdd\2026-10-21-prism-m3-navidrome\progress.md`. Ledger rulings and "Binding for Task N" items are mandatory. Read the ledger, but never edit it.
- **Plan vs. code:** the plan is older than the merged code. Check each interface it assumes against the actual code, adapt, and list every deviation in your report.
- **Workflow:** TDD as the task's steps say.
- **Writing:**
  - Never use em-dashes or en-dashes.
  - No AI attribution, no session links.
  - Write files with Write/Edit, never with Bash heredocs, `sed -i` or scripts that write files.
- **Data:**
  - Fake data only. The fake account is `demo` / `demo-pass`.
  - Test servers bind `127.0.0.1:0`.
  - Never a real server URL or credentials.
  - Credentials never appear in errors, logs, events or Debug output.
- **Locks:**
  - New cycle steps in `navidrome/mod.rs` check `is_current(epoch)` under the DB lock before writing.
  - Never take the DB lock while holding `Core::inner`.
  - Network calls never run under the DB lock.
- **Processes:**
  - Do not launch the app.
  - Record the PIDs you start and stop only those. Never kill by image name.
  - Never touch `%APPDATA%` or `%LOCALAPPDATA%\de.wolfganglinz.prism`.
  - Reap any FFmpeg children your tests start.
- **Cargo:** set `CARGO_TARGET_DIR` to `src-tauri/target` inside your worktree.
- **Frontend:** if you change frontend files, `npm test` and `npm run check` must stay green. Run `npm ci` first if `node_modules` is missing.
- **Verify:**
  - Run the full `cargo test`, plus your filter 3x for flakiness.
  - Run `cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings` and `cargo fmt --check`.
  - Paste the numbers in your report.
- **UI tasks (15-18):**
  - Follow the existing design: graphite palette, spectral accents, source colors (navidrome `#4dffb2`, local `#cccccc`), 120 ms `--dur` transitions that keep running under reduced motion.
  - Every new string goes in `src/lib/i18n.ts` in both EN and DE.
  - Make the visual check if your machine has the memory for it. To do it:
    1. Start the fake server with `cargo test --manifest-path src-tauri/Cargo.toml serve_for_ui -- --ignored --nocapture`. Set `PRISM_FAKE_SERVER_MINUTES` if needed. It prints its URL and the `demo` account.
    2. Run your own dev instance with `PRISM_DATA_DIR` set to a fresh temp folder, a Tauri `--config` override that gives it its own identifier (`de.wolfganglinz.prism.dev<N>`), and your own Vite/HMR/CDP ports in 1500-1599. Use an uncommitted override config and delete it afterwards.
    3. Connect it to the fake server through the UI by sending DOM events over CDP.
    4. Take screenshots over CDP and save them to `C:\Projects\private\Prism\.superpowers\sdd\2026-10-21-prism-m3-navidrome\shots\task<N>-*.png`.
  - Never inject OS mouse or keyboard input. Never move or focus the owner's windows.
  - Keep the instance short-lived, then stop exactly the PIDs you started.
  - Never point anything at a real server.
- **Report:** write it to `C:\Projects\private\Prism\.superpowers\sdd\2026-10-21-prism-m3-navidrome\task<N>-report.md`. Return a short summary of commits, numbers, deviations and concerns.
