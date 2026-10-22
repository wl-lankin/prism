//! LRCLIB (lrclib.net) client. It returns the raw texts; the resolver turns them into `Lyrics`
//! with the LRC parser and `Lyrics::plain`. Blocking: callers run it off the UI thread.

use std::fmt;
use std::io::Read;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde_json::Value;

pub const DEFAULT_BASE: &str = "https://lrclib.net";
const TIMEOUT: Duration = Duration::from_secs(10);
const REQUEST_GAP: Duration = Duration::from_millis(200);
/// Largest response body read.
const MAX_BODY: u64 = 1024 * 1024;
/// Largest lyrics text kept per kind.
const MAX_TEXT: usize = 256 * 1024;
/// A search hit may differ from the track length by this much.
const DURATION_SLACK_SECS: f64 = 2.0;
const MAX_SEARCH_ENTRIES: usize = 50;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Query {
    pub artist: String,
    pub title: String,
    pub album: Option<String>,
    /// 0 when the length is unknown.
    pub duration_ms: u64,
}

impl Query {
    pub(super) fn duration_secs(&self) -> u64 {
        (self.duration_ms + 500) / 1000
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Found {
    pub synced_lrc: Option<String>,
    pub plain: Option<String>,
    pub instrumental: bool,
}

impl Found {
    pub fn is_empty(&self) -> bool {
        self.synced_lrc.is_none() && self.plain.is_none() && !self.instrumental
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LrclibError {
    /// Offline, refused or timed out.
    Unreachable,
    /// A status other than 2xx and 404.
    Server(u16),
    /// Not JSON, not the expected shape, or over the size cap.
    BadResponse,
}

impl fmt::Display for LrclibError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LrclibError::Unreachable => f.write_str("LRCLIB is not reachable"),
            LrclibError::Server(status) => write!(f, "LRCLIB answered with status {status}"),
            LrclibError::BadResponse => f.write_str("LRCLIB sent an unusable answer"),
        }
    }
}

impl std::error::Error for LrclibError {}

pub struct Lrclib {
    base: String,
    client: reqwest::blocking::Client,
    gap: Duration,
    /// Held across a request: one at a time. Holds the end of the last request.
    last: Mutex<Option<Instant>>,
}

impl Lrclib {
    pub fn new(base_url: &str) -> anyhow::Result<Lrclib> {
        Lrclib::with_timing(base_url, TIMEOUT, REQUEST_GAP)
    }

    pub(crate) fn with_timing(
        base_url: &str,
        timeout: Duration,
        gap: Duration,
    ) -> anyhow::Result<Lrclib> {
        let client = reqwest::blocking::Client::builder()
            .user_agent(format!(
                "Prism/{} ( https://github.com/wl-lankin/prism )",
                env!("CARGO_PKG_VERSION")
            ))
            .connect_timeout(timeout)
            .timeout(timeout)
            .redirect(reqwest::redirect::Policy::none())
            .build()?;
        Ok(Lrclib {
            base: base_url.trim_end_matches('/').to_owned(),
            client,
            gap,
            last: Mutex::new(None),
        })
    }

    /// Looks the track up; an empty `Found` means LRCLIB does not know it.
    pub fn find(&self, q: &Query) -> Result<Found, LrclibError> {
        let found = self.lookup(q)?;
        if !found.is_empty() {
            return Ok(found);
        }
        match strip_version_suffix(&q.title) {
            Some(title) => self.lookup(&Query { title, ..q.clone() }),
            None => Ok(found),
        }
    }

