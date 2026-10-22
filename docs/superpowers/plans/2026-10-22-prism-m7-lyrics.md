# M7 Lyrics

Branch `PRISM-7`, cut from `PRISM-6` at `ccd41a3` (v1.0.0 plus the FFmpeg check fix). The controller commit `84113b5` adds `src-tauri/src/lyrics/mod.rs` with the shared model (`Lyrics`, `Line`, `LyricsSource`, `Lyrics::plain`, `Lyrics::synced`, `Lyrics::instrumental`) and empty submodules `cache`, `local`, `lrc`, `lrclib`. Every task builds on that model and does not change its public shape; a needed change goes to the controller.

## Goal

Prism shows the lyrics of the playing song: synced lyrics follow the song line by line (click a line to jump there), plain lyrics scroll. In Now Playing (instead of the queue column, switchable) and in a side panel in the main window (like the queue panel).

## Owner answers (2026-10-22)

- Sources: files and sidecar `.lrc`, Navidrome, LRCLIB online, YouTube Music. All four.
- Display: Now Playing plus a side panel in the main window. Not in the mini player.
- LRCLIB: on by default, can be switched off in Settings (like the MusicBrainz cover lookup).

## Global Constraints

- Lyrics text from any source is untrusted and goes through `Lyrics::plain` / `Lyrics::synced` (cleaning and caps). The frontend renders it as text, never as HTML.
- No network call under the DB lock or any service lock. No DB lock while holding another service lock.
- With "Search lyrics online" off, Prism sends nothing to lrclib.net (a test proves it). YouTube Music lyrics come from the page the user is already using, so they stay on with the setting off.
- Fixtures and tests use invented lyrics only (no real song texts) and fake servers on `127.0.0.1:0`. The only internet test is an `#[ignore]` LRCLIB live test that prints counts, not text.
- Agents never open music.youtube.com, never sign in. The YouTube Music path is tested against the fake page (`src-tauri/src/ytm/testpage/`); the live check is the owner's.
- Every UI string in EN and DE (`src/lib/i18n.ts`), real umlauts, no em or en dashes anywhere.

## Review Focus

- The YouTube Music lyrics report: strict shape, size cap, request id and video id checked, nothing from the page reaches a URL, a file path or HTML.
- Privacy: the setting gates every LRCLIB request; the user agent carries no personal data.
- Resolver order and caching (no repeated online lookups for misses within the retry window; errors are never cached as misses).
- Sync accuracy in the UI (interpolated position, line lead, seeking), scroll behaviour, reduced motion.

## Decisions pinned here

### Resolver order

The first match wins; "synced" beats "plain" across sources as listed.

| Playing | Order |
|---|---|
| Local file | sidecar `.lrc` synced, embedded synced, LRCLIB synced*, sidecar plain, embedded plain, LRCLIB plain* |
| Navidrome | Navidrome synced, LRCLIB synced*, Navidrome plain, LRCLIB plain* |
| YouTube Music | LRCLIB synced*, YouTube Music page, LRCLIB plain* |
| Radio | none (the panel says lyrics are not available for radio) |

\* only with "Search lyrics online" on. An LRCLIB "instrumental" answer ends the search as instrumental, unless a local or Navidrome source had text.

### Caching

- Local files are read on demand, never cached (cheap, and edits show at once).
- Navidrome, LRCLIB and YouTube Music results go in a `lyrics_cache` table (migration `0010_lyrics.sql`): `key TEXT PRIMARY KEY, lyrics TEXT NULL (the JSON of Lyrics, NULL = no lyrics), checked_at INTEGER (unix s)`. Found entries are kept; misses are retried after 7 days. Network or server errors are not cached.
- Keys: `lrclib:<artist>|<title>|<album>|<duration s>` (lowercased, trimmed), `navidrome:<song id>`, `youtube:<video id>`.

### Commands (main window only, like the update commands)

- `lyrics_for_track(trackId: string) -> LyricsAnswer` for local and Navidrome tracks.
- `lyrics_for_youtube() -> LyricsAnswer` for the track YouTube Music is playing now.
- `LyricsAnswer` (camelCase JSON): `{ "status": "found", "lyrics": Lyrics } | { "status": "none" } | { "status": "error" }`. `error` means a network or server error with nothing else found; the UI offers "Try again".

### Frontend

