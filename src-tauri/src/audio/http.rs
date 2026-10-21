//! A progressive, range-capable download buffer for HTTP streams (Navidrome).
//!
//! One download thread per stream writes the body in 64 KiB chunks into a spill file and records
//! what it holds in a [`RangeSet`]; readers ([`HttpStream`] and its cursors) read from that file
//! and wait on a condvar for bytes that are not there yet, up to the stall timeout.
//!
//! Seeking: a byte-seekable stream (206, or 200 with `Content-Length` and `Accept-Ranges: bytes`)
//! restarts the download with `Range: bytes=<pos>-` when a read lands far ahead of, or before,
//! the run being downloaded. A stream without range support (or a server that answers a range
//! request with a plain 200, as some reverse proxies do) is downloaded from the start and a
//! seek ahead waits until the download gets there (restart and skip).
//!
//! Errors: every reqwest error goes through [`classify_reqwest`] / [`classify_io`], which strip
//! the URL and redact anything URL-like; the stream's token and salt are scrubbed once more.
//! A connection that cannot be made, drops, stalls, or a proxy 502/503/504 is [`Offline`]; any
//! other HTTP status or a non-audio answer is a plain error.

use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime};

use reqwest::blocking::{Client, Response};
use reqwest::header::{HeaderMap, ACCEPT_RANGES, CONTENT_RANGE, CONTENT_TYPE, RANGE};
use reqwest::{StatusCode, Url};
use symphonia::core::io::MediaSource;

use super::source::HttpInput;
use crate::navidrome::client::{classify_io, classify_reqwest, NavidromeError};

/// How long a reader waits for bytes (or `open` for the response headers) while the download
/// makes no progress, before the stream counts as offline. A buffering proxy that is slow to
/// the first byte stays within it.
pub const STALL_TIMEOUT: Duration = Duration::from_secs(20);
/// How often a cut download is resumed (byte-seekable streams), with a backoff of 1, 2, 4 s.
pub const RETRIES: u32 = 3;
/// A read this far past the downloaded run restarts the download there instead of waiting.
pub const RESTART_GAP: u64 = 1024 * 1024;
/// The backoff before the first resume; it doubles per attempt.
pub const RETRY_BACKOFF: Duration = Duration::from_secs(1);

const CHUNK: usize = 64 * 1024;
const SPILL_EXTENSION: &str = "part";
const SPILL_MAX_AGE: Duration = Duration::from_secs(24 * 3600);

/// How streams are fetched and buffered; shared by every [`HttpInput`] of one source.
pub struct StreamConfig {
    /// The stream client (`SubsonicClient::stream_http`): connect timeout, a per-read timeout
    /// above `stall` (no bound on the whole response), no transparent decompression, same-host
    /// redirects only.
    pub client: Client,
    /// Where the `.part` spill files go (`<cache>/streams`).
    pub spill_dir: PathBuf,
    pub stall: Duration,
    pub retries: u32,
    /// The first resume's backoff; it doubles per attempt ([`RETRY_BACKOFF`]: 1, 2, 4 s).
    pub backoff: Duration,
}

impl StreamConfig {
    /// A config with the default stall timeout, retries and backoff.
    pub fn new(client: Client, spill_dir: PathBuf) -> StreamConfig {
        StreamConfig {
            client,
            spill_dir,
            stall: STALL_TIMEOUT,
            retries: RETRIES,
            backoff: RETRY_BACKOFF,
        }
    }
}

/// The server cannot be reached right now (connection refused, dropped or stalled, a proxy
/// 502/503/504). The engine pauses on such an error instead of skipping the track.
#[derive(Debug)]
pub struct Offline(pub String);

impl fmt::Display for Offline {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Offline {}

/// True when any error in the chain is [`Offline`], also inside an `io::Error` (a decoder
/// reports a failed read that way).
pub fn is_offline(err: &anyhow::Error) -> bool {
    err.chain().any(|inner| {
        inner.is::<Offline>()
            || inner
                .downcast_ref::<io::Error>()
                .and_then(io::Error::get_ref)
                .is_some_and(|wrapped| wrapped.is::<Offline>())
    })
}

/// The `<spill_dir>` under an app cache dir.
pub fn spill_dir(cache: &Path) -> PathBuf {
    cache.join("streams")
}

/// Removes `.part` files older than one day (left behind by a crash); errors are ignored.
pub fn cleanup_spill_dir(dir: &Path) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some(SPILL_EXTENSION) {
            continue;
        }
        let old = entry
            .metadata()
            .and_then(|meta| meta.modified())
            .ok()
            .and_then(|modified| SystemTime::now().duration_since(modified).ok())
            .is_some_and(|age| age > SPILL_MAX_AGE);
        if old {
            let _ = fs::remove_file(&path);
        }
    }
}

/// Sorted, disjoint, non-adjacent half-open byte runs `[start, end)`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct RangeSet {
    runs: Vec<(u64, u64)>,
}

impl RangeSet {
    pub fn insert(&mut self, start: u64, end: u64) {
        if start >= end {
            return;
        }
        let (mut start, mut end) = (start, end);
        let mut runs = Vec::with_capacity(self.runs.len() + 1);
        let mut placed = false;
        for &(s, e) in &self.runs {
            if e < start {
                runs.push((s, e));
            } else if end < s {
                if !placed {
                    runs.push((start, end));
                    placed = true;
                }
                runs.push((s, e));
            } else {
                start = start.min(s);
                end = end.max(e);
            }
        }
        if !placed {
            runs.push((start, end));
        }
        self.runs = runs;
    }

    /// True when every byte of `[start, end)` is held (an empty range always is).
    pub fn covers(&self, start: u64, end: u64) -> bool {
        start >= end || self.runs.iter().any(|&(s, e)| s <= start && end <= e)
    }

    /// The end of the run holding the byte at `from`.
    pub fn run_end(&self, from: u64) -> Option<u64> {
        self.runs
            .iter()
            .find(|&&(s, e)| s <= from && from < e)
            .map(|&(_, e)| e)
    }
}

