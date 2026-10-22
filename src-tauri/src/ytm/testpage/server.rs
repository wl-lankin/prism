//! The fake YouTube Music page on `127.0.0.1`: `index.html`, `player.js`, three tones
//! synthesized at start (nothing binary in the repo) and the two API calls the bridge makes
//! for lyrics, answered from `innertube.json`. Hand-rolled on a plain `TcpListener`
//! like the other fake servers, one request per connection; the tones answer byte ranges,
//! which WebKit needs before it plays a media file.
//!
//! A dev run against it: start [`serve_for_ui`], then the app with the `PRISM_YTM_URL` it
//! prints (see `ytm::dev`).

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

const INDEX: &str = include_str!("index.html");
const PLAYER: &str = include_str!("player.js");

pub(crate) const SONGS: usize = 3;
const SAMPLE_RATE: u32 = 22_050;
const SONG_SECONDS: u32 = 24;
const FREQUENCIES: [f32; SONGS] = [220.0, 262.0, 330.0];
/// Peak level, well below full scale: a dev run should not be loud.
const LEVEL: f32 = 0.12;

/// Song `n` (0-based): a mono 16-bit WAV tone, pulsing once a second, with soft ends.
pub(crate) fn tone(n: usize) -> Vec<u8> {
    let samples = SAMPLE_RATE * SONG_SECONDS;
    let data_len = samples * 2;
    let mut wav = Vec::with_capacity(44 + data_len as usize);
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + data_len).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&SAMPLE_RATE.to_le_bytes());
    wav.extend_from_slice(&(SAMPLE_RATE * 2).to_le_bytes());
    wav.extend_from_slice(&2u16.to_le_bytes());
    wav.extend_from_slice(&16u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data_len.to_le_bytes());
    let rate = SAMPLE_RATE as f32;
    let fade = rate * 0.05;
    for i in 0..samples {
        let t = i as f32 / rate;
        let pulse = 0.6 + 0.4 * (std::f32::consts::TAU * t).cos();
        let edge = (i as f32 / fade)
            .min((samples - 1 - i) as f32 / fade)
            .min(1.0);
        let value = (std::f32::consts::TAU * FREQUENCIES[n] * t).sin() * pulse * edge * LEVEL;
        wav.extend_from_slice(&((value * f32::from(i16::MAX)) as i16).to_le_bytes());
    }
    wav
}

/// The demo songs' video ids, in order (invented, in YouTube's shape).
pub(crate) const VIDEO_IDS: [&str; SONGS] = ["demoSong001", "demoSong002", "demoSong003"];
/// YouTube Music's API answers for the demo songs: song 1 has lyrics, song 2 an
/// unselectable lyrics tab, song 3 none. Shared with the bridge's specs.
const INNERTUBE: &str = include_str!("innertube.json");
const BODY_MAX: usize = 64 * 1024;

struct Request {
    head_only: bool,
    post: bool,
    path: String,
    range: Option<String>,
    body: Vec<u8>,
}

fn header<'a>(head: &'a str, wanted: &str) -> Option<&'a str> {
    head.lines().skip(1).find_map(|line| {
        let (name, value) = line.split_once(':')?;
        name.trim()
            .eq_ignore_ascii_case(wanted)
            .then(|| value.trim())
    })
}

fn read_request(stream: &mut TcpStream) -> Option<Request> {
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let mut data = Vec::new();
    let mut buf = [0u8; 2048];
    let end = loop {
        if let Some(at) = data.windows(4).position(|w| w == b"\r\n\r\n") {
            break at + 4;
        }
        let n = stream.read(&mut buf).ok()?;
        if n == 0 || data.len() > 16 * 1024 {
            return None;
        }
        data.extend_from_slice(&buf[..n]);
    };
    let head = String::from_utf8(data[..end].to_vec()).ok()?;
    let mut first = head.lines().next()?.split(' ');
    let method = first.next()?;
    let target = first.next()?;
    let length: usize = header(&head, "content-length").map_or(Some(0), |v| v.parse().ok())?;
    if length > BODY_MAX {
        return None;
    }
    let mut body = data[end..].to_vec();
    while body.len() < length {
        let n = stream.read(&mut buf).ok()?;
        if n == 0 {
            return None;
        }
        body.extend_from_slice(&buf[..n]);
    }
    body.truncate(length);
    Some(Request {
        head_only: method == "HEAD",
        post: method == "POST",
        path: target.split(['?', '#']).next().unwrap_or("/").to_owned(),
        range: header(&head, "range").map(str::to_owned),
        body,
    })
}

