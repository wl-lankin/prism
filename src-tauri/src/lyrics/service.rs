//! The lyrics resolver: which source answers for the playing track, in which order, what is
//! cached, and how concurrent calls for one track share a lookup. Everything here blocks
//! (files, Navidrome, LRCLIB, the YouTube Music page), so callers run it on a blocking
//! thread. No lock is held across any of it: the DB lock covers single cache statements.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;

use super::cache::{self, Cached, FOUND_TTL_PAGE_SECS};
use super::lrclib::{Found, Lrclib, Query};
use super::{local, lrc, Lyrics, LyricsSource};
use crate::db::Db;
use crate::library::model::{navidrome_raw, Source, Track};
use crate::library::scan::lock;
use crate::navidrome::client::SongLyrics;
use crate::navidrome::NavidromeService;
use crate::ytm::state::{YtmState, YtmStatus};
use crate::ytm::{PageLyricsError, YtmService, LYRICS_TIMEOUT};

/// What the commands answer with.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "camelCase")]
pub enum LyricsAnswer {
    Found {
        lyrics: Lyrics,
    },
    None,
    /// A source failed and no other had lyrics.
    Error,
}

/// A source could not be asked (offline, refused, no answer in time). Never a miss.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SourceFailed;

/// What the resolver reads from the rest of the app.
pub trait Sources: Send + Sync {
    /// A library or dropped-file track by id; `None` for ids that are not tracks (radio).
    fn track(&self, id: &str) -> Option<Track>;
    /// Blocking. `raw_id` is the server's own song id.
    fn navidrome(
        &self,
        raw_id: &str,
        artist: &str,
        title: &str,
    ) -> Result<Option<SongLyrics>, SourceFailed>;
    fn ytm_state(&self) -> YtmState;
    /// Blocking: asks the YouTube Music page.
    fn ytm_lyrics(&self) -> Result<(String, Option<Lyrics>), PageLyricsError>;
}

/// The app's sources: the library (and dropped files), the Navidrome account and the
/// YouTube Music page.
pub struct AppSources {
    pub tracks: crate::player::Resolver,
    pub navidrome: Arc<NavidromeService>,
    pub ytm: Arc<YtmService>,
}

impl Sources for AppSources {
    fn track(&self, id: &str) -> Option<Track> {
        (self.tracks)(id)
    }

    fn navidrome(
        &self,
        raw_id: &str,
        artist: &str,
        title: &str,
    ) -> Result<Option<SongLyrics>, SourceFailed> {
        let client = self.navidrome.client().ok_or(SourceFailed)?;
        client
            .lyrics(raw_id, artist, title)
            .map_err(|_| SourceFailed)
    }

    fn ytm_state(&self) -> YtmState {
        self.ytm.state()
    }

    fn ytm_lyrics(&self) -> Result<(String, Option<Lyrics>), PageLyricsError> {
        self.ytm.lyrics(LYRICS_TIMEOUT)
    }
}

pub struct LyricsService {
    db: Arc<Db>,
    /// `None` when the HTTP client could not be built: LRCLIB stays off.
    lrclib: Option<Lrclib>,
    sources: Arc<dyn Sources>,
    /// "Search lyrics online", read at every call.
    lookup: Arc<dyn Fn() -> bool + Send + Sync>,
    flights: Mutex<HashMap<String, Arc<Flight>>>,
}

/// One lookup in progress; later callers for the same key wait for its answer.
#[derive(Default)]
struct Flight {
    answer: Mutex<Option<LyricsAnswer>>,
    done: Condvar,
}

/// Ends a flight even when the lookup panics, so waiters never hang.
struct Leader<'a> {
    service: &'a LyricsService,
    key: &'a str,
    flight: &'a Flight,
    answer: Option<LyricsAnswer>,
}

impl Drop for Leader<'_> {
    fn drop(&mut self) {
        lock_plain(&self.service.flights).remove(self.key);
        *lock_plain(&self.flight.answer) = Some(self.answer.take().unwrap_or(LyricsAnswer::Error));
        self.flight.done.notify_all();
    }
}

fn lock_plain<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// What LRCLIB knows, as the one best entry: synced, else plain, else instrumental.
fn pick(found: &Found) -> Option<Lyrics> {
    if let Some(lyrics) = found
        .synced_lrc
        .as_deref()
        .and_then(|text| lrc::parse(text, LyricsSource::Lrclib))
    {
        if lyrics.synced {
            return Some(lyrics);
        }
    }
    found
        .plain
        .as_deref()
        .and_then(|text| Lyrics::plain(LyricsSource::Lrclib, text))
        .or_else(|| {
            found
                .synced_lrc
                .as_deref()
                .and_then(|text| lrc::parse(text, LyricsSource::Lrclib))
        })
        .or_else(|| {
            found
                .instrumental
                .then(|| Lyrics::instrumental(LyricsSource::Lrclib))
        })
}

fn found(lyrics: Lyrics) -> LyricsAnswer {
    LyricsAnswer::Found { lyrics }
}

/// The answer when no tier produced lyrics.
fn nothing(failed: bool) -> LyricsAnswer {
    if failed {
        LyricsAnswer::Error
    } else {
        LyricsAnswer::None
    }
}

impl LyricsService {
    pub fn new(
        db: Arc<Db>,
        lrclib: Option<Lrclib>,
        sources: Arc<dyn Sources>,
        lookup: Arc<dyn Fn() -> bool + Send + Sync>,
    ) -> LyricsService {
        LyricsService {
            db,
            lrclib,
            sources,
            lookup,
            flights: Mutex::new(HashMap::new()),
        }
    }

