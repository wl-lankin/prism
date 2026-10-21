//! Fake servers for the radio tests, hand-rolled on a plain `TcpListener` bound to `127.0.0.1`
//! (one request per connection, `Connection: close`), like the fake Subsonic server.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

/// Panics unless `url` points at this machine: a test that would leave it fails loudly.
#[track_caller]
pub(crate) fn assert_loopback(url: &str) {
    let host = reqwest::Url::parse(url)
        .ok()
        .and_then(|url| url.host_str().map(str::to_owned));
    let ip = host
        .as_deref()
        .map(|host| host.trim_start_matches('[').trim_end_matches(']'))
        .and_then(|host| host.parse::<std::net::IpAddr>().ok());
    let local = host.as_deref() == Some("localhost")
        || ip.is_some_and(|ip| match ip {
            std::net::IpAddr::V4(v4) => v4.is_loopback(),
            std::net::IpAddr::V6(v6) => {
                v6.is_loopback() || v6.to_ipv4_mapped().is_some_and(|v4| v4.is_loopback())
            }
        });
    assert!(local, "a test request would leave the machine: {url}");
}

fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        302 => "Found",
        404 => "Not Found",
        500 => "Internal Server Error",
        503 => "Service Unavailable",
        _ => "Status",
    }
}

/// Reads the request head and returns the path without its query.
fn read_path(stream: &mut TcpStream) -> Option<String> {
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let mut head = Vec::new();
    let mut buf = [0u8; 2048];
    while !head.windows(4).any(|w| w == b"\r\n\r\n") {
        let n = stream.read(&mut buf).ok()?;
        if n == 0 || head.len() > 16 * 1024 {
            return None;
        }
        head.extend_from_slice(&buf[..n]);
    }
    let head = String::from_utf8(head).ok()?;
    let target = head.lines().next()?.split(' ').nth(1)?.to_owned();
    Some(target.split('?').next().unwrap_or("/").to_owned())
}

fn write_reply(stream: &mut TcpStream, status: u16, body: &str) {
    let head = format!(
        "HTTP/1.1 {status} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        reason(status),
        body.len()
    );
    // A client that gave up is not the server's problem.
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(body.as_bytes());
}

/// Path of the fake GeoJS endpoint (the `FakeGeo` section starts here).
const GEOJS_PATH: &str = "/v1/ip/geo.json";
/// Path of the fake ipinfo endpoint.
const IPINFO_PATH: &str = "/json";

struct GeoState {
    answers: HashMap<&'static str, (u16, String)>,
    hits: HashMap<&'static str, usize>,
}

