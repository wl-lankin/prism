//! Online cover lookup for albums without a cover: MusicBrainz finds the release, the Cover
//! Art Archive serves its front image. The only network traffic of the library, and only while
//! the `coverLookup` setting is on. Hosts contacted: musicbrainz.org, coverartarchive.org, and
//! the archive.org host the Cover Art Archive redirects its image downloads to.

use std::io::Read;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::Context;
use rusqlite::params;

use super::covers::{image_extension, CoverStore};
use super::scan::lock;
use super::tags::{UNKNOWN_ALBUM, UNKNOWN_ARTIST};
use crate::db::Db;

/// A `notfound` answer is asked again after this long.
const NOTFOUND_RETRY_SECS: i64 = 30 * 24 * 60 * 60;
/// Releases scoring below this are not trusted to be the album.
const MIN_SCORE: i64 = 90;
/// Fetch errors in a row that end a pass.
const MAX_ERRORS_IN_A_ROW: usize = 3;
const REQUEST_GAP: Duration = Duration::from_secs(1);
const TIMEOUT: Duration = Duration::from_secs(10);
const MAX_IMAGE_BYTES: u64 = 10 * 1024 * 1024;
/// `library://changed` after found covers is sent at most this often.
const EMIT_GAP: Duration = Duration::from_secs(2);

/// Finds the front cover of an album. `Ok(None)` means there is none; `Err` means the lookup
/// could not be completed (network, server) and should be retried later.
pub trait CoverFetcher: Send + Sync {
    fn fetch(&self, artist: &str, album: &str) -> anyhow::Result<Option<Vec<u8>>>;
}

/// A fetch failed because the server could not be reached (connect error or timeout). It ends
/// the pass; other errors only skip the album.
#[derive(Debug)]
pub struct Unreachable;

impl std::fmt::Display for Unreachable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the server could not be reached")
    }
}

impl std::error::Error for Unreachable {}

fn classify(err: reqwest::Error) -> anyhow::Error {
    if err.is_connect() || err.is_timeout() {
        anyhow::Error::new(err).context(Unreachable)
    } else {
        err.into()
    }
}

/// Lucene query for a release search. Quotes and backslashes are escaped inside the phrases.
pub fn mb_query(artist: &str, album: &str) -> String {
    fn escape(text: &str) -> String {
        text.replace('\\', "\\\\").replace('"', "\\\"")
    }
    format!(
        "release:\"{}\" AND artist:\"{}\"",
        escape(album),
        escape(artist)
    )
}

/// The MBID of the first release in a search response that scores at least 90.
pub fn pick_release(json: &serde_json::Value) -> Option<String> {
    json.get("releases")?
        .as_array()?
        .iter()
        .find_map(|release| {
            let score = release.get("score")?.as_i64()?;
            if score < MIN_SCORE {
                return None;
            }
            release.get("id")?.as_str().map(str::to_owned)
        })
}

/// Spaces requests at least `gap` apart, across threads.
struct RateLimiter {
    gap: Duration,
    last: Mutex<Option<Instant>>,
}

impl RateLimiter {
    fn new(gap: Duration) -> RateLimiter {
        RateLimiter {
            gap,
            last: Mutex::new(None),
        }
    }

    /// Blocks until a request may start. The lock is held while sleeping so callers queue up.
    fn wait(&self) {
        let mut last = self.last.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(at) = *last {
            let elapsed = at.elapsed();
            if elapsed < self.gap {
                std::thread::sleep(self.gap - elapsed);
            }
        }
        *last = Some(Instant::now());
    }
}

/// MusicBrainz + Cover Art Archive over HTTPS, at most one request per second.
pub struct MusicBrainz {
    client: reqwest::blocking::Client,
    limiter: RateLimiter,
}

impl MusicBrainz {
    pub fn new() -> anyhow::Result<MusicBrainz> {
        let client = reqwest::blocking::Client::builder()
            .user_agent(format!(
                "Prism/{} ( https://wolfgang-linz.de )",
                env!("CARGO_PKG_VERSION")
            ))
            .timeout(TIMEOUT)
            .connect_timeout(TIMEOUT)
            .build()
            .context("building the HTTP client")?;
        Ok(MusicBrainz {
            client,
            limiter: RateLimiter::new(REQUEST_GAP),
        })
    }
}

