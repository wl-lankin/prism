//! Live radio streams: an endless HTTP body in a fixed in-memory ring.
//!
//! One download thread per stream reads the body, cuts out the ICY metadata blocks (the titles go
//! to a [`TitleSink`], not into the audio) and writes the audio bytes into a ring of fixed size.
//! A full ring blocks the download thread: audio is never dropped and nothing goes to disk, so
//! memory stays constant however long a station plays. One [`LiveReader`] at a time takes the
//! bytes out.
//!
//! Opening resolves a PLS or M3U playlist once (one level) and recognizes an HLS playlist, which
//! is not downloaded here: the caller hands its URL to FFmpeg.
//!
//! Drops: once a stream played, a cut, a stall, a connection error or any bad answer on
//! reconnect (an Icecast mount answers 404 while its source encoder reconnects) is retried with
//! a backoff while the outage (the time since the last bytes came in) stays under `outage`; the
//! reader keeps getting what the ring holds and then waits, so a short drop never shows. After
//! that the reader fails with [`Offline`]. At open, an HTTP error, an HTML page or anything else
//! that is not audio is a plain [`StreamFailure`] at once. Error texts name the station label
//! and the host, never the URL.
//!
//! Shoutcast v1 servers answer `ICY 200 OK`, which the HTTP client rejects; for http URLs such a
//! station is then requested over a plain socket with HTTP/1.0 (`raw_get`).

#[cfg(test)]
pub(crate) mod testserver;

use std::error::Error as StdError;
use std::fmt;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::net::TcpStream;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use anyhow::Context;
use reqwest::blocking::Client;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue, CONTENT_TYPE};
use reqwest::Url;
use symphonia::core::io::MediaSource;

use super::http::{Offline, StreamFailure};
use crate::navidrome::client::{classify_reqwest, NavidromeError};

pub const USER_AGENT: &str = concat!("Prism/", env!("CARGO_PKG_VERSION"));
/// The ring: about 26 s at 320 kbps.
pub const RING_BYTES: usize = 1024 * 1024;
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(8);
pub const STALL_TIMEOUT: Duration = Duration::from_secs(10);
pub const OUTAGE: Duration = Duration::from_secs(20);
pub const BACKOFF_MS: [u64; 5] = [500, 1000, 2000, 4000, 8000];
pub const MAX_REDIRECTS: usize = 5;

pub const OFFLINE_TEXT: &str = "The station is not reachable";
pub const NOT_AUDIO: &str = "This station is not sending audio";
pub const PLAYLIST_NOWHERE: &str = "This station's playlist points nowhere";
const NOT_AN_ADDRESS: &str = "This station's address is not a web address";
const SECURE_FAILED: &str = "The station's secure connection failed";
const TOO_MANY_REDIRECTS: &str = "The station redirects too often";

/// Playlists larger than this are not parsed.
const PLAYLIST_LIMIT: usize = 64 * 1024;
const MAX_METAINT: usize = 1_048_576;
const TITLE_CHARS: usize = 256;
const ICY_NAME_CHARS: usize = 200;
const CHUNK: usize = 16 * 1024;
/// Bytes read at open to tell a playlist from audio when the content type does not say.
const SNIFF_BYTES: usize = 16;
const TCP_KEEPALIVE: Duration = Duration::from_secs(15);

const PLAYLIST_TYPES: [&str; 7] = [
    "audio/x-scpls",
    "audio/scpls",
    "audio/x-mpegurl",
    "audio/mpegurl",
    "application/pls+xml",
    "application/x-mpegurl",
    "application/vnd.apple.mpegurl",
];

/// Receives the stream title: `Some` on every change, from the download thread.
pub type TitleSink = Arc<dyn Fn(Option<String>) + Send + Sync>;

/// How live streams are fetched and buffered.
pub struct LiveConfig {
    /// User agent, connect timeout, `stall` as the per-operation timeout (the wait for the
    /// headers, every body read), no transparent decompression, at most 5 redirects (https to
    /// http allowed: many stations still stream over http and nothing secret is sent), no idle
    /// connections kept.
    pub client: Client,
    pub ring: usize,
    /// No byte for this long on a connection: it is dropped and reconnected.
    pub stall: Duration,
    /// Reconnects stop once no byte came in for this long; the reader then fails offline.
    pub outage: Duration,
    /// The wait before each reconnect attempt; the last one repeats.
    pub backoff: Vec<Duration>,
}

impl LiveConfig {
    pub fn new() -> anyhow::Result<LiveConfig> {
        Ok(LiveConfig {
            client: live_client(CONNECT_TIMEOUT, STALL_TIMEOUT)?,
            ring: RING_BYTES,
            stall: STALL_TIMEOUT,
            outage: OUTAGE,
            backoff: BACKOFF_MS.map(Duration::from_millis).to_vec(),
        })
    }

