use std::io::{Read, Seek, SeekFrom};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::testserver::{Body, FakeRadio, Fault};
use super::*;
use crate::audio::http::{is_offline, is_stream_failure, StreamFailure};

const LABEL: &str = "Demo Radio 1";

fn config(ring: usize, outage: Duration) -> Arc<LiveConfig> {
    Arc::new(LiveConfig::for_tests(
        ring,
        Duration::from_secs(2),
        outage,
        vec![Duration::from_millis(50), Duration::from_millis(100)],
    ))
}

fn input(url: String, config: &Arc<LiveConfig>, titles: Option<TitleSink>) -> LiveInput {
    LiveInput {
        url,
        label: LABEL.into(),
        codec_hint: Some("mp3".into()),
        config: Arc::clone(config),
        titles,
    }
}

fn tone_station(server: &FakeRadio, path: &str, metaint: Option<usize>, titles: &[&str]) {
    server.station(
        path,
        Body::Mp3Tone {
            seconds_per_loop: 0.0,
        },
        metaint,
        titles,
    );
}

fn open(input: &LiveInput) -> LiveStream {
    LiveStream::open(input).unwrap_or_else(|err| panic!("open failed: {err:#}"))
}

fn read_n(reader: &mut LiveReader, len: usize) -> Vec<u8> {
    let mut out = vec![0; len];
    reader
        .read_exact(&mut out)
        .unwrap_or_else(|err| panic!("read failed: {err}"));
    out
}

/// The tone looped from its start, `len` bytes.
fn looped(tone: &[u8], len: usize) -> Vec<u8> {
    tone.iter().copied().cycle().take(len).collect()
}

/// A title sink that records what it is given.
fn recorder() -> (TitleSink, Arc<Mutex<Vec<Option<String>>>>) {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let sink_seen = Arc::clone(&seen);
    let sink: TitleSink = Arc::new(move |title| sink_seen.lock().unwrap().push(title));
    (sink, seen)
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

/// No rendering of the error carries the URL or its path; the label and the host do.
fn assert_clean(err: &anyhow::Error) {
    for text in [format!("{err}"), format!("{err:#}"), format!("{err:?}")] {
        for needle in ["http://", "/station", ".pls", ".m3u"] {
            assert!(!text.contains(needle), "{needle:?} in {text:?}");
        }
    }
    let text = format!("{err:#}");
    assert!(text.contains(LABEL), "{text}");
    assert!(text.contains("127.0.0.1"), "{text}");
}

/// Feeds an ICY byte stream through a demux in pieces of `piece` bytes.
fn demux_all(metaint: usize, wire: &[u8], piece: usize) -> (Vec<u8>, Vec<String>) {
    let mut demux = IcyDemux::new(Some(metaint));
    let (mut audio, mut titles) = (Vec::new(), Vec::new());
    for part in wire.chunks(piece) {
        demux.feed(part, &mut audio, &mut titles);
    }
    (audio, titles)
}

fn meta(text: &[u8]) -> Vec<u8> {
    let blocks = text.len().div_ceil(16);
    let mut out = vec![blocks as u8];
    out.extend_from_slice(text);
    out.resize(1 + blocks * 16, 0);
    out
}

#[test]
fn icy_titles_are_cut_out_and_reported() {
    let server = FakeRadio::start();
    tone_station(&server, "/station", Some(8192), &["A - One", "B - Two"]);
    let config = config(256 * 1024, Duration::from_secs(5));
    let (sink, seen) = recorder();
    let stream = open(&input(server.url("/station"), &config, Some(sink)));
    assert_eq!(stream.content_type(), Some("audio/mpeg"));
    assert_eq!(stream.icy_name(), Some("Demo Radio"));
    let mut reader = stream.reader();
    let out = read_n(&mut reader, 300_000);
    assert!(
        out == looped(server.tone(), out.len()),
        "the audio bytes differ"
    );
    let seen = seen.lock().unwrap().clone();
    assert_eq!(
        seen,
        [Some("A - One".to_owned()), Some("B - Two".to_owned())]
    );
}

#[test]
fn icy_demux_handles_any_split() {
    let mut wire = b"abcd".to_vec();
    wire.extend(meta(b"StreamTitle='One';"));
    wire.extend(b"efgh");
    wire.push(0);
    wire.extend(b"ijkl");
    wire.extend(meta(b"StreamTitle='Two';StreamUrl='x';"));
    wire.extend(b"mn");
    for piece in 1..wire.len() {
        let (audio, titles) = demux_all(4, &wire, piece);
        assert_eq!(audio, b"abcdefghijklmn", "piece {piece}");
        assert_eq!(titles, ["One", "Two"], "piece {piece}");
    }
}

#[test]
fn latin1_title_decodes() {
    let mut wire = b"abcd".to_vec();
    wire.extend(meta(b"StreamTitle='Caf\xe9';"));
    wire.extend(b"efgh");
    let (audio, titles) = demux_all(4, &wire, 3);
    assert_eq!(audio, b"abcdefgh");
    assert_eq!(titles, ["Café"]);
    // UTF-8 stays UTF-8.
    assert_eq!(
        stream_title("StreamTitle='Caf\u{e9} \u{2013} Zoë';".as_bytes()).as_deref(),
        Some("Caf\u{e9} \u{2013} Zoë")
    );
}

#[test]
fn titles_are_cleaned_and_capped() {
    assert_eq!(
        stream_title(b"StreamTitle='\x07A\x00 - B\r\n';").as_deref(),
        Some("A - B")
    );
    assert_eq!(stream_title(b"StreamTitle='';"), None);
    assert_eq!(stream_title(b"StreamTitle='   ';"), None);
    assert_eq!(stream_title(b"StreamUrl='x';"), None);
    // A quote inside the title, the end found at the closing `';`.
    assert_eq!(
        stream_title(b"StreamTitle='Rock 'n' Roll';StreamUrl='';").as_deref(),
        Some("Rock 'n' Roll")
    );
    // No `';`: up to the last quote before the NUL padding.
    assert_eq!(
        stream_title(b"StreamTitle='Open'\0\0\0").as_deref(),
        Some("Open")
    );
    let long = format!("StreamTitle='{}';", "\u{e9}".repeat(1000));
    let title = stream_title(long.as_bytes()).unwrap();
    assert_eq!(title.chars().count(), 256);
}

#[test]
fn garbage_metadata_never_panics() {
    // Random wire bytes through the demux: no panic, nothing grows past its bound.
    let mut seed = 0x2545_f491_4f6c_dd1du64;
    let mut random = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed as u8
    };
    for metaint in [1, 7, 16, 8192] {
        let wire: Vec<u8> = (0..200_000).map(|_| random()).collect();
        let (audio, titles) = demux_all(metaint, &wire, 977);
        assert!(audio.len() <= wire.len());
        for title in titles {
            assert!(title.chars().count() <= 256);
            assert!(!title.chars().any(char::is_control), "{title:?}");
        }
    }
    for len in 0..300 {
        let block: Vec<u8> = (0..len).map(|_| random()).collect();
        let mut baited = b"StreamTitle='".to_vec();
        baited.extend_from_slice(&block);
        let _ = stream_title(&block);
        let _ = stream_title(&baited);
    }

    // The same from the fake station: the audio comes through intact.
    let server = FakeRadio::start();
    tone_station(&server, "/station", Some(1000), &["A - One"]);
    server.set_fault(Fault::GarbageMeta);
    let config = config(128 * 1024, Duration::from_secs(5));
    let (sink, seen) = recorder();
    let stream = open(&input(server.url("/station"), &config, Some(sink)));
    let mut reader = stream.reader();
    let out = read_n(&mut reader, 400_000);
    assert!(
        out == looped(server.tone(), out.len()),
        "the audio bytes differ"
    );
    for title in seen.lock().unwrap().iter().flatten() {
        assert!(title.chars().count() <= 256);
        assert!(!title.chars().any(char::is_control), "{title:?}");
    }
}