    /// Lyrics for a track of the library or a dropped file. Blocking.
    pub fn for_track(&self, id: &str) -> LyricsAnswer {
        self.single_flight(format!("track:{id}"), || self.resolve_track(id))
    }

    /// Lyrics for the song YouTube Music plays now. Blocking.
    pub fn for_youtube(&self) -> LyricsAnswer {
        let state = self.sources.ytm_state();
        if matches!(state.status, YtmStatus::Idle | YtmStatus::Unavailable) {
            return LyricsAnswer::None;
        }
        let key = format!("ytm:{}", state.video_id.as_deref().unwrap_or(""));
        self.single_flight(key, || self.resolve_youtube(&state))
    }

    fn single_flight(&self, key: String, work: impl FnOnce() -> LyricsAnswer) -> LyricsAnswer {
        let (flight, leads) = {
            let mut flights = lock_plain(&self.flights);
            match flights.get(&key) {
                Some(flight) => (Arc::clone(flight), false),
                None => {
                    let flight = Arc::new(Flight::default());
                    flights.insert(key.clone(), Arc::clone(&flight));
                    (flight, true)
                }
            }
        };
        if leads {
            let mut leader = Leader {
                service: self,
                key: &key,
                flight: &flight,
                answer: None,
            };
            let answer = work();
            leader.answer = Some(answer.clone());
            return answer;
        }
        let mut slot = lock_plain(&flight.answer);
        loop {
            if let Some(answer) = slot.as_ref() {
                return answer.clone();
            }
            slot = flight
                .done
                .wait(slot)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
        }
    }

    fn online(&self) -> bool {
        self.lrclib.is_some() && (self.lookup)()
    }

    fn cache_get(&self, key: &str, found_ttl: Option<i64>) -> Cached {
        lock(&self.db)
            .and_then(|conn| cache::get_expiring(&conn, key, now(), found_ttl))
            .unwrap_or(Cached::Miss)
    }

    fn cache_put(&self, key: &str, lyrics: Option<&Lyrics>) {
        let stored = lock(&self.db).and_then(|conn| cache::put(&conn, key, lyrics, now()));
        if let Err(err) = stored {
            eprintln!("keeping lyrics in the cache failed: {err:#}");
        }
    }

    /// LRCLIB's best entry for the query. A network or server error sets `failed` and is
    /// not cached; "not known" is cached as a miss.
    fn lrclib_best(&self, q: &Query, failed: &mut bool) -> Option<Lyrics> {
        let lrclib = self.lrclib.as_ref()?;
        if q.artist.trim().is_empty() || q.title.trim().is_empty() {
            return None;
        }
        let key = cache::lrclib_key(q);
        if let Cached::Hit(cached) = self.cache_get(&key, None) {
            return cached;
        }
        match lrclib.find(q) {
            Ok(result) => {
                let best = pick(&result);
                self.cache_put(&key, best.as_ref());
                best
            }
            Err(_) => {
                *failed = true;
                None
            }
        }
    }

    fn resolve_track(&self, id: &str) -> LyricsAnswer {
        let Some(track) = self.sources.track(id) else {
            return LyricsAnswer::None;
        };
        match track.source {
            Source::Radio => LyricsAnswer::None,
            Source::Navidrome => match navidrome_raw(&track.id) {
                Some(raw) => self.resolve_navidrome(&track, raw),
                None => LyricsAnswer::None,
            },
            Source::Local => match track.path.as_deref() {
                Some(path) => self.resolve_local(&track, path),
                None => LyricsAnswer::None,
            },
        }
    }

    fn resolve_local(&self, track: &Track, path: &str) -> LyricsAnswer {
        let files = local::read(Path::new(path));
        if let Some(synced) = files.synced {
            return found(synced);
        }
        let mut failed = false;
        let online = if self.online() {
            self.lrclib_best(&track_query(track), &mut failed)
        } else {
            None
        };
        if let Some(lyrics) = online.as_ref().filter(|l| l.synced) {
            return found(lyrics.clone());
        }
        if let Some(plain) = files.plain {
            return found(plain);
        }
        match online {
            Some(lyrics) => found(lyrics),
            None => nothing(failed),
        }
    }

    fn resolve_navidrome(&self, track: &Track, raw: &str) -> LyricsAnswer {
        let mut failed = false;
        let server = self.navidrome_best(track, raw, &mut failed);
        if let Some(lyrics) = server.as_ref().filter(|l| l.synced) {
            return found(lyrics.clone());
        }
        let online = if self.online() {
            self.lrclib_best(&track_query(track), &mut failed)
        } else {
            None
        };
        if let Some(lyrics) = online.as_ref().filter(|l| l.synced) {
            return found(lyrics.clone());
        }
        if let Some(plain) = server {
            return found(plain);
        }
        match online {
            Some(lyrics) => found(lyrics),
            None => nothing(failed),
        }
    }

    /// The server's synced lyrics, else its plain ones; cached under `navidrome:<id>`.
    fn navidrome_best(&self, track: &Track, raw: &str, failed: &mut bool) -> Option<Lyrics> {
        let key = format!("navidrome:{raw}");
        if let Cached::Hit(cached) = self.cache_get(&key, None) {
            return cached;
        }
        match self.sources.navidrome(raw, &track.artist, &track.title) {
            Ok(answer) => {
                let best = answer.and_then(|(synced, plain)| synced.or(plain));
                self.cache_put(&key, best.as_ref());
                best
            }
            Err(SourceFailed) => {
                *failed = true;
                None
            }
        }
    }