- Types `Lyrics`, `LyricsLine`, `LyricsSource`, `LyricsAnswer` in `src/lib/types.ts` mirroring the Rust JSON.
- Side panel: one right-side slot shows either the queue panel or the lyrics panel. Opening one closes the other. Ctrl+Q keeps toggling the queue, Ctrl+L toggles lyrics (check Ctrl+L is free in `App.svelte`). A lyrics button sits next to the queue button in `TransportStrip`. The open panel is remembered as the queue panel is today.
- Now Playing: the side column gets a two-way switch "Up next | Lyrics", remembered.
- Active line: the last line with `startMs <= position + 150 ms`. Position is interpolated with `requestAnimationFrame` between the 250 ms engine updates (Prism) and from `ytm.positionMs` (YouTube Music). While paused it does not run on.
- Auto-scroll keeps the active line near the vertical center. Smooth normally, instant under reduced motion. Wheel, touch or drag scrolling pauses following for 5 s and shows "Back to current line".
- Click (or Enter on) a synced line seeks to its start through `playerSeek`, which routes to the active player.
- Plain lyrics: no highlight, no following, a note "Not synced" at the top.
- A small source line at the bottom: "Lyrics: LRCLIB" / "from the file" / "from Navidrome" / "from YouTube Music".
- States: loading, found, none ("No lyrics found", plus "Online search is off" with a link to Settings when the setting is off), instrumental, radio, error (with "Try again"), nothing playing.

## Tasks

Waves: A = Tasks 1 to 5 in parallel. B = Task 6 (wiring). C = Task 7 (integration), Task 8 (owner hand check).

### Task 1: LRC parser and local lyrics (Sonnet)

Files: `src-tauri/src/lyrics/lrc.rs`, `src-tauri/src/lyrics/local.rs`.

- `lrc::parse(text: &str, source: LyricsSource) -> Option<Lyrics>`:
  - time tags `[mm:ss]`, `[mm:ss.x]`, `[mm:ss.xx]`, `[mm:ss.xxx]`, `[mm:ss:xx]`, minutes above 59; several tags on one line repeat the line.
  - `[offset:+/-ms]` applies to all lines (positive offset means earlier, as in the LRC convention; clamp at 0).
  - metadata tags (`[ar:]`, `[ti:]`, `[al:]`, `[by:]`, `[length:]`, `[re:]`, `[ve:]`, unknown `[xx:...]`) are ignored.
  - enhanced LRC word tags `<mm:ss.xx>` are removed from the text.
  - BOM, CRLF, blank lines.
  - text with no time tag at all goes to `Lyrics::plain`; text with time tags ignores untimed lines (except metadata).
- `local::read(path: &Path) -> LocalLyrics { synced: Option<Lyrics>, plain: Option<Lyrics> }`:
  - sidecar: `<stem>.lrc` next to the file, case-insensitive extension match, size cap 512 KiB, UTF-8 with lossy fallback (and UTF-16 with BOM). Source `LrcFile`.
  - embedded via lofty 0.25: unsynchronized lyrics (ID3 USLT, Vorbis `LYRICS`/`UNSYNCEDLYRICS`, MP4 `©lyr`, APE `Lyrics`) and ID3 SYLT (millisecond timestamps only; frames in MPEG frames are skipped). Embedded text that parses as LRC counts as synced. Source `Embedded`.
  - `synced`: sidecar synced, else embedded synced. `plain`: sidecar plain, else embedded plain.
  - Never panics on broken files; errors are `None`.
- Tests: the parser cases above; a temp dir with an `.lrc` sidecar (upper-case `.LRC` too); MP3, FLAC and M4A files with lyrics written by lofty in the test (start from the existing test audio generators in the repo, find them first); a SYLT frame; broken files.
- Commit: `PRISM-7: LRC parser and lyrics from files and sidecar .lrc`.

### Task 2: LRCLIB client and lyrics cache (Sonnet)

Files: `src-tauri/src/lyrics/lrclib.rs`, `src-tauri/src/lyrics/cache.rs`, `src-tauri/src/db/migrations/0010_lyrics.sql`, `src-tauri/src/db/mod.rs`.