/// `POST /youtubei/v1/next` and `/browse` as YouTube Music answers them, from the fixture.
/// A body without a client context is refused, as the real API does.
fn innertube(request: &Request) -> Option<Reply> {
    let endpoint = request.path.strip_prefix("/youtubei/v1/")?;
    let not_found = || {
        reply(
            404,
            "application/json",
            b"{\"error\":{\"code\":404}}".to_vec(),
        )
    };
    if !request.post || !matches!(endpoint, "next" | "browse") {
        return Some(not_found());
    }
    let body: serde_json::Value = match serde_json::from_slice(&request.body) {
        Ok(body) => body,
        Err(_) => return Some(reply(400, "application/json", b"{}".to_vec())),
    };
    if !body["context"]["client"]["clientName"].is_string() {
        return Some(reply(400, "application/json", b"{}".to_vec()));
    }
    let key = if endpoint == "next" {
        "videoId"
    } else {
        "browseId"
    };
    let fixture: serde_json::Value = serde_json::from_str(INNERTUBE).expect("innertube.json");
    let answer = body[key]
        .as_str()
        .and_then(|id| fixture[endpoint].get(id))
        .map(|answer| serde_json::to_vec(answer).expect("fixture answer"));
    Some(match answer {
        Some(answer) => reply(200, "application/json; charset=utf-8", answer),
        None => not_found(),
    })
}

/// The one range of a `Range: bytes=...` header, inclusive; `None` if it cannot be served.
fn byte_range(header: &str, len: usize) -> Option<(usize, usize)> {
    let spec = header.strip_prefix("bytes=")?.split(',').next()?.trim();
    let (start, end) = spec.split_once('-')?;
    if len == 0 {
        return None;
    }
    let (start, end) = if start.is_empty() {
        let suffix: usize = end.parse().ok()?;
        if suffix == 0 {
            return None;
        }
        (len.saturating_sub(suffix), len - 1)
    } else {
        let start: usize = start.parse().ok()?;
        let end = if end.is_empty() {
            len - 1
        } else {
            end.parse::<usize>().ok()?.min(len - 1)
        };
        (start, end)
    };
    (start <= end && start < len).then_some((start, end))
}

struct Reply {
    status: u16,
    headers: Vec<(&'static str, String)>,
    body: Vec<u8>,
}

fn reply(status: u16, content_type: &str, body: Vec<u8>) -> Reply {
    Reply {
        status,
        headers: vec![("Content-Type", content_type.to_owned())],
        body,
    }
}

fn answer(request: &Request, tones: &[Vec<u8>]) -> Reply {
    if let Some(reply) = innertube(request) {
        return reply;
    }
    let song = request
        .path
        .strip_prefix("/tone/")
        .and_then(|rest| rest.strip_suffix(".wav"))
        .and_then(|n| n.parse::<usize>().ok())
        .filter(|n| (1..=SONGS).contains(n) && request.path == format!("/tone/{n}.wav"));
    match (request.path.as_str(), song) {
        ("/" | "/index.html", _) => reply(200, "text/html; charset=utf-8", INDEX.into()),
        ("/player.js", _) => reply(200, "text/javascript; charset=utf-8", PLAYER.into()),
        (_, Some(n)) => {
            let tone = &tones[n - 1];
            let len = tone.len();
            let mut answer = match request.range.as_deref() {
                None => reply(200, "audio/wav", tone.clone()),
                Some(header) => match byte_range(header, len) {
                    Some((start, end)) => {
                        let mut answer = reply(206, "audio/wav", tone[start..=end].to_vec());
                        answer
                            .headers
                            .push(("Content-Range", format!("bytes {start}-{end}/{len}")));
                        answer
                    }
                    None => {
                        let mut answer = reply(416, "text/plain", Vec::new());
                        answer
                            .headers
                            .push(("Content-Range", format!("bytes */{len}")));
                        answer
                    }
                },
            };
            answer.headers.push(("Accept-Ranges", "bytes".to_owned()));
            answer
        }
        _ => reply(404, "text/plain", b"not found".to_vec()),
    }
}

fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        206 => "Partial Content",
        400 => "Bad Request",
        404 => "Not Found",
        416 => "Range Not Satisfiable",
        _ => "Status",
    }
}

