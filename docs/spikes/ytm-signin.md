# YouTube Music sign-in spike (Windows)

Status: signed-out checks done, **owner sign-in test still open**. macOS part: open until a macOS build exists (M6).

Spike code: branch `spike-ytm`, worktree `C:\Projects\private\Prism-wt\spike-ytm` (throwaway, delete after the owner test). Nobody typed credentials; nothing was signed in.

## What was verified signed-out (Windows, WebView2, Tauri 2.12)

| Check | Result |
| --- | --- |
| Second `WebviewWindow` on `https://music.youtube.com`, label `ytm`, 1200x800 | Works. Page renders fully. First load shows the EU cookie consent page (`consent.youtube.com`); "Alle ablehnen" leads to the normal UI. |
| Different WebView2 data directory per window (`WebviewWindowBuilder::data_directory`, `<appData>/ytm-webview`) | Accepted, no error. The ytm window still appeared as a CDP target on the main window's remote debugging port, so it shares the browser process/environment. Whether the cookies really live in the separate folder was not inspected. |
| Default user agent | `Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/155.0.0.0 Safari/537.36 Edg/155.0.0.0` (Edge-flavoured Chrome UA, not modified). |
| Google sign-in page `https://accounts.google.com/ServiceLogin?continue=https://music.youtube.com` in a second window (`ytm-signin`, same data dir) | Loads and redirects to `/v3/signin/identifier?...flowName=GlifWebSignIn`. Shows the normal email/phone field and "Weiter". **No "This browser or app may not be secure" block at this step.** Caveat: Google normally decides this after the identifier/password submit, so the real answer needs the owner test. |
| `navigator.mediaSession.metadata` while playing (public, signed-out playback works: clicking a play button on the home page starts a track) | Present: `title`, `artist`, `album`, `artwork[]`. Example: title "Ich weiß, was du brauchst", artist "Kerstin Ott und Maite Kelly", album "Ich weiß, was du brauchst". Artwork sizes `60x60`, `120x120`, `226x226`, `544x544` (googleusercontent URLs). `navigator.mediaSession.playbackState` = `playing`. For a video-type item (ad-like clip) only one `320x180` artwork and empty album. |
| `#play-pause-button` | Exists, tag `YT-ICON-BUTTON`, `title`/`aria-label` = "Pausieren" (localized, do not match on it). `.click()` toggled `video.paused` true then false. Works. |
| `.next-button` | Exists, tag `YT-ICON-BUTTON`. `.click()` advanced to the next track (title changed, URL `watch?v=` changed). Works. |
| `<video>` element | Present; `paused` and `currentTime` readable. |

Selectors to use: `#play-pause-button`, `.next-button`, `video`, plus `navigator.mediaSession.metadata`.

## Probe transport

- Remote IPC for the music.youtube.com window was deliberately **not** granted (security review). No capability file for remote URLs.
- `spike_ytm_probe` runs the JS with `WebviewWindow::eval`; the injected script writes its JSON into the URL fragment (`#prismprobe=<encoded json>`) via `history.replaceState`, and Rust reads it back from `window.url()`. This works. Downsides for a real implementation: the fragment stays in the URL until the SPA navigates, and a repeat probe can read a stale value. For M5 prefer a narrow, per-window app-manifest capability for one command, or a custom `initialization_script` plus events.
- The spike commands are no-ops (`Err("dev only")`) in release builds via `cfg!(debug_assertions)`; the Settings buttons are behind `import.meta.env.DEV`.

## Still open (owner)

1. Whether Google lets the owner finish signing in inside the embedded WebView2 ("This browser or app may not be secure" typically appears only after submitting the email/password).
2. If blocked in `ytm`: whether signing in through the `ytm-signin` window (same data dir) and then reloading `ytm` carries the session over.
3. Whether the probe and button clicks behave the same when signed in (Premium, personal queue).
4. macOS (WKWebView): open until a macOS build exists (M6).

If neither path works on Windows, stop and decide before continuing past M1.

## Owner test steps

1. `cd C:\Projects\private\Prism-wt\spike-ytm`
2. `npm run tauri dev` (the branch defaults to dev ports 1450/1451 and does not clash with the main app on 1440/1441; the first Rust build takes a few minutes). If your main Prism dev app is running at the same time, it shares the app identifier and WebView2 data folder, so close it first or start the spike with `npm run tauri dev -- --config '{"identifier":"de.wolfganglinz.prism.spike"}'` (write the JSON to a file and pass the path if your shell strips the quotes).
3. Open Settings (the bottom section "YTM sign-in spike (dev only)").
4. Click "Open YouTube Music". Reject or accept cookies, then try **Sign in** with your own account in that window. Note whether Google shows "This browser or app may not be secure", asks for extra verification, or lets you in.
5. If it is blocked: click "Open Google sign-in", sign in there, close it, then close and reopen the YouTube Music window (or reload it) and check whether you are signed in.
6. When signed in, start a song and click "Probe" in Settings. The JSON appears below the buttons. Click play/pause and next inside the YouTube Music window to confirm they work.
7. Report back: (a) was sign-in allowed in `ytm`, (b) the exact Google message if blocked, (c) did the `ytm-signin` route carry the session, (d) the probe JSON while signed in, (e) anything odd (CAPTCHA loops, repeated 2FA, Premium prompts).