#[test]
fn huge_metaint_disables_the_demux() {
    for (value, expected) in [
        ("8192", Some(8192)),
        (" 16000 ", Some(16000)),
        ("1", Some(1)),
        ("1048576", Some(1_048_576)),
        ("0", None),
        ("1048577", None),
        ("1000000000", None),
        ("-5", None),
        ("abc", None),
        ("", None),
    ] {
        assert_eq!(parse_metaint(value), expected, "{value:?}");
    }
    assert!(!IcyDemux::new(None).enabled());

    let server = FakeRadio::start();
    tone_station(&server, "/station", Some(8192), &["A - One"]);
    server.set_fault(Fault::HugeMetaint);
    let config = config(128 * 1024, Duration::from_secs(5));
    let (sink, seen) = recorder();
    let stream = open(&input(server.url("/station"), &config, Some(sink)));
    let mut reader = stream.reader();
    let out = read_n(&mut reader, 200_000);
    assert!(
        out == looped(server.tone(), out.len()),
        "the audio bytes differ"
    );
    assert!(seen.lock().unwrap().is_empty());
}

#[test]
fn playlist_entries_parse() {
    let pls = "[playlist]\nNumberOfEntries=2\nFile1=http://example.invalid/a\nTitle1=A\nFile2=http://example.invalid/b\n";
    assert_eq!(
        first_entry(pls).as_deref(),
        Some("http://example.invalid/a")
    );
    let pls_crlf = "\u{feff}[Playlist]\r\nfile1 = https://example.invalid/x?y=1\r\n";
    assert_eq!(
        first_entry(pls_crlf).as_deref(),
        Some("https://example.invalid/x?y=1")
    );
    let pls_skip = "[playlist]\nFile1=rtsp://example.invalid/a\nFile2=http://example.invalid/b\n";
    assert_eq!(
        first_entry(pls_skip).as_deref(),
        Some("http://example.invalid/b")
    );
    let m3u = "#EXTM3U\n#EXTINF:-1,Demo\n\nhttp://example.invalid/live?type=.mp3\n";
    assert_eq!(
        first_entry(m3u).as_deref(),
        Some("http://example.invalid/live?type=.mp3")
    );
    assert_eq!(
        first_entry("relative/path.mp3\nftp://x/y\n# only comments\n"),
        None
    );
    assert_eq!(first_entry(""), None);
    let huge = format!(
        "#EXTM3U\n{}\nhttp://example.invalid/a\n",
        "#".repeat(70_000)
    );
    assert_eq!(first_entry(&huge), None);
}

