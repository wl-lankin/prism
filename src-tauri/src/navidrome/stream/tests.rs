//! Navidrome streams against the fake server: fake data, `127.0.0.1:0` only.

use std::fs;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use crossbeam_channel::RecvTimeoutError;
use reqwest::Url;
use tempfile::TempDir;

use super::*;
use crate::audio::ffmpeg::{ffmpeg_args, FfmpegSource};
use crate::audio::http::RETRY_BACKOFF;
use crate::audio::source::Input;
use crate::db::Db;
use crate::library::tags::fixtures;
use crate::navidrome::client::{API_TIMEOUT, CONNECT_TIMEOUT, STREAM_READ_TIMEOUT};
use crate::navidrome::keychain::{CredentialStore, MemoryStore};
use crate::navidrome::testserver::{
    FakeLibrary, FakeServer, Fault, Recorded, FAKE_PASSWORD, FAKE_USER,
};
use crate::navidrome::ServiceOptions;
use crate::secret::{Secret, SecretUrl};
use crate::tools;

const SONG: &str = "tr-0";
const LABEL: &str = "Demo Artist 1 - Demo Song 1";
const WAV_RATE: u32 = 44_100;

fn fixture(name: &str) -> Vec<u8> {
    fs::read(fixtures().join(name)).unwrap()
}

fn has_ffmpeg(test: &str) -> bool {
    let found = tools::ffmpeg_path().is_some();
    if !found {
        println!("ffmpeg not found: skipping {test}");
    }
    found
}

/// A fake server, a client of its account and a stream config spilling into a temp dir.
struct Env {
    server: FakeServer,
    client: Arc<SubsonicClient>,
    spill: TempDir,
    config: Arc<StreamConfig>,
}

fn env(lib: FakeLibrary) -> Env {
    env_with(lib, Duration::from_secs(5))
}

fn env_with(lib: FakeLibrary, stall: Duration) -> Env {
    let server = FakeServer::start(lib);
    let client = Arc::new(
        SubsonicClient::with_stream_timeout(
            server.account(),
            CONNECT_TIMEOUT,
            API_TIMEOUT,
            STREAM_READ_TIMEOUT,
        )
        .unwrap(),
    );
    let spill = tempfile::tempdir().unwrap();
    let mut config = StreamConfig::new(client.stream_http(), spill.path().to_path_buf());
    config.stall = stall;
    config.backoff = RETRY_BACKOFF;
    Env {
        server,
        client,
        spill,
        config: Arc::new(config),
    }
}

impl Env {
    fn request(&self, quality: StreamQuality, extension: &str) -> StreamRequest {
        StreamRequest {
            client: Arc::clone(&self.client),
            song_id: SONG.into(),
            quality,
            label: LABEL.into(),
            extension: Some(extension.into()),
            duration_ms: None,
            config: Arc::clone(&self.config),
        }
    }

    fn open(&self, quality: StreamQuality, extension: &str) -> NavidromeSource {
        NavidromeSource::open(self.request(quality, extension), None)
            .unwrap_or_else(|err| panic!("open failed: {err:#}"))
    }

    fn input(&self, quality: StreamQuality) -> HttpInput {
        HttpInput {
            url: self.client.stream_url(SONG, quality, None),
            label: LABEL.into(),
            extension: None,
            config: Arc::clone(&self.config),
        }
    }

    fn stream_requests(&self) -> Vec<Recorded> {
        self.server
            .requests()
            .into_iter()
            .filter(|r| r.method == "stream")
            .collect()
    }

    fn spill_files(&self) -> usize {
        fs::read_dir(self.spill.path()).map_or(0, |entries| entries.count())
    }
}

fn param<'a>(request: &'a Recorded, key: &str) -> Option<&'a str> {
    request
        .params
        .iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.as_str())
}

/// Reads a source to its end (or an error); the frames read and the error.
fn drain(source: &mut dyn AudioSource) -> (usize, Option<anyhow::Error>) {
    let channels = usize::from(source.spec().channels);
    let mut frames = 0;
    let mut buf = Vec::new();
    loop {
        buf.clear();
        match source.read(&mut buf) {
            Ok(0) => return (frames, None),
            Ok(read) => {
                assert_eq!(buf.len(), read * channels, "read returns frames");
                frames += read;
            }
            Err(err) => return (frames, Some(err)),
        }
    }
}