- `Lrclib::new(base_url)`: reqwest client, user agent `Prism/<version> ( https://github.com/wl-lankin/prism )`, 10 s timeout, at most one request at a time with a 200 ms gap.
- `Lrclib::find(&self, q: &Query) -> Result<Found, LrclibError>` with `Query { artist, title, album, duration_ms }`, `Found { synced_lrc: Option<String>, plain: Option<String>, instrumental: bool }` (raw texts, capped at 256 KiB each; empty when nothing was found; Task 6 turns them into `Lyrics` with `lrc::parse` and `Lyrics::plain`, so this task does not depend on Task 1):
  1. `GET /api/get?artist_name&track_name&album_name&duration` (seconds, rounded). 200: `syncedLyrics`, `plainLyrics`, `instrumental` (blank strings count as absent). 404: step 2.
  2. `GET /api/search?track_name&artist_name`: take the first entry with `|duration - q| <= 2 s`, preferring one with synced lyrics.
  3. If both missed and the title ends in a bracket or dash suffix mentioning remaster, live, version, edit, mono or stereo (case-insensitive, EN and DE "Version"), repeat 1 and 2 once with the suffix removed.
  - Response size cap 1 MiB. Non-2xx other than 404, timeouts and bad JSON are `LrclibError`.
- `cache::get(conn, key, now) -> Cached { Hit(Option<Lyrics>) | Miss }` (a stored miss older than 7 days counts as `Miss`), `cache::put(conn, key, Option<&Lyrics>, now)`, `cache::lrclib_key(&Query)`.
- Migration `0010_lyrics.sql` as in "Caching"; append to the migration list.
- Tests: a fake LRCLIB on `127.0.0.1:0` (follow the style of the existing fake servers) for 200 synced, 200 plain only, instrumental, 404 then search hit, search duration mismatch, suffix retry, 500, oversize body, timeout; cache expiry and round trip. One `#[ignore]` live test against lrclib.net that prints only counts.
- Commit: `PRISM-7: LRCLIB client and the lyrics cache`.

### Task 3: Navidrome lyrics (Sonnet)

Files: `src-tauri/src/navidrome/client.rs`, the Navidrome service file that owns the client, `src-tauri/src/navidrome/testserver.rs`, fixtures under `src-tauri/tests/fixtures/subsonic/`.

- Detect OpenSubsonic extensions: when `ping` says `openSubsonic`, call `getOpenSubsonicExtensions` once per connection and remember whether `songLyrics` is offered.
- `lyrics(song_id, artist, title) -> Result<Option<(Option<Lyrics>, Option<Lyrics>)>>` returning (synced, plain):
  - with `songLyrics`: `getLyricsBySongId?id=`; take the first `structuredLyrics` entry with `synced: true` (apply its `offset` in ms with the sign the OpenSubsonic spec defines; quote the spec sentence in the report), and the first unsynced one as plain. Line `start` is ms.
  - without: `getLyrics?artist&title`; `value` non-empty becomes plain.
  - Song id is the Navidrome id without the `navidrome:` prefix of Prism's track id (check the actual prefix).
- Testserver: both endpoints, the extensions list switchable per test, invented lyrics.
- Tests: synced plus plain, plain only, empty, old server without OpenSubsonic, offset, error status.
- Commit: `PRISM-7: Lyrics from Navidrome`.

### Task 4: YouTube Music lyrics from the page (Opus)

Files: `src-tauri/src/ytm/bridge.js`, `src-tauri/src/ytm/report.rs`, `src-tauri/src/ytm/mod.rs`, the fake page in `src-tauri/src/ytm/testpage/`.

- Rust asks: a service method `request_lyrics(&self) -> u32` evals the constant-format snippet `window.__prismLyricsRequest = <n>;` (n a counter, like the existing `__prismSeekMs`) and returns `n`.
- The bridge notices the request, takes the current video id (`movie_player.getVideoData().video_id`, else the `v` URL parameter), and asks YouTube Music's own API from inside the page, same origin, as the ytmusicapi project does for `get_lyrics`: `POST /youtubei/v1/next` with the page's `INNERTUBE_CONTEXT` and the video id, find the lyrics tab's browse id (none, or the tab unselectable: no lyrics), then `POST /youtubei/v1/browse` with it and read the description shelf text and its footer (the provider, e.g. "Source: LyricFind"). Timed lyrics are out of scope.
- It reports `{ kind: 'lyrics', request: n, videoId, text, provider }` or `{ kind: 'lyrics', request: n, videoId, text: null }` through the existing `ytm_report` path.
- `report.rs`: a new `Report::Lyrics(PageLyrics { request: u32, video_id: String, text: Option<String>, provider: Option<String> })`, own cap 64 KiB, video id must match `^[A-Za-z0-9_-]{11}$`, provider capped like other text. Unknown or extra fields are refused, as for the other report kinds.
- The service keeps the latest request id and video id; `lyrics(&self, timeout) -> Result<Option<(String video_id, Lyrics)>>` sends a request and waits (default 8 s) for the matching report; a report for an older request or another video is dropped. Lyrics go through `Lyrics::plain(LyricsSource::Youtube, text)`.
- Expose the current video id in the service state (needed for the cache key), from the state reports (add `videoId` to the state report, same validation).
- Fake page: serves `/youtubei/v1/next` and `/youtubei/v1/browse` with invented lyrics, plus a video without a lyrics tab.
- Tests: report parsing (valid, oversize, bad id, wrong request, extra fields), the request/answer round trip through the fake page if the existing tests drive it there, else the Rust side with a fake page trait.
- Commit: `PRISM-7: Lyrics from the YouTube Music page`.

