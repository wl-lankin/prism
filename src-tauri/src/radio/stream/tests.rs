use std::io;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::*;
use crate::audio::is_offline;
use crate::audio::live::testserver::{Body, FakeRadio, Fault};
use crate::audio::live::USER_AGENT;
use crate::library::model::{Source, Track};
use crate::radio::model::station_id;
use crate::tools;

const LABEL: &str = "Demo Radio 1";
const UUID: &str = "9617a958-0601-11e8-ae97-52543be04c81";
const DEVICE: StreamSpec = StreamSpec {
    sample_rate: 48_000,
    channels: 2,
};

fn has_ffmpeg(test: &str) -> bool {
    let found = tools::ffmpeg_path().is_some();
    if !found {
        println!("ffmpeg not found: skipping {test}");
    }
    found
}

fn config(outage: Duration) -> Arc<LiveConfig> {
    Arc::new(LiveConfig::for_tests(
        256 * 1024,
        Duration::from_secs(2),
        outage,
        vec![Duration::from_millis(50), Duration::from_millis(100)],
    ))
}

/// A title sink that records what it is given.
fn recorder() -> (TitleSink, Arc<Mutex<Vec<Option<String>>>>) {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let sink_seen = Arc::clone(&seen);
    let sink: TitleSink = Arc::new(move |title| sink_seen.lock().unwrap().push(title));
    (sink, seen)
}

fn station(url: String, codec: Option<&str>, hls: bool) -> StationStream {
    StationStream {
        url,
        label: LABEL.into(),
        codec: codec.map(str::to_owned),
        hls,
        config: config(Duration::from_secs(2)),
        titles: recorder().0,
    }
}

fn open(station: StationStream) -> RadioSource {
    RadioSource::open(station, DEVICE).unwrap_or_else(|err| panic!("open failed: {err:#}"))
}

fn tone(server: &FakeRadio, path: &str, metaint: Option<usize>, titles: &[&str]) -> String {
    server.station(
        path,
        Body::Mp3Tone {
            seconds_per_loop: 0.0,
        },
        metaint,
        titles,
    );
    server.url(path)
}

fn ogg(server: &FakeRadio, path: &str) -> String {
    server.station(path, Body::Ogg, None, &[]);
    server.url(path)
}

/// A station that sends a fixture file back to back.
fn looped(
    server: &FakeRadio,
    path: &str,
    name: &'static str,
    content_type: &'static str,
) -> String {
    server.station(
        path,
        Body::Fixture {
            name,
            content_type,
            endless: true,
        },
        None,
        &[],
    );
    server.url(path)
}

fn mka(server: &FakeRadio, path: &str) -> String {
    server.station(
        path,
        Body::Fixture {
            name: "radio/tone.mka",
            content_type: "audio/x-matroska",
            endless: true,
        },
        None,
        &[],
    );
    server.url(path)
}

/// What a read loop got: interleaved samples, or the error that ended it.
struct Heard {
    samples: Vec<f32>,
    frames: usize,
    error: Option<anyhow::Error>,
}

/// Reads at least `seconds` of audio (or until an error, or `limit`).
fn listen(source: &mut RadioSource, seconds: f32, limit: Duration) -> Heard {
    let spec = source.spec();
    let wanted = (spec.sample_rate as f32 * seconds) as usize;
    let mut samples = Vec::new();
    let mut frames = 0;
    let started = Instant::now();
    while frames < wanted && started.elapsed() < limit {
        let before = samples.len();
        match source.read(&mut samples) {
            Ok(0) => break,
            Ok(read) => {
                assert_eq!(
                    samples.len() - before,
                    read * spec.channels as usize,
                    "read returns frames in the fixed spec"
                );
                frames += read;
            }
            Err(err) => {
                return Heard {
                    samples,
                    frames,
                    error: Some(err),
                }
            }
        }
    }
    Heard {
        samples,
        frames,
        error: None,
    }
}

fn rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    (samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32).sqrt()
}

