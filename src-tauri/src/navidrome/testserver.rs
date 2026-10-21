//! A fake Subsonic server for tests: fake data, the fake account `demo` / `demo-pass`, bound to
//! `127.0.0.1` only.
//!
//! It is a small HTTP/1.1 server on a plain `TcpListener` (one request per connection,
//! `Connection: close`), so a fault can stall or cut the very socket the client reads.

use std::collections::{HashMap, HashSet};
use std::io::{Cursor, Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::Duration;

use reqwest::Url;
use serde_json::{json, Map, Value};

use super::client::{
    token, Account, AlbumId3, ArtistId3, ArtistRef, Child, PlaylistSummary, PlaylistWithSongs,
    ReplayGainInfo, API_VERSION,
};
use crate::secret::Secret;

pub(crate) const FAKE_USER: &str = "demo";
pub(crate) const FAKE_PASSWORD: &str = "demo-pass";

/// First `lastModified` of a fake library (ms).
const FIRST_MODIFIED: i64 = 1_700_000_000_000;
/// The star timestamp of every starred item.
const STARRED_AT: &str = "2026-01-01T12:00:00Z";
const WAV_RATE: u32 = 44_100;
const WAV_CHANNELS: u16 = 2;

/// What the fake server does wrong.
#[derive(Clone, Debug, Default)]
pub(crate) enum Fault {
    #[default]
    None,
    /// Every request fails with Subsonic error 40.
    WrongPassword,
    /// Every request answers this HTTP status with an HTML body (a proxy error page).
    Status(u16),
    /// Every request answers 200 with an HTML page.
    Html,
    /// Nothing listens on the port until another fault is set.
    Refuse,
    /// Requests to `method` send the headers and the first `after_bytes` body bytes, wait, then
    /// send the rest. With `after_bytes` 0 nothing at all goes out before the wait.
    Stall {
        method: &'static str,
        after_bytes: usize,
        wait: Duration,
    },
    /// The next request to `method` sends the headers and `after_bytes` body bytes, then the
    /// connection is cut. Later requests are served normally.
    CutOnce {
        method: &'static str,
        after_bytes: usize,
    },
    /// Like [`Fault::CutOnce`] on a server without range support: the cut request already
    /// advertises no `Accept-Ranges`, and the fault becomes [`Fault::NoRanges`] afterwards.
    CutOnceNoRanges {
        method: &'static str,
        after_bytes: usize,
    },
    /// `stream` ignores `Range` and advertises no `Accept-Ranges`.
    NoRanges,
    /// `search3` returns no songs.
    Search3Empty,
    /// Every request fails with this Subsonic error code.
    Code(u32),
    /// Every request answers this HTTP status with a Subsonic error body of this code.
    StatusWithCode { status: u16, code: u32 },
}

/// How the fake server redirects, as a reverse proxy would.
#[derive(Clone, Debug)]
pub(crate) enum Redirect {
    /// A request to `/rest/...` without the server's path prefix is redirected with this status
    /// to the prefixed path, query kept.
    ToPrefix(u16),
    /// Every request is redirected with this status to `<base><path>?<query>`.
    To(u16, String),
}

/// One request that reached a Subsonic method.
#[derive(Clone, Debug)]
pub(crate) struct Recorded {
    pub method: String,
    pub params: Vec<(String, String)>,
    pub range: Option<String>,
}

#[derive(Clone, Debug)]
enum Audio {
    Bytes(Vec<u8>),
    /// A generated sine WAV of this length.
    Wav {
        seconds: u32,
    },
}

/// A playlist of the fake library, songs by id.
#[derive(Clone, Debug)]
pub(crate) struct FakePlaylist {
    pub summary: PlaylistSummary,
    pub song_ids: Vec<String>,
}

/// The fake server's library and state. Tests change it through [`FakeServer::edit`].
#[derive(Clone, Debug, Default)]
pub(crate) struct FakeLibrary {
    pub artists: Vec<ArtistId3>,
    pub albums: Vec<AlbumId3>,
    pub songs: Vec<Child>,
    pub playlists: Vec<FakePlaylist>,
    pub last_modified: i64,
    pub starred_songs: HashSet<String>,
    pub starred_albums: HashSet<String>,
    pub starred_artists: HashSet<String>,
    /// `(song id, submission, time)` per scrobbled id.
    pub scrobbles: Vec<(String, bool, Option<i64>)>,
    audio: HashMap<String, Audio>,
    /// Cover bytes served as-is instead of a generated PNG.
    covers: HashMap<String, Vec<u8>>,
}

impl FakeLibrary {
    /// `albums` albums of `songs_per_album` songs, two albums per artist. Ids are `ar-<n>`,
    /// `al-<n>`, `tr-<n>` (songs numbered across the library), covers `al-<n>_cover`. Two
    /// playlists when there are songs.
    pub fn synthetic(albums: usize, songs_per_album: usize) -> FakeLibrary {
        let mut lib = FakeLibrary {
            last_modified: FIRST_MODIFIED,
            ..FakeLibrary::default()
        };
        let genres = ["Rock", "Jazz", "Ambient"];
        let mut song_number = 0;
        for a in 0..albums {
            let artist_index = a / 2;
            let artist_id = format!("ar-{artist_index}");
            let artist_name = format!("Demo Artist {}", artist_index + 1);
            if a % 2 == 0 {
                lib.artists.push(ArtistId3 {
                    id: artist_id.clone(),
                    name: artist_name.clone(),
                    starred: None,
                });
            }
            let album_id = format!("al-{a}");
            let album_name = format!("Demo Album {}", a + 1);
            let cover = format!("al-{a}_cover");
            let year = 2000 + (a % 20) as u32;
            let genre = genres[a % genres.len()].to_owned();
            let mut duration = 0;
            for t in 0..songs_per_album {
                let seconds = 180 + (song_number % 120) as u32;
                duration += seconds;
                lib.songs.push(Child {
                    id: format!("tr-{song_number}"),
                    title: format!("Demo Song {}", song_number + 1),
                    album: Some(album_name.clone()),
                    artist: Some(artist_name.clone()),
                    album_id: Some(album_id.clone()),
                    artist_id: Some(artist_id.clone()),
                    track: Some(t as u32 + 1),
                    disc_number: Some(1),
                    year: Some(year),
                    genre: Some(genre.clone()),
                    cover_art: Some(cover.clone()),
                    suffix: Some("mp3".into()),
                    content_type: Some("audio/mpeg".into()),
                    duration: Some(seconds),
                    bit_rate: Some(320),
                    sampling_rate: Some(44_100),
                    bit_depth: Some(0),
                    display_album_artist: Some(artist_name.clone()),
                    album_artists: vec![ArtistRef {
                        id: artist_id.clone(),
                        name: artist_name.clone(),
                    }],
                    replay_gain: Some(ReplayGainInfo {
                        track_gain: Some(-6.5),
                        album_gain: Some(-7.0),
                        track_peak: Some(0.98),
                        album_peak: Some(0.99),
                        ..ReplayGainInfo::default()
                    }),
                    starred: None,
                });
                song_number += 1;
            }
            lib.albums.push(AlbumId3 {
                id: album_id,
                name: album_name,
                artist: Some(artist_name),
                artist_id: Some(artist_id),
                cover_art: Some(cover),
                song_count: songs_per_album as u32,
                duration,
                year: Some(year),
                genre: Some(genre),
                starred: None,
            });
        }
        if !lib.songs.is_empty() {
            let first: Vec<String> = lib.songs.iter().take(5).map(|s| s.id.clone()).collect();
            let every_seventh: Vec<String> =
                lib.songs.iter().step_by(7).map(|s| s.id.clone()).collect();
            for (i, song_ids) in [first, every_seventh].into_iter().enumerate() {
                lib.playlists.push(FakePlaylist {
                    summary: PlaylistSummary {
                        id: format!("pl-{i}"),
                        name: format!("Demo Playlist {}", i + 1),
                        owner: Some(FAKE_USER.into()),
                        changed: Some("2026-01-01T00:00:00Z".into()),
                        ..PlaylistSummary::default()
                    },
                    song_ids,
                });
            }
        }
        lib
    }

    /// Serves these bytes as the song's stream.
    /// Serves these bytes (as `image/jpeg`) for `getCoverArt?id=<cover_id>`.
    pub fn with_cover(mut self, cover_id: &str, bytes: Vec<u8>) -> FakeLibrary {
        self.covers.insert(cover_id.to_owned(), bytes);
        self
    }

    pub fn with_audio(mut self, song_id: &str, bytes: Vec<u8>) -> FakeLibrary {
        self.audio.insert(song_id.to_owned(), Audio::Bytes(bytes));
        self
    }

    /// Serves a generated sine WAV of `seconds` (44.1 kHz, stereo, 16 bit) as the song's stream
    /// and sets the song's duration, suffix and content type to match. Honors `timeOffset`.
    pub fn with_wav(mut self, song_id: &str, seconds: u32) -> FakeLibrary {
        self.audio
            .insert(song_id.to_owned(), Audio::Wav { seconds });
        if let Some(song) = self.songs.iter_mut().find(|s| s.id == song_id) {
            song.duration = Some(seconds);
            song.suffix = Some("wav".into());
            song.content_type = Some("audio/wav".into());
            song.bit_rate = Some(WAV_RATE * u32::from(WAV_CHANNELS) * 16 / 1000);
        }
        self
    }

    fn starred_song(&self, song: &Child) -> Child {
        let mut song = song.clone();
        song.starred = self
            .starred_songs
            .contains(&song.id)
            .then(|| STARRED_AT.to_owned());
        song
    }

    fn starred_album(&self, album: &AlbumId3) -> AlbumId3 {
        let mut album = album.clone();
        album.starred = self
            .starred_albums
            .contains(&album.id)
            .then(|| STARRED_AT.to_owned());
        album
    }

    fn songs_of(&self, ids: &[String]) -> Vec<Child> {
        ids.iter()
            .filter_map(|id| self.songs.iter().find(|s| &s.id == id))
            .map(|s| self.starred_song(s))
            .collect()
    }

    fn playlist_summary(&self, playlist: &FakePlaylist) -> PlaylistSummary {
        let songs = self.songs_of(&playlist.song_ids);
        PlaylistSummary {
            song_count: songs.len() as u32,
            duration: songs.iter().filter_map(|s| s.duration).sum(),
            ..playlist.summary.clone()
        }
    }

    fn is_cover(&self, id: &str) -> bool {
        self.albums
            .iter()
            .any(|a| a.cover_art.as_deref() == Some(id))
            || self
                .songs
                .iter()
                .any(|s| s.cover_art.as_deref() == Some(id))
            || self
                .playlists
                .iter()
                .any(|p| p.summary.cover_art.as_deref() == Some(id))
    }
}

struct State {
    lib: FakeLibrary,
    fault: Fault,
    redirect: Option<Redirect>,
    gzip: bool,
    corrupt_gzip: bool,
    gzipped: usize,
    requests: Vec<Recorded>,
}

struct Shared {
    state: Mutex<State>,
    /// Path prefix (`""` or `/music`).
    prefix: String,
    port: u16,
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

/// The fake server. Stops when dropped.
pub(crate) struct FakeServer {
    shared: Arc<Shared>,
    running: Mutex<Option<Running>>,
    /// Stopped by [`Fault::Refuse`] (restarted by the next other fault).
    refused: AtomicBool,
}

impl FakeServer {
    pub fn start(lib: FakeLibrary) -> FakeServer {
        FakeServer::start_at(lib, "")
    }

    /// Serves under a path prefix, as behind a reverse proxy (`/music`).
    pub fn start_at(lib: FakeLibrary, prefix: &str) -> FakeServer {
        let listener = TcpListener::bind("127.0.0.1:0").expect("binding the fake server");
        let port = listener.local_addr().expect("fake server address").port();
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                lib,
                fault: Fault::None,
                redirect: None,
                gzip: false,
                corrupt_gzip: false,
                gzipped: 0,
                requests: Vec::new(),
            }),
            prefix: prefix.trim_end_matches('/').to_owned(),
            port,
        });
        FakeServer {
            running: Mutex::new(Some(serve(listener, Arc::clone(&shared)))),
            shared,
            refused: AtomicBool::new(false),
        }
    }

    /// `http://127.0.0.1:<port><prefix>`.
    pub fn url(&self) -> String {
        format!(
            "http://127.0.0.1:{}{}",
            self.shared.port, self.shared.prefix
        )
    }

    /// The fake account on this server.
    pub fn account(&self) -> Account {
        Account {
            url: self.url(),
            user: FAKE_USER.to_owned(),
            password: Secret::new(FAKE_PASSWORD),
        }
    }

    /// Every request that reached a Subsonic method, in order.
    pub fn requests(&self) -> Vec<Recorded> {
        self.shared.state().requests.clone()
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

    pub fn set_redirect(&self, redirect: Option<Redirect>) {
        self.shared.state().redirect = redirect;
    }

    /// Gzips JSON responses for clients that accept gzip, as a compressing proxy would.
    pub fn set_gzip(&self, on: bool) {
        self.shared.state().gzip = on;
    }

    /// Marks JSON responses `Content-Encoding: gzip` without gzipping them (a broken proxy).
    pub fn set_corrupt_gzip(&self, on: bool) {
        self.shared.state().corrupt_gzip = on;
    }

    /// How many responses went out gzipped.
    pub fn gzipped_responses(&self) -> usize {
        self.shared.state().gzipped
    }

    /// Changes the library and bumps `lastModified`.
    pub fn edit(&self, change: impl FnOnce(&mut FakeLibrary)) {
        let mut state = self.shared.state();
        change(&mut state.lib);
        state.lib.last_modified += 60_000;
    }

    /// Starred `(songs, albums, artists)`.
    pub fn starred(&self) -> (HashSet<String>, HashSet<String>, HashSet<String>) {
        let state = self.shared.state();
        (
            state.lib.starred_songs.clone(),
            state.lib.starred_albums.clone(),
            state.lib.starred_artists.clone(),
        )
    }

    pub fn scrobbles(&self) -> Vec<(String, bool, Option<i64>)> {
        self.shared.state().lib.scrobbles.clone()
    }

    /// Stops listening; the port stays this server's for [`Self::restart`].
    pub fn stop(&self) {
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
                    assert!(attempts < 100, "rebinding the fake server: {err}");
                    std::thread::sleep(Duration::from_millis(20));
                }
            }
        };
        *self.running.lock().unwrap_or_else(|e| e.into_inner()) =
            Some(serve(listener, Arc::clone(&self.shared)));
    }
}