/// Why a download (or a read) failed.
#[derive(Clone, Debug)]
enum Failure {
    Offline(String),
    Other(String),
}

impl Failure {
    fn into_anyhow(self) -> anyhow::Error {
        match self {
            Failure::Offline(text) => anyhow::Error::new(Offline(text)),
            Failure::Other(text) => anyhow::Error::msg(text),
        }
    }

    fn into_io(self) -> io::Error {
        match self {
            // Never UnexpectedEof: a decoder takes that for the end of the stream.
            Failure::Offline(text) => {
                io::Error::new(io::ErrorKind::ConnectionAborted, Offline(text))
            }
            Failure::Other(text) => io::Error::other(text),
        }
    }
}

#[derive(Clone, Debug)]
enum Status {
    /// Waiting for the response headers.
    Connecting,
    Running,
    /// The body was read to its end.
    Done,
    Failed(Failure),
}

/// The current download: it writes `[start, pos)`.
struct Download {
    start: u64,
    pos: u64,
    status: Status,
    /// A response of this generation arrived. Stays set when the body fails right after, so
    /// `open` still succeeds and the bytes that came are read before the error.
    answered: bool,
}

struct State {
    /// The spill file; `None` once the last reader is gone.
    file: Option<File>,
    readers: usize,
    ranges: RangeSet,
    len: Option<u64>,
    seekable: bool,
    /// Bumped by every (re)start; a download thread only writes while its generation is current.
    generation: u64,
    download: Download,
    /// Last time the download started or delivered bytes.
    progress: Instant,
}

struct Shared {
    client: Client,
    url: String,
    /// The URL's token and salt, scrubbed from every error text.
    secrets: Vec<String>,
    stall: Duration,
    retries: u32,
    backoff: Duration,
    path: PathBuf,
    state: Mutex<State>,
    changed: Condvar,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn failure(&self, err: NavidromeError) -> Failure {
        let mut text = err.to_string();
        for secret in &self.secrets {
            text = text.replace(secret.as_str(), "***");
        }
        if err.is_offline() {
            Failure::Offline(text)
        } else {
            Failure::Other(text)
        }
    }

    /// Requests the body from `from` (a `Range` when `from > 0`). Returns the response and the
    /// offset its body starts at (0 when the server ignored the range).
    fn connect(&self, from: u64) -> Result<(Response, Answer), Failure> {
        let mut request = self.client.get(&self.url);
        if from > 0 {
            request = request.header(RANGE, format!("bytes={from}-"));
        }
        let response = request
            .send()
            .map_err(|err| self.failure(classify_reqwest(err)))?;
        let status = response.status();
        let headers = response.headers();
        if !status.is_success() {
            let code = status.as_u16();
            return Err(if (502..=504).contains(&code) {
                Failure::Offline(format!("the server is not reachable (HTTP {code})"))
            } else if status == StatusCode::RANGE_NOT_SATISFIABLE {
                Failure::Other("the server refused the range (HTTP 416)".into())
            } else {
                Failure::Other(format!("the server answered HTTP {code}"))
            });
        }
        if let Some(kind) = not_audio(headers) {
            return Err(Failure::Other(format!(
                "the server answered {kind} instead of audio"
            )));
        }
        let length = response.content_length();
        let answer = if status == StatusCode::PARTIAL_CONTENT {
            let (start, total) = content_range(headers).unwrap_or((from, None));
            Answer {
                start,
                len: total.or(length.map(|l| start + l)),
                seekable: true,
            }
        } else {
            let ranges = headers
                .get(ACCEPT_RANGES)
                .and_then(|v| v.to_str().ok())
                .is_some_and(|v| v.trim().eq_ignore_ascii_case("bytes"));
            Answer {
                start: 0,
                len: length,
                seekable: from == 0 && ranges && length.is_some(),
            }
        };
        Ok((response, answer))
    }
}

impl Drop for Shared {
    fn drop(&mut self) {
        let state = self.state.get_mut().unwrap_or_else(|e| e.into_inner());
        if state.file.take().is_some() {
            let _ = fs::remove_file(&self.path);
        }
    }
}

/// What a response's headers say about its body.
struct Answer {
    start: u64,
    len: Option<u64>,
    seekable: bool,
}

/// A JSON, XML or HTML content type: a Subsonic error or a proxy page, not audio.
fn not_audio(headers: &HeaderMap) -> Option<String> {
    let kind = headers
        .get(CONTENT_TYPE)?
        .to_str()
        .ok()?
        .to_ascii_lowercase();
    let essence = kind.split(';').next().unwrap_or_default().trim().to_owned();
    let text = essence.ends_with("json") || essence.ends_with("xml") || essence == "text/html";
    text.then_some(essence)
}

/// `Content-Range: bytes <start>-<end>/<total|*>`.
fn content_range(headers: &HeaderMap) -> Option<(u64, Option<u64>)> {
    let value = headers.get(CONTENT_RANGE)?.to_str().ok()?;
    let (span, total) = value.trim().strip_prefix("bytes ")?.split_once('/')?;
    let start = span.split_once('-')?.0.trim().parse().ok()?;
    Some((start, total.trim().parse().ok()))
}

/// The token and salt values in a stream URL.
fn url_secrets(url: &str) -> Vec<String> {
    Url::parse(url)
        .map(|url| {
            url.query_pairs()
                .filter(|(key, value)| matches!(key.as_ref(), "t" | "s" | "p") && !value.is_empty())
                .map(|(_, value)| value.into_owned())
                .collect()
        })
        .unwrap_or_default()
}

/// Starts a new download generation at `at`; the previous one stops at its next chunk.
fn start_download(shared: &Arc<Shared>, state: &mut State, at: u64) {
    state.generation += 1;
    state.download = Download {
        start: at,
        pos: at,
        status: Status::Connecting,
        answered: false,
    };
    state.progress = Instant::now();
    let generation = state.generation;
    let thread_shared = Arc::clone(shared);
    let spawned = std::thread::Builder::new()
        .name("prism-http-stream".into())
        .spawn(move || download(&thread_shared, generation, at));
    if spawned.is_err() {
        state.download.status =
            Status::Failed(Failure::Other("could not start the stream download".into()));
    }
}