impl CoverFetcher for MusicBrainz {
    fn fetch(&self, artist: &str, album: &str) -> anyhow::Result<Option<Vec<u8>>> {
        self.limiter.wait();
        let response = self
            .client
            .get("https://musicbrainz.org/ws/2/release/")
            .query(&[
                ("query", mb_query(artist, album).as_str()),
                ("fmt", "json"),
                ("limit", "5"),
            ])
            .send()
            .map_err(classify)
            .context("MusicBrainz search")?
            .error_for_status()
            .context("MusicBrainz search")?;
        let json: serde_json::Value = response.json().context("MusicBrainz response")?;
        let Some(mbid) = pick_release(&json) else {
            return Ok(None);
        };

        self.limiter.wait();
        let response = self
            .client
            .get(format!(
                "https://coverartarchive.org/release/{mbid}/front-500"
            ))
            .send()
            .map_err(classify)
            .context("Cover Art Archive")?;
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        let response = response.error_for_status().context("Cover Art Archive")?;
        // One byte past the cap tells an oversized image from one of exactly the cap.
        let mut bytes = Vec::new();
        response
            .take(MAX_IMAGE_BYTES + 1)
            .read_to_end(&mut bytes)
            .context("reading the cover")?;
        if bytes.len() as u64 > MAX_IMAGE_BYTES {
            return Ok(None);
        }
        Ok(Some(bytes))
    }
}

/// Stands in when the HTTP client could not be built: every fetch fails as unreachable, so
/// nothing is recorded and the pass ends at once.
pub struct Unavailable;

impl CoverFetcher for Unavailable {
    fn fetch(&self, _artist: &str, _album: &str) -> anyhow::Result<Option<Vec<u8>>> {
        Err(anyhow::Error::new(Unreachable).context("cover lookup is unavailable"))
    }
}

/// Called after found covers so the UI reloads; debounced by the pass.
pub type Emit = Arc<dyn Fn() + Send + Sync>;

/// Looks covers up on a background thread, one pass per `kick`.
pub struct LookupWorker {
    kicks: Sender<()>,
    enabled: Arc<AtomicBool>,
}

impl LookupWorker {
    pub fn spawn(
        db: Arc<Db>,
        covers: Arc<CoverStore>,
        fetcher: Arc<dyn CoverFetcher>,
        enabled: Arc<AtomicBool>,
        emit: Emit,
    ) -> LookupWorker {
        let (kicks, inbox) = channel::<()>();
        let flag = Arc::clone(&enabled);
        std::thread::spawn(move || {
            while inbox.recv().is_ok() {
                // Kicks that piled up during a pass are one more pass.
                while inbox.try_recv().is_ok() {}
                if let Err(err) = run_pass(&db, &covers, fetcher.as_ref(), &flag, &*emit, now()) {
                    eprintln!("cover lookup pass failed: {err:#}");
                }
            }
        });
        LookupWorker { kicks, enabled }
    }

    /// Asks for a pass over the albums that still lack a cover.
    pub fn kick(&self) {
        let _ = self.kicks.send(());
    }