/// A fake location service with the two endpoints Prism asks. Both answer the Testhausen
/// fixtures until a test changes them. Stops when dropped.
pub(crate) struct FakeGeo {
    port: u16,
    state: Arc<Mutex<GeoState>>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl FakeGeo {
    pub fn start() -> FakeGeo {
        let listener = TcpListener::bind("127.0.0.1:0").expect("binding the fake geo server");
        let port = listener.local_addr().expect("fake geo address").port();
        listener
            .set_nonblocking(true)
            .expect("non-blocking fake geo server");
        let state = Arc::new(Mutex::new(GeoState {
            answers: HashMap::from([
                (
                    GEOJS_PATH,
                    (
                        200,
                        include_str!("../../tests/fixtures/radio/geojs.json").to_owned(),
                    ),
                ),
                (
                    IPINFO_PATH,
                    (
                        200,
                        include_str!("../../tests/fixtures/radio/ipinfo.json").to_owned(),
                    ),
                ),
            ]),
            hits: HashMap::new(),
        }));
        let stop = Arc::new(AtomicBool::new(false));
        let thread = {
            let state = Arc::clone(&state);
            let stop = Arc::clone(&stop);
            std::thread::spawn(move || {
                while !stop.load(Ordering::SeqCst) {
                    match listener.accept() {
                        Ok((mut stream, _)) => {
                            let state = Arc::clone(&state);
                            std::thread::spawn(move || {
                                let Some(path) = read_path(&mut stream) else {
                                    return;
                                };
                                let answer = {
                                    let mut state = state.lock().unwrap_or_else(|e| e.into_inner());
                                    let key = [GEOJS_PATH, IPINFO_PATH]
                                        .into_iter()
                                        .find(|known| *known == path);
                                    key.map(|key| {
                                        *state.hits.entry(key).or_default() += 1;
                                        state.answers[key].clone()
                                    })
                                };
                                match answer {
                                    Some((status, body)) => write_reply(&mut stream, status, &body),
                                    None => write_reply(&mut stream, 404, "{}"),
                                }
                            });
                        }
                        Err(_) => std::thread::sleep(Duration::from_millis(5)),
                    }
                }
            })
        };
        FakeGeo {
            port,
            state,
            stop,
            thread: Some(thread),
        }
    }

    pub fn geojs_url(&self) -> String {
        format!("http://127.0.0.1:{}{GEOJS_PATH}", self.port)
    }

    pub fn ipinfo_url(&self) -> String {
        format!("http://127.0.0.1:{}{IPINFO_PATH}", self.port)
    }

    pub fn set_geojs(&self, status: u16, body: &str) {
        self.set(GEOJS_PATH, status, body);
    }

    pub fn set_ipinfo(&self, status: u16, body: &str) {
        self.set(IPINFO_PATH, status, body);
    }

    fn set(&self, path: &'static str, status: u16, body: &str) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.answers.insert(path, (status, body.to_owned()));
    }

    pub fn geojs_hits(&self) -> usize {
        self.hits(GEOJS_PATH)
    }

    pub fn ipinfo_hits(&self) -> usize {
        self.hits(IPINFO_PATH)
    }

    fn hits(&self, path: &str) -> usize {
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.hits.get(path).copied().unwrap_or(0)
    }
}

impl Drop for FakeGeo {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

// ---------------------------------------------------------------------------------------------
// FakeDirectory: a Radio Browser look-alike
// ---------------------------------------------------------------------------------------------

use serde_json::{json, Value};

/// Where fake stations point their `url` unless a test says otherwise.
pub(crate) const DEFAULT_STREAM_BASE: &str = "http://stream.example.org";

const SERVERS_JSON: &str = include_str!("../../tests/fixtures/radio/servers.json");
const COUNTRIES_JSON: &str = include_str!("../../tests/fixtures/radio/countries.json");
const STATES_JSON: &str = include_str!("../../tests/fixtures/radio/states.json");
const TAGS_JSON: &str = include_str!("../../tests/fixtures/radio/tags.json");
const URL_CLICK_JSON: &str = include_str!("../../tests/fixtures/radio/url_click.json");

/// One fake station. Everything is fake: names "Demo Radio <n>", uuids made of `n`.
#[derive(Debug, Clone)]
pub(crate) struct FakeStation {
    pub n: u32,
    name: String,
    url: Option<String>,
    tags: String,
    country_code: String,
    state: String,
    codec: String,
    bitrate: u32,
    hls: bool,
    favicon: String,
    homepage: String,
    clicks: u32,
    votes: u32,
    geo: Option<(f64, f64)>,
    last_check_ok: bool,
}

impl FakeStation {
    pub fn new(n: u32) -> FakeStation {
        FakeStation {
            n,
            name: format!("Demo Radio {n}"),
            url: None,
            tags: "pop,hits".to_owned(),
            country_code: "DE".to_owned(),
            state: "Testland".to_owned(),
            codec: "MP3".to_owned(),
            bitrate: 128,
            hls: false,
            favicon: String::new(),
            homepage: String::new(),
            clicks: 0,
            votes: 0,
            geo: None,
            last_check_ok: true,
        }
    }

    pub fn uuid(&self) -> String {
        format!("00000000-0000-4000-8000-{:012x}", self.n)
    }

    pub fn id(&self) -> String {
        format!("radio:{}", self.uuid())
    }

    pub fn name(mut self, name: &str) -> FakeStation {
        self.name = name.to_owned();
        self
    }

    pub fn url(mut self, url: &str) -> FakeStation {
        self.url = Some(url.to_owned());
        self
    }

    pub fn tags(mut self, tags: &str) -> FakeStation {
        self.tags = tags.to_owned();
        self
    }