    #[cfg(test)]
    pub(crate) fn for_tests(
        ring: usize,
        stall: Duration,
        outage: Duration,
        backoff: Vec<Duration>,
    ) -> LiveConfig {
        LiveConfig {
            client: live_client(CONNECT_TIMEOUT.min(stall), stall).expect("the live client"),
            ring,
            stall,
            outage,
            backoff,
        }
    }

    fn backoff(&self, attempt: usize) -> Duration {
        self.backoff
            .get(attempt)
            .or(self.backoff.last())
            .copied()
            .unwrap_or(Duration::from_millis(BACKOFF_MS[0]))
    }
}

fn live_client(connect: Duration, stall: Duration) -> anyhow::Result<Client> {
    Client::builder()
        .user_agent(USER_AGENT)
        .connect_timeout(connect)
        .timeout(stall)
        .tcp_keepalive(TCP_KEEPALIVE)
        .no_gzip()
        .redirect(reqwest::redirect::Policy::limited(MAX_REDIRECTS))
        .pool_max_idle_per_host(0)
        .build()
        .context("building the radio HTTP client")
}

/// A live station to play. `Debug` prints the label only.
#[derive(Clone)]
pub struct LiveInput {
    pub url: String,
    /// The station name, for error texts.
    pub label: String,
    /// "mp3", "aac", "ogg" when the directory knows the codec.
    pub codec_hint: Option<String>,
    pub config: Arc<LiveConfig>,
    pub titles: Option<TitleSink>,
}

impl PartialEq for LiveInput {
    fn eq(&self, other: &Self) -> bool {
        self.url == other.url && self.label == other.label
    }
}

impl Eq for LiveInput {}

impl fmt::Debug for LiveInput {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("LiveInput").field(&self.label).finish()
    }
}

/// Why a connection or a read failed.
#[derive(Clone, Debug)]
enum Failure {
    Offline(String),
    Other(String),
}

impl Failure {
    fn into_anyhow(self) -> anyhow::Error {
        match self {
            Failure::Offline(text) => anyhow::Error::new(Offline(text)),
            Failure::Other(text) => anyhow::Error::new(StreamFailure(text)),
        }
    }

    fn into_io(self) -> io::Error {
        match self {
            // Never UnexpectedEof: a decoder takes that for the end of the stream.
            Failure::Offline(text) => {
                io::Error::new(io::ErrorKind::ConnectionAborted, Offline(text))
            }
            Failure::Other(text) => io::Error::other(StreamFailure(text)),
        }
    }
}

fn offline() -> Failure {
    Failure::Offline(OFFLINE_TEXT.into())
}

fn not_audio() -> Failure {
    Failure::Other(NOT_AUDIO.into())
}

/// A transport error; the request URL never reaches the text.
fn transport(err: reqwest::Error) -> Failure {
    if err.is_redirect() {
        return Failure::Other(TOO_MANY_REDIRECTS.into());
    }
    match classify_reqwest(err) {
        error if error.is_offline() => offline(),
        NavidromeError::Tls(_) => Failure::Other(SECURE_FAILED.into()),
        _ => not_audio(),
    }
}

/// A fixed-size byte ring, allocated once.
struct ByteRing {
    buf: Box<[u8]>,
    head: usize,
    len: usize,
}

impl ByteRing {
    fn new(capacity: usize) -> ByteRing {
        ByteRing {
            buf: vec![0; capacity.max(1)].into_boxed_slice(),
            head: 0,
            len: 0,
        }
    }

    fn free(&self) -> usize {
        self.buf.len() - self.len
    }

    /// Copies as much of `data` as fits; returns how much.
    fn push(&mut self, data: &[u8]) -> usize {
        let cap = self.buf.len();
        let n = data.len().min(self.free());
        let tail = (self.head + self.len) % cap;
        let first = n.min(cap - tail);
        self.buf[tail..tail + first].copy_from_slice(&data[..first]);
        self.buf[..n - first].copy_from_slice(&data[first..n]);
        self.len += n;
        n
    }

    fn pop(&mut self, out: &mut [u8]) -> usize {
        let cap = self.buf.len();
        let n = out.len().min(self.len);
        let first = n.min(cap - self.head);
        out[..first].copy_from_slice(&self.buf[self.head..self.head + first]);
        out[first..n].copy_from_slice(&self.buf[..n - first]);
        self.head = (self.head + n) % cap;
        self.len -= n;
        n
    }
}