    fn resolve_youtube(&self, state: &YtmState) -> LyricsAnswer {
        let mut failed = false;
        let online = if self.online() {
            self.lrclib_best(&ytm_query(state), &mut failed)
        } else {
            None
        };
        if let Some(lyrics) = online.as_ref().filter(|l| l.synced || l.instrumental) {
            return found(lyrics.clone());
        }
        if let Some(page) = self.page_lyrics(state, &mut failed) {
            return found(page);
        }
        match online {
            Some(lyrics) => found(lyrics),
            None => nothing(failed),
        }
    }

    /// The page's lyrics for the video playing, cached under `youtube:<video id>` (found
    /// entries for 30 days, misses for 7).
    fn page_lyrics(&self, state: &YtmState, failed: &mut bool) -> Option<Lyrics> {
        let Some(asked) = state.video_id.as_deref() else {
            *failed = true;
            return None;
        };
        let key = format!("youtube:{asked}");
        if let Cached::Hit(cached) = self.cache_get(&key, Some(FOUND_TTL_PAGE_SECS)) {
            return cached;
        }
        match self.sources.ytm_lyrics() {
            Ok((video_id, lyrics)) if video_id == asked => {
                self.cache_put(&key, lyrics.as_ref());
                lyrics
            }
            _ => {
                *failed = true;
                None
            }
        }
    }
}

fn track_query(track: &Track) -> Query {
    Query {
        artist: track.artist.clone(),
        title: track.title.clone(),
        album: Some(track.album.clone()).filter(|a| !a.trim().is_empty()),
        duration_ms: track.duration_ms,
    }
}