    pub fn country(mut self, code: &str) -> FakeStation {
        self.country_code = code.to_owned();
        self
    }

    pub fn region(mut self, state: &str) -> FakeStation {
        self.state = state.to_owned();
        self
    }

    pub fn codec(mut self, codec: &str, bitrate: u32) -> FakeStation {
        self.codec = codec.to_owned();
        self.bitrate = bitrate;
        self
    }

    pub fn hls(mut self) -> FakeStation {
        self.hls = true;
        self
    }

    pub fn favicon(mut self, url: &str) -> FakeStation {
        self.favicon = url.to_owned();
        self
    }

    pub fn homepage(mut self, url: &str) -> FakeStation {
        self.homepage = url.to_owned();
        self
    }

    pub fn clicks(mut self, clicks: u32) -> FakeStation {
        self.clicks = clicks;
        self
    }

    pub fn votes(mut self, votes: u32) -> FakeStation {
        self.votes = votes;
        self
    }

    pub fn geo(mut self, lat: f64, lon: f64) -> FakeStation {
        self.geo = Some((lat, lon));
        self
    }

    pub fn broken(mut self) -> FakeStation {
        self.last_check_ok = false;
        self
    }

    fn to_json(&self, stream_base: &str, distance_m: Option<f64>) -> Value {
        let url = self
            .url
            .clone()
            .unwrap_or_else(|| format!("{stream_base}/stream/{}", self.n));
        let mut station = json!({
            "changeuuid": format!("10000000-0000-4000-8000-{:012x}", self.n),
            "stationuuid": self.uuid(),
            "serveruuid": null,
            "name": self.name,
            "url": url,
            "url_resolved": url,
            "homepage": self.homepage,
            "favicon": self.favicon,
            "tags": self.tags,
            "country": match self.country_code.as_str() {
                "DE" => "Germany",
                "AT" => "Austria",
                "CH" => "Switzerland",
                other => other,
            },
            "countrycode": self.country_code,
            "iso_3166_2": "",
            "state": self.state,
            "language": "german",
            "languagecodes": "de",
            "votes": self.votes,
            "lastchangetime": "2026-10-01 10:00:00",
            "codec": self.codec,
            "bitrate": self.bitrate,
            "hls": u8::from(self.hls),
            "lastcheckok": u8::from(self.last_check_ok),
            "lastchecktime": "2026-10-20 10:00:00",
            "clickcount": self.clicks,
            "clicktrend": 0,
            "ssl_error": 0,
            "geo_lat": self.geo.map(|(lat, _)| lat),
            "geo_long": self.geo.map(|(_, lon)| lon),
            "has_extended_info": false,
        });
        if let Some(distance) = distance_m {
            station["geo_distance"] = json!(distance);
        }
        station
    }
}

/// What the fake directory does instead of answering normally.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Fault {
    None,
    /// Every request gets this status and an empty JSON object.
    Status(u16),
    /// The connection is accepted and closed without a reply.
    Refuse,
    /// The reply comes after this long.
    Stall(Duration),
    /// A 200 with an HTML page.
    Html,
    /// A 200 with a 9 MiB body.
    Huge,
    /// A 200 with a 9 MiB body and no Content-Length (the body ends when the connection closes).
    HugeUnsized,
}

/// One request the fake directory received.
#[derive(Debug, Clone)]
pub(crate) struct Recorded {
    pub path: String,
    pub params: Vec<(String, String)>,
    pub user_agent: Option<String>,
}

impl Recorded {
    pub fn param(&self, name: &str) -> Option<&str> {
        self.params
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }
}

struct DirectoryState {
    stations: Vec<FakeStation>,
    stream_base: String,
    fault: Fault,
    requests: Vec<Recorded>,
}

struct Request {
    path: String,
    params: Vec<(String, String)>,
    user_agent: Option<String>,
}

fn read_request(stream: &mut TcpStream) -> Option<Request> {
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let mut head = Vec::new();
    let mut buf = [0u8; 2048];
    while !head.windows(4).any(|w| w == b"\r\n\r\n") {
        let n = stream.read(&mut buf).ok()?;
        if n == 0 || head.len() > 16 * 1024 {
            return None;
        }
        head.extend_from_slice(&buf[..n]);
    }
    let head = String::from_utf8(head).ok()?;
    let mut lines = head.lines();
    let target = lines.next()?.split(' ').nth(1)?.to_owned();
    let user_agent = lines.find_map(|line| {
        let (name, value) = line.split_once(':')?;
        name.eq_ignore_ascii_case("user-agent")
            .then(|| value.trim().to_owned())
    });
    let url = reqwest::Url::parse(&format!("http://fake{target}")).ok()?;
    Some(Request {
        path: url.path().to_owned(),
        params: url
            .query_pairs()
            .map(|(key, value)| (key.into_owned(), value.into_owned()))
            .collect(),
        user_agent,
    })
}

fn haversine_m(a: (f64, f64), b: (f64, f64)) -> f64 {
    let (lat1, lon1) = (a.0.to_radians(), a.1.to_radians());
    let (lat2, lon2) = (b.0.to_radians(), b.1.to_radians());
    let h = ((lat2 - lat1) / 2.0).sin().powi(2)
        + lat1.cos() * lat2.cos() * ((lon2 - lon1) / 2.0).sin().powi(2);
    2.0 * 6_371_000.0 * h.sqrt().asin()
}

fn param<'a>(request: &'a Request, name: &str) -> Option<&'a str> {
    request
        .params
        .iter()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.as_str())
}

