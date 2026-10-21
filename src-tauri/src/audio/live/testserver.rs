//! A fake radio server for tests: endless ICY streams, playlists, HLS and HTML pages, bound to
//! `127.0.0.1` only, with fake station names.
//!
//! Like the fake Subsonic server it is a small HTTP/1.1 server on a plain `TcpListener` (one
//! request per connection, `Connection: close`), so a fault can stall or cut the very socket the
//! client reads.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::library::tags::fixtures;

/// The metaint the server announces under [`Fault::HugeMetaint`].
pub(crate) const HUGE_METAINT: u64 = 1_000_000_000;
const WIRE_CHUNK: usize = 4096;

/// What a station path serves.
#[derive(Clone, Debug)]
pub(crate) enum Body {
    /// `tests/fixtures/radio/tone.mp3` back to back, forever. One loop takes `seconds_per_loop`
    /// on the wire; 0 sends as fast as the client reads.
    Mp3Tone { seconds_per_loop: f32 },
    /// `tests/fixtures/notags.ogg` back to back, forever (a chain of Ogg streams).
    Ogg,
    /// A 200 with an HTML page.
    Html,
    /// A PLS playlist pointing at this server's `target` path.
    Pls(String),
    /// An M3U playlist pointing at this server's `target` path.
    M3u(String),
    /// A live HLS media playlist.
    Hls,
    /// A 302 to this server's `target` path.
    Redirect(String),
}

/// What the server does wrong.
#[derive(Clone, Debug, Default)]
pub(crate) enum Fault {
    #[default]
    None,
    /// The next audio response is cut after this many audio bytes.
    CutAfter(usize),
    /// The next audio response stops after this many audio bytes for the duration, then goes on.
    StallAfter(usize, Duration),
    /// Nothing listens on the port, and open connections are cut, until another fault is set.
    Refuse,
    /// Every request answers this status with an HTML page.
    Status(u16),
    /// Metadata blocks carry random bytes (the framing stays valid).
    GarbageMeta,
    /// Audio responses announce `icy-metaint: 1000000000` and carry no metadata.
    HugeMetaint,
    /// The next audio response is cut after `after` audio bytes; from the cut on, every request
    /// answers `status` for `lasting` (an Icecast mount while its source encoder reconnects).
    CutThen {
        after: usize,
        status: u16,
        lasting: Duration,
    },
    /// Audio responses start with Shoutcast's `ICY 200 OK` and a Latin-1 `icy-name`.
    IcyStatusLine,
}

/// One request as the server saw it.
#[derive(Clone, Debug)]
pub(crate) struct Request {
    pub path: String,
    pub user_agent: Option<String>,
    pub icy_metadata: Option<String>,
}

struct Station {
    body: Body,
    metaint: Option<usize>,
    titles: Vec<String>,
}

struct State {
    stations: HashMap<String, Station>,
    fault: Fault,
    requests: Vec<Request>,
    /// Set by a [`Fault::CutThen`] cut: every request answers this status until then.
    status_until: Option<(u16, Instant)>,
}

struct Shared {
    state: Mutex<State>,
    port: u16,
    /// Bumped by `stop`: every open connection of an older epoch is cut.
    epoch: AtomicU64,
    connections: AtomicUsize,
    tone: Vec<u8>,
    ogg: Vec<u8>,
}

impl Shared {
    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }
}

struct Running {
    stop: Arc<AtomicBool>,
    thread: JoinHandle<()>,
}

/// The fake radio server. Stops when dropped.
pub(crate) struct FakeRadio {
    shared: Arc<Shared>,
    running: Mutex<Option<Running>>,
    refused: AtomicBool,
}