    fn lookup(&self, q: &Query) -> Result<Found, LrclibError> {
        // LRCLIB answers 400 for these; that would read as a server error on every play.
        if q.artist.trim().is_empty() || q.title.trim().is_empty() {
            return Ok(Found::default());
        }
        let mut params = vec![
            ("artist_name", q.artist.clone()),
            ("track_name", q.title.clone()),
        ];
        if let Some(album) = q.album.as_deref().filter(|a| !a.trim().is_empty()) {
            params.push(("album_name", album.to_owned()));
        }
        if (1..=3600).contains(&q.duration_secs()) {
            params.push(("duration", q.duration_secs().to_string()));
        }
        if let Some(body) = self.get("/api/get", &params)? {
            return entry_from(&body)
                .map(|e| e.found)
                .ok_or(LrclibError::BadResponse);
        }

        let params = [
            ("track_name", q.title.clone()),
            ("artist_name", q.artist.clone()),
        ];
        let Some(body) = self.get("/api/search", &params)? else {
            return Ok(Found::default());
        };
        let Value::Array(items) = body else {
            return Err(LrclibError::BadResponse);
        };
        let wanted = q.duration_ms as f64 / 1000.0;
        let mut first = None;
        for item in items.iter().take(MAX_SEARCH_ENTRIES) {
            let Some(entry) = entry_from(item) else {
                continue;
            };
            let near = q.duration_ms == 0
                || entry
                    .duration
                    .is_some_and(|d| (d - wanted).abs() <= DURATION_SLACK_SECS);
            if !near || entry.found.is_empty() {
                continue;
            }
            if entry.found.synced_lrc.is_some() {
                return Ok(entry.found);
            }
            first.get_or_insert(entry.found);
        }
        Ok(first.unwrap_or_default())
    }

    /// One GET. `None` on 404.
    fn get(&self, path: &str, params: &[(&str, String)]) -> Result<Option<Value>, LrclibError> {
        let mut last = self.last.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(at) = *last {
            let wait = self.gap.saturating_sub(at.elapsed());
            if !wait.is_zero() {
                std::thread::sleep(wait);
            }
        }
        let result = self.send(path, params);
        *last = Some(Instant::now());
        result
    }

    fn send(&self, path: &str, params: &[(&str, String)]) -> Result<Option<Value>, LrclibError> {
        let url = format!("{}{path}", self.base);
        #[cfg(test)]
        tests::assert_local(&url);
        let response = self
            .client
            .get(&url)
            .query(params)
            .header("Accept", "application/json")
            .send()
            .map_err(|_| LrclibError::Unreachable)?;
        let status = response.status();
        if status.as_u16() == 404 {
            return Ok(None);
        }
        if !status.is_success() {
            return Err(LrclibError::Server(status.as_u16()));
        }
        if response.content_length().is_some_and(|len| len > MAX_BODY) {
            return Err(LrclibError::BadResponse);
        }
        let mut bytes = Vec::new();
        response
            .take(MAX_BODY + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| LrclibError::Unreachable)?;
        if bytes.len() as u64 > MAX_BODY {
            return Err(LrclibError::BadResponse);
        }
        serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|_| LrclibError::BadResponse)
    }
}

struct Entry {
    duration: Option<f64>,
    found: Found,
}

fn entry_from(value: &Value) -> Option<Entry> {
    let object = value.as_object()?;
    let text = |key: &str| {
        object
            .get(key)
            .and_then(Value::as_str)
            .filter(|t| !t.trim().is_empty())
            .map(capped)
    };
    Some(Entry {
        duration: object.get("duration").and_then(Value::as_f64),
        found: Found {
            synced_lrc: text("syncedLyrics"),
            plain: text("plainLyrics"),
            instrumental: object
                .get("instrumental")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        },
    })
}

fn capped(text: &str) -> String {
    if text.len() <= MAX_TEXT {
        return text.to_owned();
    }
    let mut end = MAX_TEXT;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_owned()
}

fn mentions_version(suffix: &str) -> bool {
    suffix
        .to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .any(|word| {
            [
                "live",
                "remaster",
                "remastered",
                "version",
                "versions",
                "edit",
                "mono",
                "stereo",
            ]
            .contains(&word)
        })
}