fn drain_ok(source: &mut dyn AudioSource) -> usize {
    let (frames, err) = drain(source);
    if let Some(err) = err {
        panic!("reading failed: {err:#}");
    }
    frames
}

/// Reads at least `ms` of audio.
fn read_ms(source: &mut dyn AudioSource, ms: u64) {
    let want = ms * u64::from(source.spec().sample_rate) / 1000;
    let mut frames = 0u64;
    let mut buf = Vec::new();
    while frames < want {
        buf.clear();
        let read = source.read(&mut buf).unwrap();
        assert!(read > 0, "the stream ended early");
        frames += read as u64;
    }
}

fn assert_seconds_left(source: &mut dyn AudioSource, seconds: f64) {
    let rate = f64::from(source.spec().sample_rate);
    let left = drain_ok(source) as f64 / rate;
    assert!(
        (left - seconds).abs() <= seconds * 0.05,
        "{left:.3} s left, expected about {seconds} s"
    );
}

/// The error's every rendering carries no URL and no credential.
fn assert_clean(err: &anyhow::Error, url: &SecretUrl) {
    let secrets: Vec<String> = Url::parse(url.expose())
        .unwrap()
        .query_pairs()
        .filter(|(key, _)| key == "t" || key == "s")
        .map(|(_, value)| value.into_owned())
        .collect();
    for text in [format!("{err}"), format!("{err:#}"), format!("{err:?}")] {
        for needle in ["http://", "127.0.0.1", "rest/", "t=", "s=", FAKE_PASSWORD] {
            assert!(!text.contains(needle), "{needle:?} in {text:?}");
        }
        for secret in &secrets {
            assert!(!text.contains(secret.as_str()), "a secret in {text:?}");
        }
    }
}

fn track(id: &str, source: Source, format: &str) -> Track {
    Track {
        id: id.into(),
        source,
        title: "Demo Song 1".into(),
        artist: "Demo Artist 1".into(),
        album_artist: "Demo Artist 1".into(),
        album: "Demo Album 1".into(),
        album_id: "navidrome:al-0".into(),
        track_no: Some(1),
        disc_no: Some(1),
        duration_ms: 1000,
        year: None,
        genre: None,
        format: format.into(),
        sample_rate: None,
        bit_depth: None,
        bitrate_kbps: None,
        cover_id: None,
        path: None,
        replay_gain_track_db: None,
        replay_gain_album_db: None,
        replay_gain_track_peak: None,
        replay_gain_album_peak: None,
    }
}

/// A Navidrome service (no sync cycle runs) signed in to `server`, or to nothing.
struct Service {
    _dir: TempDir,
    settings: Arc<SettingsStore>,
    service: Arc<NavidromeService>,
}

impl Drop for Service {
    fn drop(&mut self) {
        self.service.shutdown();
    }
}

fn service(server: Option<&FakeServer>) -> Service {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.json");
    let creds = Arc::new(MemoryStore::new());
    if let Some(server) = server {
        SettingsStore::load(&path)
            .set_navidrome_account(Some((server.url(), FAKE_USER.into())))
            .unwrap();
        creds
            .set(&server.url(), FAKE_USER, &Secret::new(FAKE_PASSWORD))
            .unwrap();
    }
    let settings = Arc::new(SettingsStore::load(&path));
    let never = Duration::from_secs(3600);
    let service = NavidromeService::spawn_with_options(
        Arc::new(Db::open_in_memory().unwrap()),
        Arc::clone(&settings),
        creds as Arc<dyn CredentialStore>,
        Arc::new(|_| {}),
        ServiceOptions {
            first_delay: never,
            interval: never,
            offline_retry: never,
        },
    );
    Service {
        _dir: dir,
        settings,
        service,
    }
}

/// A fallback opener that counts its calls and fails.
fn counting_fallback(calls: &Arc<AtomicUsize>) -> Opener {
    let calls = Arc::clone(calls);
    Arc::new(move |_track: &Track, _spec: StreamSpec| {
        calls.fetch_add(1, Ordering::SeqCst);
        Err(anyhow::anyhow!("the fallback opener"))
    })
}