struct State {
    ring: ByteRing,
    /// Set once the download gave up; the reader hands it out after the buffered bytes.
    failed: Option<Failure>,
    /// The stream was dropped: the download thread ends, readers fail.
    closed: bool,
    /// The last time bytes came in from the network (or the ring took bytes after being full).
    last_input: Instant,
    reconnects: u64,
    /// The id of the reader that may read; bumped by every new reader and every cancel.
    reader: u64,
    writer_waiting: bool,
    #[cfg(test)]
    peak: usize,
}

struct Shared {
    config: Arc<LiveConfig>,
    url: Url,
    titles: Option<TitleSink>,
    state: Mutex<State>,
    changed: Condvar,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn fail(&self, failure: Failure) {
        let mut state = self.lock();
        state.failed = Some(failure);
        self.changed.notify_all();
    }
}

/// An open live stream. Dropping it stops the download thread and closes the connection.
pub struct LiveStream {
    shared: Arc<Shared>,
    resolved_url: String,
    content_type: Option<String>,
    icy_name: Option<String>,
    hls: bool,
}

/// Stops one reader from another thread: its blocked or next read fails at once. It never
/// touches a reader made after it (a replacement keeps reading).
pub struct Canceller {
    shared: Arc<Shared>,
    reader: u64,
}

impl Canceller {
    pub fn cancel(&self) {
        let mut state = self.shared.lock();
        if state.reader == self.reader {
            state.reader += 1;
            self.shared.changed.notify_all();
        }
    }
}

impl LiveStream {
    /// Connects, resolves a playlist once, checks that the answer is audio and starts the
    /// download thread. An HLS playlist is only recognized ([`Self::is_hls`]).
    pub fn open(input: &LiveInput) -> anyhow::Result<LiveStream> {
        let url = Url::parse(input.url.trim())
            .ok()
            .filter(|url| matches!(url.scheme(), "http" | "https") && url.host_str().is_some());
        let Some(url) = url else {
            return Err(anyhow::Error::new(StreamFailure(NOT_AN_ADDRESS.into()))
                .context(format!("streaming {}", input.label)));
        };
        let context = format!(
            "streaming {} from {}",
            input.label,
            url.host_str().unwrap_or_default()
        );
        let config = &input.config;
        // One playlist level: an entry that is a playlist again fails inside `connect`.
        let answer = match connect(config, &url, true) {
            Ok(Answer::Playlist(entry)) => connect(config, &entry, false),
            other => other,
        };
        let (connection, hls_url, content_type) = match answer {
            Ok(Answer::Audio(connection)) => {
                let kind = connection.content_type.clone();
                (Some(connection), None, kind)
            }
            Ok(Answer::Hls { url, content_type }) => (None, Some(url), content_type),
            Ok(Answer::Playlist(_)) => {
                return Err(Failure::Other(PLAYLIST_NOWHERE.into())
                    .into_anyhow()
                    .context(context))
            }
            Err(failure) => return Err(failure.into_anyhow().context(context)),
        };
        let resolved = connection
            .as_ref()
            .map(|c| c.url.clone())
            .or(hls_url)
            .unwrap_or(url);
        let icy_name = connection.as_ref().and_then(|c| c.icy_name.clone());
        let shared = Arc::new(Shared {
            config: Arc::clone(config),
            url: resolved.clone(),
            titles: input.titles.clone(),
            state: Mutex::new(State {
                ring: ByteRing::new(if connection.is_some() { config.ring } else { 1 }),
                failed: None,
                closed: false,
                last_input: Instant::now(),
                reconnects: 0,
                reader: 0,
                writer_waiting: false,
                #[cfg(test)]
                peak: 0,
            }),
            changed: Condvar::new(),
        });
        let hls = connection.is_none();
        match connection {
            Some(connection) => {
                if let Some(value) = &connection.bad_metaint {
                    eprintln!(
                        "{}: icy-metaint {value:?} is out of range, no titles",
                        input.label
                    );
                }
                let thread_shared = Arc::clone(&shared);
                std::thread::Builder::new()
                    .name("prism-live-stream".into())
                    .spawn(move || download(&thread_shared, connection))
                    .context("starting the stream download")
                    .context(context)?;
            }
            None => shared.fail(Failure::Other(
                "an HLS stream is decoded from its URL".into(),
            )),
        }
        Ok(LiveStream {
            shared,
            resolved_url: resolved.to_string(),
            content_type,
            icy_name,
            hls,
        })
    }

    /// The content type of the audio answer (or of the HLS playlist), without parameters.
    pub fn content_type(&self) -> Option<&str> {
        self.content_type.as_deref()
    }

    /// The station name the stream announces (`icy-name`).
    pub fn icy_name(&self) -> Option<&str> {
        self.icy_name.as_deref()
    }

    /// The answer was an HLS playlist: nothing is downloaded, the caller decodes
    /// [`Self::resolved_url`] with FFmpeg.
    pub fn is_hls(&self) -> bool {
        self.hls
    }