fn flag(request: &Request, name: &str) -> bool {
    param(request, name).is_some_and(|value| value == "true" || value == "1")
}

fn search(state: &DirectoryState, request: &Request) -> Value {
    let name = param(request, "name").map(str::to_lowercase);
    let code = param(request, "countrycode").map(str::to_lowercase);
    let region = param(request, "state").map(str::to_lowercase);
    let tag = param(request, "tag").map(str::to_lowercase);
    let geo = match (
        param(request, "geo_lat").and_then(|v| v.parse::<f64>().ok()),
        param(request, "geo_long").and_then(|v| v.parse::<f64>().ok()),
        param(request, "geo_distance").and_then(|v| v.parse::<f64>().ok()),
    ) {
        (Some(lat), Some(lon), Some(max)) => Some(((lat, lon), max)),
        _ => None,
    };
    let mut hits: Vec<(&FakeStation, Option<f64>)> = state
        .stations
        .iter()
        .filter(|s| !flag(request, "hidebroken") || s.last_check_ok)
        .filter(|s| {
            name.as_ref()
                .is_none_or(|n| s.name.to_lowercase().contains(n))
        })
        .filter(|s| {
            code.as_ref()
                .is_none_or(|c| s.country_code.to_lowercase() == *c)
        })
        .filter(|s| {
            region.as_ref().is_none_or(|r| {
                let have = s.state.to_lowercase();
                if flag(request, "stateExact") {
                    have == *r
                } else {
                    have.contains(r)
                }
            })
        })
        .filter(|s| {
            tag.as_ref().is_none_or(|wanted| {
                s.tags
                    .split(',')
                    .any(|t| t.trim().to_lowercase().contains(wanted))
            })
        })
        .filter(|s| !flag(request, "has_geo_info") || s.geo.is_some())
        .filter_map(|s| match geo {
            None => Some((s, None)),
            Some((origin, max)) => {
                let distance = haversine_m(origin, s.geo?);
                (distance <= max).then_some((s, Some(distance)))
            }
        })
        .collect();
    match param(request, "order").unwrap_or("name") {
        "clickcount" => hits.sort_by_key(|(s, _)| s.clicks),
        "votes" => hits.sort_by_key(|(s, _)| s.votes),
        "name" => hits.sort_by_key(|(s, _)| s.name.to_lowercase()),
        _ => {}
    }
    if flag(request, "reverse") {
        hits.reverse();
    }
    let offset = param(request, "offset")
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(0);
    let limit = param(request, "limit")
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(100_000);
    Value::Array(
        hits.into_iter()
            .skip(offset)
            .take(limit)
            .map(|(s, distance)| s.to_json(&state.stream_base, distance))
            .collect(),
    )
}