const DEVICE: StreamSpec = StreamSpec {
    sample_rate: 48_000,
    channels: 2,
};

#[test]
fn plays_a_navidrome_track_end_to_end() {
    let server =
        FakeServer::start(FakeLibrary::synthetic(1, 1).with_audio(SONG, fixture("tagged.flac")));
    let svc = service(Some(&server));
    let spill = tempfile::tempdir().unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let opener = navidrome_opener(
        Arc::clone(&svc.service),
        Arc::clone(&svc.settings),
        spill.path().to_path_buf(),
        counting_fallback(&calls),
    );
    let mut source = opener(&track("navidrome:tr-0", Source::Navidrome, "flac"), DEVICE)
        .unwrap_or_else(|err| panic!("open failed: {err:#}"));
    let rate = f64::from(source.spec().sample_rate);
    let frames = drain_ok(source.as_mut()) as f64;
    assert!(
        (frames - rate).abs() <= rate * 0.05,
        "{frames} frames at {rate} Hz is not about 1 s"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    // One request: no trailing-tag probe at the tail, no restart.
    let streams: Vec<Recorded> = server
        .requests()
        .into_iter()
        .filter(|r| r.method == "stream")
        .collect();
    assert_eq!(streams.len(), 1, "{streams:?}");
    drop(source);
    assert_eq!(fs::read_dir(spill.path()).unwrap().count(), 0);
}

#[test]
fn opening_makes_one_request() {
    // 5 MB: a look at the tail (trailing tags) would restart the download there and back.
    let env = env(FakeLibrary::synthetic(1, 1).with_wav(SONG, 30));
    let mut source = env.open(StreamQuality::Original, "wav");
    read_ms(&mut source, 500);
    let requests = env.stream_requests();
    assert_eq!(requests.len(), 1, "{requests:?}");
    // Seeking still uses ranges once the decoder is open.
    source.seek(29_000).unwrap();
    assert_seconds_left(&mut source, 1.0);
    let requests = env.stream_requests();
    assert_eq!(requests.len(), 2, "{requests:?}");
    assert!(requests[1].range.is_some());
}

#[test]
fn original_quality_requests_raw() {
    let env = env(FakeLibrary::synthetic(1, 1).with_wav(SONG, 1));
    let mut source = env.open(StreamQuality::Original, "wav");
    drain_ok(&mut source);
    let requests = env.stream_requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(param(&requests[0], "format"), Some("raw"));
    assert_eq!(param(&requests[0], "maxBitRate"), None);
    assert_eq!(param(&requests[0], "timeOffset"), None);
}

#[test]
fn the_opener_reads_the_quality_setting() {
    let server = FakeServer::start(FakeLibrary::synthetic(1, 1).with_wav(SONG, 1));
    let svc = service(Some(&server));
    svc.settings
        .set(serde_json::json!({ "navidromeQuality": "128" }))
        .unwrap();
    let spill = tempfile::tempdir().unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let opener = navidrome_opener(
        Arc::clone(&svc.service),
        Arc::clone(&svc.settings),
        spill.path().to_path_buf(),
        counting_fallback(&calls),
    );
    let source = opener(&track("navidrome:tr-0", Source::Navidrome, "wav"), DEVICE).unwrap();
    drop(source);
    let streams: Vec<Recorded> = server
        .requests()
        .into_iter()
        .filter(|r| r.method == "stream")
        .collect();
    assert_eq!(param(&streams[0], "format"), Some("mp3"));
    assert_eq!(param(&streams[0], "maxBitRate"), Some("128"));
}

#[test]
fn transcoded_quality_requests_mp3_at_bitrate() {
    let env = env(FakeLibrary::synthetic(1, 1).with_wav(SONG, 1));
    let mut source = env.open(StreamQuality::Kbps192, "mp3");
    drain_ok(&mut source);
    let requests = env.stream_requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(param(&requests[0], "format"), Some("mp3"));
    assert_eq!(param(&requests[0], "maxBitRate"), Some("192"));
    assert_eq!(param(&requests[0], "timeOffset"), None);
}

#[test]
fn byte_seek_uses_range() {
    let env = env(FakeLibrary::synthetic(1, 1).with_wav(SONG, 30));
    // The first download stalls after 256 KiB, so it cannot get near the seek target alone.
    env.server.set_fault(Fault::Stall {
        method: "stream",
        after_bytes: 256 * 1024,
        wait: Duration::from_secs(2),
    });
    let mut source = env.open(StreamQuality::Original, "wav");
    source.seek(25_000).unwrap();
    assert_seconds_left(&mut source, 5.0);
    let requests = env.stream_requests();
    assert_eq!(requests.len(), 2, "{requests:?}");
    assert!(requests[1].range.is_some(), "{requests:?}");
    assert!(requests.iter().all(|r| param(r, "timeOffset").is_none()));
}

#[test]
fn transcoded_seek_uses_time_offset() {
    let env = env(FakeLibrary::synthetic(1, 1).with_wav(SONG, 5));
    env.server.set_fault(Fault::NoRanges);
    let mut source = env.open(StreamQuality::Kbps192, "mp3");
    read_ms(&mut source, 300);
    source.seek(1500).unwrap();
    assert_seconds_left(&mut source, 3.5);
    let requests = env.stream_requests();
    assert_eq!(requests.len(), 2, "{requests:?}");
    assert_eq!(param(&requests[1], "timeOffset"), Some("1"));
    assert_eq!(param(&requests[1], "format"), Some("mp3"));
    assert_eq!(param(&requests[1], "maxBitRate"), Some("192"));
    // Seeking back into the first second asks for the transcode from 0.
    source.seek(400).unwrap();
    assert_seconds_left(&mut source, 4.6);
    let requests = env.stream_requests();
    assert_eq!(requests.len(), 3, "{requests:?}");
    assert_eq!(param(&requests[2], "timeOffset"), None);
}

#[test]
fn raw_stream_without_ranges_seeks_back_through_the_buffer() {
    let env = env(FakeLibrary::synthetic(1, 1).with_wav(SONG, 5));
    env.server.set_fault(Fault::NoRanges);
    let mut source = env.open(StreamQuality::Original, "wav");
    read_ms(&mut source, 2000);
    // Back: the decoder opens again on the buffered bytes.
    source.seek(1000).unwrap();
    read_ms(&mut source, 500);
    // Forward: decoded ahead.
    source.seek(3000).unwrap();
    read_ms(&mut source, 500);
    // Back again, to the very start.
    source.seek(0).unwrap();
    assert_seconds_left(&mut source, 5.0);
    let requests = env.stream_requests();
    assert_eq!(requests.len(), 1, "every seek is served from the buffer");
    assert_eq!(requests[0].range, None);
    assert_eq!(param(&requests[0], "timeOffset"), None);
}

#[test]
fn raw_flac_without_ranges_seeks_back_through_the_buffer() {
    let env = env(FakeLibrary::synthetic(1, 1).with_audio(SONG, fixture("tagged.flac")));
    env.server.set_fault(Fault::NoRanges);
    let mut source = env.open(StreamQuality::Original, "flac");
    read_ms(&mut source, 700);
    source.seek(200).unwrap();
    assert_seconds_left(&mut source, 0.8);
    assert_eq!(env.stream_requests().len(), 1);
}

#[test]
fn a_transcode_served_as_a_file_seeks_through_the_buffer() {
    // Navidrome plays a file directly when it already fits the transcode request, and then
    // ignores `timeOffset`; behind a proxy without ranges that file has a length but no ranges.
    let env = env(FakeLibrary::synthetic(1, 1).with_audio(SONG, fixture("tagged.flac")));
    env.server.set_fault(Fault::NoRanges);
    let mut source = env.open(StreamQuality::Kbps320, "mp3");
    read_ms(&mut source, 700);
    source.seek(300).unwrap();
    assert_seconds_left(&mut source, 0.7);
    let requests = env.stream_requests();
    assert_eq!(requests.len(), 1, "{requests:?}");
    assert_eq!(param(&requests[0], "timeOffset"), None);
}

#[test]
fn a_cut_stream_is_offline_after_the_buffered_audio() {
    let env = env(FakeLibrary::synthetic(1, 1).with_wav(SONG, 5));
    env.server.set_fault(Fault::CutOnceThen {
        method: "stream",
        after_bytes: 200_000,
        then: Box::new(Fault::NoRanges),
    });
    let input = env.input(StreamQuality::Original);
    let mut source = env.open(StreamQuality::Original, "wav");
    let (frames, err) = drain(&mut source);
    let err = err.expect("the cut surfaces as an error");
    assert!(crate::audio::is_offline(&err), "{err:#}");
    assert_clean(&err, &input.url);
    assert!(frames > 0, "the buffered audio plays first");
}

#[test]
fn ffmpeg_fallback_reads_stdin() {
    if !has_ffmpeg("ffmpeg_fallback_reads_stdin") {
        return;
    }
    let env = env(FakeLibrary::synthetic(1, 1).with_audio(SONG, fixture("opus.opus")));
    let mut source = NavidromeSource::open(
        env.request(StreamQuality::Original, "opus"),
        Some(StreamSpec {
            sample_rate: 44_100,
            channels: 2,
        }),
    )
    .unwrap_or_else(|err| panic!("open failed: {err:#}"));
    assert_eq!(
        source.spec(),
        StreamSpec {
            sample_rate: 44_100,
            channels: 2
        },
        "FFmpeg decodes at the device rate"
    );
    read_ms(&mut source, 600);
    // A seek restarts FFmpeg over the buffered bytes: no new request.
    source.seek(500).unwrap();
    assert_seconds_left(&mut source, 0.5);
    assert_eq!(env.stream_requests().len(), 1);
}

#[test]
fn ffmpeg_reports_a_cut_stream_as_offline() {
    if !has_ffmpeg("ffmpeg_reports_a_cut_stream_as_offline") {
        return;
    }
    let env = env(FakeLibrary::synthetic(1, 1).with_wav(SONG, 10));
    env.server.set_fault(Fault::CutOnceThen {
        method: "stream",
        after_bytes: 300_000,
        then: Box::new(Fault::NoRanges),
    });
    let input = env.input(StreamQuality::Original);
    let mut source = FfmpegSource::open(&Input::Http(input.clone()), DEVICE)
        .unwrap_or_else(|err| panic!("open failed: {err:#}"));
    let (frames, err) = drain(&mut source);
    let err = err.expect("a cut stream is an error, not the end of the track");
    assert!(crate::audio::is_offline(&err), "{err:#}");
    assert_clean(&err, &input.url);
    assert!(frames > 0);
}

#[test]
fn ffmpeg_args_carry_no_url_or_token() {
    let env = env(FakeLibrary::synthetic(1, 1).with_wav(SONG, 1));
    let input = env.input(StreamQuality::Kbps192);
    let secrets: Vec<String> = Url::parse(input.url.expose())
        .unwrap()
        .query_pairs()
        .filter(|(key, _)| key == "t" || key == "s")
        .map(|(_, value)| value.into_owned())
        .collect();
    assert_eq!(secrets.len(), 2);
    for start_ms in [0, 1500] {
        let args = ffmpeg_args(&Input::Http(input.clone()), 48_000, start_ms);
        let args: Vec<String> = args
            .iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        assert!(args.iter().any(|arg| arg == "pipe:0"), "{args:?}");
        for arg in &args {
            for needle in ["http", "rest/", "t=", "127.0.0.1", FAKE_PASSWORD] {
                assert!(!arg.contains(needle), "{needle:?} in {args:?}");
            }
            for secret in &secrets {
                assert!(!arg.contains(secret.as_str()), "a secret in {args:?}");
            }
        }
        // A pipe cannot seek: `-ss` comes after `-i` (decode and drop).
        if start_ms > 0 {
            let input_at = args.iter().position(|a| a == "-i").unwrap();
            let seek_at = args.iter().position(|a| a == "-ss").unwrap();
            assert!(seek_at > input_at, "{args:?}");
        }
    }
}

#[test]
fn dropping_an_ffmpeg_stream_ends_the_feeder() {
    if !has_ffmpeg("dropping_an_ffmpeg_stream_ends_the_feeder") {
        return;
    }
    // 30 s of WAV (5 MB): nobody reads FFmpeg's output, so FFmpeg stops reading its stdin and
    // the feeder blocks in its write.
    let env = env(FakeLibrary::synthetic(1, 1).with_wav(SONG, 30));
    let input = env.input(StreamQuality::Original);
    let source = FfmpegSource::open(&Input::Http(input), DEVICE)
        .unwrap_or_else(|err| panic!("open failed: {err:#}"));
    let done = source.feeder_done().expect("a stream has a feeder");
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(
        done.try_recv(),
        Err(crossbeam_channel::TryRecvError::Empty),
        "the feeder is still running"
    );
    let started = Instant::now();
    drop(source);
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "dropping does not wait"
    );
    assert_eq!(
        done.recv_timeout(Duration::from_secs(2)),
        Err(RecvTimeoutError::Disconnected),
        "the feeder ended"
    );
    // The feeder's cursor is gone with it: the spill file too.
    let deadline = Instant::now() + Duration::from_secs(2);
    while env.spill_files() > 0 {
        assert!(Instant::now() < deadline, "the spill file is left behind");
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn an_ffmpeg_seek_stops_the_old_feeder_first() {
    if !has_ffmpeg("an_ffmpeg_seek_stops_the_old_feeder_first") {
        return;
    }
    let env = env(FakeLibrary::synthetic(1, 1).with_wav(SONG, 30));
    let input = env.input(StreamQuality::Original);
    let mut source = FfmpegSource::open(&Input::Http(input), DEVICE).unwrap();
    let first = source.feeder_done().unwrap();
    source.seek(1000).unwrap();
    assert_eq!(
        first.try_recv(),
        Err(crossbeam_channel::TryRecvError::Disconnected),
        "the old feeder ended before the new one started"
    );
    assert!(source.read(&mut Vec::new()).unwrap() > 0);
    assert_eq!(env.stream_requests().len(), 1);
}

#[test]
fn not_connected_is_offline() {
    let svc = service(None);
    let spill = tempfile::tempdir().unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let opener = navidrome_opener(
        Arc::clone(&svc.service),
        Arc::clone(&svc.settings),
        spill.path().to_path_buf(),
        counting_fallback(&calls),
    );
    let err = opener(&track("navidrome:tr-0", Source::Navidrome, "flac"), DEVICE)
        .err()
        .expect("no server, no stream");
    assert!(crate::audio::is_offline(&err), "{err:#}");
    assert_eq!(err.to_string(), NOT_CONNECTED);
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[test]
fn local_tracks_use_the_fallback() {
    let svc = service(None);
    let spill = tempfile::tempdir().unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let opener = navidrome_opener(
        Arc::clone(&svc.service),
        Arc::clone(&svc.settings),
        spill.path().to_path_buf(),
        counting_fallback(&calls),
    );
    let err = opener(&track("local-1", Source::Local, "flac"), DEVICE)
        .err()
        .expect("the fallback fails");
    assert_eq!(err.to_string(), "the fallback opener");
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[test]
fn an_unreachable_server_is_offline_and_clean() {
    let env = env(FakeLibrary::synthetic(1, 1).with_wav(SONG, 1));
    let url = env.client.stream_url(SONG, StreamQuality::Original, None);
    env.server.set_fault(Fault::Refuse);
    let err = NavidromeSource::open(env.request(StreamQuality::Original, "wav"), None)
        .err()
        .expect("open fails");
    assert!(crate::audio::is_offline(&err), "{err:#}");
    assert_clean(&err, &url);
}

#[test]
fn a_wav_stream_decodes_at_its_native_rate() {
    let env = env(FakeLibrary::synthetic(1, 1).with_wav(SONG, 1));
    let source = env.open(StreamQuality::Original, "wav");
    assert_eq!(
        source.spec(),
        StreamSpec {
            sample_rate: WAV_RATE,
            channels: 2
        }
    );
    assert_eq!(source.duration_ms(), Some(1000));
}