/// The title without a trailing "(... Remastered)", "[Live]" or " - 2011 Remaster" style part,
/// when there is one and something is left.
fn strip_version_suffix(title: &str) -> Option<String> {
    let title = title.trim();
    let bracketed = [(')', '('), (']', '[')].iter().find_map(|(close, open)| {
        let inner = title.strip_suffix(*close)?;
        let at = inner.rfind(*open)?;
        Some((&inner[..at], &inner[at + 1..]))
    });
    let dashed = || {
        let at = title.rfind(" - ")?;
        Some((&title[..at], &title[at + 3..]))
    };
    let (head, _) = bracketed
        .filter(|(_, suffix)| mentions_version(suffix))
        .or_else(|| dashed().filter(|(_, suffix)| mentions_version(suffix)))?;
    let head = head.trim();
    (!head.is_empty()).then(|| head.to_owned())
}

#[cfg(test)]
pub(crate) mod tests {
    use std::io::Write;
    use std::net::{TcpListener, TcpStream};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use std::thread::JoinHandle;

    use super::*;

    static LIVE: AtomicBool = AtomicBool::new(false);

    pub(super) fn assert_local(url: &str) {
        if !LIVE.load(Ordering::SeqCst) {
            crate::radio::testserver::assert_loopback(url);
        }
    }

    struct Reply {
        status: u16,
        body: String,
        delay: Duration,
        /// Body ends when the connection closes.
        no_length: bool,
    }

    fn reply(status: u16, body: &str) -> Reply {
        Reply {
            status,
            body: body.to_owned(),
            delay: Duration::ZERO,
            no_length: false,
        }
    }

    type Handler = Box<dyn Fn(&reqwest::Url) -> Reply + Send + Sync>;

    /// A fake LRCLIB: one request per connection, answers come from the handler.
    struct FakeLrclib {
        port: u16,
        requests: Arc<Mutex<Vec<String>>>,
        agents: Arc<Mutex<Vec<String>>>,
        stop: Arc<AtomicBool>,
        thread: Option<JoinHandle<()>>,
    }

    impl FakeLrclib {
        fn start(handler: impl Fn(&reqwest::Url) -> Reply + Send + Sync + 'static) -> FakeLrclib {
            let listener = TcpListener::bind("127.0.0.1:0").expect("binding the fake LRCLIB");
            let port = listener.local_addr().unwrap().port();
            listener.set_nonblocking(true).unwrap();
            let requests = Arc::new(Mutex::new(Vec::new()));
            let agents = Arc::new(Mutex::new(Vec::new()));
            let stop = Arc::new(AtomicBool::new(false));
            let handler: Arc<Handler> = Arc::new(Box::new(handler));
            let thread = {
                let requests = Arc::clone(&requests);
                let agents = Arc::clone(&agents);
                let stop = Arc::clone(&stop);
                std::thread::spawn(move || {
                    while !stop.load(Ordering::SeqCst) {
                        match listener.accept() {
                            Ok((stream, _)) => {
                                let requests = Arc::clone(&requests);
                                let agents = Arc::clone(&agents);
                                let handler = Arc::clone(&handler);
                                std::thread::spawn(move || {
                                    serve(stream, &requests, &agents, &handler)
                                });
                            }
                            Err(_) => std::thread::sleep(Duration::from_millis(5)),
                        }
                    }
                })
            };
            FakeLrclib {
                port,
                requests,
                agents,
                stop,
                thread: Some(thread),
            }
        }

        fn client(&self) -> Lrclib {
            Lrclib::with_timing(&self.base(), Duration::from_secs(5), Duration::ZERO).unwrap()
        }

        fn base(&self) -> String {
            format!("http://127.0.0.1:{}", self.port)
        }