impl FakeRadio {
    pub fn start() -> FakeRadio {
        let listener = TcpListener::bind("127.0.0.1:0").expect("binding the fake radio server");
        let port = listener.local_addr().expect("fake radio address").port();
        let read = |name: &str| {
            std::fs::read(fixtures().join(name)).unwrap_or_else(|err| panic!("{name}: {err}"))
        };
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                stations: HashMap::new(),
                fault: Fault::None,
                requests: Vec::new(),
                status_until: None,
            }),
            port,
            epoch: AtomicU64::new(0),
            connections: AtomicUsize::new(0),
            tone: read("radio/tone.mp3"),
            ogg: read("notags.ogg"),
        });
        FakeRadio {
            running: Mutex::new(Some(serve(listener, Arc::clone(&shared)))),
            shared,
            refused: AtomicBool::new(false),
        }
    }

    /// Adds (or replaces) the station at `path`. `metaint` turns on ICY metadata for clients
    /// that ask for it; every title goes out in two blocks in a row, the last one forever.
    pub fn station(
        &self,
        path: &str,
        body: Body,
        metaint: Option<usize>,
        titles: &[&str],
    ) -> &Self {
        self.shared.state().stations.insert(
            path.to_owned(),
            Station {
                body,
                metaint,
                titles: titles.iter().map(|t| (*t).to_owned()).collect(),
            },
        );
        self
    }

    /// `http://127.0.0.1:<port><path>`.
    pub fn url(&self, path: &str) -> String {
        format!("http://127.0.0.1:{}{path}", self.shared.port)
    }

    /// The bytes of one loop of [`Body::Mp3Tone`].
    pub fn tone(&self) -> &[u8] {
        &self.shared.tone
    }

    pub fn requests(&self) -> Vec<Request> {
        self.shared.state().requests.clone()
    }

    /// Connections the server holds open right now.
    pub fn connections(&self) -> usize {
        self.shared.connections.load(Ordering::SeqCst)
    }

    pub fn set_fault(&self, fault: Fault) {
        let refuse = matches!(fault, Fault::Refuse);
        self.shared.state().fault = fault;
        if refuse {
            self.refused.store(true, Ordering::SeqCst);
            self.stop();
        } else if self.refused.swap(false, Ordering::SeqCst) {
            self.restart();
        }
    }

    /// Stops listening and cuts every open connection; the port stays this server's.
    pub fn stop(&self) {
        self.shared.epoch.fetch_add(1, Ordering::SeqCst);
        let running = self
            .running
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();
        if let Some(running) = running {
            running.stop.store(true, Ordering::SeqCst);
            let _ = running.thread.join();
        }
    }

    /// Listens again on the same port.
    pub fn restart(&self) {
        self.stop();
        let address = format!("127.0.0.1:{}", self.shared.port);
        let mut attempts = 0;
        let listener = loop {
            match TcpListener::bind(&address) {
                Ok(listener) => break listener,
                Err(err) => {
                    attempts += 1;
                    assert!(attempts < 100, "rebinding the fake radio server: {err}");
                    std::thread::sleep(Duration::from_millis(20));
                }
            }
        };
        *self.running.lock().unwrap_or_else(|e| e.into_inner()) =
            Some(serve(listener, Arc::clone(&self.shared)));
    }
}

impl Drop for FakeRadio {
    fn drop(&mut self) {
        self.stop();
    }
}

fn serve(listener: TcpListener, shared: Arc<Shared>) -> Running {
    let stop = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&stop);
    listener
        .set_nonblocking(true)
        .expect("non-blocking fake radio server");
    let thread = std::thread::spawn(move || {
        while !flag.load(Ordering::SeqCst) {
            match listener.accept() {
                Ok((stream, _)) => {
                    let shared = Arc::clone(&shared);
                    shared.connections.fetch_add(1, Ordering::SeqCst);
                    std::thread::spawn(move || {
                        handle(stream, &shared);
                        shared.connections.fetch_sub(1, Ordering::SeqCst);
                    });
                }
                Err(_) => std::thread::sleep(Duration::from_millis(5)),
            }
        }
    });
    Running { stop, thread }
}

fn read_head(stream: &mut TcpStream) -> Option<String> {
    let mut head = Vec::new();
    let mut buf = [0u8; 4096];
    while !head.windows(4).any(|w| w == b"\r\n\r\n") {
        let n = stream.read(&mut buf).ok()?;
        if n == 0 || head.len() > 64 * 1024 {
            return None;
        }
        head.extend_from_slice(&buf[..n]);
    }
    String::from_utf8(head).ok()
}

fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        404 => "Not Found",
        410 => "Gone",
        500 => "Internal Server Error",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        504 => "Gateway Timeout",
        _ => "Status",
    }
}

fn whole(stream: &mut TcpStream, status: u16, content_type: &str, body: &[u8]) {
    let head = format!(
        "HTTP/1.1 {status} {}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        reason(status),
        body.len()
    );
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(body);
    let _ = stream.flush();
    let _ = stream.shutdown(Shutdown::Write);
}

fn html_page(status: u16) -> Vec<u8> {
    format!("<!doctype html><html><head><title>{status}</title></head><body>Fake station page</body></html>").into_bytes()
}