    /// Switches lookups on or off; turning them on starts a pass.
    pub fn set_enabled(&self, on: bool) {
        self.enabled.store(on, Ordering::SeqCst);
        if on {
            self.kick();
        }
    }
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

struct Candidate {
    album_id: String,
    artist: String,
    title: String,
}

fn candidates(db: &Db, now: i64) -> anyhow::Result<Vec<Candidate>> {
    let conn = lock(db)?;
    let mut stmt = conn.prepare(
        "SELECT a.id, a.artist, a.title FROM albums a
         LEFT JOIN cover_lookups l ON l.album_id = a.id
         WHERE a.cover_id IS NULL AND a.title <> ?1 AND a.artist <> ?2
           AND (l.album_id IS NULL OR (l.status = 'notfound' AND l.checked_at <= ?3))
         ORDER BY a.artist COLLATE NOCASE, a.title COLLATE NOCASE, a.id",
    )?;
    let rows = stmt.query_map(
        params![UNKNOWN_ALBUM, UNKNOWN_ARTIST, now - NOTFOUND_RETRY_SECS],
        |row| {
            Ok(Candidate {
                album_id: row.get(0)?,
                artist: row.get(1)?,
                title: row.get(2)?,
            })
        },
    )?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

fn record(
    db: &Db,
    album_id: &str,
    status: &str,
    cover_id: Option<&str>,
    now: i64,
) -> anyhow::Result<()> {
    let conn = lock(db)?;
    conn.execute(
        "INSERT INTO cover_lookups (album_id, status, cover_id, checked_at) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(album_id) DO UPDATE SET
            status = excluded.status, cover_id = excluded.cover_id, checked_at = excluded.checked_at",
        params![album_id, status, cover_id, now],
    )?;
    if let Some(cover_id) = cover_id {
        conn.execute(
            "UPDATE albums SET cover_id = ?2 WHERE id = ?1 AND cover_id IS NULL",
            params![album_id, cover_id],
        )?;
    }
    Ok(())
}

/// One pass: looks up every candidate album while `enabled` stays on. Returns how many covers
/// were found. A fetch error ends the pass without recording anything (the next kick retries).
pub(crate) fn run_pass(
    db: &Db,
    covers: &CoverStore,
    fetcher: &dyn CoverFetcher,
    enabled: &AtomicBool,
    emit: &dyn Fn(),
    now: i64,
) -> anyhow::Result<usize> {
    let mut found = 0;
    let mut last_emit: Option<Instant> = None;
    let mut pending = false;
    let mut errors_in_a_row = 0;
    if enabled.load(Ordering::SeqCst) {
        for album in candidates(db, now)? {
            // Checked before every album, so switching off stops within one request.
            if !enabled.load(Ordering::SeqCst) {
                break;
            }
            let bytes = match fetcher.fetch(&album.artist, &album.title) {
                Ok(bytes) => {
                    errors_in_a_row = 0;
                    bytes
                }
                Err(err) => {
                    // Not recorded: the album is asked again on the next kick. One album that
                    // keeps failing must not block the rest, but an unreachable server or a
                    // run of failures ends the pass.
                    errors_in_a_row += 1;
                    let stop = err.downcast_ref::<Unreachable>().is_some()
                        || errors_in_a_row >= MAX_ERRORS_IN_A_ROW;
                    eprintln!("cover lookup of {} failed: {err:#}", album.title);
                    if stop {
                        break;
                    }
                    continue;
                }
            };
            // Bytes that are not an image are an answer ("no cover"); failing to write the
            // cover is ours and is retried.
            let stored = match bytes {
                Some(bytes) if image_extension(&bytes).is_some() => {
                    match covers.store_bytes(&bytes) {
                        Ok(cover_id) => Some(cover_id),
                        Err(err) => {
                            eprintln!("storing a looked up cover failed: {err:#}");
                            break;
                        }
                    }
                }
                _ => None,
            };
            match stored {
                Some(cover_id) => {
                    record(db, &album.album_id, "found", Some(&cover_id), now)?;
                    found += 1;
                    if last_emit.is_none_or(|at| at.elapsed() >= EMIT_GAP) {
                        emit();
                        last_emit = Some(Instant::now());
                        pending = false;
                    } else {
                        pending = true;
                    }
                }
                None => record(db, &album.album_id, "notfound", None, now)?,
            }
        }
    }
    if pending {
        emit();
    }
    Ok(found)
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::atomic::AtomicUsize;

    use super::*;
    use crate::library::model::album_id;
    use crate::library::query;
    use crate::library::scan::rebuild_albums;

    const PNG: &[u8] = &[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 1, 2, 3, 4];

    /// Answers from a queue (the last answer repeats) and counts calls.
    struct FakeFetcher {
        answers: Mutex<VecDeque<Result<Option<Vec<u8>>, String>>>,
        calls: AtomicUsize,
    }

    impl FakeFetcher {
        fn new(answers: Vec<Result<Option<Vec<u8>>, String>>) -> FakeFetcher {
            FakeFetcher {
                answers: Mutex::new(answers.into()),
                calls: AtomicUsize::new(0),
            }
        }
        fn cover() -> FakeFetcher {
            FakeFetcher::new(vec![Ok(Some(PNG.to_vec()))])
        }
        fn calls(&self) -> usize {
            self.calls.load(Ordering::SeqCst)
        }
    }

    impl CoverFetcher for FakeFetcher {
        fn fetch(&self, _artist: &str, _album: &str) -> anyhow::Result<Option<Vec<u8>>> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let mut answers = self.answers.lock().unwrap();
            let answer = if answers.len() > 1 {
                answers.pop_front().unwrap()
            } else {
                answers.front().cloned().unwrap()
            };
            answer.map_err(|e| anyhow::anyhow!(e))
        }
    }

    struct Fixture {
        db: Db,
        covers: CoverStore,
        enabled: AtomicBool,
        _dir: tempfile::TempDir,
    }

    fn fixture() -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        Fixture {
            db: Db::open_in_memory().unwrap(),
            covers: CoverStore::new(dir.path().join("covers")),
            enabled: AtomicBool::new(true),
            _dir: dir,
        }
    }