    /// The URL that answered with audio (after a playlist and redirects), or the HLS playlist's.
    pub fn resolved_url(&self) -> &str {
        &self.resolved_url
    }

    /// The reader of the ring. One reader at a time: a new one closes the previous one and goes
    /// on where it stopped.
    pub fn reader(&self) -> LiveReader {
        let mut state = self.shared.lock();
        state.reader += 1;
        let id = state.reader;
        self.shared.changed.notify_all();
        LiveReader {
            shared: Arc::clone(&self.shared),
            id,
            pos: 0,
        }
    }

    /// How often the download reconnected; a change tells a decoder that a joint passed.
    pub fn reconnected(&self) -> u64 {
        self.shared.lock().reconnects
    }

    /// A canceller for the reader that is current now (see [`LiveReader::canceller`]).
    pub fn canceller(&self) -> Canceller {
        Canceller {
            shared: Arc::clone(&self.shared),
            reader: self.shared.lock().reader,
        }
    }

    #[cfg(test)]
    fn peak_buffered(&self) -> usize {
        self.shared.lock().peak
    }
}

impl Drop for LiveStream {
    fn drop(&mut self) {
        let mut state = self.shared.lock();
        state.closed = true;
        self.shared.changed.notify_all();
    }
}

/// Reads the ring of a [`LiveStream`]. Never ends: it waits for bytes while the download
/// reconnects and fails once the station is gone. Not seekable.
pub struct LiveReader {
    shared: Arc<Shared>,
    id: u64,
    pos: u64,
}

impl LiveReader {
    /// A handle that cancels this reader, and only this one.
    pub fn canceller(&self) -> Canceller {
        Canceller {
            shared: Arc::clone(&self.shared),
            reader: self.id,
        }
    }

    fn read_ring(&mut self, buf: &mut [u8]) -> Result<usize, Failure> {
        let shared = &self.shared;
        let config = &shared.config;
        // A backstop for a download thread that hangs; it normally fails at `outage`.
        let backstop = config.outage + config.stall + CONNECT_TIMEOUT;
        let mut state = shared.lock();
        loop {
            if state.closed {
                return Err(Failure::Other("the station stream was closed".into()));
            }
            if state.reader != self.id {
                return Err(Failure::Other("the station reader was closed".into()));
            }
            if state.ring.len > 0 {
                let n = state.ring.pop(buf);
                if state.writer_waiting {
                    state.last_input = Instant::now();
                }
                shared.changed.notify_all();
                return Ok(n);
            }
            if let Some(failure) = &state.failed {
                return Err(failure.clone());
            }
            let waited = state.last_input.elapsed();
            if waited >= backstop {
                return Err(offline());
            }
            state = shared
                .changed
                .wait_timeout(state, config.stall.min(backstop - waited))
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
    }
}

impl Read for LiveReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        let n = self.read_ring(buf).map_err(Failure::into_io)?;
        self.pos += n as u64;
        Ok(n)
    }
}

impl Seek for LiveReader {
    /// Only to the current position (`stream_position`).
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        let target = match pos {
            SeekFrom::Start(at) => Some(at),
            SeekFrom::Current(delta) => self.pos.checked_add_signed(delta),
            SeekFrom::End(_) => None,
        };
        if target == Some(self.pos) {
            Ok(self.pos)
        } else {
            Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "a live stream cannot seek",
            ))
        }
    }
}

impl MediaSource for LiveReader {
    fn is_seekable(&self) -> bool {
        false
    }

    fn byte_len(&self) -> Option<u64> {
        None
    }
}

/// An answer's status line and headers, with its body (a reqwest response or a raw socket).
struct Reply {
    status: u16,
    headers: HeaderMap,
    url: Url,
    body: Box<dyn Read + Send>,
}

/// An audio response, ready for the download thread.
struct Connection {
    body: Box<dyn Read + Send>,
    /// Body bytes read at open (before any demux).
    head: Vec<u8>,
    url: Url,
    content_type: Option<String>,
    metaint: Option<usize>,
    /// An `icy-metaint` header that is not a usable interval.
    bad_metaint: Option<String>,
    icy_name: Option<String>,
}

enum Answer {
    Audio(Box<Connection>),
    Playlist(Url),
    Hls {
        url: Url,
        content_type: Option<String>,
    },
}

fn header<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers.get(name).and_then(|value| value.to_str().ok())
}

/// The content type without parameters, lowercased.
fn essence(headers: &HeaderMap) -> Option<String> {
    let value = header(headers, CONTENT_TYPE.as_str())?;
    let essence = value.split(';').next().unwrap_or_default().trim();
    (!essence.is_empty()).then(|| essence.to_ascii_lowercase())
}