fn handle(mut stream: TcpStream, shared: &Shared) {
    let epoch = shared.epoch.load(Ordering::SeqCst);
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let Some(head) = read_head(&mut stream) else {
        return;
    };
    let mut lines = head.split("\r\n");
    let path = lines
        .next()
        .and_then(|line| line.split(' ').nth(1))
        .unwrap_or("/")
        .to_owned();
    let mut user_agent = None;
    let mut icy_metadata = None;
    for line in lines {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        match name.trim().to_ascii_lowercase().as_str() {
            "user-agent" => user_agent = Some(value.trim().to_owned()),
            "icy-metadata" => icy_metadata = Some(value.trim().to_owned()),
            _ => {}
        }
    }
    let wants_meta = icy_metadata.as_deref() == Some("1");
    let (station, fault, held) = {
        let mut state = shared.state();
        state.requests.push(Request {
            path: path.clone(),
            user_agent,
            icy_metadata,
        });
        let fault = state.fault.clone();
        let station = state
            .stations
            .get(&path)
            .map(|s| (s.body.clone(), s.metaint, s.titles.clone()));
        // One-shot faults apply to the next audio response only.
        if matches!(station, Some((Body::Mp3Tone { .. } | Body::Ogg, _, _)))
            && matches!(
                fault,
                Fault::CutAfter(_) | Fault::StallAfter(..) | Fault::CutThen { .. }
            )
        {
            state.fault = Fault::None;
        }
        let held = state
            .status_until
            .filter(|(_, until)| Instant::now() < *until)
            .map(|(status, _)| status);
        (station, fault, held)
    };
    if let Some(status) = held {
        whole(
            &mut stream,
            status,
            "text/html; charset=utf-8",
            &html_page(status),
        );
        return;
    }
    if let Fault::Status(status) = fault {
        whole(
            &mut stream,
            status,
            "text/html; charset=utf-8",
            &html_page(status),
        );
        return;
    }
    let Some((body, metaint, titles)) = station else {
        whole(
            &mut stream,
            404,
            "text/html; charset=utf-8",
            &html_page(404),
        );
        return;
    };
    let target = |path: &str| format!("http://127.0.0.1:{}{path}", shared.port);
    match body {
        Body::Html => whole(
            &mut stream,
            200,
            "text/html; charset=utf-8",
            &html_page(200),
        ),
        Body::Pls(path) => {
            let list = format!(
                "[playlist]\nNumberOfEntries=1\nFile1={}\nTitle1=Demo Radio\nLength1=-1\nVersion=2\n",
                target(&path)
            );
            whole(&mut stream, 200, "audio/x-scpls", list.as_bytes());
        }
        Body::M3u(path) => {
            let list = format!("#EXTM3U\n#EXTINF:-1,Demo Radio\n{}\n", target(&path));
            whole(&mut stream, 200, "audio/x-mpegurl", list.as_bytes());
        }
        Body::Hls => {
            let list = "#EXTM3U\n#EXT-X-VERSION:3\n#EXT-X-TARGETDURATION:2\n#EXT-X-MEDIA-SEQUENCE:1\n#EXTINF:2.0,\nseg1.ts\n#EXTINF:2.0,\nseg2.ts\n";
            whole(
                &mut stream,
                200,
                "application/vnd.apple.mpegurl",
                list.as_bytes(),
            );
        }
        Body::Redirect(path) => {
            let head = format!(
                "HTTP/1.1 302 Found\r\nLocation: {}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                target(&path)
            );
            let _ = stream.write_all(head.as_bytes());
            let _ = stream.flush();
            let _ = stream.shutdown(Shutdown::Write);
        }
        Body::Mp3Tone { seconds_per_loop } => {
            let pace = Duration::from_secs_f32(seconds_per_loop.max(0.0));
            endless(
                &mut stream,
                shared,
                epoch,
                Endless {
                    content_type: "audio/mpeg",
                    data: &shared.tone,
                    pace,
                    metaint: metaint.filter(|_| wants_meta),
                    titles: &titles,
                    fault: &fault,
                },
            );
        }
        Body::Ogg => endless(
            &mut stream,
            shared,
            epoch,
            Endless {
                content_type: "application/ogg",
                data: &shared.ogg,
                pace: Duration::ZERO,
                metaint: metaint.filter(|_| wants_meta),
                titles: &titles,
                fault: &fault,
            },
        ),
    }
}

struct Endless<'a> {
    content_type: &'static str,
    data: &'a [u8],
    /// How long one loop of `data` takes on the wire.
    pace: Duration,
    metaint: Option<usize>,
    titles: &'a [String],
    fault: &'a Fault,
}

/// A tiny xorshift for garbage metadata (deterministic per connection).
struct Noise(u64);

impl Noise {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
}