impl Drop for FakeServer {
    fn drop(&mut self) {
        self.stop();
    }
}

fn serve(listener: TcpListener, shared: Arc<Shared>) -> Running {
    let stop = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&stop);
    listener
        .set_nonblocking(true)
        .expect("non-blocking fake server");
    let thread = std::thread::spawn(move || {
        while !flag.load(Ordering::SeqCst) {
            match listener.accept() {
                Ok((stream, _)) => {
                    let shared = Arc::clone(&shared);
                    std::thread::spawn(move || handle(stream, &shared));
                }
                Err(_) => std::thread::sleep(Duration::from_millis(5)),
            }
        }
    });
    Running { stop, thread }
}

/// One response before it goes out.
struct Reply {
    status: u16,
    content_type: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Reply {
    fn new(status: u16, content_type: &str, body: Vec<u8>) -> Reply {
        Reply {
            status,
            content_type: content_type.to_owned(),
            headers: Vec::new(),
            body,
        }
    }

    fn json(value: Value) -> Reply {
        Reply::new(200, "application/json", value.to_string().into_bytes())
    }

    fn html(status: u16) -> Reply {
        Reply::new(
            status,
            "text/html; charset=utf-8",
            format!("<!doctype html><html><head><title>{status}</title></head><body>Fake proxy page</body></html>").into_bytes(),
        )
    }
}

/// How a reply is written.
enum Delivery {
    Whole,
    Stall { after: usize, wait: Duration },
    Cut { after: usize },
}

fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        206 => "Partial Content",
        301 => "Moved Permanently",
        302 => "Found",
        307 => "Temporary Redirect",
        308 => "Permanent Redirect",
        401 => "Unauthorized",
        404 => "Not Found",
        416 => "Range Not Satisfiable",
        500 => "Internal Server Error",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        504 => "Gateway Timeout",
        _ => "Status",
    }
}