fn by_uuid(state: &DirectoryState, request: &Request) -> Value {
    let wanted: Vec<&str> = param(request, "uuids")
        .unwrap_or_default()
        .split(',')
        .collect();
    Value::Array(
        state
            .stations
            .iter()
            .filter(|s| wanted.contains(&s.uuid().as_str()))
            .map(|s| s.to_json(&state.stream_base, None))
            .collect(),
    )
}

fn fixture(text: &str) -> Value {
    serde_json::from_str(text).expect("radio fixture is valid JSON")
}

fn answer(state: &DirectoryState, request: &Request) -> (u16, String) {
    let path = request.path.as_str();
    let body = match path {
        "/json/servers" => fixture(SERVERS_JSON),
        "/json/stations/search" => search(state, request),
        "/json/stations/byuuid" => by_uuid(state, request),
        "/json/countries" => fixture(COUNTRIES_JSON),
        "/json/states" => fixture(STATES_JSON),
        "/json/tags" => fixture(TAGS_JSON),
        _ if path.starts_with("/json/url/") => fixture(URL_CLICK_JSON),
        _ => return (404, "{}".to_owned()),
    };
    (200, body.to_string())
}

fn serve_directory(
    listener: TcpListener,
    state: Arc<Mutex<DirectoryState>>,
    stop: Arc<AtomicBool>,
) -> JoinHandle<()> {
    listener
        .set_nonblocking(true)
        .expect("non-blocking fake directory");
    std::thread::spawn(move || {
        while !stop.load(Ordering::SeqCst) {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    let state = Arc::clone(&state);
                    std::thread::spawn(move || {
                        let Some(request) = read_request(&mut stream) else {
                            return;
                        };
                        let (fault, reply) = {
                            let mut state = state.lock().unwrap_or_else(|e| e.into_inner());
                            state.requests.push(Recorded {
                                path: request.path.clone(),
                                params: request.params.clone(),
                                user_agent: request.user_agent.clone(),
                            });
                            (state.fault, answer(&state, &request))
                        };
                        match fault {
                            Fault::None => write_reply(&mut stream, reply.0, &reply.1),
                            Fault::Status(status) => write_reply(&mut stream, status, "{}"),
                            Fault::Refuse => {}
                            Fault::Stall(delay) => {
                                std::thread::sleep(delay);
                                write_reply(&mut stream, reply.0, &reply.1);
                            }
                            Fault::Html => write_reply(
                                &mut stream,
                                200,
                                "<html><body>Maintenance</body></html>",
                            ),
                            Fault::HugeUnsized => {
                                let head = "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n";
                                let _ = stream.write_all(head.as_bytes());
                                let chunk = vec![b' '; 64 * 1024];
                                for _ in 0..(9 * 1024 * 1024) / chunk.len() {
                                    if stream.write_all(&chunk).is_err() {
                                        break;
                                    }
                                }
                            }
                            Fault::Huge => {
                                let size = 9 * 1024 * 1024;
                                let head = format!(
                                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {size}\r\nConnection: close\r\n\r\n"
                                );
                                let _ = stream.write_all(head.as_bytes());
                                let chunk = vec![b' '; 64 * 1024];
                                for _ in 0..size / chunk.len() {
                                    if stream.write_all(&chunk).is_err() {
                                        break;
                                    }
                                }
                            }
                        }
                    });
                }
                Err(_) => std::thread::sleep(Duration::from_millis(5)),
            }
        }
    })
}