/// A text, JSON, XML or image answer is not audio (playlist types are handled before).
fn is_not_audio_type(essence: &str) -> bool {
    essence.starts_with("text/")
        || essence.starts_with("image/")
        || essence.ends_with("json")
        || essence.ends_with("xml")
}

fn looks_like_playlist(head: &[u8]) -> bool {
    let text = head.strip_prefix(b"\xef\xbb\xbf").unwrap_or(head);
    let start = text
        .iter()
        .position(|b| !b.is_ascii_whitespace())
        .unwrap_or(text.len());
    let text = &text[start..];
    text.starts_with(b"#EXTM3U")
        || text.len() >= 10 && text[..10].eq_ignore_ascii_case(b"[playlist]")
}

/// Reads until `out` holds at least `want` bytes, the body ends, or `cap` is reached.
fn read_into(
    body: &mut dyn Read,
    out: &mut Vec<u8>,
    want: usize,
    cap: usize,
) -> Result<(), Failure> {
    let mut buf = vec![0u8; CHUNK];
    while out.len() < want && out.len() < cap {
        let room = (cap - out.len()).min(CHUNK);
        match body.read(&mut buf[..room]) {
            Ok(0) => break,
            Ok(n) => out.extend_from_slice(&buf[..n]),
            Err(err) if err.kind() == io::ErrorKind::Interrupted => {}
            Err(_) => return Err(offline()),
        }
    }
    Ok(())
}