/// The download thread of one generation. Holds no lock while it waits for the network.
fn download(shared: &Shared, generation: u64, from: u64) {
    let mut pos = from;
    let mut attempts = 0;
    // Set once the first response of this generation arrived: later connects are resumes.
    let mut resuming = false;
    loop {
        let failure = match shared.connect(pos) {
            Ok((response, answer)) => {
                {
                    let mut state = shared.lock();
                    if state.generation != generation || state.file.is_none() {
                        return;
                    }
                    if !resuming {
                        if from == 0 {
                            state.seekable = answer.seekable;
                        }
                        resuming = true;
                    }
                    if answer.len.is_some() {
                        state.len = answer.len;
                    }
                    if pos > 0 && answer.start == 0 {
                        // The server ignored the range: no seeking any more, read from the start.
                        state.seekable = false;
                        state.download.start = 0;
                    }
                    pos = answer.start;
                    state.download.pos = pos;
                    state.download.status = Status::Running;
                    state.download.answered = true;
                    state.progress = Instant::now();
                    shared.changed.notify_all();
                }
                match copy_body(shared, generation, response, &mut pos, &mut attempts) {
                    BodyEnd::Finished | BodyEnd::Stopped => return,
                    BodyEnd::Failed(failure) => failure,
                }
            }
            Err(failure) => failure,
        };
        let mut state = shared.lock();
        if state.generation != generation || state.file.is_none() {
            return;
        }
        let retry = resuming
            && state.seekable
            && attempts < shared.retries
            && matches!(failure, Failure::Offline(_));
        if !retry {
            state.download.status = Status::Failed(failure);
            shared.changed.notify_all();
            return;
        }
        attempts += 1;
        let backoff = shared.backoff * (1u32 << (attempts - 1).min(5));
        let until = Instant::now() + backoff;
        loop {
            if state.generation != generation || state.file.is_none() {
                return;
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
}

enum BodyEnd {
    /// The body was read to its (expected) end.
    Finished,
    /// A newer generation took over, or every reader is gone.
    Stopped,
    Failed(Failure),
}

/// Copies a response body into the spill file from `*pos`.
fn copy_body(
    shared: &Shared,
    generation: u64,
    mut response: Response,
    pos: &mut u64,
    attempts: &mut u32,
) -> BodyEnd {
    let mut buf = vec![0u8; CHUNK];
    loop {
        // A read timeout of the stream client comes back here too (classified offline).
        let read = match read_chunk(&mut response, &mut buf) {
            Ok(read) => read,
            Err(err) => return BodyEnd::Failed(shared.failure(classify_io(err))),
        };
        let mut state = shared.lock();
        if state.generation != generation {
            return BodyEnd::Stopped;
        }
        let Some(file) = state.file.as_mut() else {
            return BodyEnd::Stopped;
        };
        if read == 0 {
            if state.len.is_some_and(|len| *pos < len) {
                return BodyEnd::Failed(Failure::Offline("the stream ended early".into()));
            }
            state.len = Some(state.len.unwrap_or(*pos));
            state.download.status = Status::Done;
            shared.changed.notify_all();
            return BodyEnd::Finished;
        }
        let written = file
            .seek(SeekFrom::Start(*pos))
            .and_then(|_| file.write_all(&buf[..read]));
        if let Err(err) = written {
            state.download.status =
                Status::Failed(Failure::Other(format!("writing the stream buffer: {err}")));
            shared.changed.notify_all();
            return BodyEnd::Stopped;
        }
        state.ranges.insert(*pos, *pos + read as u64);
        *pos += read as u64;
        *attempts = 0;
        state.download.pos = *pos;
        state.progress = Instant::now();
        // Everything from here to the end is already held (a seek back into a gap).
        if let Some(len) = state.len {
            if *pos >= len || state.ranges.covers(*pos, len) {
                state.download.status = Status::Done;
                shared.changed.notify_all();
                return BodyEnd::Finished;
            }
        }
        shared.changed.notify_all();
    }
}

/// One read of up to a chunk: whatever arrived is handed on at once (a stalled server must not
/// hold back bytes it already sent). `Ok(0)` is the end of the body.
fn read_chunk(response: &mut Response, buf: &mut [u8]) -> io::Result<usize> {
    loop {
        match response.read(buf) {
            Err(err) if err.kind() == io::ErrorKind::Interrupted => {}
            result => return result,
        }
    }
}

/// A reader over a progressively downloaded HTTP body. Implements `Read`, `Seek` and symphonia's
/// `MediaSource`; [`HttpStream::cursor`] gives more readers over the same buffer. The spill file
/// is deleted when the last reader drops.
pub struct HttpStream {
    shared: Arc<Shared>,
    pos: u64,
}

impl HttpStream {
    /// Starts the download and returns once the response headers are in (or the stall timeout
    /// passed without them).
    pub fn open(input: &HttpInput) -> anyhow::Result<HttpStream> {
        let config = &input.config;
        fs::create_dir_all(&config.spill_dir)
            .map_err(|err| anyhow::anyhow!("creating the stream buffer folder: {err}"))?;
        let path = config
            .spill_dir
            .join(format!("{:016x}.{SPILL_EXTENSION}", rand::random::<u64>()));
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|err| anyhow::anyhow!("creating the stream buffer: {err}"))?;
        let shared = Arc::new(Shared {
            client: config.client.clone(),
            url: input.url.expose().to_owned(),
            secrets: url_secrets(input.url.expose()),
            stall: config.stall,
            retries: config.retries,
            backoff: config.backoff,
            path,
            state: Mutex::new(State {
                file: Some(file),
                readers: 1,
                ranges: RangeSet::default(),
                len: None,
                seekable: false,
                generation: 0,
                download: Download {
                    start: 0,
                    pos: 0,
                    status: Status::Connecting,
                    answered: false,
                },
                progress: Instant::now(),
            }),
            changed: Condvar::new(),
        });
        // From here on, dropping `stream` on an error removes the spill file.
        let stream = HttpStream { shared, pos: 0 };
        {
            let shared = &stream.shared;
            let mut state = shared.lock();
            start_download(shared, &mut state, 0);
            #[cfg(test)]
            {
                let pause = tests::OPEN_PAUSE.with(std::cell::Cell::get);
                if !pause.is_zero() {
                    drop(state);
                    std::thread::sleep(pause);
                    state = shared.lock();
                }
            }
            loop {
                // Once the server answered, `open` succeeds even when the body failed right
                // after: the bytes that came are read first, then the error.
                if state.download.answered {
                    break;
                }
                if let Status::Failed(failure) = &state.download.status {
                    let failure = failure.clone();
                    drop(state);
                    return Err(failure
                        .into_anyhow()
                        .context(format!("streaming {}", input.label)));
                }
                let waited = state.progress.elapsed();
                if waited >= shared.stall {
                    drop(state);
                    return Err(anyhow::Error::new(Offline(format!(
                        "the server did not answer within {} s",
                        shared.stall.as_secs_f32()
                    )))
                    .context(format!("streaming {}", input.label)));
                }
                state = shared
                    .changed
                    .wait_timeout(state, shared.stall - waited)
                    .unwrap_or_else(|e| e.into_inner())
                    .0;
            }
        }
        Ok(stream)
    }

    /// True when the server serves byte ranges (seeking restarts the download there).
    pub fn seekable(&self) -> bool {
        self.shared.lock().seekable
    }

    /// The body length, when the server sent it (or once the download finished).
    pub fn len(&self) -> Option<u64> {
        self.shared.lock().len
    }

    /// True when the body is known to be empty.
    pub fn is_empty(&self) -> bool {
        self.len() == Some(0)
    }

    /// A second reader at 0 over the same buffer.
    pub fn cursor(&self) -> HttpStream {
        self.shared.lock().readers += 1;
        HttpStream {
            shared: Arc::clone(&self.shared),
            pos: 0,
        }
    }

    fn read_at(&self, buf: &mut [u8]) -> Result<usize, Failure> {
        let shared = &self.shared;
        let pos = self.pos;
        let mut state = shared.lock();
        loop {
            if state.len.is_some_and(|len| pos >= len) || buf.is_empty() {
                return Ok(0);
            }
            if let Some(end) = state.ranges.run_end(pos) {
                let n = (end - pos).min(buf.len() as u64) as usize;
                let file = state
                    .file
                    .as_mut()
                    .ok_or_else(|| Failure::Other("the stream is closed".into()))?;
                file.seek(SeekFrom::Start(pos))
                    .and_then(|_| file.read_exact(&mut buf[..n]))
                    .map_err(|err| Failure::Other(format!("reading the stream buffer: {err}")))?;
                return Ok(n);
            }
            let seekable = state.seekable;
            let download = &state.download;
            let elsewhere = pos < download.start || pos > download.pos + RESTART_GAP;
            match &download.status {
                Status::Done if !seekable => return Ok(0),
                Status::Done => {
                    start_download(shared, &mut state, pos);
                    continue;
                }
                Status::Failed(_) if seekable && elsewhere => {
                    start_download(shared, &mut state, pos);
                    continue;
                }
                Status::Failed(failure) => return Err(failure.clone()),
                Status::Connecting | Status::Running if seekable && elsewhere => {
                    start_download(shared, &mut state, pos);
                    continue;
                }
                Status::Connecting | Status::Running => {}
            }
            let waited = state.progress.elapsed();
            if waited >= shared.stall {
                return Err(Failure::Offline(format!(
                    "no data from the server for {} s",
                    shared.stall.as_secs_f32()
                )));
            }
            state = shared
                .changed
                .wait_timeout(state, shared.stall - waited)
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
    }
}

impl Drop for HttpStream {
    fn drop(&mut self) {
        let mut state = self.shared.lock();
        state.readers -= 1;
        if state.readers == 0 {
            // Stop the download thread (it may be blocked on the network for a while; it holds
            // no file handle) and delete the spill file.
            state.generation += 1;
            if state.file.take().is_some() {
                let _ = fs::remove_file(&self.shared.path);
            }
            self.shared.changed.notify_all();
        }
    }
}

impl Read for HttpStream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.read_at(buf).map_err(Failure::into_io)?;
        self.pos += n as u64;
        Ok(n)
    }
}

impl Seek for HttpStream {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        let target = match pos {
            SeekFrom::Start(at) => Some(at),
            SeekFrom::Current(delta) => self.pos.checked_add_signed(delta),
            SeekFrom::End(delta) => {
                let len = self.len().ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::Unsupported,
                        "the stream length is not known yet",
                    )
                })?;
                len.checked_add_signed(delta)
            }
        };
        let target = target.ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "seek before the stream start")
        })?;
        // Seeking itself is free; the next read fetches what is missing.
        self.pos = target;
        Ok(target)
    }
}