#[test]
fn pls_and_m3u_resolve_once() {
    let server = FakeRadio::start();
    tone_station(&server, "/tone", None, &[]);
    server.station("/station.pls", Body::Pls("/tone".into()), None, &[]);
    server.station("/station.m3u", Body::M3u("/tone".into()), None, &[]);
    let config = config(64 * 1024, Duration::from_secs(5));
    for list in ["/station.pls", "/station.m3u"] {
        let before = server.requests().len();
        server.set_fault(Fault::CutAfter(100_000));
        let stream = open(&input(server.url(list), &config, None));
        assert_eq!(stream.resolved_url(), server.url("/tone"));
        assert!(!stream.is_hls());
        let mut reader = stream.reader();
        let out = read_n(&mut reader, 300_000);
        assert!(out[..100_000] == looped(server.tone(), 100_000));
        assert_eq!(stream.reconnected(), 1);
        // The playlist once, then the stream twice (the reconnect goes to the entry).
        let paths: Vec<String> = server.requests()[before..]
            .iter()
            .map(|r| r.path.clone())
            .collect();
        assert_eq!(paths, [list, "/tone", "/tone"], "{list}");
    }
}

#[test]
fn playlist_loop_is_an_error() {
    let server = FakeRadio::start();
    server.station(
        "/station-a.pls",
        Body::Pls("/station-b.pls".into()),
        None,
        &[],
    );
    server.station(
        "/station-b.pls",
        Body::Pls("/station-a.pls".into()),
        None,
        &[],
    );
    server.station(
        "/station-c.m3u",
        Body::M3u("/station-c.m3u".into()),
        None,
        &[],
    );
    let config = config(64 * 1024, Duration::from_secs(5));
    for list in ["/station-a.pls", "/station-c.m3u"] {
        let err = LiveStream::open(&input(server.url(list), &config, None))
            .err()
            .expect("a playlist loop fails");
        assert!(!is_offline(&err), "{err:#}");
        assert!(format!("{err:#}").contains(PLAYLIST_NOWHERE), "{err:#}");
        assert_clean(&err);
    }
    // One level: the first playlist and its entry, nothing more.
    assert_eq!(server.requests().len(), 4);
}

#[test]
fn html_is_not_audio() {
    let server = FakeRadio::start();
    server.station("/station", Body::Html, None, &[]);
    let config = config(64 * 1024, Duration::from_secs(5));
    let err = LiveStream::open(&input(server.url("/station"), &config, None))
        .err()
        .expect("an HTML page fails");
    assert!(!is_offline(&err), "{err:#}");
    assert!(err.chain().any(|e| e.is::<StreamFailure>()), "{err:#}");
    assert!(format!("{err:#}").contains(NOT_AUDIO), "{err:#}");
    assert_clean(&err);
}

#[test]
fn not_a_web_address_is_a_plain_failure() {
    let config = config(64 * 1024, Duration::from_secs(5));
    for url in ["ftp://127.0.0.1/station", "not a url", ""] {
        let err = LiveStream::open(&input(url.into(), &config, None))
            .err()
            .expect("a bad address fails");
        assert!(!is_offline(&err), "{err:#}");
    }
}

#[test]
fn status_404_is_a_plain_failure() {
    let server = FakeRadio::start();
    let config = config(64 * 1024, Duration::from_secs(5));
    let err = LiveStream::open(&input(server.url("/station"), &config, None))
        .err()
        .expect("a 404 fails");
    assert!(!is_offline(&err), "{err:#}");
    assert!(
        format!("{err:#}").contains("The station answered 404"),
        "{err:#}"
    );
    assert_clean(&err);
    // No retries for a plain failure.
    assert_eq!(server.requests().len(), 1);
}

#[test]
fn status_503_is_offline() {
    let server = FakeRadio::start();
    tone_station(&server, "/station", None, &[]);
    let config = config(64 * 1024, Duration::from_secs(5));
    for status in [502, 503, 504] {
        server.set_fault(Fault::Status(status));
        let err = LiveStream::open(&input(server.url("/station"), &config, None))
            .err()
            .expect("a proxy error fails");
        assert!(is_offline(&err), "{status}: {err:#}");
        assert_clean(&err);
    }
    server.set_fault(Fault::Refuse);
    let err = LiveStream::open(&input(server.url("/station"), &config, None))
        .err()
        .expect("a refused connection fails");
    assert!(is_offline(&err), "{err:#}");
    assert_clean(&err);
}

#[test]
fn reconnects_after_a_cut_without_an_error() {
    let server = FakeRadio::start();
    tone_station(&server, "/station", None, &[]);
    server.set_fault(Fault::CutAfter(64 * 1024));
    let config = config(128 * 1024, Duration::from_secs(5));
    let stream = open(&input(server.url("/station"), &config, None));
    let mut reader = stream.reader();
    let out = read_n(&mut reader, 512 * 1024);
    // The second connection starts the loop again; the reader sees one byte stream.
    let mut expected = looped(server.tone(), 64 * 1024);
    expected.extend(looped(server.tone(), out.len() - expected.len()));
    assert!(out == expected, "the bytes around the joint differ");
    assert_eq!(stream.reconnected(), 1);
    assert_eq!(server.requests().len(), 2);
}