### Task 5: Lyrics UI (Sonnet)

Files: `src/lib/types.ts`, `src/lib/ipc.ts`, new `src/lib/stores/lyrics.svelte.ts`, new `src/lib/components/LyricsView.svelte` and `LyricsPanel.svelte`, `NowPlaying.svelte`, `QueuePanel.svelte` / `src/lib/stores/queue.svelte.ts` (the shared side slot), `TransportStrip.svelte`, `App.svelte`, `Settings.svelte`, `src/lib/stores/settings.svelte.ts`, `src/lib/i18n.ts`, specs next to the existing ones.

- Everything under "Frontend" in the decisions.
- `ipc.ts`: `lyricsForTrack(trackId)`, `lyricsForYoutube()` calling the commands above.
- Store: loads when the playing track changes (Prism: `player.track.id`; YouTube Music: title, artist and duration from `ytm.state`; radio: the radio state), keeps one answer per track key so switching views does not refetch, drops answers for a track that is no longer playing, "Try again" refetches.
- Settings: `lyricsLookup: boolean` (default `true`) in the settings type and store; a toggle "Search lyrics online" with the hint "Sends artist, title, album and length to lrclib.net." next to the cover lookup toggle. DE: „Songtexte online suchen", „Schickt Künstler, Titel, Album und Länge an lrclib.net."
- Look: lyrics in the same type scale as the Now Playing title area, past lines dimmed, the active line full white with a soft spectral glow (no animation loop), future lines at medium opacity. Follow the existing graphite style.
- Specs (vitest, mocked IPC): active line selection at boundaries, interpolation pause, follow pause and resume after 5 s, click seeks, each state renders, the side slot switches between queue and lyrics, the setting toggles.
- Commit: `PRISM-7: Lyrics in Now Playing and the side panel`.

### Task 6: Resolver, commands and setting (Sonnet, after wave A)

Files: `src-tauri/src/lyrics/mod.rs` (a `LyricsService`, new code below the model), `src-tauri/src/commands.rs`, `src-tauri/src/lib.rs`, `src-tauri/src/settings.rs`, capabilities.

- `LyricsService` holds the LRCLIB client, the DB handle, the Navidrome service and the YouTube Music service. Implements the resolver order and caching above, single flight per key (two calls for the same track wait for one lookup).
- Setting `lyrics_lookup: bool` (default true, `lyricsLookup` in JSON), checked on every call.
- Commands `lyrics_for_track` and `lyrics_for_youtube` as above, main window only.
- Tests with the fake LRCLIB, the fake Navidrome and a fake YouTube Music source: every row of the resolver table, setting off sends nothing to the fake LRCLIB, cache hit makes no request, error is not cached, single flight.
- Commit: `PRISM-7: Lyrics resolver and commands`.

### Task 7: Integration (controller)

Merge, run every check, Windows `cargo check`, fix the seams, update `CHANGELOG.md` (Unreleased: Lyrics), README feature list, About licenses if a crate was added (`node scripts/licenses.mjs`).

### Task 8: Owner hand check

A dev build with a scratch data dir and the demo library plus the owner's own choice of songs:
- local file with a sidecar `.lrc`, one with embedded lyrics, one without (LRCLIB)
- a Navidrome song (if the owner wants to use the server; otherwise skipped)
- YouTube Music: lyrics of a playing song
- sync feels right, click to seek, scroll pause, the side panel and Now Playing switch, setting off.

## Execution notes

- Worktrees `~/Projects/Prism-wt/m7-t0N`, branches `PRISM-70N`, cut from `PRISM-7` after `84113b5`.
- Wave A tasks are independent of each other; they share only the model in `84113b5`. Expected merge seams: `lyrics/mod.rs` (none if tasks leave it alone), `i18n.ts`, `db/mod.rs`.