/// A fake Radio Browser server on `127.0.0.1`. Stops when dropped.
pub(crate) struct FakeDirectory {
    port: u16,
    state: Arc<Mutex<DirectoryState>>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl FakeDirectory {
    pub fn start(stations: Vec<FakeStation>) -> FakeDirectory {
        FakeDirectory::start_with_stream_base(stations, DEFAULT_STREAM_BASE)
    }

    /// Stations without their own `url` point at `<stream_base>/stream/<n>`.
    pub fn start_with_stream_base(stations: Vec<FakeStation>, stream_base: &str) -> FakeDirectory {
        let listener = TcpListener::bind("127.0.0.1:0").expect("binding the fake directory");
        let port = listener
            .local_addr()
            .expect("fake directory address")
            .port();
        let state = Arc::new(Mutex::new(DirectoryState {
            stations,
            stream_base: stream_base.trim_end_matches('/').to_owned(),
            fault: Fault::None,
            requests: Vec::new(),
        }));
        let stop = Arc::new(AtomicBool::new(false));
        let thread = serve_directory(listener, Arc::clone(&state), Arc::clone(&stop));
        FakeDirectory {
            port,
            state,
            stop,
            thread: Some(thread),
        }
    }

    /// The base URL with scheme, as a `MirrorSource::Fixed` entry.
    pub fn url(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    pub fn requests(&self) -> Vec<Recorded> {
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.requests.clone()
    }

    pub fn set_fault(&self, fault: Fault) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.fault = fault;
    }

    /// Closes the listener: further connections are refused.
    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }

    /// Listens again on the same port.
    pub fn restart(&mut self) {
        self.stop();
        let listener =
            TcpListener::bind(("127.0.0.1", self.port)).expect("rebinding the fake directory");
        self.stop = Arc::new(AtomicBool::new(false));
        self.thread = Some(serve_directory(
            listener,
            Arc::clone(&self.state),
            Arc::clone(&self.stop),
        ));
    }
}

impl Drop for FakeDirectory {
    fn drop(&mut self) {
        self.stop();
    }
}

/// 300 fake stations around Testhausen (lat 50.0, lon 10.0) for the UI checks: a spread of
/// codecs, regions, click counts, some with coordinates, some without logos, one HLS in ten.
pub(crate) fn ui_stations(count: u32) -> Vec<FakeStation> {
    const REGIONS: [&str; 3] = ["Testland", "Examplestate", "Sampleland"];
    const CODECS: [(&str, u32); 4] = [("MP3", 128), ("AAC", 96), ("AAC+", 64), ("OGG", 192)];
    const TAGS: [&str; 5] = [
        "pop,hits",
        "rock,classic rock",
        "jazz,lounge",
        "news,talk",
        "dance,club",
    ];
    (1..=count)
        .map(|n| {
            let (codec, bitrate) = CODECS[(n % 4) as usize];
            let mut station = FakeStation::new(n)
                .region(REGIONS[(n % 3) as usize])
                .codec(codec, bitrate)
                .tags(TAGS[(n % 5) as usize])
                .clicks(1000u32.saturating_sub(n * 3))
                .votes(n % 40);
            if n % 10 == 0 {
                station = station.hls();
            }
            if n % 4 != 0 {
                let step = f64::from(n % 50) / 50.0;
                station = station.geo(49.6 + step * 0.8, 9.6 + step * 0.8);
            }
            if n % 3 != 0 {
                station = station.homepage(&format!("https://www.example.org/demo{n}"));
            }
            if n % 5 != 0 {
                station = station.favicon(&format!("https://www.example.org/demo{n}/logo.png"));
            }
            station
        })
        .collect()
}

/// The ICY interval of the UI stations: one metadata block about every 10 s at 128 kbps, so a
/// title (sent in two blocks) changes about every 20 s.
const UI_METAINT: usize = 160_000;
/// Slightly faster than the 2.04 s tone plays, so the player never runs dry; the ring is
/// bounded and holds the download back.
const UI_TONE_SECONDS: f32 = 1.9;
const UI_TITLES: u32 = 90;

/// What `serve_for_ui` runs: the fake directory with [`ui_stations`], the fake location
/// service, and a fake stream server that plays every one of those stations as an endless MP3
/// tone with ICY titles. All on `127.0.0.1`.
pub(crate) struct UiServers {
    pub directory: FakeDirectory,
    pub geo: FakeGeo,
    pub radio: crate::audio::live::testserver::FakeRadio,
}

impl UiServers {
    pub fn start(count: u32) -> UiServers {
        UiServers::paced(count, UI_TONE_SECONDS)
    }