#[test]
fn a_stall_reconnects_without_an_error() {
    let server = FakeRadio::start();
    tone_station(&server, "/station", None, &[]);
    server.set_fault(Fault::StallAfter(64 * 1024, Duration::from_secs(10)));
    // The client gives up on the silent connection after the 1 s stall and reconnects.
    let config = Arc::new(LiveConfig::for_tests(
        128 * 1024,
        Duration::from_secs(1),
        Duration::from_secs(5),
        vec![Duration::from_millis(50)],
    ));
    let stream = open(&input(server.url("/station"), &config, None));
    let mut reader = stream.reader();
    let started = Instant::now();
    let out = read_n(&mut reader, 256 * 1024);
    assert!(out[..64 * 1024] == looped(server.tone(), 64 * 1024));
    assert_eq!(stream.reconnected(), 1);
    assert!(started.elapsed() < Duration::from_secs(5));
}

#[test]
fn gives_up_after_the_retry_window() {
    let server = FakeRadio::start();
    tone_station(&server, "/station", None, &[]);
    let config = config(64 * 1024, Duration::from_secs(1));
    let stream = open(&input(server.url("/station"), &config, None));
    let mut reader = stream.reader();
    read_n(&mut reader, 16 * 1024);
    server.stop();
    let started = Instant::now();
    let mut buf = vec![0; 16 * 1024];
    let err = loop {
        match reader.read(&mut buf) {
            Ok(n) => assert!(n > 0, "a live stream never ends"),
            Err(err) => break err,
        }
        assert!(started.elapsed() < Duration::from_secs(10), "no error");
    };
    let took = started.elapsed();
    assert!(is_stream_failure(&err), "{err}");
    let err = anyhow::Error::new(err);
    assert!(is_offline(&err), "{err:#}");
    assert!(format!("{err:#}").contains(OFFLINE_TEXT), "{err:#}");
    assert!(took < Duration::from_secs(3), "took {took:?}");
    assert!(took >= Duration::from_millis(500), "took {took:?}");
}

#[test]
fn ring_stays_bounded_over_64_mib() {
    let server = FakeRadio::start();
    tone_station(&server, "/station", None, &[]);
    let ring = 256 * 1024;
    let config = config(ring, Duration::from_secs(5));
    let stream = open(&input(server.url("/station"), &config, None));
    let tone = server.tone().to_vec();
    let mut reader = stream.reader();
    let mut buf = vec![0; 64 * 1024];
    let mut offset = 0usize;
    let total = 64 * 1024 * 1024 + 1;
    let mut reads = 0u64;
    while offset < total {
        let n = reader.read(&mut buf).unwrap();
        // Compare in runs against the looped tone, without building 64 MiB.
        let mut checked = 0;
        while checked < n {
            let at = (offset + checked) % tone.len();
            let run = (tone.len() - at).min(n - checked);
            assert!(
                buf[checked..checked + run] == tone[at..at + run],
                "bytes differ at {}",
                offset + checked
            );
            checked += run;
        }
        offset += n;
        reads += 1;
        // A slow reader: the download runs into the full ring again and again.
        if reads.is_multiple_of(64) {
            std::thread::sleep(Duration::from_millis(2));
        }
    }
    assert!(stream.peak_buffered() <= ring, "{}", stream.peak_buffered());
    assert_eq!(stream.peak_buffered(), ring, "the ring never filled up");
    assert_eq!(stream.reconnected(), 0);
    assert_eq!(server.requests().len(), 1);
}

/// Streams a few MiB through a cut and a reconnect; [`no_files_written`] runs it in a child
/// process whose temp dir and working dir are an empty folder.
#[test]
#[ignore = "run by no_files_written in a child process"]
fn stream_without_files_child() {
    let Some(dir) = std::env::var_os("PRISM_LIVE_EMPTY_DIR") else {
        return;
    };
    assert_eq!(std::env::temp_dir(), std::path::PathBuf::from(&dir));
    let server = FakeRadio::start();
    tone_station(&server, "/station", Some(4096), &["A - One", "B - Two"]);
    server.set_fault(Fault::CutAfter(512 * 1024));
    let config = config(256 * 1024, Duration::from_secs(5));
    let (sink, _seen) = recorder();
    let stream = open(&input(server.url("/station"), &config, Some(sink)));
    let mut reader = stream.reader();
    read_n(&mut reader, 4 * 1024 * 1024);
    assert_eq!(stream.reconnected(), 1);
    println!("streamed without files");
}

#[test]
fn no_files_written() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().to_path_buf();
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "audio::live::tests::stream_without_files_child",
            "--ignored",
            "--nocapture",
            "--test-threads=1",
        ])
        .env("PRISM_LIVE_EMPTY_DIR", &path)
        .env("TMPDIR", &path)
        .env("TMP", &path)
        .env("TEMP", &path)
        .current_dir(&path)
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "{stdout}\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(stdout.contains("streamed without files"), "{stdout}");
    let left: Vec<_> = std::fs::read_dir(&path)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    assert!(left.is_empty(), "{left:?}");
}