    impl Fixture {
        /// Adds a two-track album without a cover; returns its id.
        fn album(&self, artist: &str, album: &str) -> String {
            let id = album_id(artist, album);
            let conn = self.db.0.lock().unwrap();
            for n in 1..=2 {
                conn.execute(
                    "INSERT INTO tracks (id, source, title, artist, album_artist, album, album_id,
                        track_no, duration_ms, format, path)
                     VALUES (?1, 'local', ?2, ?3, ?3, ?4, ?5, ?6, 1000, 'flac', ?1)",
                    params![
                        format!("{id}/{n}"),
                        format!("Song {n}"),
                        artist,
                        album,
                        id,
                        n
                    ],
                )
                .unwrap();
            }
            rebuild_albums(&conn, [id.clone()].iter()).unwrap();
            id
        }

        fn pass(&self, fetcher: &FakeFetcher, now: i64) -> usize {
            run_pass(&self.db, &self.covers, fetcher, &self.enabled, &|| {}, now).unwrap()
        }

        fn album_cover(&self, id: &str) -> Option<String> {
            let conn = self.db.0.lock().unwrap();
            conn.query_row("SELECT cover_id FROM albums WHERE id = ?1", [id], |r| {
                r.get(0)
            })
            .unwrap()
        }

        fn set_lookup(&self, id: &str, status: &str, checked_at: i64) {
            let conn = self.db.0.lock().unwrap();
            conn.execute(
                "INSERT INTO cover_lookups (album_id, status, cover_id, checked_at)
                 VALUES (?1, ?2, NULL, ?3)",
                params![id, status, checked_at],
            )
            .unwrap();
        }
    }

    const DAY: i64 = 24 * 60 * 60;
    const NOW: i64 = 1_800_000_000;

    #[test]
    fn found_cover_sets_album_and_tracks() {
        let f = fixture();
        let id = f.album("Radiohead", "OK Computer");
        let fetcher = FakeFetcher::cover();
        assert_eq!(f.pass(&fetcher, NOW), 1);

        let cover = f.album_cover(&id).expect("album has the cover");
        assert!(f.covers.path(&cover).is_some());
        let conn = f.db.0.lock().unwrap();
        let tracks = query::album_tracks(&conn, &id).unwrap();
        assert_eq!(tracks.len(), 2);
        assert!(tracks
            .iter()
            .all(|t| t.cover_id.as_deref() == Some(cover.as_str())));
        let status: String = conn
            .query_row(
                "SELECT status FROM cover_lookups WHERE album_id = ?1",
                [&id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(status, "found");
        drop(conn);
        // Found albums are not asked again.
        let again = FakeFetcher::cover();
        f.pass(&again, NOW + 100 * DAY);
        assert_eq!(again.calls(), 0);
    }

    #[test]
    fn not_found_waits_30_days() {
        let f = fixture();
        let young = f.album("Artist A", "Young");
        let old = f.album("Artist B", "Old");
        f.set_lookup(&young, "notfound", NOW - 10 * DAY);
        f.set_lookup(&old, "notfound", NOW - 31 * DAY);
        let fetcher = FakeFetcher::new(vec![Ok(None)]);
        f.pass(&fetcher, NOW);
        assert_eq!(fetcher.calls(), 1, "only the 31 day old row is retried");
        let conn = f.db.0.lock().unwrap();
        let checked: i64 = conn
            .query_row(
                "SELECT checked_at FROM cover_lookups WHERE album_id = ?1",
                [&old],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(checked, NOW);
    }

    #[test]
    fn nothing_found_is_recorded_as_notfound() {
        let f = fixture();
        let id = f.album("Artist", "Album");
        let fetcher = FakeFetcher::new(vec![Ok(None)]);
        assert_eq!(f.pass(&fetcher, NOW), 0);
        assert_eq!(f.album_cover(&id), None);
        f.pass(&fetcher, NOW + DAY);
        assert_eq!(fetcher.calls(), 1);
    }

    /// Answers by calling `answer(artist)`; counts calls.
    struct FnFetcher<F> {
        answer: F,
        calls: AtomicUsize,
    }

    impl<F: Fn(&str) -> anyhow::Result<Option<Vec<u8>>> + Send + Sync> CoverFetcher for FnFetcher<F> {
        fn fetch(&self, artist: &str, _album: &str) -> anyhow::Result<Option<Vec<u8>>> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            (self.answer)(artist)
        }
    }

    fn fn_fetcher<F: Fn(&str) -> anyhow::Result<Option<Vec<u8>>>>(answer: F) -> FnFetcher<F> {
        FnFetcher {
            answer,
            calls: AtomicUsize::new(0),
        }
    }

    #[test]
    fn one_failing_album_does_not_block_the_rest() {
        let f = fixture();
        let bad = f.album("A Bad", "Album");
        let good = f.album("B Good", "Album");
        let fetcher = fn_fetcher(|artist| {
            if artist == "A Bad" {
                Err(anyhow::anyhow!("HTTP 500"))
            } else {
                Ok(Some(PNG.to_vec()))
            }
        });
        let found = run_pass(&f.db, &f.covers, &fetcher, &f.enabled, &|| {}, NOW).unwrap();
        assert_eq!(found, 1);
        assert!(f.album_cover(&good).is_some());
        assert_eq!(f.album_cover(&bad), None);
        let rows: i64 =
            f.db.0
                .lock()
                .unwrap()
                .query_row(
                    "SELECT COUNT(*) FROM cover_lookups WHERE album_id = ?1",
                    [&bad],
                    |r| r.get(0),
                )
                .unwrap();
        assert_eq!(rows, 0, "the failing album is retried next kick");
    }

    #[test]
    fn three_errors_in_a_row_end_the_pass() {
        let f = fixture();
        for n in 0..5 {
            f.album(&format!("Artist {n}"), "Album");
        }
        let fetcher = fn_fetcher(|_| Err(anyhow::anyhow!("HTTP 503")));
        run_pass(&f.db, &f.covers, &fetcher, &f.enabled, &|| {}, NOW).unwrap();
        assert_eq!(fetcher.calls.load(Ordering::SeqCst), 3);
    }

    #[test]
    fn an_unreachable_server_ends_the_pass_at_once() {
        let f = fixture();
        f.album("Artist 1", "Album");
        f.album("Artist 2", "Album");
        let fetcher = fn_fetcher(|_| Err(anyhow::Error::new(Unreachable).context("search")));
        run_pass(&f.db, &f.covers, &fetcher, &f.enabled, &|| {}, NOW).unwrap();
        assert_eq!(fetcher.calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn an_unrecognized_image_is_notfound() {
        let f = fixture();
        let id = f.album("Artist", "Album");
        let fetcher = FakeFetcher::new(vec![Ok(Some(b"<html>nope</html>".to_vec()))]);
        assert_eq!(f.pass(&fetcher, NOW), 0);
        let status: String =
            f.db.0
                .lock()
                .unwrap()
                .query_row(
                    "SELECT status FROM cover_lookups WHERE album_id = ?1",
                    [&id],
                    |r| r.get(0),
                )
                .unwrap();
        assert_eq!(status, "notfound");
    }

    #[test]
    fn a_failed_cover_write_records_nothing() {
        let f = fixture();
        let id = f.album("Artist", "Album");
        // The cover directory path is a file, so creating it fails.
        let blocked = f._dir.path().join("blocked");
        std::fs::write(&blocked, b"x").unwrap();
        let covers = CoverStore::new(blocked.join("covers"));
        let fetcher = FakeFetcher::cover();
        let found = run_pass(&f.db, &covers, &fetcher, &f.enabled, &|| {}, NOW).unwrap();
        assert_eq!(found, 0);
        let rows: i64 =
            f.db.0
                .lock()
                .unwrap()
                .query_row(
                    "SELECT COUNT(*) FROM cover_lookups WHERE album_id = ?1",
                    [&id],
                    |r| r.get(0),
                )
                .unwrap();
        assert_eq!(rows, 0);
    }

    #[test]
    fn network_error_is_retried_next_kick() {
        let f = fixture();
        let id = f.album("Artist", "Album");
        let fetcher = FakeFetcher::new(vec![Err("offline".into()), Ok(Some(PNG.to_vec()))]);
        assert_eq!(f.pass(&fetcher, NOW), 0);
        let rows: i64 =
            f.db.0
                .lock()
                .unwrap()
                .query_row("SELECT COUNT(*) FROM cover_lookups", [], |r| r.get(0))
                .unwrap();
        assert_eq!(rows, 0, "errors are not recorded");
        assert_eq!(f.pass(&fetcher, NOW + 1), 1);
        assert!(f.album_cover(&id).is_some());
    }

    #[test]
    fn disabled_fetches_nothing() {
        let f = fixture();
        f.album("Artist", "Album");
        f.enabled.store(false, Ordering::SeqCst);
        let fetcher = FakeFetcher::cover();
        assert_eq!(f.pass(&fetcher, NOW), 0);
        assert_eq!(fetcher.calls(), 0);
    }

    #[test]
    fn switching_off_stops_before_the_next_album() {
        let f = fixture();
        f.album("Artist", "One");
        f.album("Artist", "Two");
        struct Off<'a>(&'a AtomicBool);
        impl CoverFetcher for Off<'_> {
            fn fetch(&self, _: &str, _: &str) -> anyhow::Result<Option<Vec<u8>>> {
                self.0.store(false, Ordering::SeqCst);
                Ok(Some(PNG.to_vec()))
            }
        }
        let found = run_pass(&f.db, &f.covers, &Off(&f.enabled), &f.enabled, &|| {}, NOW).unwrap();
        assert_eq!(found, 1);
    }

    #[test]
    fn unknown_album_is_skipped() {
        let f = fixture();
        f.album("Artist", UNKNOWN_ALBUM);
        f.album(UNKNOWN_ARTIST, "Album");
        let fetcher = FakeFetcher::cover();
        assert_eq!(f.pass(&fetcher, NOW), 0);
        assert_eq!(fetcher.calls(), 0);
    }

    #[test]
    fn albums_with_a_cover_are_skipped() {
        let f = fixture();
        let id = f.album("Artist", "Album");
        f.db.0
            .lock()
            .unwrap()
            .execute("UPDATE albums SET cover_id = 'abc' WHERE id = ?1", [&id])
            .unwrap();
        let fetcher = FakeFetcher::cover();
        f.pass(&fetcher, NOW);
        assert_eq!(fetcher.calls(), 0);
    }

    #[test]
    fn rescan_keeps_lookup_cover() {
        let f = fixture();
        let id = f.album("Artist", "Album");
        f.pass(&FakeFetcher::cover(), NOW);
        let cover = f.album_cover(&id).unwrap();
        let conn = f.db.0.lock().unwrap();
        rebuild_albums(&conn, [id.clone()].iter()).unwrap();
        let after: Option<String> = conn
            .query_row("SELECT cover_id FROM albums WHERE id = ?1", [&id], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(after, Some(cover));
    }

    #[test]
    fn a_track_cover_beats_the_lookup_cover() {
        let f = fixture();
        let id = f.album("Artist", "Album");
        f.pass(&FakeFetcher::cover(), NOW);
        let conn = f.db.0.lock().unwrap();
        conn.execute(
            "UPDATE tracks SET cover_id = 'tagged' WHERE album_id = ?1",
            [&id],
        )
        .unwrap();
        rebuild_albums(&conn, [id.clone()].iter()).unwrap();
        let after: Option<String> = conn
            .query_row("SELECT cover_id FROM albums WHERE id = ?1", [&id], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(after.as_deref(), Some("tagged"));
    }

    #[test]
    fn found_covers_emit_once_per_burst() {
        let f = fixture();
        f.album("Artist", "One");
        f.album("Artist", "Two");
        f.album("Artist", "Three");
        let emitted = AtomicUsize::new(0);
        run_pass(
            &f.db,
            &f.covers,
            &FakeFetcher::cover(),
            &f.enabled,
            &|| {
                emitted.fetch_add(1, Ordering::SeqCst);
            },
            NOW,
        )
        .unwrap();
        // The first cover emits at once; the other two arrive inside 2 s and share one more.
        assert_eq!(emitted.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn worker_looks_up_on_kick_and_when_enabled() {
        let f = fixture();
        let id = f.album("Artist", "Album");
        let Fixture { db, covers, .. } = f;
        let db = Arc::new(db);
        let covers = Arc::new(covers);
        let enabled = Arc::new(AtomicBool::new(false));
        let (tx, rx) = std::sync::mpsc::channel();
        let tx = Mutex::new(tx);
        let worker = LookupWorker::spawn(
            Arc::clone(&db),
            Arc::clone(&covers),
            Arc::new(FakeFetcher::cover()),
            Arc::clone(&enabled),
            Arc::new(move || {
                let _ = tx.lock().unwrap().send(());
            }),
        );
        worker.kick();
        assert!(
            rx.recv_timeout(Duration::from_millis(300)).is_err(),
            "off: nothing happens"
        );
        worker.set_enabled(true);
        rx.recv_timeout(Duration::from_secs(5))
            .expect("a cover was found");
        let cover: Option<String> =
            db.0.lock()
                .unwrap()
                .query_row("SELECT cover_id FROM albums WHERE id = ?1", [&id], |r| {
                    r.get(0)
                })
                .unwrap();
        assert!(cover.is_some());
    }

    #[test]
    fn mb_query_escapes_quotes() {
        assert_eq!(
            mb_query("Guns N' Roses", "Say \"Hi\""),
            r#"release:"Say \"Hi\"" AND artist:"Guns N' Roses""#
        );
        assert_eq!(mb_query("a\\b", "c"), r#"release:"c" AND artist:"a\\b""#);
    }

    #[test]
    fn pick_release_requires_score_90() {
        let json = serde_json::json!({"releases": [
            {"id": "low", "score": 89},
            {"id": "ok", "score": 90},
            {"id": "best", "score": 100},
        ]});
        assert_eq!(pick_release(&json).as_deref(), Some("ok"));
        let weak = serde_json::json!({"releases": [{"id": "low", "score": 89}]});
        assert_eq!(pick_release(&weak), None);
        assert_eq!(pick_release(&serde_json::json!({})), None);
        assert_eq!(pick_release(&serde_json::json!({"releases": []})), None);
    }

    #[test]
    fn rate_limiter_spaces_requests() {
        let limiter = RateLimiter::new(Duration::from_millis(80));
        let start = Instant::now();
        limiter.wait();
        limiter.wait();
        limiter.wait();
        assert!(start.elapsed() >= Duration::from_millis(160));
    }

    #[test]
    fn track_columns_lookup_uses_the_primary_key_index() {
        let f = fixture();
        let conn = f.db.0.lock().unwrap();
        let sql = format!(
            "EXPLAIN QUERY PLAN SELECT {} FROM tracks t",
            query::TRACK_COLUMNS
        );
        let mut stmt = conn.prepare(&sql).unwrap();
        let plan: Vec<String> = stmt
            .query_map([], |r| r.get::<_, String>(3))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert!(
            plan.iter().any(|d| d.contains("SEARCH")
                && d.contains("cover_lookups")
                && d.contains("album_id=?")),
            "the per-row lookup is an index search, got {plan:?}"
        );
    }

    /// Hits the real MusicBrainz and Cover Art Archive. Run by hand: `cargo test -- --ignored`.
    #[test]
    #[ignore]
    fn live_lookup_finds_a_known_album() {
        let fetcher = MusicBrainz::new().unwrap();
        let bytes = fetcher
            .fetch("Radiohead", "OK Computer")
            .unwrap()
            .expect("a cover for a famous album");
        assert!(crate::library::covers::image_extension(&bytes).is_some());
    }
}