/// Reads the request head (no request bodies: every Subsonic call here is a GET).
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

fn handle(mut stream: TcpStream, shared: &Shared) {
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let Some(head) = read_head(&mut stream) else {
        return;
    };
    let mut lines = head.split("\r\n");
    let target = lines
        .next()
        .and_then(|line| line.split(' ').nth(1))
        .unwrap_or("/")
        .to_owned();
    let mut range = None;
    let mut accepts_gzip = false;
    for line in lines {
        if let Some((name, value)) = line.split_once(':') {
            let value = value.trim();
            if name.eq_ignore_ascii_case("range") {
                range = Some(value.to_owned());
            } else if name.eq_ignore_ascii_case("accept-encoding") {
                accepts_gzip = value.contains("gzip");
            }
        }
    }
    let Ok(url) = Url::parse(&format!("http://fake{target}")) else {
        return;
    };
    let (reply, delivery) = route(shared, &url, range, accepts_gzip);
    write_reply(&mut stream, &reply, delivery);
}

fn route(
    shared: &Shared,
    url: &Url,
    range: Option<String>,
    accepts_gzip: bool,
) -> (Reply, Delivery) {
    let mut state = shared.state();
    let path = url.path();
    let query = url.query().map(|q| format!("?{q}")).unwrap_or_default();

    match &state.redirect {
        Some(Redirect::To(status, base)) => {
            let mut reply = Reply::html(*status);
            reply
                .headers
                .push(("Location".into(), format!("{base}{path}{query}")));
            return (reply, Delivery::Whole);
        }
        Some(Redirect::ToPrefix(status))
            if !shared.prefix.is_empty() && path.starts_with("/rest/") =>
        {
            let mut reply = Reply::html(*status);
            reply
                .headers
                .push(("Location".into(), format!("{}{path}{query}", shared.prefix)));
            return (reply, Delivery::Whole);
        }
        _ => {}
    }

    let rest_root = format!("{}/rest/", shared.prefix);
    let Some(method) = path.strip_prefix(rest_root.as_str()) else {
        return (
            Reply::new(404, "text/plain", b"404 page not found".to_vec()),
            Delivery::Whole,
        );
    };
    let method = method.trim_end_matches(".view").to_owned();
    let params: Vec<(String, String)> = url
        .query_pairs()
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    state.requests.push(Recorded {
        method: method.clone(),
        params: params.clone(),
        range: range.clone(),
    });

    let mut delivery = Delivery::Whole;
    match state.fault.clone() {
        Fault::Status(status) => return (Reply::html(status), delivery),
        Fault::Html => return (Reply::html(200), delivery),
        Fault::Code(code) => return (Reply::json(failed(code)), delivery),
        Fault::StatusWithCode { status, code } => {
            let mut reply = Reply::json(failed(code));
            reply.status = status;
            return (reply, delivery);
        }
        Fault::WrongPassword => return (Reply::json(failed(40)), delivery),
        Fault::Stall {
            method: stalled,
            after_bytes,
            wait,
        } if stalled == method => {
            delivery = Delivery::Stall {
                after: after_bytes,
                wait,
            };
        }
        Fault::CutOnce {
            method: cut,
            after_bytes,
        } if cut == method => {
            state.fault = Fault::None;
            delivery = Delivery::Cut { after: after_bytes };
        }
        Fault::CutOnceNoRanges {
            method: cut,
            after_bytes,
        } if cut == method => {
            state.fault = Fault::NoRanges;
            delivery = Delivery::Cut { after: after_bytes };
        }
        _ => {}
    }

    if !authenticated(&params) {
        return (Reply::json(failed(40)), delivery);
    }
    let mut reply = answer(&mut state, &method, &params, range.as_deref());
    if state.corrupt_gzip && reply.content_type.contains("json") {
        // Claims gzip, sends something else: the client's decoder fails.
        reply.body = b"this is not gzip at all, just plain bytes".to_vec();
        reply
            .headers
            .push(("Content-Encoding".into(), "gzip".into()));
    } else if state.gzip && accepts_gzip && reply.content_type.contains("json") {
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(&reply.body).expect("gzip in memory");
        reply.body = encoder.finish().expect("gzip in memory");
        reply
            .headers
            .push(("Content-Encoding".into(), "gzip".into()));
        state.gzipped += 1;
    }
    (reply, delivery)
}