#[test]
fn hls_playlist_is_detected() {
    let server = FakeRadio::start();
    server.station("/station.m3u8", Body::Hls, None, &[]);
    server.station("/station.m3u", Body::M3u("/station.m3u8".into()), None, &[]);
    let config = config(64 * 1024, Duration::from_secs(5));
    let stream = open(&input(server.url("/station.m3u8"), &config, None));
    assert!(stream.is_hls());
    assert_eq!(stream.resolved_url(), server.url("/station.m3u8"));
    assert_eq!(stream.content_type(), Some("application/vnd.apple.mpegurl"));
    // Nothing is downloaded: the playlist was read, the connection is closed.
    assert!(wait_until(Duration::from_secs(1), || server.connections() == 0));
    assert_eq!(server.requests().len(), 1);
    let err = stream.reader().read(&mut [0; 16]).unwrap_err();
    assert!(!is_offline(&anyhow::Error::new(err)));
    // An M3U list whose entry is the HLS playlist resolves to it.
    let stream = open(&input(server.url("/station.m3u"), &config, None));
    assert!(stream.is_hls());
    assert_eq!(stream.resolved_url(), server.url("/station.m3u8"));
}

#[test]
fn drop_closes_the_connection() {
    let server = FakeRadio::start();
    tone_station(&server, "/station", Some(8192), &["A - One"]);
    let config = config(64 * 1024, Duration::from_secs(5));
    let stream = open(&input(server.url("/station"), &config, None));
    let mut reader = stream.reader();
    read_n(&mut reader, 200_000);
    assert_eq!(server.connections(), 1);
    drop(stream);
    let started = Instant::now();
    assert!(
        wait_until(Duration::from_secs(1), || server.connections() == 0),
        "still {} connections",
        server.connections()
    );
    assert!(started.elapsed() < Duration::from_secs(1));
    // The reader of a dropped stream fails, and not as offline.
    let err = reader.read(&mut [0; 16]).unwrap_err();
    assert!(!is_offline(&anyhow::Error::new(err)));
}

#[test]
fn user_agent_and_icy_header_sent() {
    let server = FakeRadio::start();
    tone_station(&server, "/station", None, &[]);
    let config = config(64 * 1024, Duration::from_secs(5));
    let stream = open(&input(server.url("/station"), &config, None));
    let requests = server.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].user_agent.as_deref(), Some(USER_AGENT));
    assert!(USER_AGENT.starts_with("Prism/"));
    assert_eq!(requests[0].icy_metadata.as_deref(), Some("1"));
    drop(stream);
}

#[test]
fn reader_is_live_and_single() {
    let server = FakeRadio::start();
    tone_station(&server, "/station", None, &[]);
    let config = config(64 * 1024, Duration::from_secs(5));
    let stream = open(&input(server.url("/station"), &config, None));
    let mut first = stream.reader();
    assert!(!first.is_seekable());
    assert_eq!(first.byte_len(), None);
    read_n(&mut first, 1000);
    assert_eq!(first.stream_position().unwrap(), 1000);
    assert_eq!(first.seek(SeekFrom::Start(1000)).unwrap(), 1000);
    assert!(first.seek(SeekFrom::Start(0)).is_err());
    assert!(first.seek(SeekFrom::End(0)).is_err());
    // A second reader takes over where the first stopped; the first is closed.
    let mut second = stream.reader();
    assert!(first.read(&mut [0; 16]).is_err());
    let out = read_n(&mut second, 1000);
    assert!(out == looped(server.tone(), 2000)[1000..]);
    // A cancelled reader stops at once.
    let canceller = stream.canceller();
    canceller.cancel();
    assert!(second.read(&mut [0; 16]).is_err());
    let mut third = stream.reader();
    read_n(&mut third, 1000);
}

#[test]
fn cancel_wakes_a_waiting_reader() {
    let server = FakeRadio::start();
    tone_station(&server, "/station", None, &[]);
    server.set_fault(Fault::StallAfter(10_000, Duration::from_secs(10)));
    let config = config(64 * 1024, Duration::from_secs(20));
    let stream = open(&input(server.url("/station"), &config, None));
    let mut reader = stream.reader();
    let canceller = stream.canceller();
    let waiting = std::thread::spawn(move || {
        let mut buf = vec![0; 20_000];
        let started = Instant::now();
        let result = reader.read_exact(&mut buf);
        (result, started.elapsed())
    });
    std::thread::sleep(Duration::from_millis(300));
    canceller.cancel();
    let (result, took) = waiting.join().unwrap();
    assert!(result.is_err());
    assert!(took < Duration::from_secs(2), "took {took:?}");
}

#[test]
fn input_debug_names_the_label_only() {
    let config = config(64 * 1024, Duration::from_secs(5));
    let live = input("http://127.0.0.1:9/station".into(), &config, None);
    let text = format!("{:?}", crate::audio::source::Input::Live(live.clone()));
    assert!(text.contains(LABEL), "{text}");
    assert!(!text.contains("http"), "{text}");
    assert_eq!(
        crate::audio::source::describe(&crate::audio::source::Input::Live(live.clone())),
        LABEL
    );
    let err = crate::audio::source::open_source(&crate::audio::source::Input::Live(live))
        .err()
        .expect("live inputs do not open here");
    assert!(format!("{err:#}").contains("radio opener"), "{err:#}");
}