/// True when the client failed on the status line of an answer that is not HTTP (Shoutcast's
/// `ICY 200 OK`), not on the network.
fn speaks_icy(err: &reqwest::Error) -> bool {
    if err.is_connect() || err.is_timeout() || err.is_redirect() {
        return false;
    }
    let mut current: Option<&(dyn StdError + 'static)> = Some(err);
    while let Some(inner) = current {
        if inner.to_string().contains("invalid HTTP version") {
            return true;
        }
        current = inner.source();
    }
    false
}

/// GETs `url` with `Icy-MetaData: 1`, through the client, or over a plain socket for an http
/// station that does not speak HTTP.
fn fetch(config: &LiveConfig, url: &Url) -> Result<Reply, Failure> {
    let sent = config
        .client
        .get(url.clone())
        .header("Icy-MetaData", "1")
        .send();
    match sent {
        Ok(response) => Ok(Reply {
            status: response.status().as_u16(),
            headers: response.headers().clone(),
            url: response.url().clone(),
            body: Box::new(response),
        }),
        Err(err) if speaks_icy(&err) => {
            // After a redirect the error names the URL that answered.
            match err.url().filter(|target| target.scheme() == "http") {
                Some(target) => raw_get(config, &target.clone()),
                None => Err(transport(err)),
            }
        }
        Err(err) => Err(transport(err)),
    }
}

/// The largest response head a raw request reads.
const RAW_HEAD_LIMIT: usize = 16 * 1024;

/// A plain HTTP/1.0 GET over a socket, for Shoutcast v1 servers: `ICY 200` or `HTTP/1.x` is
/// accepted, headers are kept as bytes (Latin-1 survives), redirects are not followed.
fn raw_get(config: &LiveConfig, url: &Url) -> Result<Reply, Failure> {
    let addrs = url.socket_addrs(|| Some(80)).map_err(|_| offline())?;
    let connect = CONNECT_TIMEOUT.min(config.stall);
    let mut stream = addrs
        .iter()
        .find_map(|addr| TcpStream::connect_timeout(addr, connect).ok())
        .ok_or_else(offline)?;
    stream
        .set_read_timeout(Some(config.stall))
        .and_then(|()| stream.set_write_timeout(Some(config.stall)))
        .map_err(|_| offline())?;
    let mut host = url.host_str().unwrap_or_default().to_owned();
    if let Some(port) = url.port() {
        host.push_str(&format!(":{port}"));
    }
    let mut target = url.path().to_owned();
    if let Some(query) = url.query() {
        target.push('?');
        target.push_str(query);
    }
    let request = format!(
        "GET {target} HTTP/1.0\r\nHost: {host}\r\nUser-Agent: {USER_AGENT}\r\nAccept: */*\r\nIcy-MetaData: 1\r\nConnection: close\r\n\r\n"
    );
    stream
        .write_all(request.as_bytes())
        .map_err(|_| offline())?;
    let mut head = Vec::new();
    let mut buf = [0u8; 4096];
    let (head_len, body_start) = loop {
        if let Some(end) = head_end(&head) {
            break end;
        }
        if head.len() >= RAW_HEAD_LIMIT {
            return Err(not_audio());
        }
        let room = (RAW_HEAD_LIMIT - head.len()).min(buf.len());
        match stream.read(&mut buf[..room]) {
            Ok(0) => return Err(offline()),
            Ok(n) => head.extend_from_slice(&buf[..n]),
            Err(err) if err.kind() == io::ErrorKind::Interrupted => {}
            Err(_) => return Err(offline()),
        }
    };
    let (status, headers) = parse_raw_head(&head[..head_len]).ok_or_else(not_audio)?;
    let rest = head[body_start..].to_vec();
    Ok(Reply {
        status,
        headers,
        url: url.clone(),
        body: Box::new(io::Cursor::new(rest).chain(stream)),
    })
}

/// Where a response head ends: its length and where the body starts (`\r\n\r\n` or `\n\n`).
fn head_end(bytes: &[u8]) -> Option<(usize, usize)> {
    let crlf = find(bytes, b"\r\n\r\n").map(|at| (at, at + 4));
    let lf = find(bytes, b"\n\n").map(|at| (at, at + 2));
    match (crlf, lf) {
        (Some(a), Some(b)) => Some(if a.0 <= b.0 { a } else { b }),
        (a, b) => a.or(b),
    }
}

/// The status of `ICY <code>` or `HTTP/1.x <code>` and the headers that parse.
fn parse_raw_head(head: &[u8]) -> Option<(u16, HeaderMap)> {
    let mut lines = head
        .split(|&b| b == b'\n')
        .map(|line| line.strip_suffix(b"\r").unwrap_or(line));
    let status_line = std::str::from_utf8(lines.next()?).ok()?;
    let mut parts = status_line.split_whitespace();
    let protocol = parts.next()?;
    if protocol != "ICY" && !protocol.starts_with("HTTP/1.") {
        return None;
    }
    let code = parts.next()?;
    if code.len() != 3 {
        return None;
    }
    let status: u16 = code.parse().ok()?;
    let mut headers = HeaderMap::new();
    for line in lines {
        let Some(colon) = line.iter().position(|&b| b == b':') else {
            continue;
        };
        let name = HeaderName::from_bytes(line[..colon].trim_ascii());
        let value = HeaderValue::from_bytes(line[colon + 1..].trim_ascii());
        if let (Ok(name), Ok(value)) = (name, value) {
            headers.append(name, value);
        }
    }
    Some((status, headers))
}

/// Requests `url` and tells what answered. `playlist`: a PLS or M3U answer is parsed for its
/// entry (else it is an error).
fn connect(config: &LiveConfig, url: &Url, playlist: bool) -> Result<Answer, Failure> {
    let Reply {
        status,
        headers,
        url: final_url,
        body: mut response,
    } = fetch(config, url)?;
    if !(200..300).contains(&status) {
        return Err(if (502..=504).contains(&status) {
            offline()
        } else if (300..400).contains(&status) {
            not_audio()
        } else {
            Failure::Other(format!("The station answered {status}"))
        });
    }
    let headers = &headers;
    let content_type = essence(headers);
    let raw_metaint = header(headers, "icy-metaint").map(str::to_owned);
    let metaint = raw_metaint.as_deref().and_then(parse_metaint);
    let bad_metaint = raw_metaint.filter(|_| metaint.is_none());
    let icy_name = headers
        .get("icy-name")
        .and_then(|value| clean_icy_name(&decode_text(value.as_bytes())));
    let kind = content_type.as_deref().unwrap_or_default();
    let typed_playlist = PLAYLIST_TYPES.contains(&kind);
    if kind == "text/html" || !typed_playlist && kind != "text/plain" && is_not_audio_type(kind) {
        return Err(not_audio());
    }
    let sniff = typed_playlist
        || kind == "text/plain"
        || (metaint.is_none() && matches!(kind, "" | "application/octet-stream"));
    let mut head = Vec::new();
    read_into(
        &mut response,
        &mut head,
        if sniff { SNIFF_BYTES } else { 1 },
        CHUNK,
    )?;
    if head.is_empty() {
        return Err(not_audio());
    }
    let is_list = typed_playlist || sniff && looks_like_playlist(&head);
    if is_list || kind == "text/plain" {
        read_into(
            &mut response,
            &mut head,
            PLAYLIST_LIMIT + 1,
            PLAYLIST_LIMIT + 1,
        )?;
        drop(response);
        let text = String::from_utf8_lossy(&head);
        if text.contains("#EXT-X-") {
            return Ok(Answer::Hls {
                url: final_url,
                content_type,
            });
        }
        if !playlist {
            return Err(Failure::Other(PLAYLIST_NOWHERE.into()));
        }
        let entry = first_entry(&text).and_then(|entry| Url::parse(&entry).ok());
        return match entry {
            Some(entry) => Ok(Answer::Playlist(entry)),
            None if is_list => Err(Failure::Other(PLAYLIST_NOWHERE.into())),
            None => Err(not_audio()),
        };
    }
    Ok(Answer::Audio(Box::new(Connection {
        body: response,
        head,
        url: final_url,
        content_type,
        metaint,
        bad_metaint,
        icy_name,
    })))
}

fn clean_icy_name(raw: &str) -> Option<String> {
    let name: String = raw
        .chars()
        .filter(|c| !c.is_control())
        .take(ICY_NAME_CHARS)
        .collect();
    let name = name.trim();
    (!name.is_empty()).then(|| name.to_owned())
}

/// Reports a title when it changed; outside the state lock.
struct Titles {
    sink: Option<TitleSink>,
    last: Option<String>,
}

impl Titles {
    fn report(&mut self, title: String) {
        if self.last.as_deref() == Some(title.as_str()) {
            return;
        }
        if let Some(sink) = &self.sink {
            sink(Some(title.clone()));
        }
        self.last = Some(title);
    }
}

enum Pump {
    Closed,
    /// A cut, a read timeout (stall) or a broken body: all worth a reconnect.
    Dropped,
}

/// The download thread: pumps connections into the ring and reconnects on drops.
fn download(shared: &Shared, mut connection: Box<Connection>) {
    let mut titles = Titles {
        sink: shared.titles.clone(),
        last: None,
    };
    loop {
        if let Pump::Closed = pump(shared, connection, &mut titles) {
            return;
        }
        match reconnect(shared) {
            Some(next) => connection = next,
            None => return,
        }
    }
}

/// Copies one connection's body into the ring until it fails or the stream is closed. The
/// connection is closed when this returns.
fn pump(shared: &Shared, mut connection: Box<Connection>, titles: &mut Titles) -> Pump {
    let mut demux = IcyDemux::new(connection.metaint);
    let mut audio = Vec::with_capacity(CHUNK);
    let mut found = Vec::new();
    let head = std::mem::take(&mut connection.head);
    if !deliver(shared, &mut demux, &head, &mut audio, &mut found, titles) {
        return Pump::Closed;
    }
    let mut buf = vec![0u8; CHUNK];
    loop {
        let n = match connection.body.read(&mut buf) {
            Ok(0) => return Pump::Dropped,
            Ok(n) => n,
            Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
            Err(_) => return Pump::Dropped,
        };
        if !deliver(
            shared,
            &mut demux,
            &buf[..n],
            &mut audio,
            &mut found,
            titles,
        ) {
            return Pump::Closed;
        }
    }
}

/// Demuxes `data` and pushes its audio into the ring, waiting while the ring is full. False once
/// the stream is closed.
fn deliver(
    shared: &Shared,
    demux: &mut IcyDemux,
    data: &[u8],
    audio: &mut Vec<u8>,
    found: &mut Vec<String>,
    titles: &mut Titles,
) -> bool {
    audio.clear();
    found.clear();
    demux.feed(data, audio, found);
    for title in found.drain(..) {
        titles.report(title);
    }
    let mut rest = &audio[..];
    let mut state = shared.lock();
    state.last_input = Instant::now();
    loop {
        if state.closed {
            return false;
        }
        if rest.is_empty() {
            return true;
        }
        let n = state.ring.push(rest);
        if n > 0 {
            rest = &rest[n..];
            #[cfg(test)]
            {
                state.peak = state.peak.max(state.ring.len);
            }
            shared.changed.notify_all();
            continue;
        }
        state.writer_waiting = true;
        state = shared
            .changed
            .wait(state)
            .unwrap_or_else(|e| e.into_inner());
        state.writer_waiting = false;
        state.last_input = Instant::now();
    }
}

/// Reconnects with the backoff while the outage lasts. `None` when the stream was closed or the
/// station stays away (the offline failure is recorded for the reader). Every failed attempt
/// counts the same, a plain one too: a station that played is only away for a moment.
fn reconnect(shared: &Shared) -> Option<Box<Connection>> {
    let config = &shared.config;
    let mut attempt = 0;
    loop {
        let backoff = config.backoff(attempt);
        {
            let mut state = shared.lock();
            if state.closed {
                return None;
            }
            if state.last_input.elapsed() + backoff >= config.outage {
                state.failed = Some(offline());
                shared.changed.notify_all();
                return None;
            }
            let until = Instant::now() + backoff;
            loop {
                if state.closed {
                    return None;
                }
                let now = Instant::now();
                if now >= until {
                    break;
                }
                state = shared
                    .changed
                    .wait_timeout(state, until - now)
                    .unwrap_or_else(|e| e.into_inner())
                    .0;
            }
        }
        match connect(config, &shared.url, false) {
            Ok(Answer::Audio(connection)) => {
                let mut state = shared.lock();
                if state.closed {
                    return None;
                }
                state.reconnects += 1;
                return Some(connection);
            }
            // An Icecast mount answers 404 while its source encoder reconnects; a proxy may
            // answer 500 or an empty 200 meanwhile.
            Ok(Answer::Hls { .. } | Answer::Playlist(_)) | Err(_) => attempt += 1,
        }
    }
}

/// `icy-metaint` as a usable interval (1 to 1 MiB); anything else turns the demux off.
fn parse_metaint(value: &str) -> Option<usize> {
    value
        .trim()
        .parse::<usize>()
        .ok()
        .filter(|n| (1..=MAX_METAINT).contains(n))
}

enum Demux {
    Audio { left: usize },
    Length,
    Meta { left: usize },
}

/// Cuts the ICY metadata blocks out of a body: every `metaint` audio bytes, one length byte
/// (times 16) and that many metadata bytes. Its buffer never holds more than one block (4080
/// bytes).
struct IcyDemux {
    metaint: usize,
    state: Demux,
    block: Vec<u8>,
}

impl IcyDemux {
    fn new(metaint: Option<usize>) -> IcyDemux {
        let metaint = metaint.unwrap_or(0);
        IcyDemux {
            metaint,
            state: Demux::Audio { left: metaint },
            block: Vec::new(),
        }
    }

    fn enabled(&self) -> bool {
        self.metaint > 0
    }

    fn feed(&mut self, mut data: &[u8], audio: &mut Vec<u8>, titles: &mut Vec<String>) {
        if !self.enabled() {
            audio.extend_from_slice(data);
            return;
        }
        while !data.is_empty() {
            match &mut self.state {
                Demux::Audio { left } => {
                    let take = (*left).min(data.len());
                    audio.extend_from_slice(&data[..take]);
                    data = &data[take..];
                    *left -= take;
                    if *left == 0 {
                        self.state = Demux::Length;
                    }
                }
                Demux::Length => {
                    let len = data[0] as usize * 16;
                    data = &data[1..];
                    self.state = if len == 0 {
                        Demux::Audio { left: self.metaint }
                    } else {
                        self.block.clear();
                        Demux::Meta { left: len }
                    };
                }
                Demux::Meta { left } => {
                    let take = (*left).min(data.len());
                    self.block.extend_from_slice(&data[..take]);
                    data = &data[take..];
                    *left -= take;
                    if *left == 0 {
                        titles.extend(stream_title(&self.block));
                        self.block.clear();
                        self.state = Demux::Audio { left: self.metaint };
                    }
                }
            }
        }
    }
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

/// `StreamTitle='...';` of a metadata block: UTF-8, else Latin-1, control characters removed,
/// at most 256 characters. An empty title is `None`.
fn stream_title(block: &[u8]) -> Option<String> {
    const KEY: &[u8] = b"StreamTitle='";
    let rest = &block[find(block, KEY)? + KEY.len()..];
    let end = find(rest, b"';").or_else(|| {
        let padded = rest
            .iter()
            .rposition(|&b| b != 0)
            .map_or(0, |last| last + 1);
        rest[..padded].iter().rposition(|&b| b == b'\'')
    })?;
    let text = decode_text(&rest[..end]);
    let clean: String = text.chars().filter(|c| !c.is_control()).collect();
    let title: String = clean.trim().chars().take(TITLE_CHARS).collect();
    let title = title.trim_end();
    (!title.is_empty()).then(|| title.to_owned())
}

/// UTF-8, else Latin-1 (every byte one char), as ICY servers send either.
fn decode_text(raw: &[u8]) -> String {
    match std::str::from_utf8(raw) {
        Ok(text) => text.to_owned(),
        Err(_) => raw.iter().map(|&b| char::from(b)).collect(),
    }
}

fn is_web_url(value: &str) -> bool {
    Url::parse(value).is_ok_and(|url| matches!(url.scheme(), "http" | "https"))
}

/// The first http(s) entry of a PLS (`FileN=`) or M3U (first non-comment line) playlist.
/// Bodies over 64 KiB are not parsed.
pub(crate) fn first_entry(body: &str) -> Option<String> {
    if body.len() > PLAYLIST_LIMIT {
        return None;
    }
    for line in body.lines() {
        let line = line.trim_start_matches('\u{feff}').trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with('[') {
            continue;
        }
        let value = match line.split_once('=') {
            Some((key, value)) if is_pls_file_key(key) => value.trim(),
            Some(_) if !line.contains("://") => continue,
            _ => line,
        };
        if is_web_url(value) {
            return Some(value.to_owned());
        }
    }
    None
}

fn is_pls_file_key(key: &str) -> bool {
    let key = key.trim();
    key.len() > 4
        && key.get(..4).is_some_and(|k| k.eq_ignore_ascii_case("file"))
        && key[4..].bytes().all(|b| b.is_ascii_digit())
}

#[cfg(test)]
mod tests;