fn param<'a>(params: &'a [(String, String)], key: &str) -> Option<&'a str> {
    params
        .iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.as_str())
}

fn params_all<'a>(params: &'a [(String, String)], key: &'a str) -> impl Iterator<Item = &'a str> {
    params
        .iter()
        .filter(move |(k, _)| k == key)
        .map(|(_, v)| v.as_str())
}

fn number(params: &[(String, String)], key: &str, default: usize) -> usize {
    param(params, key)
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

/// `u` is the fake user and `t` is md5(`demo-pass` + `s`).
fn authenticated(params: &[(String, String)]) -> bool {
    let (Some(user), Some(t), Some(s)) =
        (param(params, "u"), param(params, "t"), param(params, "s"))
    else {
        return false;
    };
    user == FAKE_USER && t == token(FAKE_PASSWORD, s)
}

fn base_response() -> Map<String, Value> {
    let mut map = Map::new();
    map.insert("status".into(), "ok".into());
    map.insert("version".into(), API_VERSION.into());
    map.insert("type".into(), "navidrome".into());
    map.insert("serverVersion".into(), "0.58.0 (fake)".into());
    map.insert("openSubsonic".into(), true.into());
    map
}

fn ok(payload: Value) -> Value {
    let mut response = base_response();
    if let Value::Object(fields) = payload {
        response.extend(fields);
    }
    json!({ "subsonic-response": response })
}

fn failed(code: u32) -> Value {
    let message = match code {
        40 => "Wrong username or password",
        41 => "Token authentication not supported for LDAP users",
        50 => "User is not authorized for the given operation",
        70 => "The requested data was not found",
        _ => "A generic error",
    };
    let mut response = base_response();
    response.insert("status".into(), "failed".into());
    response.insert("error".into(), json!({ "code": code, "message": message }));
    json!({ "subsonic-response": response })
}

fn to_json<T: serde::Serialize>(value: &T) -> Value {
    serde_json::to_value(value).expect("fake data serializes")
}

fn answer(
    state: &mut State,
    method: &str,
    params: &[(String, String)],
    range: Option<&str>,
) -> Reply {
    let fault = state.fault.clone();
    let lib = &mut state.lib;
    let not_found = || Reply::json(failed(70));
    match method {
        "ping" => Reply::json(ok(json!({}))),
        "getIndexes" => {
            let mut index: Vec<(String, Vec<Value>)> = Vec::new();
            let mut artists = lib.artists.clone();
            artists.sort_by(|a, b| a.name.cmp(&b.name));
            for artist in &artists {
                let letter = artist
                    .name
                    .chars()
                    .next()
                    .map(|c| c.to_uppercase().to_string())
                    .unwrap_or_else(|| "#".into());
                let entry = json!({ "id": artist.id, "name": artist.name, "albumCount": 2 });
                match index.last_mut() {
                    Some((name, list)) if *name == letter => list.push(entry),
                    _ => index.push((letter, vec![entry])),
                }
            }
            let index: Vec<Value> = index
                .into_iter()
                .map(|(name, artist)| json!({ "name": name, "artist": artist }))
                .collect();
            Reply::json(ok(json!({ "indexes": {
                "lastModified": lib.last_modified,
                "ignoredArticles": "The El La Los Las Le Les",
                "index": index,
            }})))
        }
        "getAlbumList2" => {
            let size = number(params, "size", 10).min(500);
            let offset = number(params, "offset", 0);
            let mut albums: Vec<AlbumId3> =
                lib.albums.iter().map(|a| lib.starred_album(a)).collect();
            albums.sort_by(|a, b| a.name.cmp(&b.name).then(a.id.cmp(&b.id)));
            let page: Vec<Value> = albums.iter().skip(offset).take(size).map(to_json).collect();
            Reply::json(ok(json!({ "albumList2": { "album": page } })))
        }
        "getAlbum" => {
            let id = param(params, "id").unwrap_or_default();
            let Some(album) = lib.albums.iter().find(|a| a.id == id) else {
                return not_found();
            };
            let mut album_json = to_json(&lib.starred_album(album));
            let songs: Vec<Value> = lib
                .songs
                .iter()
                .filter(|s| s.album_id.as_deref() == Some(id))
                .map(|s| to_json(&lib.starred_song(s)))
                .collect();
            album_json["song"] = Value::Array(songs);
            Reply::json(ok(json!({ "album": album_json })))
        }
        "search3" => {
            let count = number(params, "songCount", 20);
            let offset = number(params, "songOffset", 0);
            let songs: Vec<Value> = if matches!(fault, Fault::Search3Empty) {
                Vec::new()
            } else {
                lib.songs
                    .iter()
                    .skip(offset)
                    .take(count)
                    .map(|s| to_json(&lib.starred_song(s)))
                    .collect()
            };
            Reply::json(ok(json!({ "searchResult3": { "song": songs } })))
        }
        "getStarred2" => {
            let songs: Vec<Value> = lib
                .songs
                .iter()
                .filter(|s| lib.starred_songs.contains(&s.id))
                .map(|s| to_json(&lib.starred_song(s)))
                .collect();
            let albums: Vec<Value> = lib
                .albums
                .iter()
                .filter(|a| lib.starred_albums.contains(&a.id))
                .map(|a| to_json(&lib.starred_album(a)))
                .collect();
            let artists: Vec<Value> = lib
                .artists
                .iter()
                .filter(|a| lib.starred_artists.contains(&a.id))
                .map(|a| {
                    to_json(&ArtistId3 {
                        starred: Some(STARRED_AT.into()),
                        ..a.clone()
                    })
                })
                .collect();
            Reply::json(ok(json!({ "starred2": {
                "song": songs, "album": albums, "artist": artists,
            }})))
        }
        "star" | "unstar" => {
            let on = method == "star";
            let apply = |set: &mut HashSet<String>, id: &str| {
                if on {
                    set.insert(id.to_owned());
                } else {
                    set.remove(id);
                }
            };
            for id in params_all(params, "id") {
                apply(&mut lib.starred_songs, id);
            }
            for id in params_all(params, "albumId") {
                apply(&mut lib.starred_albums, id);
            }
            for id in params_all(params, "artistId") {
                apply(&mut lib.starred_artists, id);
            }
            Reply::json(ok(json!({})))
        }
        "scrobble" => {
            let submission = param(params, "submission") != Some("false");
            let time = param(params, "time").and_then(|t| t.parse().ok());
            for id in params_all(params, "id") {
                lib.scrobbles.push((id.to_owned(), submission, time));
            }
            Reply::json(ok(json!({})))
        }
        "getPlaylists" => {
            let playlists: Vec<Value> = lib
                .playlists
                .iter()
                .map(|p| to_json(&lib.playlist_summary(p)))
                .collect();
            Reply::json(ok(json!({ "playlists": { "playlist": playlists } })))
        }
        "getPlaylist" => {
            let id = param(params, "id").unwrap_or_default();
            let Some(playlist) = lib.playlists.iter().find(|p| p.summary.id == id) else {
                return not_found();
            };
            let full = PlaylistWithSongs {
                summary: lib.playlist_summary(playlist),
                entry: lib.songs_of(&playlist.song_ids),
            };
            Reply::json(ok(json!({ "playlist": to_json(&full) })))
        }
        "getCoverArt" => {
            let id = param(params, "id").unwrap_or_default();
            if let Some(bytes) = lib.covers.get(id) {
                return Reply::new(200, "image/jpeg", bytes.clone());
            }
            if !lib.is_cover(id) {
                return not_found();
            }
            let size = number(params, "size", 300).clamp(1, 1024) as u32;
            Reply::new(200, "image/png", cover_png(id, size))
        }
        "stream" => {
            let id = param(params, "id").unwrap_or_default();
            let Some(song) = lib.songs.iter().find(|s| s.id == id) else {
                return not_found();
            };
            let content_type = song
                .content_type
                .clone()
                .unwrap_or_else(|| "application/octet-stream".into());
            let offset = param(params, "timeOffset").and_then(|t| t.parse::<u32>().ok());
            let (bytes, seekable) = match (lib.audio.get(id), offset) {
                (None, _) => return not_found(),
                (Some(Audio::Bytes(bytes)), _) => (bytes.clone(), true),
                (Some(Audio::Wav { seconds }), None) => (wav_bytes(*seconds, 0), true),
                // A transcoded start: a fresh stream, not byte-seekable.
                (Some(Audio::Wav { seconds }), Some(offset)) => {
                    (wav_bytes(*seconds, offset.min(*seconds)), false)
                }
            };
            let ranges = seekable && !matches!(fault, Fault::NoRanges);
            stream_reply(
                bytes,
                &content_type,
                if ranges { range } else { None },
                ranges,
            )
        }
        _ => Reply::new(404, "text/plain", b"404 page not found".to_vec()),
    }
}

/// A full (200) or partial (206) stream reply for `bytes=<start>-[<end>]`.
fn stream_reply(bytes: Vec<u8>, content_type: &str, range: Option<&str>, ranges: bool) -> Reply {
    let total = bytes.len();
    let parsed = range.and_then(|r| r.strip_prefix("bytes=")).and_then(|r| {
        let (start, end) = r.split_once('-')?;
        let start: usize = start.trim().parse().ok()?;
        let end: usize = match end.trim() {
            "" => total.saturating_sub(1),
            end => end.parse::<usize>().ok()?.min(total.saturating_sub(1)),
        };
        Some((start, end))
    });
    let mut reply = match parsed {
        Some((start, _)) if start >= total => {
            let mut reply = Reply::new(416, "text/plain", Vec::new());
            reply
                .headers
                .push(("Content-Range".into(), format!("bytes */{total}")));
            reply
        }
        Some((start, end)) if start <= end => {
            let mut reply = Reply::new(206, content_type, bytes[start..=end].to_vec());
            reply.headers.push((
                "Content-Range".into(),
                format!("bytes {start}-{end}/{total}"),
            ));
            reply
        }
        _ => Reply::new(200, content_type, bytes),
    };
    if ranges {
        reply.headers.push(("Accept-Ranges".into(), "bytes".into()));
    }
    reply
}

/// A square PNG of one color derived from the id.
fn cover_png(id: &str, size: u32) -> Vec<u8> {
    let hash = id.bytes().fold(2_166_136_261u32, |h, b| {
        (h ^ u32::from(b)).wrapping_mul(16_777_619)
    });
    let [r, g, b, _] = hash.to_le_bytes();
    let image = image::RgbImage::from_pixel(size, size, image::Rgb([r, g, b]));
    let mut out = Cursor::new(Vec::new());
    image
        .write_to(&mut out, image::ImageFormat::Png)
        .expect("encoding the fake cover");
    out.into_inner()
}

/// A 440 Hz sine WAV of `seconds`, starting `offset` seconds in (phase continuous).
fn wav_bytes(seconds: u32, offset: u32) -> Vec<u8> {
    let frames = (seconds - offset) * WAV_RATE;
    let block = u32::from(WAV_CHANNELS) * 2;
    let data_len = frames * block;
    let mut out = Vec::with_capacity(44 + data_len as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&WAV_CHANNELS.to_le_bytes());
    out.extend_from_slice(&WAV_RATE.to_le_bytes());
    out.extend_from_slice(&(WAV_RATE * block).to_le_bytes());
    out.extend_from_slice(&(block as u16).to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    let first = u64::from(offset) * u64::from(WAV_RATE);
    for frame in 0..u64::from(frames) {
        let t = (first + frame) as f64 / f64::from(WAV_RATE);
        let sample =
            ((t * 440.0 * std::f64::consts::TAU).sin() * 0.25 * f64::from(i16::MAX)) as i16;
        for _ in 0..WAV_CHANNELS {
            out.extend_from_slice(&sample.to_le_bytes());
        }
    }
    out
}

fn write_reply(stream: &mut TcpStream, reply: &Reply, delivery: Delivery) {
    let mut head = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n",
        reply.status,
        reason(reply.status),
        reply.content_type,
        reply.body.len()
    );
    for (name, value) in &reply.headers {
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    head.push_str("\r\n");
    let head = head.into_bytes();
    let body = &reply.body;
    // Write errors mean the client gave up (a timeout test); nothing to do about them.
    match delivery {
        Delivery::Whole => {
            let _ = stream.write_all(&head);
            let _ = stream.write_all(body);
        }
        Delivery::Stall { after, wait } => {
            let after = after.min(body.len());
            if after > 0 {
                let _ = stream.write_all(&head);
                let _ = stream.write_all(&body[..after]);
                let _ = stream.flush();
                std::thread::sleep(wait);
                let _ = stream.write_all(&body[after..]);
            } else {
                std::thread::sleep(wait);
                let _ = stream.write_all(&head);
                let _ = stream.write_all(body);
            }
        }
        Delivery::Cut { after } => {
            let _ = stream.write_all(&head);
            let _ = stream.write_all(&body[..after.min(body.len())]);
            let _ = stream.flush();
            let _ = stream.shutdown(Shutdown::Both);
            return;
        }
    }
    let _ = stream.flush();
    let _ = stream.shutdown(Shutdown::Write);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::navidrome::client::{StreamQuality, SubsonicClient};

    #[test]
    fn wav_offset_shortens_the_stream() {
        let full = wav_bytes(3, 0);
        let tail = wav_bytes(3, 1);
        assert_eq!(full.len() - 44, 3 * 44_100 * 4);
        assert_eq!(tail.len() - 44, 2 * 44_100 * 4);
        // Phase continuous: the tail's first frame is the full stream's frame at 1 s.
        let at_one = 44 + 44_100 * 4;
        assert_eq!(&tail[44..48], &full[at_one..at_one + 4]);
    }

    #[test]
    fn stream_honors_ranges_cuts_and_no_ranges() {
        let lib = FakeLibrary::synthetic(1, 2)
            .with_audio("tr-0", (0..=255u8).cycle().take(4_000).collect())
            .with_wav("tr-1", 4);
        let server = FakeServer::start(lib);
        let client = SubsonicClient::new(server.account()).unwrap();
        let http = client.stream_http();
        let get = |id: &str, offset: Option<u32>, range: Option<&str>| {
            let url = client.stream_url(id, StreamQuality::Original, offset);
            let mut request = http.get(url.expose());
            if let Some(range) = range {
                request = request.header("Range", range);
            }
            request.send().unwrap()
        };

        let response = get("tr-0", None, None);
        assert_eq!(response.status().as_u16(), 200);
        assert_eq!(
            response.headers()["accept-ranges"].to_str().unwrap(),
            "bytes"
        );
        assert_eq!(response.bytes().unwrap().len(), 4_000);

        let response = get("tr-0", None, Some("bytes=10-19"));
        assert_eq!(response.status().as_u16(), 206);
        assert_eq!(
            response.headers()["content-range"].to_str().unwrap(),
            "bytes 10-19/4000"
        );
        assert_eq!(
            response.bytes().unwrap().to_vec(),
            (10..20u8).collect::<Vec<_>>()
        );

        let response = get("tr-1", Some(1), None);
        assert_eq!(response.status().as_u16(), 200);
        assert!(response.headers().get("accept-ranges").is_none());
        assert_eq!(response.bytes().unwrap().len(), 44 + 3 * 44_100 * 4);

        server.set_fault(Fault::NoRanges);
        let response = get("tr-0", None, Some("bytes=10-"));
        assert_eq!(response.status().as_u16(), 200);
        assert!(response.headers().get("accept-ranges").is_none());

        server.set_fault(Fault::CutOnce {
            method: "stream",
            after_bytes: 100,
        });
        assert!(get("tr-0", None, None).bytes().is_err());
        assert_eq!(get("tr-0", None, None).bytes().unwrap().len(), 4_000);
    }

    /// Serves a fake library for the UI tasks' visual checks:
    /// `cargo test --manifest-path src-tauri/Cargo.toml serve_for_ui -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn serve_for_ui() {
        let mut lib = FakeLibrary::synthetic(60, 10);
        let songs: Vec<(String, u32)> = lib
            .songs
            .iter()
            .map(|s| (s.id.clone(), s.duration.unwrap_or(180)))
            .collect();
        for (id, seconds) in songs {
            lib = lib.with_wav(&id, seconds);
        }
        let server = FakeServer::start(lib);
        println!("fake Navidrome at {}", server.url());
        println!("user {FAKE_USER}, password {FAKE_PASSWORD}");
        let minutes = std::env::var("PRISM_FAKE_SERVER_MINUTES")
            .ok()
            .and_then(|m| m.parse().ok())
            .unwrap_or(10);
        std::thread::sleep(Duration::from_secs(60 * minutes));
    }
}