#[test]
fn source_drop_404_is_bridged() {
    let server = FakeRadio::start();
    tone_station(&server, "/station", None, &[]);
    server.set_fault(Fault::CutThen {
        after: 64 * 1024,
        status: 404,
        lasting: Duration::from_millis(300),
    });
    let config = config(128 * 1024, Duration::from_secs(5));
    let stream = open(&input(server.url("/station"), &config, None));
    let mut reader = stream.reader();
    let out = read_n(&mut reader, 512 * 1024);
    let mut expected = looped(server.tone(), 64 * 1024);
    expected.extend(looped(server.tone(), out.len() - expected.len()));
    assert!(out == expected, "the bytes around the joint differ");
    assert_eq!(stream.reconnected(), 1);
    // The 404s in between were retried, not reported.
    assert!(server.requests().len() > 2, "{:?}", server.requests());
}

#[test]
fn a_lasting_error_after_a_cut_goes_offline() {
    let config = config(64 * 1024, Duration::from_secs(1));
    for status in [404, 500] {
        let server = FakeRadio::start();
        tone_station(&server, "/station", None, &[]);
        server.set_fault(Fault::CutThen {
            after: 16 * 1024,
            status,
            lasting: Duration::from_secs(3600),
        });
        let stream = open(&input(server.url("/station"), &config, None));
        let mut reader = stream.reader();
        let started = Instant::now();
        let mut buf = vec![0; 16 * 1024];
        let err = loop {
            match reader.read(&mut buf) {
                Ok(n) => assert!(n > 0),
                Err(err) => break err,
            }
            assert!(started.elapsed() < Duration::from_secs(10), "no error");
        };
        let err = anyhow::Error::new(err);
        assert!(is_offline(&err), "{status}: {err:#}");
        assert!(started.elapsed() < Duration::from_secs(3));
        // The reconnects kept trying until the window ended.
        assert!(server.requests().len() > 2, "{status}");
    }
}

#[test]
fn shoutcast_icy_status_line_plays() {
    let server = FakeRadio::start();
    tone_station(&server, "/station", Some(8192), &["A - One", "B - Two"]);
    server.set_fault(Fault::IcyStatusLine);
    let config = config(128 * 1024, Duration::from_secs(5));
    // The HTTP client cannot read this answer, and it is told apart from the network failing.
    let err = config
        .client
        .get(server.url("/station"))
        .header("Icy-MetaData", "1")
        .send()
        .expect_err("the client rejects ICY 200 OK");
    assert!(speaks_icy(&err), "{err:?}");
    let (sink, seen) = recorder();
    let stream = open(&input(server.url("/station"), &config, Some(sink)));
    assert_eq!(stream.content_type(), Some("audio/mpeg"));
    assert_eq!(stream.icy_name(), Some("Demo Radio Café"));
    assert_eq!(stream.resolved_url(), server.url("/station"));
    let mut reader = stream.reader();
    let out = read_n(&mut reader, 300_000);
    assert!(
        out == looped(server.tone(), out.len()),
        "the audio bytes differ"
    );
    assert_eq!(
        seen.lock().unwrap().clone(),
        [Some("A - One".to_owned()), Some("B - Two".to_owned())]
    );
    // A cut reconnects over the same raw path.
    server.stop();
    server.restart();
    // Socket buffers still hold bytes of the cut connection; read through them.
    let started = Instant::now();
    while stream.reconnected() == 0 {
        read_n(&mut reader, 64 * 1024);
        assert!(started.elapsed() < Duration::from_secs(5), "no reconnect");
    }
    read_n(&mut reader, 100_000);
    assert_eq!(stream.reconnected(), 1);
    let requests = server.requests();
    assert!(requests.len() >= 2, "{requests:?}");
    for request in &requests {
        assert_eq!(request.path, "/station");
        assert_eq!(request.user_agent.as_deref(), Some(USER_AGENT));
        assert_eq!(request.icy_metadata.as_deref(), Some("1"));
    }
}

#[test]
fn raw_heads_parse() {
    let head = b"ICY 200 OK\r\nicy-name: Caf\xe9\r\ncontent-type: audio/mpeg\r\nicy-metaint:8192\r\nbroken line\r\n";
    let (status, headers) = parse_raw_head(head).unwrap();
    assert_eq!(status, 200);
    assert_eq!(headers.get("icy-name").unwrap().as_bytes(), b"Caf\xe9");
    assert_eq!(headers.get("icy-metaint").unwrap(), "8192");
    assert_eq!(parse_raw_head(b"HTTP/1.0 404 Not Found\n").unwrap().0, 404);
    assert!(parse_raw_head(b"SOMETHING 200 OK").is_none());
    assert!(parse_raw_head(b"ICY 20 OK").is_none());
    assert!(parse_raw_head(b"").is_none());
    assert_eq!(head_end(b"ICY 200 OK\r\nx: y\r\n\r\nbody"), Some((16, 20)));
    assert_eq!(head_end(b"ICY 200 OK\nx: y\n\nbody"), Some((15, 17)));
    assert_eq!(head_end(b"ICY 200 OK\r\nx: y\r\n"), None);
}