fn meta_block(titles: &[String], index: usize, garbage: Option<&mut Noise>) -> Vec<u8> {
    if let Some(noise) = garbage {
        let len = (noise.next() % 256) as usize * 16;
        let mut block: Vec<u8> = (0..len).map(|_| noise.next() as u8).collect();
        // Now and then something that looks like a title but never ends.
        let bait = b"StreamTitle='\x01\xff\xfe";
        if len >= bait.len() && noise.next() % 3 == 0 {
            block[..bait.len()].copy_from_slice(bait);
        }
        let mut out = vec![(len / 16) as u8];
        out.extend_from_slice(&block);
        return out;
    }
    let Some(title) = titles.get((index / 2).min(titles.len().saturating_sub(1))) else {
        return vec![0];
    };
    let mut text = format!("StreamTitle='{title}';StreamUrl='';").into_bytes();
    text.truncate(255 * 16);
    let blocks = text.len().div_ceil(16);
    text.resize(blocks * 16, 0);
    let mut out = vec![blocks as u8];
    out.extend_from_slice(&text);
    out
}

fn endless(stream: &mut TcpStream, shared: &Shared, epoch: u64, body: Endless<'_>) {
    let huge = matches!(body.fault, Fault::HugeMetaint);
    let metaint = if huge { None } else { body.metaint };
    let mut head = Vec::new();
    if matches!(body.fault, Fault::IcyStatusLine) {
        // Shoutcast v1: no HTTP version, headers in Latin-1.
        head.extend_from_slice(b"ICY 200 OK\r\nicy-name: Demo Radio Caf\xe9\r\n");
    } else {
        head.extend_from_slice(b"HTTP/1.1 200 OK\r\nicy-name: Demo Radio\r\n");
    }
    head.extend_from_slice(
        format!(
            "Content-Type: {}\r\nConnection: close\r\n",
            body.content_type
        )
        .as_bytes(),
    );
    if huge {
        head.extend_from_slice(format!("icy-metaint: {HUGE_METAINT}\r\n").as_bytes());
    } else if let Some(metaint) = metaint {
        head.extend_from_slice(format!("icy-metaint: {metaint}\r\n").as_bytes());
    }
    head.extend_from_slice(b"\r\n");
    if stream.write_all(&head).is_err() {
        return;
    }
    let (cut, stall) = match body.fault {
        Fault::CutAfter(bytes) | Fault::CutThen { after: bytes, .. } => (Some(*bytes), None),
        Fault::StallAfter(bytes, wait) => (None, Some((*bytes, *wait))),
        _ => (None, None),
    };
    let mut stall = stall;
    let mut noise =
        matches!(body.fault, Fault::GarbageMeta).then_some(Noise(0x9e37_79b9_7f4a_7c15));
    let data = body.data;
    let mut at = 0usize;
    let mut sent = 0usize;
    let mut until_meta = metaint.unwrap_or(usize::MAX);
    let mut block = 0usize;
    let mut wire = Vec::with_capacity(WIRE_CHUNK + 4096);
    let started = Instant::now();
    loop {
        if shared.epoch.load(Ordering::SeqCst) != epoch {
            let _ = stream.shutdown(Shutdown::Both);
            return;
        }
        let mut take = WIRE_CHUNK.min(data.len() - at).min(until_meta);
        if let Some(cut) = cut {
            if sent >= cut {
                if let Fault::CutThen {
                    status, lasting, ..
                } = body.fault
                {
                    shared.state().status_until = Some((*status, Instant::now() + *lasting));
                }
                let _ = stream.flush();
                let _ = stream.shutdown(Shutdown::Both);
                return;
            }
            take = take.min(cut - sent);
        }
        if let Some((after, _)) = stall {
            if sent < after {
                take = take.min(after - sent);
            }
        }
        wire.clear();
        wire.extend_from_slice(&data[at..at + take]);
        at = (at + take) % data.len();
        sent += take;
        if let Some(metaint) = metaint {
            until_meta -= take;
            if until_meta == 0 {
                wire.extend_from_slice(&meta_block(body.titles, block, noise.as_mut()));
                block += 1;
                until_meta = metaint;
            }
        }
        if stream.write_all(&wire).is_err() {
            return;
        }
        if let Some((after, wait)) = stall {
            if sent >= after {
                let _ = stream.flush();
                let until = Instant::now() + wait;
                while Instant::now() < until {
                    if shared.epoch.load(Ordering::SeqCst) != epoch {
                        let _ = stream.shutdown(Shutdown::Both);
                        return;
                    }
                    std::thread::sleep(Duration::from_millis(10));
                }
                stall = None;
            }
        }
        if !body.pace.is_zero() {
            // Paced against the start, so the rate holds however long each write took.
            let due = body.pace.mul_f64(sent as f64 / data.len() as f64);
            let elapsed = started.elapsed();
            if due > elapsed {
                std::thread::sleep(due - elapsed);
            }
        }
    }
}