        fn requests(&self) -> Vec<String> {
            self.requests.lock().unwrap().clone()
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

    fn serve(
        mut stream: TcpStream,
        requests: &Mutex<Vec<String>>,
        agents: &Mutex<Vec<String>>,
        handler: &Handler,
    ) {
        let _ = stream.set_nonblocking(false);
        let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
        let mut head = Vec::new();
        let mut buf = [0u8; 2048];
        while !head.windows(4).any(|w| w == b"\r\n\r\n") {
            match stream.read(&mut buf) {
                Ok(n) if n > 0 && head.len() < 16 * 1024 => head.extend_from_slice(&buf[..n]),
                _ => return,
            }
        }
        let head = String::from_utf8_lossy(&head).into_owned();
        let Some(target) = head.lines().next().and_then(|l| l.split(' ').nth(1)) else {
            return;
        };
        let Ok(url) = reqwest::Url::parse(&format!("http://fake{target}")) else {
            return;
        };
        requests.lock().unwrap().push(target.to_owned());
        if let Some(agent) = head.lines().find_map(|l| {
            l.strip_prefix("user-agent: ")
                .or_else(|| l.strip_prefix("User-Agent: "))
        }) {
            agents.lock().unwrap().push(agent.to_owned());
        }
        let reply = handler(&url);
        std::thread::sleep(reply.delay);
        let text = match reply.status {
            200 => "OK",
            404 => "Not Found",
            500 => "Internal Server Error",
            _ => "Status",
        };
        let length = if reply.no_length {
            String::new()
        } else {
            format!("Content-Length: {}\r\n", reply.body.len())
        };
        let out = format!(
            "HTTP/1.1 {} {text}\r\nContent-Type: application/json\r\n{length}Connection: close\r\n\r\n",
            reply.status
        );
        let _ = stream.write_all(out.as_bytes());
        let _ = stream.write_all(reply.body.as_bytes());
    }

    fn param(url: &reqwest::Url, name: &str) -> Option<String> {
        url.query_pairs()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.into_owned())
    }

    fn query() -> Query {
        Query {
            artist: "Test Artist".into(),
            title: "Song One".into(),
            album: Some("Test Album".into()),
            duration_ms: 200_400,
        }
    }

    fn entry(
        duration: f64,
        synced: Option<&str>,
        plain: Option<&str>,
        instrumental: bool,
    ) -> Value {
        serde_json::json!({
            "id": 1,
            "name": "Song One",
            "trackName": "Song One",
            "artistName": "Test Artist",
            "albumName": "Test Album",
            "duration": duration,
            "instrumental": instrumental,
            "hasWordSync": false,
            "plainLyrics": plain,
            "syncedLyrics": synced,
            "lyricsfile": "",
        })
    }

    const SYNCED: &str = "[00:01.00] Invented line one\n[00:03.50] Invented line two";
    const PLAIN: &str = "Invented line one\nInvented line two";

    #[test]
    fn get_with_synced_and_plain() {
        let fake = FakeLrclib::start(|url| {
            assert_eq!(url.path(), "/api/get");
            assert_eq!(param(url, "artist_name").as_deref(), Some("Test Artist"));
            assert_eq!(param(url, "track_name").as_deref(), Some("Song One"));
            assert_eq!(param(url, "album_name").as_deref(), Some("Test Album"));
            assert_eq!(param(url, "duration").as_deref(), Some("200"));
            reply(
                200,
                &entry(200.4, Some(SYNCED), Some(PLAIN), false).to_string(),
            )
        });
        let found = fake.client().find(&query()).unwrap();
        assert_eq!(found.synced_lrc.as_deref(), Some(SYNCED));
        assert_eq!(found.plain.as_deref(), Some(PLAIN));
        assert!(!found.instrumental);
        assert_eq!(fake.requests().len(), 1);
    }

    #[test]
    fn plain_only_and_blank_strings_count_as_absent() {
        let fake = FakeLrclib::start(|_| {
            reply(
                200,
                &entry(200.0, Some("  \n"), Some(PLAIN), false).to_string(),
            )
        });
        let found = fake.client().find(&query()).unwrap();
        assert_eq!(found.synced_lrc, None);
        assert_eq!(found.plain.as_deref(), Some(PLAIN));
    }