#[test]
fn redirects_are_limited() {
    let server = FakeRadio::start();
    tone_station(&server, "/tone", None, &[]);
    for hop in 1..=6 {
        let next = if hop == 6 {
            "/tone".to_owned()
        } else {
            format!("/hop-{}", hop + 1)
        };
        server.station(&format!("/hop-{hop}"), Body::Redirect(next), None, &[]);
    }
    let config = config(64 * 1024, Duration::from_secs(5));
    // Five redirects are followed.
    let stream = open(&input(server.url("/hop-2"), &config, None));
    assert_eq!(stream.resolved_url(), server.url("/tone"));
    read_n(&mut stream.reader(), 10_000);
    drop(stream);
    // A sixth fails, plainly, naming the host only.
    let err = LiveStream::open(&input(server.url("/hop-1"), &config, None))
        .err()
        .expect("six redirects fail");
    assert!(!is_offline(&err), "{err:#}");
    assert!(!format!("{err:#}").contains("hop-"), "{err:#}");
    assert_clean(&err);
}

#[test]
fn a_title_repeated_after_a_reconnect_is_not_reported() {
    let server = FakeRadio::start();
    tone_station(&server, "/station", Some(8192), &["A - One"]);
    server.set_fault(Fault::CutAfter(64 * 1024));
    let config = config(64 * 1024, Duration::from_secs(5));
    let (sink, seen) = recorder();
    let stream = open(&input(server.url("/station"), &config, Some(sink)));
    read_n(&mut stream.reader(), 400_000);
    assert_eq!(stream.reconnected(), 1);
    assert_eq!(seen.lock().unwrap().clone(), [Some("A - One".to_owned())]);
}

#[test]
fn a_canceller_binds_to_its_reader() {
    let server = FakeRadio::start();
    tone_station(&server, "/station", None, &[]);
    let config = config(64 * 1024, Duration::from_secs(5));
    let stream = open(&input(server.url("/station"), &config, None));
    let first = stream.reader();
    let old = first.canceller();
    let mut second = stream.reader();
    // Cancelling the replaced reader leaves its replacement alone.
    old.cancel();
    read_n(&mut second, 1000);
    second.canceller().cancel();
    assert!(second.read(&mut [0; 16]).is_err());
}

#[test]
fn ogg_station_streams_its_pages() {
    let server = FakeRadio::start();
    server.station("/station.ogg", Body::Ogg, None, &[]);
    let config = config(64 * 1024, Duration::from_secs(5));
    let stream = open(&input(server.url("/station.ogg"), &config, None));
    assert_eq!(stream.content_type(), Some("application/ogg"));
    let ogg = std::fs::read(crate::library::tags::fixtures().join("notags.ogg")).unwrap();
    let out = read_n(&mut stream.reader(), ogg.len() * 3);
    assert!(out == looped(&ogg, out.len()));
}

/// Threads of this process, where the platform can tell cheaply.
fn thread_count() -> Option<usize> {
    #[cfg(target_os = "linux")]
    {
        std::fs::read_dir("/proc/self/task").ok().map(|d| d.count())
    }
    #[cfg(target_os = "macos")]
    {
        let out = std::process::Command::new("ps")
            .args(["-M", "-p", &std::process::id().to_string()])
            .output()
            .ok()?;
        let lines = String::from_utf8_lossy(&out.stdout).lines().count();
        lines.checked_sub(1)
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        None
    }
}

fn radio_station(url: String, config: &Arc<LiveConfig>) -> crate::radio::stream::StationStream {
    crate::radio::stream::StationStream {
        url,
        label: LABEL.into(),
        codec: Some("MP3".into()),
        hls: false,
        config: Arc::clone(config),
        titles: Arc::new(|_| {}),
    }
}

const DEVICE: crate::audio::source::StreamSpec = crate::audio::source::StreamSpec {
    sample_rate: 48_000,
    channels: 2,
};