impl MediaSource for HttpStream {
    fn is_seekable(&self) -> bool {
        self.seekable()
    }

    fn byte_len(&self) -> Option<u64> {
        self.len()
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::fs;
    use std::sync::Arc;
    use std::time::{Instant, SystemTime};

    use reqwest::Url;

    use super::*;
    use crate::audio::source::Input;
    use crate::navidrome::client::{
        StreamQuality, SubsonicClient, API_TIMEOUT, CONNECT_TIMEOUT, STREAM_READ_TIMEOUT,
    };
    use crate::navidrome::testserver::{FakeLibrary, FakeServer, Fault, FAKE_PASSWORD};
    use crate::secret::SecretUrl;

    const SONG: &str = "tr-0";
    const LABEL: &str = "Demo Artist 1 - Demo Song 1";

    /// Bytes that do not repeat with a short period, so an offset error shows.
    fn body(len: usize) -> Vec<u8> {
        (0..len as u32)
            .map(|i| (i.wrapping_mul(2_654_435_761) >> 13) as u8)
            .collect()
    }

    fn server_with(len: usize) -> FakeServer {
        FakeServer::start(FakeLibrary::synthetic(1, 1).with_audio(SONG, body(len)))
    }

    thread_local! {
        /// How long `open` lets go of the lock right after starting the download (this test
        /// thread only), so the download can get ahead of it deterministically.
        pub(super) static OPEN_PAUSE: Cell<Duration> = const { Cell::new(Duration::ZERO) };
    }

    /// A stream config over the shared stream client, spilling into a fresh temp dir.
    fn config(server: &FakeServer, stall: Duration) -> (tempfile::TempDir, Arc<StreamConfig>) {
        config_with(server, stall, STREAM_READ_TIMEOUT, RETRY_BACKOFF)
    }

    /// Like [`config`], with the stream client's per-read timeout and the resume backoff.
    fn config_with(
        server: &FakeServer,
        stall: Duration,
        read_timeout: Duration,
        backoff: Duration,
    ) -> (tempfile::TempDir, Arc<StreamConfig>) {
        let spill = tempfile::tempdir().unwrap();
        let client = SubsonicClient::with_stream_timeout(
            server.account(),
            CONNECT_TIMEOUT,
            API_TIMEOUT,
            read_timeout,
        )
        .unwrap();
        let mut config = StreamConfig::new(client.stream_http(), spill.path().to_path_buf());
        config.stall = stall;
        config.backoff = backoff;
        (spill, Arc::new(config))
    }

    fn input_at(url: SecretUrl, config: &Arc<StreamConfig>) -> HttpInput {
        HttpInput {
            url,
            label: LABEL.into(),
            extension: Some("flac".into()),
            config: Arc::clone(config),
        }
    }

    fn input(server: &FakeServer, config: &Arc<StreamConfig>) -> HttpInput {
        let client = SubsonicClient::new(server.account()).unwrap();
        input_at(
            client.stream_url(SONG, StreamQuality::Original, None),
            config,
        )
    }

    fn open(input: &HttpInput) -> HttpStream {
        HttpStream::open(input).unwrap_or_else(|err| panic!("open failed: {err:#}"))
    }

    fn read_at(stream: &mut HttpStream, at: u64, len: usize) -> Vec<u8> {
        stream.seek(SeekFrom::Start(at)).unwrap();
        let mut out = vec![0; len];
        stream.read_exact(&mut out).unwrap();
        out
    }

    /// Reads until EOF or an error; the bytes read and the error.
    fn read_until_error(stream: &mut HttpStream) -> (Vec<u8>, Option<io::Error>) {
        let mut out = Vec::new();
        let mut buf = vec![0; 16 * 1024];
        loop {
            match stream.read(&mut buf) {
                Ok(0) => return (out, None),
                Ok(n) => out.extend_from_slice(&buf[..n]),
                Err(err) => return (out, Some(err)),
            }
        }
    }

    fn spill_files(dir: &Path) -> usize {
        fs::read_dir(dir).map_or(0, |entries| entries.count())
    }

    /// The secrets in a stream URL: the token and salt values.
    fn url_secrets(url: &SecretUrl) -> Vec<String> {
        Url::parse(url.expose())
            .unwrap()
            .query_pairs()
            .filter(|(key, _)| key == "t" || key == "s")
            .map(|(_, value)| value.into_owned())
            .collect()
    }

    /// The error's every rendering carries no URL and no credential.
    fn assert_clean(err: &anyhow::Error, input: &HttpInput) {
        let texts = [format!("{err}"), format!("{err:#}"), format!("{err:?}")];
        for text in &texts {
            for needle in ["http://", "127.0.0.1", "rest/", "t=", "s=", FAKE_PASSWORD] {
                assert!(!text.contains(needle), "{needle:?} in {text:?}");
            }
            for secret in url_secrets(&input.url) {
                assert!(!text.contains(&secret), "a secret in {text:?}");
            }
        }
    }

    #[test]
    fn range_set_merges_and_reports_runs() {
        let mut set = RangeSet::default();
        set.insert(0, 10);
        set.insert(20, 30);
        assert_eq!(set.runs, [(0, 10), (20, 30)]);
        assert!(set.covers(0, 10));
        assert!(set.covers(22, 25));
        assert!(!set.covers(5, 25));
        assert_eq!(set.run_end(0), Some(10));
        assert_eq!(set.run_end(9), Some(10));
        assert_eq!(set.run_end(10), None);
        assert_eq!(set.run_end(25), Some(30));
        // Adjacent runs merge, overlapping ones too.
        set.insert(10, 15);
        assert_eq!(set.runs, [(0, 15), (20, 30)]);
        set.insert(14, 22);
        assert_eq!(set.runs, [(0, 30)]);
        assert!(set.covers(5, 25));
        // A run spanning several.
        set.insert(40, 50);
        set.insert(60, 70);
        set.insert(35, 80);
        assert_eq!(set.runs, [(0, 30), (35, 80)]);
        // Empty inserts change nothing.
        set.insert(90, 90);
        assert_eq!(set.runs, [(0, 30), (35, 80)]);
        assert!(set.covers(7, 7));
    }

    #[test]
    fn reads_the_whole_body() {
        let server = server_with(2_000_000);
        let (_spill, config) = config(&server, STALL_TIMEOUT);
        let mut stream = open(&input(&server, &config));
        assert!(stream.seekable());
        assert_eq!(stream.len(), Some(2_000_000));
        let mut out = Vec::new();
        stream.read_to_end(&mut out).unwrap();
        assert!(out == body(2_000_000), "the body differs");
        assert_eq!(server.requests().len(), 1);
        assert_eq!(server.requests()[0].range, None);
    }

    #[test]
    fn seek_ahead_restarts_with_a_range_request() {
        let server = server_with(4_000_000);
        // Every stream response stalls after 256 KiB, so the first download cannot get near
        // the seek target on its own.
        server.set_fault(Fault::Stall {
            method: "stream",
            after_bytes: 256 * 1024,
            wait: Duration::from_secs(2),
        });
        let (_spill, config) = config(&server, STALL_TIMEOUT);
        let mut stream = open(&input(&server, &config));
        let source = body(4_000_000);
        assert_eq!(read_at(&mut stream, 0, 1000), source[..1000]);
        assert_eq!(
            read_at(&mut stream, 3_000_000, 1000),
            source[3_000_000..3_001_000]
        );
        let requests = server.requests();
        assert_eq!(requests.len(), 2, "{requests:?}");
        assert_eq!(requests[1].range.as_deref(), Some("bytes=3000000-"));
        // Inside the restart gap: no new request, the reader waits for the download.
        assert_eq!(
            read_at(&mut stream, 3_100_000, 1000),
            source[3_100_000..3_101_000]
        );
        assert_eq!(server.requests().len(), 2);
    }

    #[test]
    fn seek_back_inside_the_buffer_makes_no_request() {
        let server = server_with(2_000_000);
        let (_spill, config) = config(&server, STALL_TIMEOUT);
        let mut stream = open(&input(&server, &config));
        let mut out = Vec::new();
        stream.read_to_end(&mut out).unwrap();
        let source = body(2_000_000);
        assert_eq!(read_at(&mut stream, 1000, 1000), source[1000..2000]);
        assert_eq!(read_at(&mut stream, 0, 10), source[..10]);
        assert_eq!(stream.seek(SeekFrom::End(-10)).unwrap(), 1_999_990);
        assert_eq!(server.requests().len(), 1);
    }

    #[test]
    fn cut_connection_resumes_with_range() {
        let server = server_with(2_000_000);
        server.set_fault(Fault::CutOnce {
            method: "stream",
            after_bytes: 100_000,
        });
        let (_spill, config) = config(&server, STALL_TIMEOUT);
        let mut stream = open(&input(&server, &config));
        let mut out = Vec::new();
        stream.read_to_end(&mut out).unwrap();
        assert!(out == body(2_000_000), "the body differs");
        let requests = server.requests();
        assert_eq!(requests.len(), 2, "{requests:?}");
        let start: u64 = requests[1]
            .range
            .as_deref()
            .and_then(|r| r.strip_prefix("bytes="))
            .and_then(|r| r.strip_suffix('-'))
            .and_then(|r| r.parse().ok())
            .expect("the resume asks for a range");
        assert!((1..=100_000).contains(&start), "resumed at {start}");
    }

    #[test]
    fn cut_without_range_support_reports_offline_after_buffer() {
        let server = server_with(2_000_000);
        server.set_fault(Fault::CutOnceThen {
            method: "stream",
            after_bytes: 100_000,
            then: Box::new(Fault::NoRanges),
        });
        let (_spill, config) = config(&server, STALL_TIMEOUT);
        let input = input(&server, &config);
        let mut stream = open(&input);
        assert!(!stream.seekable());
        let started = Instant::now();
        let (out, err) = read_until_error(&mut stream);
        let err = anyhow::Error::new(err.expect("the cut surfaces as an error"));
        assert!(is_offline(&err), "{err:#}");
        assert_clean(&err, &input);
        // Every buffered byte was handed out first, and nothing was retried.
        assert!(
            !out.is_empty() && out.len() <= 100_000,
            "{} bytes",
            out.len()
        );
        assert!(
            out == body(2_000_000)[..out.len()],
            "the buffered bytes differ"
        );
        assert_eq!(server.requests().len(), 1);
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn seeking_without_range_support_skips_through_the_download() {
        let server = server_with(2_000_000);
        server.set_fault(Fault::NoRanges);
        let (_spill, config) = config(&server, Duration::from_secs(5));
        let mut stream = open(&input(&server, &config));
        assert!(!stream.seekable());
        let source = body(2_000_000);
        assert_eq!(
            read_at(&mut stream, 1_500_000, 1000),
            source[1_500_000..1_501_000]
        );
        assert_eq!(read_at(&mut stream, 10, 1000), source[10..1010]);
        let requests = server.requests();
        assert_eq!(requests.len(), 1, "{requests:?}");
        assert_eq!(requests[0].range, None);
    }

    #[test]
    fn an_ignored_range_falls_back_to_restart_and_skip() {
        let server = server_with(4_000_000);
        server.set_fault(Fault::Stall {
            method: "stream",
            after_bytes: 256 * 1024,
            wait: Duration::from_secs(2),
        });
        let (_spill, config) = config(&server, Duration::from_secs(5));
        let mut stream = open(&input(&server, &config));
        assert!(stream.seekable());
        // From now on the server (a proxy) ignores Range and answers 200 with the whole body.
        server.set_fault(Fault::NoRanges);
        let source = body(4_000_000);
        assert_eq!(
            read_at(&mut stream, 3_000_000, 1000),
            source[3_000_000..3_001_000]
        );
        assert!(!stream.seekable());
        assert_eq!(read_at(&mut stream, 0, 1000), source[..1000]);
        let requests = server.requests();
        assert_eq!(requests.len(), 2, "{requests:?}");
        assert_eq!(requests[1].range.as_deref(), Some("bytes=3000000-"));
    }

    #[test]
    fn stall_reports_offline_within_the_timeout() {
        let server = server_with(2_000_000);
        server.set_fault(Fault::Stall {
            method: "stream",
            after_bytes: 100_000,
            wait: Duration::from_secs(10),
        });
        // The stall timeout also bounds the wait for the headers in `open`; 1 s (not less) keeps
        // this test from failing on a machine busy with parallel builds.
        let stall = Duration::from_secs(1);
        let (_spill, config) = config(&server, stall);
        let input = input(&server, &config);
        let mut stream = open(&input);
        let started = Instant::now();
        let (out, err) = read_until_error(&mut stream);
        let took = started.elapsed();
        let err = anyhow::Error::new(err.expect("a stall surfaces as an error"));
        assert!(is_offline(&err), "{err:#}");
        assert_clean(&err, &input);
        assert_eq!(out.len(), 100_000);
        // Well before the server would go on (10 s): the stall timeout, plus slack.
        assert!(took < stall * 4, "took {took:?}");
    }

    #[test]
    fn a_slow_first_byte_within_the_stall_timeout_is_fine() {
        let server = server_with(200_000);
        // A buffering proxy: nothing at all for a while, then the whole response.
        server.set_fault(Fault::Stall {
            method: "stream",
            after_bytes: 0,
            wait: Duration::from_millis(600),
        });
        let (_spill, config) = config(&server, Duration::from_secs(5));
        let mut stream = open(&input(&server, &config));
        let mut out = Vec::new();
        stream.read_to_end(&mut out).unwrap();
        assert!(out == body(200_000), "the body differs");
    }

    #[test]
    fn no_headers_within_the_stall_timeout_is_offline() {
        let server = server_with(200_000);
        server.set_fault(Fault::Stall {
            method: "stream",
            after_bytes: 0,
            wait: Duration::from_secs(5),
        });
        let (_spill, config) = config(&server, Duration::from_millis(300));
        let input = input(&server, &config);
        let started = Instant::now();
        let err = HttpStream::open(&input).err().expect("open fails");
        assert!(is_offline(&err), "{err:#}");
        assert_clean(&err, &input);
        // Well before the server would answer (5 s).
        assert!(started.elapsed() < Duration::from_secs(3));
    }

    #[test]
    fn refused_connection_is_offline() {
        let server = server_with(1000);
        let (_spill, config) = config(&server, STALL_TIMEOUT);
        let input = input(&server, &config);
        server.set_fault(Fault::Refuse);
        let err = HttpStream::open(&input).err().expect("open fails");
        assert!(is_offline(&err), "{err:#}");
        assert_clean(&err, &input);
    }

    #[test]
    fn proxy_errors_are_offline() {
        let server = server_with(1000);
        let (_spill, config) = config(&server, STALL_TIMEOUT);
        let input = input(&server, &config);
        for status in [502, 503, 504] {
            server.set_fault(Fault::Status(status));
            let err = HttpStream::open(&input).err().expect("open fails");
            assert!(is_offline(&err), "{status}: {err:#}");
            assert_clean(&err, &input);
        }
    }

    #[test]
    fn http_404_is_not_offline() {
        let server = server_with(1000);
        let (_spill, config) = config(&server, STALL_TIMEOUT);
        let input = input_at(
            SecretUrl::new(format!("{}/nothing-here", server.url())),
            &config,
        );
        let err = HttpStream::open(&input).err().expect("open fails");
        assert!(!is_offline(&err), "{err:#}");
        assert!(format!("{err:#}").contains("404"), "{err:#}");
        assert_clean(&err, &input);
    }

    #[test]
    fn a_subsonic_error_instead_of_audio_is_not_offline() {
        let server = server_with(1000);
        let (_spill, config) = config(&server, STALL_TIMEOUT);
        let client = SubsonicClient::new(server.account()).unwrap();
        let input = input_at(
            client.stream_url("no-such-song", StreamQuality::Original, None),
            &config,
        );
        let err = HttpStream::open(&input).err().expect("open fails");
        assert!(!is_offline(&err), "{err:#}");
        assert_clean(&err, &input);
    }

    #[test]
    fn spill_file_removed_on_drop() {
        let server = server_with(500_000);
        let (spill, config) = config(&server, STALL_TIMEOUT);
        let mut stream = open(&input(&server, &config));
        let mut cursor = stream.cursor();
        let mut out = Vec::new();
        stream.read_to_end(&mut out).unwrap();
        assert_eq!(spill_files(spill.path()), 1);
        drop(stream);
        // The cursor still reads the same buffer from the start.
        let mut again = Vec::new();
        cursor.read_to_end(&mut again).unwrap();
        assert!(again == out, "the cursor reads other bytes");
        assert_eq!(server.requests().len(), 1);
        assert_eq!(spill_files(spill.path()), 1);
        drop(cursor);
        assert_eq!(spill_files(spill.path()), 0);
    }

    #[test]
    fn a_failed_open_leaves_no_spill_file() {
        let server = server_with(1000);
        let (spill, config) = config(&server, STALL_TIMEOUT);
        server.set_fault(Fault::Status(503));
        assert!(HttpStream::open(&input(&server, &config)).is_err());
        assert_eq!(spill_files(spill.path()), 0);
    }

    #[test]
    fn input_debug_hides_url() {
        let server = server_with(1000);
        let (_spill, config) = config(&server, STALL_TIMEOUT);
        let input = input(&server, &config);
        let secrets = url_secrets(&input.url);
        assert_eq!(secrets.len(), 2);
        let text = format!("{:?}", Input::Http(input.clone()));
        assert!(text.contains(LABEL), "{text}");
        for needle in ["http", "127.0.0.1", "rest/", "t=", "s=", FAKE_PASSWORD] {
            assert!(!text.contains(needle), "{needle:?} in {text}");
        }
        for secret in &secrets {
            assert!(!text.contains(secret.as_str()), "a secret in {text}");
        }
        assert_eq!(crate::audio::source::describe(&Input::Http(input)), LABEL);
    }

    #[test]
    fn is_offline_looks_through_contexts_and_io_errors() {
        let direct = anyhow::Error::new(Offline("gone".into())).context("reading");
        assert!(is_offline(&direct));
        let io = io::Error::new(io::ErrorKind::ConnectionAborted, Offline("gone".into()));
        assert!(is_offline(&anyhow::Error::new(io).context("decoding")));
        assert!(!is_offline(&anyhow::anyhow!("HTTP 404")));
        let other = io::Error::other("bad data");
        assert!(!is_offline(&anyhow::Error::new(other)));
    }

    #[test]
    fn open_succeeds_when_the_body_fails_right_after_the_headers() {
        let server = server_with(2_000_000);
        server.set_fault(Fault::CutOnceThen {
            method: "stream",
            after_bytes: 1024,
            then: Box::new(Fault::NoRanges),
        });
        let (_spill, config) = config(&server, STALL_TIMEOUT);
        let input = input(&server, &config);
        // `open` looks only after the download answered, got its 1 KB and failed on the cut.
        OPEN_PAUSE.with(|pause| pause.set(Duration::from_secs(1)));
        let opened = HttpStream::open(&input);
        OPEN_PAUSE.with(|pause| pause.set(Duration::ZERO));
        let mut stream = opened.unwrap_or_else(|err| panic!("open failed: {err:#}"));
        assert!(!stream.seekable());
        let (out, err) = read_until_error(&mut stream);
        let err = anyhow::Error::new(err.expect("the cut surfaces as an error"));
        assert!(is_offline(&err), "{err:#}");
        assert_clean(&err, &input);
        assert!(!out.is_empty() && out.len() <= 1024, "{} bytes", out.len());
        assert!(
            out == body(2_000_000)[..out.len()],
            "the buffered bytes differ"
        );
        assert_eq!(server.requests().len(), 1);
    }

    #[test]
    fn retries_run_out_then_offline_after_the_buffered_bytes() {
        let server = server_with(2_000_000);
        // One cut, then every resume gets a proxy 503.
        server.set_fault(Fault::CutOnceThen {
            method: "stream",
            after_bytes: 100_000,
            then: Box::new(Fault::Status(503)),
        });
        let (_spill, config) = config_with(
            &server,
            STALL_TIMEOUT,
            STREAM_READ_TIMEOUT,
            Duration::from_millis(20),
        );
        let input = input(&server, &config);
        let mut stream = open(&input);
        assert!(stream.seekable());
        let (out, err) = read_until_error(&mut stream);
        let err = anyhow::Error::new(err.expect("running out of retries surfaces as an error"));
        assert!(is_offline(&err), "{err:#}");
        assert_clean(&err, &input);
        assert!(
            !out.is_empty() && out.len() <= 100_000,
            "{} bytes",
            out.len()
        );
        assert!(
            out == body(2_000_000)[..out.len()],
            "the buffered bytes differ"
        );
        let requests = server.requests();
        assert_eq!(requests.len(), 1 + RETRIES as usize, "{requests:?}");
        assert!(requests[1..].iter().all(|r| r.range.is_some()));
    }

    #[test]
    fn a_read_timeout_is_offline() {
        // A transcoded stream (timeOffset): not byte-seekable, so nothing is resumed.
        let server = FakeServer::start(FakeLibrary::synthetic(1, 1).with_wav(SONG, 3));
        server.set_fault(Fault::Stall {
            method: "stream",
            after_bytes: 100_000,
            wait: Duration::from_secs(20),
        });
        // The reader would wait 15 s; the stream client's read timeout (2 s) ends it first.
        // The timeout also bounds the wait for the headers, so it stays well above what a
        // machine busy with parallel test runs needs for those.
        let (_spill, config) = config_with(
            &server,
            Duration::from_secs(15),
            Duration::from_secs(2),
            RETRY_BACKOFF,
        );
        let client = SubsonicClient::new(server.account()).unwrap();
        let input = input_at(
            client.stream_url(SONG, StreamQuality::Original, Some(0)),
            &config,
        );
        let mut stream = open(&input);
        assert!(!stream.seekable());
        let started = Instant::now();
        let (out, err) = read_until_error(&mut stream);
        let err = anyhow::Error::new(err.expect("a read timeout surfaces as an error"));
        assert!(is_offline(&err), "{err:#}");
        assert_clean(&err, &input);
        assert_eq!(out.len(), 100_000);
        // Well before the reader's own stall timeout (15 s) and the server (20 s).
        assert!(started.elapsed() < Duration::from_secs(10));
    }

    #[test]
    fn a_silent_server_ends_the_download_thread() {
        let server = server_with(2_000_000);
        server.set_fault(Fault::Stall {
            method: "stream",
            after_bytes: 100_000,
            wait: Duration::from_secs(20),
        });
        let (_spill, config) = config_with(
            &server,
            STALL_TIMEOUT,
            Duration::from_secs(2),
            RETRY_BACKOFF,
        );
        let mut stream = open(&input(&server, &config));
        let mut first = vec![0; 100_000];
        stream.read_exact(&mut first).unwrap();
        // The download thread holds the only other reference to the shared state.
        let shared = Arc::downgrade(&stream.shared);
        drop(stream);
        let started = Instant::now();
        while shared.upgrade().is_some() {
            // The read timeout is 2 s; the server stays silent for 20 s.
            assert!(
                started.elapsed() < Duration::from_secs(12),
                "the download thread is still blocked on the silent server"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn a_slow_but_steady_stream_is_not_cut() {
        let server = server_with(200_000);
        // 10 pieces 400 ms apart: about 4 s in all, far over the 1.5 s read timeout.
        server.set_fault(Fault::Trickle {
            method: "stream",
            chunk: 20_000,
            every: Duration::from_millis(400),
        });
        let read_timeout = Duration::from_millis(1500);
        let (_spill, config) = config_with(
            &server,
            Duration::from_secs(10),
            read_timeout,
            RETRY_BACKOFF,
        );
        let mut stream = open(&input(&server, &config));
        let started = Instant::now();
        let mut out = Vec::new();
        stream.read_to_end(&mut out).unwrap();
        assert!(out == body(200_000), "the body differs");
        assert!(started.elapsed() > read_timeout);
        assert_eq!(server.requests().len(), 1);
    }

    #[test]
    fn cleanup_spill_dir_removes_old_part_files() {
        let dir = tempfile::tempdir().unwrap();
        let old = SystemTime::now() - Duration::from_secs(2 * 24 * 3600);
        let write = |name: &str, modified: Option<SystemTime>| {
            let path = dir.path().join(name);
            let file = fs::File::create(&path).unwrap();
            if let Some(modified) = modified {
                file.set_modified(modified).unwrap();
            }
        };
        write("old.part", Some(old));
        write("new.part", None);
        write("old.txt", Some(old));
        cleanup_spill_dir(dir.path());
        let mut left: Vec<String> = fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        left.sort();
        assert_eq!(left, ["new.part", "old.txt"]);
        // A missing dir is fine.
        cleanup_spill_dir(&dir.path().join("missing"));
    }
}