/// Frames in which every sample is exactly 0.
fn silent_frames(samples: &[f32], channels: usize) -> usize {
    samples
        .chunks(channels)
        .filter(|frame| frame.iter().all(|s| *s == 0.0))
        .count()
}

fn assert_heard(heard: &Heard, spec: StreamSpec, seconds: f32) {
    if let Some(err) = &heard.error {
        panic!("reading failed: {err:#}");
    }
    let wanted = (spec.sample_rate as f32 * seconds) as usize;
    assert!(
        heard.frames >= wanted,
        "{} of {wanted} frames",
        heard.frames
    );
    let level = rms(&heard.samples);
    assert!(level > 0.05, "the tone is audible, rms {level}");
}

fn requests_for(server: &FakeRadio, path: &str) -> usize {
    server.requests().iter().filter(|r| r.path == path).count()
}

/// The process exists (running or a zombie nobody reaped).
fn process_exists(pid: u32) -> bool {
    #[cfg(unix)]
    {
        let out = std::process::Command::new("ps")
            .args(["-o", "stat=", "-p", &pid.to_string()])
            .output()
            .expect("running ps");
        !String::from_utf8_lossy(&out.stdout).trim().is_empty()
    }
    #[cfg(windows)]
    {
        let out = tools::command(std::path::Path::new("tasklist"))
            .args(["/FI", &format!("PID eq {pid}"), "/NH", "/FO", "CSV"])
            .output()
            .expect("running tasklist");
        String::from_utf8_lossy(&out.stdout).contains(&format!("\"{pid}\""))
    }
}