    /// `seconds_per_loop` 0 streams as fast as the client reads (for tests).
    fn paced(count: u32, seconds_per_loop: f32) -> UiServers {
        use crate::audio::live::testserver::{Body, FakeRadio};
        let radio = FakeRadio::start();
        let titles: Vec<String> = (1..=UI_TITLES)
            .map(|k| format!("Demo Artist {k} - Demo Song {k}"))
            .collect();
        let titles: Vec<&str> = titles.iter().map(String::as_str).collect();
        let stations = ui_stations(count);
        // Every station is the MP3 tone, whatever codec it announces: the opener goes by the
        // content type first, and an "HLS" station still takes FFmpeg's URL path.
        for station in &stations {
            radio.station(
                &format!("/stream/{}", station.n),
                Body::Mp3Tone { seconds_per_loop },
                Some(UI_METAINT),
                &titles,
            );
        }
        let directory = FakeDirectory::start_with_stream_base(stations, &radio.url(""));
        let geo = FakeGeo::start();
        UiServers {
            directory,
            geo,
            radio,
        }
    }
}

/// Serves the fake directory, the fake location service and the fake station streams for the
/// dev instance until `PRISM_FAKE_SERVER_MINUTES` (default 10) are over:
/// `cargo test --manifest-path src-tauri/Cargo.toml radio::testserver::serve_for_ui -- --exact --ignored --nocapture`.
#[test]
#[ignore = "serves fake radio data for a manual UI check"]
fn serve_for_ui() {
    let minutes = std::env::var("PRISM_FAKE_SERVER_MINUTES")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(10);
    let servers = UiServers::start(300);
    println!("PRISM_RADIO_DIRECTORY={}", servers.directory.url());
    println!("PRISM_RADIO_GEO={}", servers.geo.geojs_url());
    println!("(ipinfo fallback: {})", servers.geo.ipinfo_url());
    println!(
        "station streams: {} (endless MP3 tone with ICY titles)",
        servers.radio.url("/stream/<n>")
    );
    println!("serving for {minutes} minutes");
    std::thread::sleep(Duration::from_secs(minutes * 60));
}

/// The stations `serve_for_ui` lists really stream: the directory's URL answers with MP3 audio,
/// `icy-metaint` and a first title.
#[test]
fn ui_stations_stream_from_the_fake_radio() {
    let servers = UiServers::paced(12, 0.0);
    let station = FakeStation::new(7);
    let found: Value = reqwest::blocking::get(format!(
        "{}/json/stations/byuuid?uuids={}",
        servers.directory.url(),
        station.uuid()
    ))
    .expect("asking the fake directory")
    .json()
    .expect("a station list");
    let url = found[0]["url_resolved"].as_str().expect("a stream url");
    assert_loopback(url);
    assert_eq!(url, servers.radio.url("/stream/7"));

    let mut stream =
        TcpStream::connect(url.trim_start_matches("http://").split('/').next().unwrap())
            .expect("connecting to the fake stream");
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    write!(
        stream,
        "GET /stream/7 HTTP/1.1\r\nHost: 127.0.0.1\r\nIcy-MetaData: 1\r\nUser-Agent: Prism/test\r\n\r\n"
    )
    .unwrap();
    // The head, then the first metadata block right after `UI_METAINT` audio bytes.
    let mut got = Vec::new();
    let mut buf = [0u8; 16 * 1024];
    let head_end = loop {
        let n = stream.read(&mut buf).expect("reading the head");
        assert!(n > 0, "the fake stream closed early");
        got.extend_from_slice(&buf[..n]);
        if let Some(at) = got.windows(4).position(|w| w == b"\r\n\r\n") {
            break at + 4;
        }
    };
    let head = String::from_utf8_lossy(&got[..head_end]).to_ascii_lowercase();
    assert!(head.starts_with("http/1.1 200"), "{head}");
    assert!(head.contains("content-type: audio/mpeg"), "{head}");
    assert!(
        head.contains(&format!("icy-metaint: {UI_METAINT}")),
        "{head}"
    );
    let block_at = head_end + UI_METAINT;
    while got.len() <= block_at || got.len() < block_at + 1 + usize::from(got[block_at]) * 16 {
        let n = stream.read(&mut buf).expect("reading the stream");
        assert!(n > 0, "the fake stream closed early");
        got.extend_from_slice(&buf[..n]);
    }
    // The audio starts with an MP3 frame sync.
    assert_eq!(got[head_end], 0xff);
    assert_eq!(got[head_end + 1] & 0xe0, 0xe0);
    let block = &got[block_at + 1..block_at + 1 + usize::from(got[block_at]) * 16];
    let text = String::from_utf8_lossy(block);
    assert!(
        text.starts_with("StreamTitle='Demo Artist 1 - Demo Song 1';"),
        "{text}"
    );
}