    #[test]
    fn instrumental_track() {
        let fake = FakeLrclib::start(|_| reply(200, &entry(200.0, None, None, true).to_string()));
        let found = fake.client().find(&query()).unwrap();
        assert!(found.instrumental);
        assert_eq!((found.synced_lrc, found.plain), (None, None));
    }

    #[test]
    fn missing_album_and_duration_are_left_out() {
        let fake = FakeLrclib::start(|url| {
            assert_eq!(param(url, "album_name"), None);
            assert_eq!(param(url, "duration"), None);
            reply(200, &entry(200.0, None, Some(PLAIN), false).to_string())
        });
        let q = Query {
            album: None,
            duration_ms: 0,
            ..query()
        };
        assert!(fake.client().find(&q).unwrap().plain.is_some());
    }

    #[test]
    fn not_found_then_search_hit_prefers_synced() {
        let fake = FakeLrclib::start(|url| match url.path() {
            "/api/get" => reply(404, r#"{"code":404}"#),
            _ => {
                assert_eq!(param(url, "track_name").as_deref(), Some("Song One"));
                assert_eq!(param(url, "artist_name").as_deref(), Some("Test Artist"));
                let list = vec![
                    entry(200.0, None, Some("Plain first"), false),
                    entry(201.5, Some(SYNCED), Some(PLAIN), false),
                ];
                reply(200, &Value::Array(list).to_string())
            }
        });
        let found = fake.client().find(&query()).unwrap();
        assert_eq!(found.synced_lrc.as_deref(), Some(SYNCED));
        assert_eq!(fake.requests().len(), 2);
    }

    #[test]
    fn search_without_synced_takes_the_first_near_entry() {
        let fake = FakeLrclib::start(|url| match url.path() {
            "/api/get" => reply(404, "{}"),
            _ => {
                let list = vec![
                    entry(230.0, Some(SYNCED), None, false),
                    entry(199.0, None, Some("Near one"), false),
                    entry(200.0, None, Some("Near two"), false),
                ];
                reply(200, &Value::Array(list).to_string())
            }
        });
        let found = fake.client().find(&query()).unwrap();
        assert_eq!(found.plain.as_deref(), Some("Near one"));
    }

    #[test]
    fn search_with_a_duration_mismatch_finds_nothing() {
        let fake = FakeLrclib::start(|url| match url.path() {
            "/api/get" => reply(404, "{}"),
            _ => {
                let list = vec![
                    entry(203.0, Some(SYNCED), None, false),
                    entry(190.0, None, Some(PLAIN), false),
                ];
                reply(200, &Value::Array(list).to_string())
            }
        });
        let found = fake.client().find(&query()).unwrap();
        assert!(found.is_empty());
        assert_eq!(fake.requests().len(), 2);
    }

    #[test]
    fn suffix_is_removed_for_one_retry() {
        let fake = FakeLrclib::start(|url| {
            let title = param(url, "track_name").unwrap();
            match (url.path(), title.as_str()) {
                ("/api/get", "Song One") => {
                    reply(200, &entry(200.0, Some(SYNCED), None, false).to_string())
                }
                (_, "Song One (2011 Remastered Version)") => reply(404, "{}"),
                _ => panic!("unexpected request {url}"),
            }
        });
        let q = Query {
            title: "Song One (2011 Remastered Version)".into(),
            ..query()
        };
        let found = fake.client().find(&q).unwrap();
        assert_eq!(found.synced_lrc.as_deref(), Some(SYNCED));
        // get and search with the suffix, then get without it.
        assert_eq!(fake.requests().len(), 3);
    }

    #[test]
    fn suffix_retry_happens_only_once() {
        let fake = FakeLrclib::start(|url| match url.path() {
            "/api/get" => reply(404, "{}"),
            _ => reply(200, "[]"),
        });
        let q = Query {
            title: "Song One - Live".into(),
            ..query()
        };
        assert!(fake.client().find(&q).unwrap().is_empty());
        assert_eq!(fake.requests().len(), 4);
    }

    #[test]
    fn suffix_rules() {
        let cases = [
            ("Song (Remastered 2009)", Some("Song")),
            ("Song [Live at Testhausen]", Some("Song")),
            ("Song - 2011 Remaster", Some("Song")),
            ("Song - Radio Edit", Some("Song")),
            ("Song (Mono)", Some("Song")),
            ("Song (Stereo Version)", Some("Song")),
            ("Song (Neue Version)", Some("Song")),
            ("Song (feat. Someone)", None),
            ("Song (Deliverance)", None),
            ("Song - Part 2", None),
            ("Song (Monologue)", None),
            ("Song - Stereotype", None),
            ("Song (Editorial)", None),
            ("Song - Live (Bonus Track)", Some("Song")),
            (
                "Song (Bonus Track) - Remastered",
                Some("Song (Bonus Track)"),
            ),
            ("Song", None),
            ("(Live)", None),
        ];
        for (title, want) in cases {
            assert_eq!(strip_version_suffix(title).as_deref(), want, "{title}");
        }
    }

    #[test]
    fn server_error_is_an_error() {
        let fake = FakeLrclib::start(|_| reply(500, "oops"));
        assert_eq!(fake.client().find(&query()), Err(LrclibError::Server(500)));
    }

    #[test]
    fn bad_json_is_an_error() {
        let fake = FakeLrclib::start(|_| reply(200, "not json"));
        assert_eq!(fake.client().find(&query()), Err(LrclibError::BadResponse));
    }

    #[test]
    fn oversize_body_is_refused() {
        let fake = FakeLrclib::start(|_| {
            let big = "x".repeat(MAX_BODY as usize + 10);
            reply(200, &entry(200.0, None, Some(&big), false).to_string())
        });
        assert_eq!(fake.client().find(&query()), Err(LrclibError::BadResponse));
    }

    #[test]
    fn oversize_body_without_a_length_is_cut_off() {
        let fake = FakeLrclib::start(|_| {
            let big = "x".repeat(MAX_BODY as usize + 10);
            Reply {
                no_length: true,
                ..reply(200, &entry(200.0, None, Some(&big), false).to_string())
            }
        });
        assert_eq!(fake.client().find(&query()), Err(LrclibError::BadResponse));
    }

    #[test]
    fn blank_artist_or_title_sends_nothing() {
        let fake = FakeLrclib::start(|_| reply(400, "{}"));
        for (artist, title) in [(" ", "Song One"), ("Test Artist", "")] {
            let q = Query {
                artist: artist.into(),
                title: title.into(),
                ..query()
            };
            assert!(fake.client().find(&q).unwrap().is_empty());
        }
        assert!(fake.requests().is_empty());
    }

    #[test]
    fn duration_outside_the_accepted_range_is_left_out() {
        let fake = FakeLrclib::start(|url| {
            assert_eq!(param(url, "duration"), None, "{url}");
            reply(200, &entry(0.0, None, Some(PLAIN), false).to_string())
        });
        for duration_ms in [0, 400, 3_600_500, 7_200_000] {
            let q = Query {
                duration_ms,
                ..query()
            };
            assert!(fake.client().find(&q).unwrap().plain.is_some());
        }
        let fake = FakeLrclib::start(|url| {
            assert_eq!(param(url, "duration").as_deref(), Some("3600"));
            reply(200, &entry(3600.0, None, Some(PLAIN), false).to_string())
        });
        let q = Query {
            duration_ms: 3_600_000,
            ..query()
        };
        assert!(fake.client().find(&q).unwrap().plain.is_some());
    }

    #[test]
    fn long_texts_are_capped() {
        let fake = FakeLrclib::start(|_| {
            let long = "ä".repeat(MAX_TEXT);
            reply(200, &entry(200.0, None, Some(&long), false).to_string())
        });
        let plain = fake.client().find(&query()).unwrap().plain.unwrap();
        assert!(plain.len() <= MAX_TEXT && plain.len() > MAX_TEXT - 4);
    }

    #[test]
    fn a_slow_server_times_out() {
        let fake = FakeLrclib::start(|_| Reply {
            delay: Duration::from_millis(800),
            ..reply(200, "{}")
        });
        let client =
            Lrclib::with_timing(&fake.base(), Duration::from_millis(200), Duration::ZERO).unwrap();
        assert_eq!(client.find(&query()), Err(LrclibError::Unreachable));
    }

    #[test]
    fn requests_keep_their_gap() {
        let fake =
            FakeLrclib::start(|_| reply(200, &entry(200.0, None, Some(PLAIN), false).to_string()));
        let client = Lrclib::with_timing(
            &fake.base(),
            Duration::from_secs(5),
            Duration::from_millis(200),
        )
        .unwrap();
        let started = Instant::now();
        client.find(&query()).unwrap();
        client.find(&query()).unwrap();
        assert!(started.elapsed() >= Duration::from_millis(200));
    }

    #[test]
    fn concurrent_requests_keep_their_gap() {
        let arrivals = Arc::new(Mutex::new(Vec::<Instant>::new()));
        let seen = Arc::clone(&arrivals);
        let fake = FakeLrclib::start(move |_| {
            seen.lock().unwrap().push(Instant::now());
            reply(200, &entry(200.0, None, Some(PLAIN), false).to_string())
        });
        let client = Arc::new(
            Lrclib::with_timing(
                &fake.base(),
                Duration::from_secs(5),
                Duration::from_millis(200),
            )
            .unwrap(),
        );
        let threads: Vec<_> = (0..2)
            .map(|_| {
                let client = Arc::clone(&client);
                std::thread::spawn(move || client.find(&query()).unwrap())
            })
            .collect();
        for thread in threads {
            thread.join().unwrap();
        }
        let mut at = arrivals.lock().unwrap().clone();
        at.sort();
        assert_eq!(at.len(), 2);
        assert!(at[1] - at[0] >= Duration::from_millis(180));
    }

    #[test]
    fn the_user_agent_names_prism() {
        let fake = FakeLrclib::start(|_| reply(404, "{}"));
        fake.client().find(&query()).unwrap();
        let agents = fake.agents.lock().unwrap().clone();
        let want = format!(
            "Prism/{} ( https://github.com/wl-lankin/prism )",
            env!("CARGO_PKG_VERSION")
        );
        assert!(!agents.is_empty() && agents.iter().all(|a| *a == want));
    }

    /// Prints only counts, never lyrics. Run by name: `cargo test live_lrclib -- --ignored`.
    #[test]
    #[ignore = "reaches lrclib.net"]
    fn live_lrclib_smoke() {
        if std::env::var("PRISM_LIVE_LYRICS").is_ok_and(|v| v == "1") {
            LIVE.store(true, Ordering::SeqCst);
        } else {
            println!("set PRISM_LIVE_LYRICS=1 to reach lrclib.net");
            return;
        }
        let client = Lrclib::new(DEFAULT_BASE).unwrap();
        let found = client
            .find(&Query {
                artist: "Queen".into(),
                title: "Bohemian Rhapsody".into(),
                album: None,
                duration_ms: 354_000,
            })
            .unwrap();
        println!(
            "synced lines: {}, plain lines: {}, instrumental: {}",
            found.synced_lrc.as_deref().map_or(0, |t| t.lines().count()),
            found.plain.as_deref().map_or(0, |t| t.lines().count()),
            found.instrumental
        );
    }
}