fn wait_until(limit: Duration, mut done: impl FnMut() -> bool) -> bool {
    let started = Instant::now();
    while started.elapsed() < limit {
        if done() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    done()
}

#[test]
fn mp3_station_decodes_audio() {
    let server = FakeRadio::start();
    let mut source = open(station(
        tone(&server, "/mp3", None, &[]),
        Some("MP3"),
        false,
    ));
    assert_eq!(source.decoder_kind(), DecoderKind::Symphonia);
    let spec = source.spec();
    assert_eq!(
        spec,
        StreamSpec {
            sample_rate: 44_100,
            channels: 2
        }
    );
    let heard = listen(&mut source, 1.0, Duration::from_secs(10));
    assert_heard(&heard, spec, 1.0);
}

#[test]
fn ogg_station_decodes_audio() {
    let server = FakeRadio::start();
    let mut source = open(station(ogg(&server, "/ogg"), Some("OGG"), false));
    assert_eq!(source.decoder_kind(), DecoderKind::Symphonia);
    let spec = source.spec();
    assert_eq!(
        spec,
        StreamSpec {
            sample_rate: 22_050,
            channels: 1
        }
    );
    let heard = listen(&mut source, 0.9, Duration::from_secs(10));
    assert_heard(&heard, spec, 0.9);
}

#[test]
fn unknown_codec_falls_back_to_ffmpeg() {
    if !has_ffmpeg("unknown_codec_falls_back_to_ffmpeg") {
        return;
    }
    let server = FakeRadio::start();
    let mut source = open(station(mka(&server, "/mka"), None, false));
    assert_eq!(source.decoder_kind(), DecoderKind::FfmpegReader);
    assert_eq!(source.spec(), DEVICE, "FFmpeg decodes at the device rate");
    let heard = listen(&mut source, 1.0, Duration::from_secs(15));
    assert_heard(&heard, DEVICE, 1.0);
}

#[test]
fn hls_goes_to_ffmpeg_by_url() {
    if !has_ffmpeg("hls_goes_to_ffmpeg_by_url") {
        return;
    }
    let server = FakeRadio::start();
    let playlist = server.hls_station("/hls");
    // The directory's hls flag, and an HLS playlist found behind a plain station URL.
    for hls in [true, false] {
        let mut source = open(station(server.url(&playlist), Some("AAC"), hls));
        assert_eq!(source.decoder_kind(), DecoderKind::FfmpegUrl, "hls {hls}");
        assert_eq!(source.spec(), DEVICE);
        let heard = listen(&mut source, 1.0, Duration::from_secs(15));
        assert_heard(&heard, DEVICE, 1.0);
    }
    let segments: Vec<_> = server
        .requests()
        .into_iter()
        .filter(|r| r.path == "/hls/seg1.ts")
        .collect();
    assert!(!segments.is_empty(), "FFmpeg fetched the segments");
    for request in segments {
        assert_eq!(request.user_agent.as_deref(), Some(USER_AGENT));
    }
}

#[test]
fn hls_errors_name_the_station_not_the_url() {
    if !has_ffmpeg("hls_errors_name_the_station_not_the_url") {
        return;
    }
    let server = FakeRadio::start();
    let err = RadioSource::open(
        station(server.url("/missing/live.m3u8"), None, true),
        DEVICE,
    )
    .err()
    .expect("a missing playlist fails");
    assert!(!is_offline(&err), "{err:#}");
    for text in [format!("{err}"), format!("{err:#}"), format!("{err:?}")] {
        assert!(text.contains(LABEL), "{text}");
        assert!(!text.contains("://"), "{text}");
        assert!(!text.contains("/missing"), "{text}");
    }
}

#[test]
fn seek_is_refused() {
    let server = FakeRadio::start();
    let mut source = open(station(
        tone(&server, "/mp3", None, &[]),
        Some("MP3"),
        false,
    ));
    let err = source.seek(1_000).expect_err("a station cannot seek");
    assert!(format!("{err:#}").contains("cannot seek"), "{err:#}");
    let heard = listen(&mut source, 0.2, Duration::from_secs(5));
    assert!(
        heard.error.is_none(),
        "it keeps playing after a refused seek"
    );
}

#[test]
fn duration_is_unknown() {
    let server = FakeRadio::start();
    let source = open(station(
        tone(&server, "/mp3", None, &[]),
        Some("MP3"),
        false,
    ));
    assert_eq!(source.duration_ms(), None);
}

#[test]
fn mp3_joint_continues_without_a_reopen() {
    let server = FakeRadio::start();
    let url = tone(&server, "/mp3", None, &[]);
    server.set_fault(Fault::CutAfter(64 * 1024));
    let mut source = open(station(url, Some("MP3"), false));
    // Far past the cut (64 KiB is about 4 s at 128 kbps).
    let heard = listen(&mut source, 8.0, Duration::from_secs(20));
    assert_heard(&heard, source.spec(), 8.0);
    assert_eq!(
        source.reopens(),
        0,
        "MP3 resyncs on the joint, no fresh decoder"
    );
    assert_eq!(
        requests_for(&server, "/mp3"),
        2,
        "the open and one reconnect"
    );
}

/// `radio/chain.ogg`: a 2 s link at 22.05 kHz, then a 2 s link at 44.1 kHz, back to back.
fn chain(server: &FakeRadio, path: &str) -> String {
    looped(server, path, "radio/chain.ogg", "application/ogg")
}

/// `radio/short.ogg`: 0.2 s links, each too short to follow, back to back.
fn short_links(server: &FakeRadio, path: &str) -> String {
    looped(server, path, "radio/short.ogg", "application/ogg")
}

const CHAIN_FIRST: StreamSpec = StreamSpec {
    sample_rate: 22_050,
    channels: 2,
};

#[test]
fn ogg_chains_continue_without_a_reconnect() {
    let server = FakeRadio::start();
    let mut source = open(station(chain(&server, "/chain"), Some("OGG"), false));
    assert_eq!(source.spec(), CHAIN_FIRST);
    // Two links of 2 s each and into the third.
    let heard = listen(&mut source, 5.0, Duration::from_secs(20));
    assert_heard(&heard, CHAIN_FIRST, 5.0);
    assert_eq!(source.reopens(), 0, "no reconnect");
    assert_eq!(
        source.fresh_decoders(),
        0,
        "the decoder follows the chain itself"
    );
    assert_eq!(requests_for(&server, "/chain"), 1);
    let silence = silent_frames(&heard.samples, 2);
    assert!(
        silence <= CHAIN_FIRST.sample_rate as usize / 50,
        "{silence} silent frames across the chain links"
    );
}

#[test]
fn a_chain_link_at_another_rate_keeps_the_first_spec() {
    let server = FakeRadio::start();
    let mut source = open(station(chain(&server, "/chain"), Some("OGG"), false));
    let heard = listen(&mut source, 3.0, Duration::from_secs(20));
    assert_heard(&heard, CHAIN_FIRST, 3.0);
    assert_eq!(
        source.decoder_spec(),
        Some(StreamSpec {
            sample_rate: 44_100,
            channels: 2
        }),
        "1 s into the second link"
    );
    assert_eq!(
        source.spec(),
        CHAIN_FIRST,
        "the spec handed out stays fixed"
    );
    // Back at the first link's rate: the adapter goes again.
    let heard = listen(&mut source, 2.0, Duration::from_secs(20));
    assert_heard(&heard, CHAIN_FIRST, 2.0);
    assert_eq!(source.decoder_spec(), Some(CHAIN_FIRST));
    assert_eq!(source.reopens(), 0);
}

#[test]
fn reopens_are_capped() {
    let server = FakeRadio::start();
    let mut source = open(station(short_links(&server, "/short"), Some("OGG"), false));
    let heard = listen(&mut source, 600.0, Duration::from_secs(30));
    let err = heard.error.expect("the decoder keeps breaking");
    assert!(
        is_offline(&err),
        "a played station is held, not ended: {err:#}"
    );
    assert!(format!("{err:#}").contains(LABEL), "{err:#}");
    assert_eq!(source.reopens(), MAX_REOPENS);
    assert_eq!(requests_for(&server, "/short"), 1 + MAX_REOPENS);
}

#[test]
fn a_failed_reconnect_after_play_is_offline() {
    let server = FakeRadio::start();
    let mut live = station(short_links(&server, "/short"), Some("OGG"), false);
    live.config = config(Duration::from_secs(1));
    let mut source = open(live);
    // The connection that is open keeps streaming; every new request gets a 404 (an Icecast
    // mount while its encoder reconnects).
    server.set_fault(Fault::Status(404));
    let started = Instant::now();
    let heard = listen(&mut source, 600.0, Duration::from_secs(20));
    let err = heard.error.expect("the station does not come back");
    assert!(is_offline(&err), "{err:#}");
    assert!(format!("{err:#}").contains(LABEL), "{err:#}");
    assert!(started.elapsed() < Duration::from_secs(10));
    assert_eq!(source.reopens(), 1);
    assert!(
        requests_for(&server, "/short") > 2,
        "the reconnect was retried within the outage"
    );
}

#[test]
fn aac_stations_go_to_ffmpeg() {
    if !has_ffmpeg("aac_stations_go_to_ffmpeg") {
        return;
    }
    let server = FakeRadio::start();
    for (path, content_type, codec) in [
        ("/aacp", "audio/aacp", None),
        ("/aac", "audio/aac", Some("MP3")),
        ("/plus", "application/octet-stream", Some("AAC+")),
    ] {
        let url = looped(&server, path, "radio/tone.aac", content_type);
        let mut source = open(station(url, codec, false));
        assert_eq!(source.decoder_kind(), DecoderKind::FfmpegReader, "{path}");
        assert_eq!(source.spec(), DEVICE);
        let heard = listen(&mut source, 1.0, Duration::from_secs(15));
        assert_heard(&heard, DEVICE, 1.0);
    }
}

#[test]
fn only_stream_failures_skip_the_ffmpeg_fallback() {
    use crate::audio::symph::ChainFailed;
    assert!(!reader_failed(&anyhow!("malformed stream: bad frame")));
    assert!(!reader_failed(&anyhow::Error::new(ChainFailed(
        "no audio".into()
    ))));
    let offline = io::Error::new(io::ErrorKind::ConnectionAborted, Offline("gone".into()));
    assert!(reader_failed(
        &anyhow::Error::new(offline).context("reading")
    ));
    let failed = io::Error::other(StreamFailure("the station stream was closed".into()));
    assert!(reader_failed(&anyhow::Error::new(failed)));
}

#[test]
fn an_ffmpeg_reader_station_goes_offline() {
    if !has_ffmpeg("an_ffmpeg_reader_station_goes_offline") {
        return;
    }
    let server = FakeRadio::start();
    let mut live = station(
        looped(&server, "/aacp", "radio/tone.aac", "audio/aacp"),
        None,
        false,
    );
    live.config = config(Duration::from_secs(1));
    let mut source = open(live);
    assert_eq!(source.decoder_kind(), DecoderKind::FfmpegReader);
    assert!(listen(&mut source, 0.3, Duration::from_secs(5))
        .error
        .is_none());
    server.stop();
    let heard = listen(&mut source, 6_000.0, Duration::from_secs(30));
    let err = heard.error.expect("the station is gone");
    assert!(is_offline(&err), "{err:#}");
    assert_eq!(source.reopens(), 0);
}

#[test]
fn an_hls_station_that_goes_away_is_offline() {
    if !has_ffmpeg("an_hls_station_that_goes_away_is_offline") {
        return;
    }
    let server = FakeRadio::start();
    let playlist = server.hls_station("/hls");
    let mut source = open(station(server.url(&playlist), None, true));
    assert!(listen(&mut source, 0.2, Duration::from_secs(10))
        .error
        .is_none());
    server.set_fault(Fault::Refuse);
    let heard = listen(&mut source, 6_000.0, Duration::from_secs(60));
    let err = heard.error.expect("the station is gone");
    assert!(is_offline(&err), "{err:#}");
    assert!(!format!("{err:#}").contains("://"), "{err:#}");
}

#[test]
fn offline_stays_offline() {
    let server = FakeRadio::start();
    let url = tone(&server, "/mp3", None, &[]);
    let mut live = station(url, Some("MP3"), false);
    live.config = config(Duration::from_secs(1));
    let mut source = open(live);
    let heard = listen(&mut source, 0.3, Duration::from_secs(5));
    assert!(heard.error.is_none());
    server.stop();
    let started = Instant::now();
    let heard = listen(&mut source, 600.0, Duration::from_secs(15));
    let err = heard.error.expect("the station is gone");
    assert!(is_offline(&err), "{err:#}");
    assert!(started.elapsed() < Duration::from_secs(10));
    assert_eq!(source.reopens(), 0, "offline is never reopened");
}

#[test]
fn titles_reset_on_open() {
    let server = FakeRadio::start();
    let url = tone(&server, "/mp3", Some(8_192), &["A - One"]);
    let (sink, seen) = recorder();
    let mut live = station(url, Some("MP3"), false);
    live.titles = sink;
    let mut source = open(live);
    assert_eq!(
        seen.lock().unwrap().first(),
        Some(&None),
        "None comes first"
    );
    assert!(wait_until(Duration::from_secs(5), || {
        let _ = listen(&mut source, 0.1, Duration::from_secs(1));
        seen.lock().unwrap().contains(&Some("A - One".to_string()))
    }));
    drop(source);
    // A second station starts clean again.
    let mut live = station(server.url("/mp3"), Some("MP3"), false);
    live.titles = {
        let seen = Arc::clone(&seen);
        Arc::new(move |title| seen.lock().unwrap().push(title))
    };
    let before = seen.lock().unwrap().len();
    let _source = open(live);
    assert_eq!(seen.lock().unwrap().get(before), Some(&None));
}

fn demo_station(url: String) -> Station {
    Station {
        id: station_id(UUID).unwrap(),
        name: LABEL.into(),
        url: "http://127.0.0.1:9/unused".into(),
        url_resolved: Some(url),
        homepage: None,
        favicon: None,
        cover_id: None,
        country_code: Some("DE".into()),
        region: Some("Testland".into()),
        language: None,
        tags: Vec::new(),
        codec: Some("MP3".into()),
        bitrate_kbps: Some(128),
        hls: false,
        last_check_ok: true,
        distance_km: None,
    }
}

#[test]
fn other_sources_go_to_the_fallback() {
    let server = FakeRadio::start();
    let known = demo_station(tone(&server, "/mp3", None, &[]));
    let stations = [known.clone()];
    let lookup: StationLookup =
        Arc::new(move |id: &str| stations.iter().find(|s| s.id == id).cloned());
    let fallen = Arc::new(Mutex::new(Vec::new()));
    let fallback: Opener = {
        let fallen = Arc::clone(&fallen);
        Arc::new(move |track: &Track, _spec: StreamSpec| {
            fallen.lock().unwrap().push(track.id.clone());
            anyhow::bail!("the fallback opener")
        })
    };
    let asked = Arc::new(Mutex::new(Vec::new()));
    let (sink, seen) = recorder();
    let titles: TitleSinks = {
        let asked = Arc::clone(&asked);
        Arc::new(move |id: &str| {
            asked.lock().unwrap().push(id.to_owned());
            Arc::clone(&sink)
        })
    };
    let opener = radio_opener(lookup, config(Duration::from_secs(2)), titles, fallback);

    let mut local = known.as_track();
    local.id = "local:1".into();
    local.source = Source::Local;
    assert!(opener(&local, DEVICE).is_err());
    assert_eq!(*fallen.lock().unwrap(), vec!["local:1".to_string()]);

    let mut source = opener(&known.as_track(), DEVICE).expect("the station opens");
    assert_eq!(source.duration_ms(), None);
    assert!(source.read(&mut Vec::new()).unwrap() > 0);
    assert_eq!(requests_for(&server, "/mp3"), 1, "it plays url_resolved");
    assert_eq!(
        *asked.lock().unwrap(),
        vec![known.id.clone()],
        "the titles go to the sink of the station that sends them"
    );
    assert_eq!(seen.lock().unwrap().first(), Some(&None));

    let mut unknown = known.as_track();
    unknown.id = station_id("00000000-0000-4000-8000-000000000000").unwrap();
    let err = opener(&unknown, DEVICE).err().expect("an unknown station");
    assert!(!is_offline(&err), "{err:#}");
    assert_eq!(fallen.lock().unwrap().len(), 1, "radio never falls back");
    assert_eq!(
        asked.lock().unwrap().len(),
        1,
        "no sink for an unknown station"
    );
}

#[test]
fn ffmpeg_children_are_reaped() {
    if !has_ffmpeg("ffmpeg_children_are_reaped") {
        return;
    }
    let server = FakeRadio::start();
    let playlist = server.hls_station("/hls");
    for live in [
        station(mka(&server, "/mka"), None, false),
        station(server.url(&playlist), None, true),
    ] {
        let mut source = open(live);
        let pid = source.ffmpeg_pid().expect("FFmpeg decodes this station");
        assert!(process_exists(pid));
        let _ = listen(&mut source, 0.2, Duration::from_secs(10));
        drop(source);
        assert!(
            wait_until(Duration::from_secs(2), || !process_exists(pid)),
            "ffmpeg {pid} is killed and reaped"
        );
    }
    assert!(
        wait_until(Duration::from_secs(2), || server.connections() == 0),
        "no connection stays open"
    );
}

#[test]
fn hints_come_from_the_content_type_then_the_codec() {
    assert_eq!(decoder_hint(Some("audio/mpeg"), Some("AAC")), Some("mp3"));
    assert_eq!(decoder_hint(Some("audio/aacp"), None), Some("aac"));
    assert_eq!(decoder_hint(Some("application/ogg"), None), Some("ogg"));
    assert_eq!(decoder_hint(Some("audio/flac"), None), Some("flac"));
    assert_eq!(
        decoder_hint(Some("application/octet-stream"), Some("AAC+")),
        Some("aac")
    );
    assert_eq!(decoder_hint(None, Some("MP3")), Some("mp3"));
    assert_eq!(decoder_hint(None, Some("OGG")), Some("ogg"));
    assert_eq!(decoder_hint(None, Some("UNKNOWN")), None);
    assert_eq!(decoder_hint(None, None), None);
}