fn ytm_query(state: &YtmState) -> Query {
    Query {
        artist: state.artist.clone(),
        title: state.title.clone(),
        album: Some(state.album.clone()).filter(|a| !a.trim().is_empty()),
        duration_ms: state.duration_ms,
    }
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::time::Duration;

    use super::*;
    use crate::navidrome::client::SongLyrics;

    type PageAnswer = Result<(String, Option<Lyrics>), PageLyricsError>;

    /// A fake LRCLIB: counts requests and answers `GET /api/get` from a closure of
    /// (artist, title); the search endpoint answers an empty list.
    struct FakeLrclib {
        base: String,
        requests: Arc<AtomicUsize>,
        stop: Arc<AtomicBool>,
        thread: Option<std::thread::JoinHandle<()>>,
    }

    type Reply = Box<dyn Fn(&str, &str) -> (u16, String) + Send + Sync>;

    impl FakeLrclib {
        fn start(reply: impl Fn(&str, &str) -> (u16, String) + Send + Sync + 'static) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let base = format!("http://{}", listener.local_addr().unwrap());
            crate::radio::testserver::assert_loopback(&base);
            listener.set_nonblocking(true).unwrap();
            let requests = Arc::new(AtomicUsize::new(0));
            let stop = Arc::new(AtomicBool::new(false));
            let reply: Arc<Reply> = Arc::new(Box::new(reply));
            let thread = {
                let (requests, stop) = (Arc::clone(&requests), Arc::clone(&stop));
                std::thread::spawn(move || {
                    while !stop.load(Ordering::SeqCst) {
                        let Ok((mut stream, _)) = listener.accept() else {
                            std::thread::sleep(Duration::from_millis(5));
                            continue;
                        };
                        let (requests, reply) = (Arc::clone(&requests), Arc::clone(&reply));
                        std::thread::spawn(move || {
                            let _ = stream.set_nonblocking(false);
                            let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
                            let mut head = Vec::new();
                            let mut buf = [0u8; 2048];
                            while !head.windows(4).any(|w| w == b"\r\n\r\n") {
                                match stream.read(&mut buf) {
                                    Ok(n) if n > 0 => head.extend_from_slice(&buf[..n]),
                                    _ => return,
                                }
                            }
                            requests.fetch_add(1, Ordering::SeqCst);
                            let head = String::from_utf8_lossy(&head).into_owned();
                            let target = head
                                .lines()
                                .next()
                                .and_then(|l| l.split(' ').nth(1))
                                .unwrap_or("");
                            let url = reqwest::Url::parse(&format!("http://fake{target}")).unwrap();
                            let param = |name: &str| {
                                url.query_pairs()
                                    .find(|(k, _)| k == name)
                                    .map(|(_, v)| v.into_owned())
                                    .unwrap_or_default()
                            };
                            let (status, body) = if url.path() == "/api/get" {
                                reply(&param("artist_name"), &param("track_name"))
                            } else {
                                (200, "[]".to_owned())
                            };
                            let text = if status == 200 { "OK" } else { "Status" };
                            let out = format!(
                                "HTTP/1.1 {status} {text}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                                body.len()
                            );
                            let _ = stream.write_all(out.as_bytes());
                        });
                    }
                })
            };
            FakeLrclib {
                base,
                requests,
                stop,
                thread: Some(thread),
            }
        }

        fn client(&self) -> Lrclib {
            Lrclib::with_timing(&self.base, Duration::from_secs(5), Duration::ZERO).unwrap()
        }

        fn requests(&self) -> usize {
            self.requests.load(Ordering::SeqCst)
        }
    }

    impl Drop for FakeLrclib {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::SeqCst);
            if let Some(thread) = self.thread.take() {
                let _ = thread.join();
            }
        }
    }

    fn json_synced(lrc: &str) -> (u16, String) {
        (
            200,
            serde_json::json!({ "syncedLyrics": lrc, "plainLyrics": null, "instrumental": false })
                .to_string(),
        )
    }

    fn json_plain(text: &str) -> (u16, String) {
        (
            200,
            serde_json::json!({ "syncedLyrics": null, "plainLyrics": text, "instrumental": false })
                .to_string(),
        )
    }

    fn json_instrumental() -> (u16, String) {
        (
            200,
            serde_json::json!({ "syncedLyrics": null, "plainLyrics": null, "instrumental": true })
                .to_string(),
        )
    }

    fn not_found() -> (u16, String) {
        (404, "{}".to_owned())
    }

    const LRC: &str = "[00:01.00]Online one\n[00:03.00]Online two\n";

    fn synced(source: LyricsSource, first: &str) -> Lyrics {
        Lyrics::synced(source, vec![(1_000, first.into()), (3_000, "More".into())]).unwrap()
    }

    fn plain(source: LyricsSource, text: &str) -> Lyrics {
        Lyrics::plain(source, text).unwrap()
    }

    #[derive(Default)]
    struct FakeSources {
        tracks: Mutex<HashMap<String, Track>>,
        navidrome: Mutex<Option<Result<Option<SongLyrics>, SourceFailed>>>,
        navidrome_calls: AtomicUsize,
        navidrome_delay: Mutex<Duration>,
        state: Mutex<YtmState>,
        page: Mutex<Option<PageAnswer>>,
        page_calls: AtomicUsize,
    }

    impl Sources for FakeSources {
        fn track(&self, id: &str) -> Option<Track> {
            lock_plain(&self.tracks).get(id).cloned()
        }

        fn navidrome(&self, _: &str, _: &str, _: &str) -> Result<Option<SongLyrics>, SourceFailed> {
            self.navidrome_calls.fetch_add(1, Ordering::SeqCst);
            std::thread::sleep(*lock_plain(&self.navidrome_delay));
            lock_plain(&self.navidrome).clone().unwrap_or(Ok(None))
        }

        fn ytm_state(&self) -> YtmState {
            lock_plain(&self.state).clone()
        }

        fn ytm_lyrics(&self) -> Result<(String, Option<Lyrics>), PageLyricsError> {
            self.page_calls.fetch_add(1, Ordering::SeqCst);
            lock_plain(&self.page)
                .clone()
                .unwrap_or(Err(PageLyricsError::Failed))
        }
    }

    fn track(id: &str, source: Source, path: Option<&str>) -> Track {
        Track {
            id: id.to_owned(),
            source,
            title: "Song One".into(),
            artist: "Test Artist".into(),
            album_artist: "Test Artist".into(),
            album: "Test Album".into(),
            album_id: "al".into(),
            track_no: None,
            disc_no: None,
            duration_ms: 200_000,
            year: None,
            genre: None,
            format: "mp3".into(),
            sample_rate: None,
            bit_depth: None,
            bitrate_kbps: None,
            cover_id: None,
            path: path.map(str::to_owned),
            replay_gain_track_db: None,
            replay_gain_album_db: None,
            replay_gain_track_peak: None,
            replay_gain_album_peak: None,
        }
    }

    struct Rig {
        service: Arc<LyricsService>,
        sources: Arc<FakeSources>,
        lookup: Arc<AtomicBool>,
        db: Arc<Db>,
        lrclib: FakeLrclib,
    }

    fn rig(reply: impl Fn(&str, &str) -> (u16, String) + Send + Sync + 'static) -> Rig {
        let lrclib = FakeLrclib::start(reply);
        let db = Arc::new(Db::open_in_memory().unwrap());
        let sources = Arc::new(FakeSources::default());
        let lookup = Arc::new(AtomicBool::new(true));
        let service = Arc::new(LyricsService::new(
            Arc::clone(&db),
            Some(lrclib.client()),
            Arc::clone(&sources) as Arc<dyn Sources>,
            {
                let lookup = Arc::clone(&lookup);
                Arc::new(move || lookup.load(Ordering::SeqCst))
            },
        ));
        Rig {
            service,
            sources,
            lookup,
            db,
            lrclib,
        }
    }

    impl Rig {
        fn add(&self, track: Track) {
            lock_plain(&self.sources.tracks).insert(track.id.clone(), track);
        }

        fn navidrome_says(&self, answer: Result<Option<SongLyrics>, SourceFailed>) {
            *lock_plain(&self.sources.navidrome) = Some(answer);
        }

        fn playing(&self, status: YtmStatus, video: Option<&str>) {
            *lock_plain(&self.sources.state) = YtmState {
                status,
                title: "Song One".into(),
                artist: "Test Artist".into(),
                album: "Test Album".into(),
                duration_ms: 200_000,
                video_id: video.map(str::to_owned),
                ..YtmState::default()
            };
        }

        fn page_says(&self, answer: PageAnswer) {
            *lock_plain(&self.sources.page) = Some(answer);
        }

        fn cached(&self, key: &str) -> Cached {
            cache::get(&lock(&self.db).unwrap(), key, now()).unwrap()
        }
    }

    /// A local file with a sidecar `.lrc` and/or plain lyrics in a sidecar-less setup.
    fn local_file(dir: &Path, lrc: Option<&str>) -> String {
        let audio = dir.join("song.mp3");
        std::fs::write(&audio, b"not audio").unwrap();
        if let Some(lrc) = lrc {
            std::fs::write(dir.join("song.lrc"), lrc).unwrap();
        }
        audio.to_string_lossy().into_owned()
    }

    fn lines(answer: &LyricsAnswer) -> Vec<String> {
        match answer {
            LyricsAnswer::Found { lyrics } => lyrics.lines.iter().map(|l| l.text.clone()).collect(),
            other => panic!("not found: {other:?}"),
        }
    }

    fn source_of(answer: &LyricsAnswer) -> LyricsSource {
        match answer {
            LyricsAnswer::Found { lyrics } => lyrics.source,
            other => panic!("not found: {other:?}"),
        }
    }

    // Local files

    #[test]
    fn local_sidecar_synced_wins_without_asking_lrclib() {
        let r = rig(|_, _| json_synced(LRC));
        let dir = tempfile::tempdir().unwrap();
        let path = local_file(dir.path(), Some("[00:02.00]From the file\n"));
        r.add(track("l1", Source::Local, Some(&path)));
        let answer = r.service.for_track("l1");
        assert_eq!(lines(&answer), ["From the file"]);
        assert_eq!(source_of(&answer), LyricsSource::LrcFile);
        assert_eq!(r.lrclib.requests(), 0);
    }

    #[test]
    fn local_sidecar_with_plain_text_yields_to_lrclib_synced() {
        let r = rig(|_, _| json_synced(LRC));
        let dir = tempfile::tempdir().unwrap();
        let path = local_file(dir.path(), Some("Plain line one\nPlain line two\n"));
        r.add(track("l1", Source::Local, Some(&path)));
        let answer = r.service.for_track("l1");
        assert_eq!(lines(&answer), ["Online one", "Online two"]);
        assert_eq!(source_of(&answer), LyricsSource::Lrclib);
    }

    #[test]
    fn local_plain_beats_lrclib_plain() {
        let r = rig(|_, _| json_plain("Online plain"));
        let dir = tempfile::tempdir().unwrap();
        let path = local_file(dir.path(), Some("Plain line one\n"));
        r.add(track("l1", Source::Local, Some(&path)));
        let answer = r.service.for_track("l1");
        assert_eq!(lines(&answer), ["Plain line one"]);
        assert_eq!(source_of(&answer), LyricsSource::LrcFile);
    }

    #[test]
    fn local_without_lyrics_falls_back_to_lrclib_plain() {
        let r = rig(|_, _| json_plain("Online plain"));
        let dir = tempfile::tempdir().unwrap();
        r.add(track(
            "l1",
            Source::Local,
            Some(&local_file(dir.path(), None)),
        ));
        assert_eq!(lines(&r.service.for_track("l1")), ["Online plain"]);
    }

    #[test]
    fn local_text_beats_an_instrumental_answer_but_nothing_else_does() {
        let r = rig(|_, _| json_instrumental());
        let dir = tempfile::tempdir().unwrap();
        let path = local_file(dir.path(), Some("Plain line one\n"));
        r.add(track("l1", Source::Local, Some(&path)));
        assert_eq!(lines(&r.service.for_track("l1")), ["Plain line one"]);

        let other = tempfile::tempdir().unwrap();
        r.add(track(
            "l2",
            Source::Local,
            Some(&local_file(other.path(), None)),
        ));
        match r.service.for_track("l2") {
            LyricsAnswer::Found { lyrics } => assert!(lyrics.instrumental),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn local_nothing_anywhere_is_none_and_the_miss_is_cached() {
        let r = rig(|_, _| not_found());
        let dir = tempfile::tempdir().unwrap();
        r.add(track(
            "l1",
            Source::Local,
            Some(&local_file(dir.path(), None)),
        ));
        assert_eq!(r.service.for_track("l1"), LyricsAnswer::None);
        let after_first = r.lrclib.requests();
        assert!(after_first > 0);
        assert_eq!(r.service.for_track("l1"), LyricsAnswer::None);
        assert_eq!(
            r.lrclib.requests(),
            after_first,
            "the miss came from the cache"
        );
        assert_eq!(
            r.cached("lrclib:test artist|song one|test album|200"),
            Cached::Hit(None)
        );
    }

    #[test]
    fn local_files_are_never_cached() {
        let r = rig(|_, _| not_found());
        let dir = tempfile::tempdir().unwrap();
        let path = local_file(dir.path(), Some("[00:02.00]From the file\n"));
        r.add(track("l1", Source::Local, Some(&path)));
        assert_eq!(lines(&r.service.for_track("l1")), ["From the file"]);
        std::fs::write(dir.path().join("song.lrc"), "[00:02.00]Edited\n").unwrap();
        assert_eq!(lines(&r.service.for_track("l1")), ["Edited"]);
    }

    #[test]
    fn a_local_track_without_a_path_or_unknown_ids_answer_none() {
        let r = rig(|_, _| json_synced(LRC));
        r.add(track("file:abc", Source::Local, None));
        assert_eq!(r.service.for_track("file:abc"), LyricsAnswer::None);
        assert_eq!(r.service.for_track("file:gone"), LyricsAnswer::None);
        assert_eq!(r.service.for_track("nope"), LyricsAnswer::None);
        assert_eq!(r.lrclib.requests(), 0);
    }

    #[test]
    fn dropped_file_ids_resolve_like_local_tracks() {
        let r = rig(|_, _| not_found());
        let dir = tempfile::tempdir().unwrap();
        let path = local_file(dir.path(), Some("[00:02.00]Dropped\n"));
        r.add(track("file:0123456789", Source::Local, Some(&path)));
        assert_eq!(lines(&r.service.for_track("file:0123456789")), ["Dropped"]);
    }

    #[test]
    fn radio_is_never_asked_about() {
        let r = rig(|_, _| json_synced(LRC));
        r.add(track("radio:abc", Source::Radio, None));
        assert_eq!(r.service.for_track("radio:abc"), LyricsAnswer::None);
        assert_eq!(r.lrclib.requests(), 0);
        assert_eq!(r.sources.navidrome_calls.load(Ordering::SeqCst), 0);
    }

    // The setting

    #[test]
    fn with_the_lookup_off_lrclib_gets_no_request_but_files_and_the_server_still_answer() {
        let r = rig(|_, _| json_synced(LRC));
        r.lookup.store(false, Ordering::SeqCst);
        let dir = tempfile::tempdir().unwrap();
        r.add(track(
            "l1",
            Source::Local,
            Some(&local_file(dir.path(), None)),
        ));
        assert_eq!(r.service.for_track("l1"), LyricsAnswer::None);

        let dir = tempfile::tempdir().unwrap();
        r.add(track(
            "l2",
            Source::Local,
            Some(&local_file(dir.path(), Some("[00:01.00]File\n"))),
        ));
        assert_eq!(lines(&r.service.for_track("l2")), ["File"]);

        r.add(track("navidrome:7", Source::Navidrome, None));
        r.navidrome_says(Ok(Some((
            None,
            Some(plain(LyricsSource::Navidrome, "Server")),
        ))));
        assert_eq!(lines(&r.service.for_track("navidrome:7")), ["Server"]);

        r.playing(YtmStatus::Playing, Some("demoSong001"));
        r.page_says(Ok((
            "demoSong001".into(),
            Some(plain(LyricsSource::Youtube, "Page")),
        )));
        assert_eq!(lines(&r.service.for_youtube()), ["Page"]);

        assert_eq!(r.lrclib.requests(), 0, "nothing reached LRCLIB");
    }

    #[test]
    fn the_setting_is_read_at_every_call() {
        let r = rig(|_, _| json_plain("Online"));
        let dir = tempfile::tempdir().unwrap();
        r.add(track(
            "l1",
            Source::Local,
            Some(&local_file(dir.path(), None)),
        ));
        r.lookup.store(false, Ordering::SeqCst);
        assert_eq!(r.service.for_track("l1"), LyricsAnswer::None);
        r.lookup.store(true, Ordering::SeqCst);
        assert_eq!(lines(&r.service.for_track("l1")), ["Online"]);
        r.lookup.store(false, Ordering::SeqCst);
        assert_eq!(r.service.for_track("l1"), LyricsAnswer::None);
    }

    // Navidrome

    fn nav_synced() -> Option<SongLyrics> {
        Some((Some(synced(LyricsSource::Navidrome, "Server one")), None))
    }

    #[test]
    fn navidrome_synced_wins_and_is_cached_by_the_server_id() {
        let r = rig(|_, _| json_synced(LRC));
        r.add(track("navidrome:abc", Source::Navidrome, None));
        r.navidrome_says(Ok(nav_synced()));
        let answer = r.service.for_track("navidrome:abc");
        assert_eq!(source_of(&answer), LyricsSource::Navidrome);
        assert_eq!(r.lrclib.requests(), 0);
        assert!(matches!(r.cached("navidrome:abc"), Cached::Hit(Some(_))));
        let again = r.service.for_track("navidrome:abc");
        assert_eq!(again, answer);
        assert_eq!(r.sources.navidrome_calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn navidrome_plain_yields_to_lrclib_synced_then_beats_lrclib_plain() {
        let r = rig(|_, _| json_synced(LRC));
        r.add(track("navidrome:abc", Source::Navidrome, None));
        r.navidrome_says(Ok(Some((
            None,
            Some(plain(LyricsSource::Navidrome, "Server")),
        ))));
        let answer = r.service.for_track("navidrome:abc");
        assert_eq!(source_of(&answer), LyricsSource::Lrclib);

        let r = rig(|_, _| json_plain("Online"));
        r.add(track("navidrome:abc", Source::Navidrome, None));
        r.navidrome_says(Ok(Some((
            None,
            Some(plain(LyricsSource::Navidrome, "Server")),
        ))));
        assert_eq!(lines(&r.service.for_track("navidrome:abc")), ["Server"]);
    }

    #[test]
    fn navidrome_without_lyrics_falls_back_to_lrclib() {
        let r = rig(|_, _| json_plain("Online"));
        r.add(track("navidrome:abc", Source::Navidrome, None));
        r.navidrome_says(Ok(None));
        assert_eq!(lines(&r.service.for_track("navidrome:abc")), ["Online"]);
        assert_eq!(r.cached("navidrome:abc"), Cached::Hit(None));
    }

    #[test]
    fn navidrome_offline_is_an_error_only_when_nothing_else_is_found() {
        let r = rig(|_, _| not_found());
        r.add(track("navidrome:abc", Source::Navidrome, None));
        r.navidrome_says(Err(SourceFailed));
        assert_eq!(r.service.for_track("navidrome:abc"), LyricsAnswer::Error);
        assert_eq!(
            r.cached("navidrome:abc"),
            Cached::Miss,
            "an error is not a miss"
        );

        let r = rig(|_, _| json_plain("Online"));
        r.add(track("navidrome:abc", Source::Navidrome, None));
        r.navidrome_says(Err(SourceFailed));
        assert_eq!(lines(&r.service.for_track("navidrome:abc")), ["Online"]);
    }

    #[test]
    fn navidrome_offline_with_the_lookup_off_is_an_error() {
        let r = rig(|_, _| json_plain("Online"));
        r.lookup.store(false, Ordering::SeqCst);
        r.add(track("navidrome:abc", Source::Navidrome, None));
        r.navidrome_says(Err(SourceFailed));
        assert_eq!(r.service.for_track("navidrome:abc"), LyricsAnswer::Error);
        assert_eq!(r.lrclib.requests(), 0);
    }

    // LRCLIB errors and the cache

    #[test]
    fn an_lrclib_error_is_not_cached_and_is_retried() {
        let broken = Arc::new(AtomicBool::new(true));
        let r = rig({
            let broken = Arc::clone(&broken);
            move |_, _| {
                if broken.load(Ordering::SeqCst) {
                    (500, "{}".to_owned())
                } else {
                    json_plain("Online")
                }
            }
        });
        let dir = tempfile::tempdir().unwrap();
        r.add(track(
            "l1",
            Source::Local,
            Some(&local_file(dir.path(), None)),
        ));
        assert_eq!(r.service.for_track("l1"), LyricsAnswer::Error);
        assert_eq!(
            r.cached("lrclib:test artist|song one|test album|200"),
            Cached::Miss
        );
        broken.store(false, Ordering::SeqCst);
        assert_eq!(lines(&r.service.for_track("l1")), ["Online"]);
    }

    #[test]
    fn a_found_lrclib_answer_is_served_from_the_cache() {
        let r = rig(|_, _| json_plain("Online"));
        let dir = tempfile::tempdir().unwrap();
        r.add(track(
            "l1",
            Source::Local,
            Some(&local_file(dir.path(), None)),
        ));
        assert_eq!(lines(&r.service.for_track("l1")), ["Online"]);
        let after_first = r.lrclib.requests();
        assert_eq!(lines(&r.service.for_track("l1")), ["Online"]);
        assert_eq!(r.lrclib.requests(), after_first);
    }

    #[test]
    fn an_expired_miss_is_asked_again() {
        let r = rig(|_, _| json_plain("Online"));
        let dir = tempfile::tempdir().unwrap();
        let t = track("l1", Source::Local, Some(&local_file(dir.path(), None)));
        let key = cache::lrclib_key(&track_query(&t));
        r.add(t);
        cache::put(
            &lock(&r.db).unwrap(),
            &key,
            None,
            now() - cache::MISS_TTL_SECS - 10,
        )
        .unwrap();
        assert_eq!(lines(&r.service.for_track("l1")), ["Online"]);
        assert_eq!(r.lrclib.requests(), 1);
    }

    // YouTube Music

    #[test]
    fn youtube_idle_or_unavailable_answers_none_without_asking_anyone() {
        let r = rig(|_, _| json_synced(LRC));
        for status in [YtmStatus::Idle, YtmStatus::Unavailable] {
            r.playing(status, Some("demoSong001"));
            assert_eq!(r.service.for_youtube(), LyricsAnswer::None);
        }
        assert_eq!(r.lrclib.requests(), 0);
        assert_eq!(r.sources.page_calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn youtube_lrclib_synced_beats_the_page() {
        let r = rig(|_, _| json_synced(LRC));
        r.playing(YtmStatus::Playing, Some("demoSong001"));
        r.page_says(Ok((
            "demoSong001".into(),
            Some(plain(LyricsSource::Youtube, "Page")),
        )));
        let answer = r.service.for_youtube();
        assert_eq!(source_of(&answer), LyricsSource::Lrclib);
        assert_eq!(r.sources.page_calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn youtube_page_beats_lrclib_plain_and_is_cached() {
        let r = rig(|_, _| json_plain("Online"));
        r.playing(YtmStatus::Paused, Some("demoSong001"));
        r.page_says(Ok((
            "demoSong001".into(),
            Some(plain(LyricsSource::Youtube, "Page")),
        )));
        assert_eq!(lines(&r.service.for_youtube()), ["Page"]);
        assert!(matches!(
            r.cached("youtube:demoSong001"),
            Cached::Hit(Some(_))
        ));
        assert_eq!(lines(&r.service.for_youtube()), ["Page"]);
        assert_eq!(r.sources.page_calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn youtube_page_without_lyrics_falls_back_to_lrclib_plain_and_caches_the_miss() {
        let r = rig(|_, _| json_plain("Online"));
        r.playing(YtmStatus::Playing, Some("demoSong001"));
        r.page_says(Ok(("demoSong001".into(), None)));
        assert_eq!(lines(&r.service.for_youtube()), ["Online"]);
        assert_eq!(r.cached("youtube:demoSong001"), Cached::Hit(None));
    }

    #[test]
    fn youtube_lrclib_instrumental_ends_the_search() {
        let r = rig(|_, _| json_instrumental());
        r.playing(YtmStatus::Playing, Some("demoSong001"));
        r.page_says(Ok((
            "demoSong001".into(),
            Some(plain(LyricsSource::Youtube, "Page")),
        )));
        match r.service.for_youtube() {
            LyricsAnswer::Found { lyrics } => assert!(lyrics.instrumental),
            other => panic!("{other:?}"),
        }
        assert_eq!(r.sources.page_calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn youtube_page_errors_are_not_cached_and_show_as_error() {
        let r = rig(|_, _| not_found());
        r.playing(YtmStatus::Playing, Some("demoSong001"));
        for error in [
            PageLyricsError::Timeout,
            PageLyricsError::Failed,
            PageLyricsError::Changed,
        ] {
            r.page_says(Err(error));
            assert_eq!(r.service.for_youtube(), LyricsAnswer::Error);
            assert_eq!(r.cached("youtube:demoSong001"), Cached::Miss);
        }
    }

    #[test]
    fn youtube_page_error_still_gives_lrclib_plain() {
        let r = rig(|_, _| json_plain("Online"));
        r.playing(YtmStatus::Playing, Some("demoSong001"));
        r.page_says(Err(PageLyricsError::Timeout));
        assert_eq!(lines(&r.service.for_youtube()), ["Online"]);
    }

    #[test]
    fn youtube_answer_for_another_video_is_an_error_not_a_cache_entry() {
        let r = rig(|_, _| not_found());
        r.playing(YtmStatus::Playing, Some("demoSong001"));
        r.page_says(Ok((
            "demoSong002".into(),
            Some(plain(LyricsSource::Youtube, "Other")),
        )));
        assert_eq!(r.service.for_youtube(), LyricsAnswer::Error);
        assert_eq!(r.cached("youtube:demoSong002"), Cached::Miss);
        assert_eq!(r.cached("youtube:demoSong001"), Cached::Miss);
    }

    #[test]
    fn youtube_without_a_video_id_is_an_error_when_lrclib_has_nothing() {
        let r = rig(|_, _| not_found());
        r.playing(YtmStatus::Playing, None);
        assert_eq!(r.service.for_youtube(), LyricsAnswer::Error);
        assert_eq!(r.sources.page_calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn youtube_found_entries_expire_after_thirty_days_and_misses_after_seven() {
        let r = rig(|_, _| not_found());
        r.playing(YtmStatus::Playing, Some("demoSong001"));
        let stored = plain(LyricsSource::Youtube, "Old page");
        let put = |lyrics: Option<&Lyrics>, age: i64| {
            cache::put(
                &lock(&r.db).unwrap(),
                "youtube:demoSong001",
                lyrics,
                now() - age,
            )
            .unwrap();
        };
        r.page_says(Ok((
            "demoSong001".into(),
            Some(plain(LyricsSource::Youtube, "New page")),
        )));

        put(Some(&stored), FOUND_TTL_PAGE_SECS - 60);
        assert_eq!(lines(&r.service.for_youtube()), ["Old page"]);
        assert_eq!(r.sources.page_calls.load(Ordering::SeqCst), 0);

        put(Some(&stored), FOUND_TTL_PAGE_SECS + 60);
        assert_eq!(lines(&r.service.for_youtube()), ["New page"]);
        assert_eq!(r.sources.page_calls.load(Ordering::SeqCst), 1);

        put(None, cache::MISS_TTL_SECS + 60);
        assert_eq!(lines(&r.service.for_youtube()), ["New page"]);
        assert_eq!(r.sources.page_calls.load(Ordering::SeqCst), 2);
    }

    // Single flight

    #[test]
    fn concurrent_calls_for_one_track_share_one_lookup() {
        let r = rig(|_, _| json_plain("Online"));
        r.add(track("navidrome:abc", Source::Navidrome, None));
        r.navidrome_says(Ok(nav_synced()));
        *lock_plain(&r.sources.navidrome_delay) = Duration::from_millis(300);
        let answers: Vec<LyricsAnswer> = std::thread::scope(|scope| {
            let handles: Vec<_> = (0..4)
                .map(|_| scope.spawn(|| r.service.for_track("navidrome:abc")))
                .collect();
            handles.into_iter().map(|h| h.join().unwrap()).collect()
        });
        assert!(answers.iter().all(|a| *a == answers[0]));
        assert_eq!(source_of(&answers[0]), LyricsSource::Navidrome);
        assert_eq!(r.sources.navidrome_calls.load(Ordering::SeqCst), 1);
        assert!(lock_plain(&r.service.flights).is_empty());
    }

    #[test]
    fn different_tracks_do_not_wait_for_each_other_and_flights_end() {
        let r = rig(|_, _| json_plain("Online"));
        r.add(track("navidrome:a", Source::Navidrome, None));
        r.add(track("navidrome:b", Source::Navidrome, None));
        r.navidrome_says(Ok(nav_synced()));
        r.service.for_track("navidrome:a");
        r.service.for_track("navidrome:b");
        assert_eq!(r.sources.navidrome_calls.load(Ordering::SeqCst), 2);
        r.service.for_track("navidrome:a");
        assert_eq!(
            r.sources.navidrome_calls.load(Ordering::SeqCst),
            2,
            "cached"
        );
        assert!(lock_plain(&r.service.flights).is_empty());
    }

    #[test]
    fn a_panicking_lookup_ends_the_flight_with_an_error() {
        let r = rig(|_, _| not_found());
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            r.service
                .single_flight("k".into(), || panic!("lookup failed"))
        }));
        assert!(result.is_err());
        assert!(lock_plain(&r.service.flights).is_empty());
        assert_eq!(
            r.service.single_flight("k".into(), || LyricsAnswer::None),
            LyricsAnswer::None
        );
    }

    #[test]
    fn answers_serialize_with_a_status_tag() {
        assert_eq!(
            serde_json::to_value(LyricsAnswer::None).unwrap(),
            serde_json::json!({ "status": "none" })
        );
        assert_eq!(
            serde_json::to_value(LyricsAnswer::Error).unwrap(),
            serde_json::json!({ "status": "error" })
        );
        let value = serde_json::to_value(found(synced(LyricsSource::Lrclib, "A"))).unwrap();
        assert_eq!(value["status"], "found");
        assert_eq!(value["lyrics"]["source"], "lrclib");
        assert_eq!(value["lyrics"]["lines"][0]["startMs"], 1000);
    }

    #[test]
    fn a_real_service_without_lrclib_never_goes_online() {
        let r = rig(|_, _| json_plain("Online"));
        let service = LyricsService::new(
            Arc::clone(&r.db),
            None,
            Arc::clone(&r.sources) as Arc<dyn Sources>,
            Arc::new(|| true),
        );
        let dir = tempfile::tempdir().unwrap();
        r.add(track(
            "l1",
            Source::Local,
            Some(&local_file(dir.path(), None)),
        ));
        assert_eq!(service.for_track("l1"), LyricsAnswer::None);
        assert_eq!(r.lrclib.requests(), 0);
    }
}