/// One hour of a 320 kbps station is 144 MB. The fixture station is 128 kbps and is served as
/// fast as the loopback allows (no pacing), so both passes run far above 50x real time.
#[test]
#[ignore = "endurance run, use --release"]
fn live_one_hour_simulated() {
    use crate::audio::source::AudioSource;
    use crate::radio::stream::RadioSource;

    let ring = 256 * 1024;
    let config = config(ring, Duration::from_secs(5));
    let started = Instant::now();
    let before = thread_count();

    {
        let server = FakeRadio::start();
        tone_station(&server, "/station", None, &[]);

        // Pass 1, bytes: 144 MB through a small ring, checked against the looped tone.
        let stream = open(&input(server.url("/station"), &config, None));
        let tone = server.tone().to_vec();
        let mut reader = stream.reader();
        let mut buf = vec![0; 64 * 1024];
        let total: usize = 320_000 / 8 * 3600;
        let mut offset = 0usize;
        while offset < total {
            let n = reader.read(&mut buf).unwrap();
            let mut checked = 0;
            while checked < n {
                let at = (offset + checked) % tone.len();
                let run = (tone.len() - at).min(n - checked);
                assert!(
                    buf[checked..checked + run] == tone[at..at + run],
                    "bytes differ at {}",
                    offset + checked
                );
                checked += run;
            }
            offset += n;
        }
        println!(
            "pass 1: {} MB in {:?}, peak buffered {} of {ring}",
            offset / 1_000_000,
            started.elapsed(),
            stream.peak_buffered()
        );
        assert!(stream.peak_buffered() <= ring, "{}", stream.peak_buffered());
        assert_eq!(stream.reconnected(), 0);
        assert_eq!(server.requests().len(), 1);
        drop(reader);
        drop(stream);

        // Pass 2, audio: one hour of decoded frames through RadioSource, nothing kept.
        let pass = Instant::now();
        let mut source =
            RadioSource::open(radio_station(server.url("/station"), &config), DEVICE).unwrap();
        let spec = source.spec();
        let wanted = spec.sample_rate as u64 * 3600;
        let channels = spec.channels as usize;
        let mut samples = Vec::new();
        let (mut frames, mut zero_run, mut longest_zero) = (0u64, 0usize, 0usize);
        while frames < wanted {
            samples.clear();
            let read = source
                .read(&mut samples)
                .expect("a live source never errors");
            assert!(read > 0, "a live source never ends");
            assert_eq!(samples.len(), read * channels);
            for frame in samples.chunks_exact(channels) {
                if frame.iter().all(|s| *s == 0.0) {
                    zero_run += 1;
                    longest_zero = longest_zero.max(zero_run);
                } else {
                    zero_run = 0;
                }
            }
            frames += read as u64;
        }
        let hours = frames as f64 / spec.sample_rate as f64 / 3600.0;
        println!(
            "pass 2: {frames} frames ({hours:.3} h of audio) in {:?}, {:.0}x real time, \
             longest silent run {longest_zero} frames",
            pass.elapsed(),
            frames as f64 / spec.sample_rate as f64 / pass.elapsed().as_secs_f64()
        );
        assert!((1.0..1.001).contains(&hours), "{hours}");
        // A reopen would hand out at least 20 ms (882 frames) of silence.
        assert!(longest_zero < 200, "silent run of {longest_zero} frames");
        assert_eq!(
            server.requests().len(),
            2,
            "one connection per pass, no reopen"
        );
        drop(source);
        assert!(wait_until(Duration::from_secs(3), || server.connections() == 0));
    }

    if let Some(before) = before {
        let mut after = thread_count().unwrap();
        let settle = Instant::now();
        while after > before && settle.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(50));
            after = thread_count().unwrap();
        }
        println!("threads before {before}, after {after}");
        assert!(after <= before, "threads leaked: {before} -> {after}");
    } else {
        println!("thread count not available on this platform: skipped");
    }
}

/// The station drops every 2 s (the port is down for 300 ms each time) for a minute. The
/// source must not error, must reconnect once per cut and must never wait long.
#[test]
#[ignore = "runs for a minute"]
fn reconnect_storm() {
    use crate::audio::source::AudioSource;
    use crate::radio::stream::RadioSource;
    use std::sync::atomic::{AtomicUsize, Ordering};

    let server = Arc::new(FakeRadio::start());
    // Served at the pace of the audio (2 s per loop), like a real station.
    server.station(
        "/station",
        Body::Mp3Tone {
            seconds_per_loop: 2.0,
        },
        None,
        &[],
    );
    let config = config(128 * 1024, Duration::from_secs(20));
    let mut source =
        RadioSource::open(radio_station(server.url("/station"), &config), DEVICE).unwrap();
    let spec = source.spec();
    let cuts = AtomicUsize::new(0);
    let storm = Duration::from_secs(60);
    let started = Instant::now();
    let (mut frames, mut longest_gap) = (0u64, Duration::ZERO);
    let mut samples = Vec::new();
    std::thread::scope(|scope| {
        scope.spawn(|| {
            let mut next = Duration::from_secs(2);
            while started.elapsed() + Duration::from_millis(400) < storm {
                if started.elapsed() >= next {
                    server.stop();
                    std::thread::sleep(Duration::from_millis(300));
                    server.restart();
                    cuts.fetch_add(1, Ordering::SeqCst);
                    next += Duration::from_secs(2);
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        });
        let mut last = Instant::now();
        while started.elapsed() < storm {
            samples.clear();
            let read = source
                .read(&mut samples)
                .unwrap_or_else(|err| panic!("the source errored during the storm: {err:#}"));
            assert!(read > 0, "a live source never ends");
            frames += read as u64;
            longest_gap = longest_gap.max(last.elapsed());
            last = Instant::now();
        }
    });
    let cuts = cuts.load(Ordering::SeqCst);
    let requests = server.requests().len();
    let seconds = frames as f64 / spec.sample_rate as f64;
    println!(
        "{cuts} cuts, {requests} requests, {seconds:.1} s of audio in {:?}, longest wait {longest_gap:?}",
        started.elapsed()
    );
    assert!(cuts >= 25, "{cuts}");
    assert_eq!(requests, cuts + 1, "exactly one reconnect per cut");
    // 300 ms down, then the 50/100 ms test backoff, plus the decoder's own latency.
    assert!(longest_gap < Duration::from_secs(2), "{longest_gap:?}");
    assert!(seconds > 40.0, "{seconds}");
    drop(source);
}

#[test]
fn config_defaults() {
    let config = LiveConfig::new().unwrap();
    assert_eq!(config.ring, RING_BYTES);
    assert_eq!(config.stall, Duration::from_secs(10));
    assert_eq!(config.outage, Duration::from_secs(20));
    assert_eq!(
        config.backoff,
        [500, 1000, 2000, 4000, 8000].map(Duration::from_millis)
    );
}