/// One answer of [`FakeWeb`].
#[derive(Debug, Clone)]
pub(crate) struct Page {
    status: u16,
    content_type: String,
    body: Vec<u8>,
    sized: bool,
    location: Option<String>,
}

impl Page {
    pub fn ok(content_type: &str, body: impl Into<Vec<u8>>) -> Page {
        Page {
            status: 200,
            content_type: content_type.to_owned(),
            body: body.into(),
            sized: true,
            location: None,
        }
    }

    pub fn status(status: u16) -> Page {
        Page {
            status,
            ..Page::ok("text/plain", "")
        }
    }

    /// A 302 to `location` (a path or a full URL).
    pub fn redirect(location: &str) -> Page {
        Page {
            location: Some(location.to_owned()),
            ..Page::status(302)
        }
    }

    /// Sent without a `Content-Length`: the body ends when the connection closes.
    pub fn without_length(mut self) -> Page {
        self.sized = false;
        self
    }
}

struct WebState {
    pages: HashMap<String, Page>,
    hits: HashMap<String, usize>,
}

fn write_page(stream: &mut TcpStream, page: &Page) {
    let mut head = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nConnection: close\r\n",
        page.status,
        reason(page.status),
        page.content_type
    );
    if page.sized {
        head.push_str(&format!("Content-Length: {}\r\n", page.body.len()));
    }
    if let Some(location) = &page.location {
        head.push_str(&format!("Location: {location}\r\n"));
    }
    head.push_str("\r\n");
    let _ = stream.write_all(head.as_bytes());
    for chunk in page.body.chunks(64 * 1024) {
        if stream.write_all(chunk).is_err() {
            break;
        }
    }
}

/// A fake web host for favicons and station homepages: fixed pages by path (404 otherwise),
/// with a hit count per path. Stops when dropped.
pub(crate) struct FakeWeb {
    port: u16,
    state: Arc<Mutex<WebState>>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl FakeWeb {
    pub fn start() -> FakeWeb {
        let listener = TcpListener::bind("127.0.0.1:0").expect("binding the fake web host");
        let port = listener.local_addr().expect("fake web address").port();
        listener
            .set_nonblocking(true)
            .expect("non-blocking fake web host");
        let state = Arc::new(Mutex::new(WebState {
            pages: HashMap::new(),
            hits: HashMap::new(),
        }));
        let stop = Arc::new(AtomicBool::new(false));
        let thread = {
            let state = Arc::clone(&state);
            let stop = Arc::clone(&stop);
            std::thread::spawn(move || {
                while !stop.load(Ordering::SeqCst) {
                    match listener.accept() {
                        Ok((mut stream, _)) => {
                            let state = Arc::clone(&state);
                            std::thread::spawn(move || {
                                let Some(path) = read_path(&mut stream) else {
                                    return;
                                };
                                let page = {
                                    let mut state = state.lock().unwrap_or_else(|e| e.into_inner());
                                    *state.hits.entry(path.clone()).or_default() += 1;
                                    state.pages.get(&path).cloned()
                                };
                                write_page(&mut stream, &page.unwrap_or_else(|| Page::status(404)));
                            });
                        }
                        Err(_) => std::thread::sleep(Duration::from_millis(5)),
                    }
                }
            })
        };
        FakeWeb {
            port,
            state,
            stop,
            thread: Some(thread),
        }
    }

    pub fn url(&self, path: &str) -> String {
        format!("http://127.0.0.1:{}{path}", self.port)
    }

    pub fn set(&self, path: &str, page: Page) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.pages.insert(path.to_owned(), page);
    }

    pub fn hits(&self, path: &str) -> usize {
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.hits.get(path).copied().unwrap_or(0)
    }
}

impl Drop for FakeWeb {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