fn serve(mut stream: TcpStream, tones: &[Vec<u8>]) {
    let Some(request) = read_request(&mut stream) else {
        return;
    };
    let answer = answer(&request, tones);
    let mut head = format!(
        "HTTP/1.1 {} {}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n",
        answer.status,
        reason(answer.status),
        answer.body.len()
    );
    for (name, value) in &answer.headers {
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    head.push_str("\r\n");
    // A client that gave up (a media element often does) is not the server's problem.
    if stream.write_all(head.as_bytes()).is_ok() && !request.head_only {
        let _ = stream.write_all(&answer.body);
    }
}

/// The fake page on `127.0.0.1:<free port>`. Stops when dropped.
pub(crate) struct FakeYtm {
    port: u16,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl FakeYtm {
    pub fn start() -> FakeYtm {
        let listener =
            TcpListener::bind("127.0.0.1:0").expect("binding the fake YouTube Music page");
        let port = listener.local_addr().expect("fake page address").port();
        listener
            .set_nonblocking(true)
            .expect("non-blocking fake page server");
        let tones: Arc<Vec<Vec<u8>>> = Arc::new((0..SONGS).map(tone).collect());
        let stop = Arc::new(AtomicBool::new(false));
        let thread = {
            let stop = Arc::clone(&stop);
            std::thread::spawn(move || {
                while !stop.load(Ordering::SeqCst) {
                    match listener.accept() {
                        Ok((stream, _)) => {
                            let tones = Arc::clone(&tones);
                            std::thread::spawn(move || serve(stream, &tones));
                        }
                        Err(_) => std::thread::sleep(Duration::from_millis(5)),
                    }
                }
            })
        };
        FakeYtm {
            port,
            stop,
            thread: Some(thread),
        }
    }

    /// The page, in the one shape `PRISM_YTM_URL` accepts.
    pub fn url(&self) -> String {
        format!("http://127.0.0.1:{}/", self.port)
    }
}

impl Drop for FakeYtm {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Serves the fake YouTube Music page until `PRISM_FAKE_SERVER_MINUTES` (default 10) are over:
/// `cargo test --manifest-path src-tauri/Cargo.toml ytm::testpage::serve_for_ui -- --exact --ignored --nocapture`.
#[test]
#[ignore = "serves the fake YouTube Music page for a manual UI check"]
fn serve_for_ui() {
    let minutes = std::env::var("PRISM_FAKE_SERVER_MINUTES")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(10);
    let page = FakeYtm::start();
    println!("PRISM_YTM_URL={}", page.url());
    println!("serving for {minutes} minutes");
    std::thread::sleep(Duration::from_secs(minutes * 60));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn get(url: &str, range: Option<&str>) -> reqwest::blocking::Response {
        crate::radio::testserver::assert_loopback(url);
        let client = reqwest::blocking::Client::new();
        let mut request = client.get(url);
        if let Some(range) = range {
            request = request.header("Range", range);
        }
        request.send().expect("asking the fake page")
    }

    fn inside<'a>(text: &'a str, open: &str, close: &str) -> &'a str {
        let start = text.find(open).unwrap_or_else(|| panic!("{open}")) + open.len();
        let end = text[start..]
            .find(close)
            .unwrap_or_else(|| panic!("{close}"));
        &text[start..start + end]
    }

    #[test]
    fn page_has_what_the_bridge_reads() {
        let page = FakeYtm::start();
        assert!(crate::ytm::dev::parse_dev_url(&page.url()).is_some());

        let html = get(&page.url(), None);
        assert_eq!(html.status(), 200);
        assert!(html.headers()["content-type"]
            .to_str()
            .unwrap()
            .starts_with("text/html"));
        let html = html.text().unwrap();
        // The bridge's selectors: the video inside the player, the controls inside the bar.
        let player = inside(&html, "<ytmusic-player>", "</ytmusic-player>");
        assert!(player.contains("<video"));
        let bar = inside(&html, "<ytmusic-player-bar>", "</ytmusic-player-bar>");
        for control in [
            "id=\"play-pause-button\"",
            "class=\"next-button\"",
            "class=\"previous-button\"",
        ] {
            assert!(bar.contains(control), "{control}");
            assert_eq!(html.matches(control).count(), 1, "{control}");
        }
        // The queue: one entry per song, each with its play button, the first one current.
        let queue = inside(&html, "<ytmusic-player-queue>", "</ytmusic-player-queue>");
        assert_eq!(html.matches("<ytmusic-player-queue>").count(), 1);
        assert_eq!(queue.matches("<ytmusic-player-queue-item").count(), SONGS);
        assert_eq!(
            queue.matches("<ytmusic-play-button-renderer>").count(),
            SONGS
        );
        assert_eq!(
            queue
                .matches("<ytmusic-player-queue-item selected>")
                .count(),
            1
        );
        for k in 1..=SONGS {
            assert!(queue.contains(&format!("<span class=\"song-title\">Demo Song {k}</span>")));
            assert!(queue.contains(&format!("<span class=\"byline\">Demo Artist {k}</span>")));
        }
        assert_eq!(html.matches("<video").count(), 1);
        assert!(html.contains("<script src=\"player.js\"></script>"));

        let script = get(&format!("{}player.js", page.url()), None);
        assert_eq!(script.status(), 200);
        let script = script.text().unwrap();
        for k in 1..=SONGS {
            assert!(script.contains(&format!("title: 'Demo Song {k}'")));
            assert!(script.contains(&format!("artist: 'Demo Artist {k}'")));
        }
        assert!(script.contains("navigator.mediaSession.metadata = new MediaMetadata("));
        assert!(script.contains("'tone/' + (index + 1) + '.wav'"));
        // Nothing on the page reaches past the fake server.
        for text in [&html, &script] {
            assert!(!text.contains("https://"));
            assert!(!text.contains("youtube.com"));
        }

        for path in [
            "tone/0.wav",
            "tone/4.wav",
            "tone/01.wav",
            "tone/1.wav/x",
            "favicon.ico",
        ] {
            assert_eq!(
                get(&format!("{}{path}", page.url()), None).status(),
                404,
                "{path}"
            );
        }
    }

    fn innertube(
        page: &FakeYtm,
        endpoint: &str,
        body: serde_json::Value,
    ) -> (u16, serde_json::Value) {
        let url = format!("{}youtubei/v1/{endpoint}?prettyPrint=false", page.url());
        crate::radio::testserver::assert_loopback(&url);
        let answer = reqwest::blocking::Client::new()
            .post(url)
            .json(&body)
            .send()
            .expect("asking the fake innertube");
        let status = answer.status().as_u16();
        (status, answer.json().unwrap_or(serde_json::Value::Null))
    }

    fn context() -> serde_json::Value {
        serde_json::json!({ "client": { "clientName": "WEB_REMIX", "clientVersion": "1.20261022.01.00" } })
    }

    /// The lyrics tab's browse id, read the way the bridge reads it.
    fn lyrics_browse_id(next: &serde_json::Value) -> Option<String> {
        let tabs = next.pointer(
            "/contents/singleColumnMusicWatchNextResultsRenderer/tabbedRenderer/watchNextTabbedResultsRenderer/tabs",
        )?;
        tabs.as_array()?.iter().find_map(|tab| {
            let tab = tab.get("tabRenderer")?;
            if tab.get("unselectable").is_some() {
                return None;
            }
            let browse = tab.pointer("/endpoint/browseEndpoint")?;
            (browse.pointer(
                "/browseEndpointContextSupportedConfigs/browseEndpointContextMusicConfig/pageType",
            )? == "MUSIC_PAGE_TYPE_TRACK_LYRICS")
                .then(|| browse["browseId"].as_str().map(str::to_owned))?
        })
    }

    #[test]
    fn innertube_answers_like_youtube_music() {
        let page = FakeYtm::start();
        let ask = |video: &str| {
            innertube(
                &page,
                "next",
                serde_json::json!({ "videoId": video, "isAudioOnly": true, "context": context() }),
            )
        };

        let (status, next) = ask(VIDEO_IDS[0]);
        assert_eq!(status, 200);
        let browse_id = lyrics_browse_id(&next).expect("song 1 has a lyrics tab");
        let (status, browse) = innertube(
            &page,
            "browse",
            serde_json::json!({ "browseId": browse_id, "context": context() }),
        );
        assert_eq!(status, 200);
        let shelf = browse
            .pointer("/contents/sectionListRenderer/contents/0/musicDescriptionShelfRenderer")
            .unwrap();
        let text = shelf
            .pointer("/description/runs/0/text")
            .unwrap()
            .as_str()
            .unwrap();
        assert!(text.lines().count() >= 3);
        assert_eq!(
            shelf.pointer("/footer/runs/0/text").unwrap(),
            "Source: Testquelle"
        );

        // Song 2 has an unselectable lyrics tab, song 3 none.
        for video in &VIDEO_IDS[1..] {
            let (status, next) = ask(video);
            assert_eq!(status, 200);
            assert_eq!(lyrics_browse_id(&next), None, "{video}");
        }

        // Unknown ids, a missing context and other paths are refused.
        assert_eq!(ask("unknownVid1").0, 404);
        assert_eq!(
            innertube(
                &page,
                "next",
                serde_json::json!({ "videoId": VIDEO_IDS[0] })
            )
            .0,
            400
        );
        assert_eq!(
            innertube(
                &page,
                "browse",
                serde_json::json!({ "browseId": "MPLYt_x", "context": context() })
            )
            .0,
            404
        );
        assert_eq!(
            innertube(&page, "search", serde_json::json!({ "context": context() })).0,
            404
        );
        assert_eq!(
            get(&format!("{}youtubei/v1/next", page.url()), None).status(),
            404
        );
    }

    #[test]
    fn page_names_its_videos_and_context() {
        assert_eq!(VIDEO_IDS.len(), SONGS);
        for id in VIDEO_IDS {
            assert!(crate::ytm::report::is_video_id(id), "{id}");
            assert!(PLAYER.contains(&format!("videoId: '{id}'")), "{id}");
        }
        assert!(PLAYER.contains("window.ytcfg"));
        assert!(PLAYER.contains("INNERTUBE_CONTEXT"));
        assert!(PLAYER.contains("getVideoData"));
        assert!(INDEX.contains("<div id=\"movie_player\"></div>"));
    }

    #[test]
    fn tones_are_short_quiet_wavs() {
        let tones: Vec<Vec<u8>> = (0..SONGS).map(tone).collect();
        for wav in &tones {
            assert_eq!(&wav[0..4], b"RIFF");
            assert_eq!(&wav[8..16], b"WAVEfmt ");
            assert_eq!(u16::from_le_bytes([wav[22], wav[23]]), 1);
            assert_eq!(
                u32::from_le_bytes(wav[24..28].try_into().unwrap()),
                SAMPLE_RATE
            );
            assert_eq!(u16::from_le_bytes([wav[34], wav[35]]), 16);
            let data = u32::from_le_bytes(wav[40..44].try_into().unwrap()) as usize;
            assert_eq!(data, (SAMPLE_RATE * SONG_SECONDS * 2) as usize);
            assert_eq!(wav.len(), 44 + data);
            let peak = wav[44..]
                .as_chunks::<2>()
                .0
                .iter()
                .map(|s| i16::from_le_bytes(*s).unsigned_abs())
                .max()
                .unwrap();
            assert!(peak > 1000);
            assert!(f32::from(peak) <= LEVEL * f32::from(i16::MAX) + 1.0);
            // Soft ends: no click when a song starts or stops.
            assert!(i16::from_le_bytes([wav[44], wav[45]]).unsigned_abs() < 10);
            let last = wav.len() - 2;
            assert!(i16::from_le_bytes([wav[last], wav[last + 1]]).unsigned_abs() < 10);
        }
        assert_ne!(tones[0], tones[1]);
        assert_ne!(tones[1], tones[2]);
    }

    #[test]
    fn tones_answer_byte_ranges() {
        let page = FakeYtm::start();
        let url = format!("{}tone/2.wav", page.url());
        let len = tone(1).len();

        let full = get(&url, None);
        assert_eq!(full.status(), 200);
        assert_eq!(full.headers()["accept-ranges"], "bytes");
        assert_eq!(full.headers()["content-type"], "audio/wav");
        assert_eq!(full.bytes().unwrap().as_ref(), tone(1).as_slice());

        let first = get(&url, Some("bytes=0-11"));
        assert_eq!(first.status(), 206);
        assert_eq!(
            first.headers()["content-range"],
            format!("bytes 0-11/{len}").as_str()
        );
        let first = first.bytes().unwrap();
        assert_eq!(&first[0..4], b"RIFF");
        assert_eq!(&first[8..12], b"WAVE");

        let tail = get(&url, Some("bytes=-4"));
        assert_eq!(tail.status(), 206);
        assert_eq!(tail.bytes().unwrap().as_ref(), &tone(1)[len - 4..]);

        let open = get(&url, Some(&format!("bytes={}-", len - 10)));
        assert_eq!(open.status(), 206);
        assert_eq!(open.bytes().unwrap().len(), 10);

        let past = get(&url, Some(&format!("bytes={len}-")));
        assert_eq!(past.status(), 416);
        assert_eq!(
            past.headers()["content-range"],
            format!("bytes */{len}").as_str()
        );

        assert_eq!(byte_range("bytes=5-2", len), None);
        assert_eq!(byte_range("items=0-1", len), None);
        assert_eq!(byte_range("bytes=-0", len), None);
        assert_eq!(byte_range("bytes=0-999999999", 100), Some((0, 99)));
        assert_eq!(byte_range("bytes=0-1, 4-5", 100), Some((0, 1)));
        assert_eq!(byte_range("bytes=0-", 0), None);
    }
}
