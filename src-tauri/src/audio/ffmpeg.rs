//! Decoding with an external FFmpeg process, for everything symphonia cannot handle.

use std::ffi::OsString;
use std::io::{self, Read, Write};
use std::process::{Child, ChildStderr, ChildStdin, ChildStdout, Stdio};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use anyhow::{anyhow, bail, Context};
use crossbeam_channel::{Receiver, RecvTimeoutError};
use reqwest::Url;

use super::http::{HttpStream, Offline, StreamFailure};
use super::live::{NOT_AN_ADDRESS, NOT_AUDIO, OFFLINE_TEXT, USER_AGENT};
use super::source::{
    describe, AudioSource, HttpInput, Input, StreamSpec, LIVE_NEEDS_THE_RADIO_OPENER,
};
use crate::tools;

/// How long FFmpeg may stay silent before it counts as hung.
const STALL_TIMEOUT: Duration = Duration::from_secs(5);
/// How long a seek waits for the old feeder to end (it ends within milliseconds once FFmpeg is
/// killed and its read cancelled).
const FEEDER_EXIT_WAIT: Duration = Duration::from_secs(2);
const CHUNK_BYTES: usize = 64 * 1024;
const STDERR_LIMIT: usize = 4096;
/// FFmpeg's own network read timeout for a URL input (`-rw_timeout`, in microseconds).
const URL_RW_TIMEOUT: Duration = Duration::from_secs(10);
/// What FFmpeg may open for a URL input: the playlist, its segments and keys, nothing local.
const URL_PROTOCOLS: &str = "http,https,tls,tcp,crypto,httpproxy";

/// Decodes with `ffmpeg ... -f f32le -ac 2 -ar <rate> pipe:1`. Seeking restarts FFmpeg with
/// `-ss`; the child is killed on drop.
///
/// A stream (`Input::Http`) never reaches FFmpeg as a URL: FFmpeg reads `pipe:0`, and a feeder
/// thread copies the stream buffer into its stdin from a fresh [`HttpStream::cursor`]. A seek
/// stops the old feeder and FFmpeg first (one reader per stream at a time), then starts both
/// again at byte 0 of the buffer with `-ss` after `-i` (bytes already buffered are not fetched
/// again).
///
/// Radio comes in two more ways: a live byte stream through stdin ([`FfmpegSource::open_reader`])
/// and an HLS station by URL ([`FfmpegSource::open_url`]). Radio URLs carry no Prism secret.
/// Neither can seek.
pub struct FfmpegSource {
    origin: Origin,
    spec: StreamSpec,
    process: Process,
    /// Bytes of a sample split across two pipe reads.
    partial: Vec<u8>,
    /// Data received while checking that FFmpeg started.
    pending: Vec<u8>,
}

/// What FFmpeg reads.
enum Origin {
    /// A file, or an HTTP stream with its buffer (held idle; each run reads a cursor of it).
    Input {
        input: Input,
        feed: Option<HttpStream>,
    },
    /// A live byte stream through stdin. It is read once: taken by the first run.
    Live {
        label: String,
        feed: Option<LiveFeed>,
    },
    /// A URL FFmpeg fetches itself (an HLS station).
    Url { url: String, label: String },
}

impl Origin {
    fn label(&self) -> String {
        match self {
            Origin::Input { input, .. } => describe(input),
            Origin::Live { label, .. } | Origin::Url { label, .. } => label.clone(),
        }
    }
}

/// A live byte stream for [`FfmpegSource::open_reader`].
pub struct LiveFeed {
    pub reader: Box<dyn Read + Send>,
    /// Ends a blocked read of `reader` from another thread (FFmpeg is being stopped).
    pub cancel: Box<dyn Fn() + Send + Sync>,
    /// How long a read of `reader` may wait for the network before it fails by itself.
    pub wait: Duration,
}

struct Process {
    child: Child,
    chunks: Receiver<Vec<u8>>,
    stderr: Option<JoinHandle<String>>,
    feeder: Option<Feeder>,
    /// How long FFmpeg may stay silent: [`STALL_TIMEOUT`], plus how long its input may wait for
    /// the network (a stream's stall timeout, a live feed's wait, FFmpeg's own `-rw_timeout`).
    stall: Duration,
    /// Set for radio: its failures are classified and name the station.
    station: Option<Station>,
    finished: bool,
    stopped: bool,
}

/// A radio input of a [`Process`], with the station label.
enum Station {
    /// A URL: a failure is offline or "not sending audio", and stderr (which names URLs) is
    /// scrubbed.
    Url(String),
    /// A live stream through stdin: a failure that is not the stream's own is "not sending
    /// audio".
    Pipe(String),
}

/// The thread copying a stream into FFmpeg's stdin.
struct Feeder {
    cancel: Box<dyn Fn() + Send + Sync>,
    /// Disconnected once the thread has ended.
    done: Receiver<()>,
    /// Why the stream failed (offline, an HTTP error); FFmpeg only sees its input end early.
    error: Arc<Mutex<Option<io::Error>>>,
}

impl FfmpegSource {
    /// Starts FFmpeg and waits for its first output, so a file FFmpeg cannot read fails here.
    /// A stream is opened (downloaded) here; use [`FfmpegSource::open_stream`] to reuse a buffer.
    pub fn open(input: &Input, target: StreamSpec) -> anyhow::Result<Self> {
        match input {
            Input::File(_) => Self::start(
                Origin::Input {
                    input: input.clone(),
                    feed: None,
                },
                target,
            ),
            Input::Http(http) => Self::open_stream(HttpStream::open(http)?, http, target),
            Input::Live(_) => bail!(LIVE_NEEDS_THE_RADIO_OPENER),
        }
    }

    /// Decodes `stream` (the buffer of `input`) through FFmpeg's stdin. `stream` is kept idle as
    /// the source of a cursor per FFmpeg run.
    pub fn open_stream(
        stream: HttpStream,
        input: &HttpInput,
        target: StreamSpec,
    ) -> anyhow::Result<Self> {
        Self::start(
            Origin::Input {
                input: Input::Http(input.clone()),
                feed: Some(stream),
            },
            target,
        )
    }

    /// Decodes a live byte stream through FFmpeg's stdin; `label` names it in errors. The
    /// stream is read once, so the source cannot seek.
    pub fn open_reader(feed: LiveFeed, label: &str, target: StreamSpec) -> anyhow::Result<Self> {
        Self::start(
            Origin::Live {
                label: label.to_owned(),
                feed: Some(feed),
            },
            target,
        )
    }

    /// Lets FFmpeg fetch and decode `url` itself (an HLS station), reconnecting on its own. The
    /// label, never the URL, appears in errors. Cannot seek.
    pub fn open_url(url: &str, label: &str, target: StreamSpec) -> anyhow::Result<Self> {
        let parsed = Url::parse(url.trim())
            .ok()
            .filter(|url| matches!(url.scheme(), "http" | "https") && url.host_str().is_some());
        let Some(parsed) = parsed else {
            return Err(anyhow::Error::new(StreamFailure(NOT_AN_ADDRESS.into()))
                .context(format!("streaming {label}")));
        };
        Self::start(
            Origin::Url {
                url: parsed.to_string(),
                label: label.to_owned(),
            },
            target,
        )
    }

    fn start(mut origin: Origin, target: StreamSpec) -> anyhow::Result<Self> {
        if target.channels != 2 {
            bail!(
                "FFmpeg output is always stereo, not {} channels",
                target.channels
            );
        }
        let mut process = Process::spawn(&mut origin, target.sample_rate, 0)?;
        let first = process
            .next_chunk()?
            .ok_or_else(|| anyhow!("ffmpeg produced no audio for {}", origin.label()))?;
        Ok(FfmpegSource {
            origin,
            spec: target,
            process,
            partial: Vec::new(),
            pending: first,
        })
    }

    /// The feeder's end signal (disconnected once the thread ended), for tests.
    #[cfg(test)]
    pub(crate) fn feeder_done(&self) -> Option<Receiver<()>> {
        self.process
            .feeder
            .as_ref()
            .map(|feeder| feeder.done.clone())
    }

    /// The FFmpeg child's process id, for tests that check it is reaped.
    #[cfg(test)]
    pub(crate) fn child_id(&self) -> u32 {
        self.process.child.id()
    }
}

/// The arguments every run starts with.
fn base_args() -> Vec<OsString> {
    ["-nostdin", "-hide_banner", "-loglevel", "error"]
        .into_iter()
        .map(OsString::from)
        .collect()
}

/// f32 stereo at `rate` on stdout.
fn push_output_args(args: &mut Vec<OsString>, rate: u32) {
    args.extend(
        ["-vn", "-f", "f32le", "-ac", "2", "-ar"]
            .into_iter()
            .map(OsString::from),
    );
    args.push(rate.to_string().into());
    args.push("pipe:1".into());
}

/// FFmpeg's arguments for `input`, from `start_ms` on, as f32 stereo at `rate` on stdout.
///
/// A file is read as `file:<path>` with `-ss` before `-i` (a fast input seek). A stream is read
/// from `pipe:0` with `-ss` after `-i` (FFmpeg decodes and drops up to the position, since a pipe
/// cannot seek); no URL and no credential ever appear in the command line.
pub(crate) fn ffmpeg_args(input: &Input, rate: u32, start_ms: u64) -> Vec<OsString> {
    let mut args = base_args();
    let seek = (start_ms > 0).then(|| {
        [
            OsString::from("-ss"),
            OsString::from(format!("{}.{:03}", start_ms / 1000, start_ms % 1000)),
        ]
    });
    match input {
        Input::File(path) => {
            args.extend(seek.into_iter().flatten());
            args.push("-i".into());
            args.push(tools::file_input(path));
        }
        Input::Http(_) | Input::Live(_) => {
            args.push("-i".into());
            args.push("pipe:0".into());
            args.extend(seek.into_iter().flatten());
        }
    }
    push_output_args(&mut args, rate);
    args
}

/// Probing a live stream through stdin is capped, so opening stays within the stall budget:
/// FFmpeg's defaults (5 MB, 5 s) would wait minutes for a slow station.
const PIPE_PROBE_BYTES: &str = "131072";
const PIPE_ANALYZE_US: &str = "2000000";

/// FFmpeg's arguments for a live stream through stdin.
pub(crate) fn pipe_args(rate: u32) -> Vec<OsString> {
    let mut args = base_args();
    args.extend(
        [
            "-probesize",
            PIPE_PROBE_BYTES,
            "-analyzeduration",
            PIPE_ANALYZE_US,
            "-i",
            "pipe:0",
        ]
        .into_iter()
        .map(OsString::from),
    );
    push_output_args(&mut args, rate);
    args
}

/// FFmpeg's arguments for a URL it fetches itself (an HLS station): Prism's User-Agent, web
/// protocols only, FFmpeg's own reconnect, and a 10 s network read timeout.
pub(crate) fn url_args(url: &str, rate: u32) -> Vec<OsString> {
    let mut args = base_args();
    let rw_timeout = URL_RW_TIMEOUT.as_micros().to_string();
    args.extend(
        [
            "-user_agent",
            USER_AGENT,
            "-protocol_whitelist",
            URL_PROTOCOLS,
            "-reconnect",
            "1",
            "-reconnect_streamed",
            "1",
            "-reconnect_delay_max",
            "4",
            "-rw_timeout",
            &rw_timeout,
            "-i",
            url,
        ]
        .into_iter()
        .map(OsString::from),
    );
    push_output_args(&mut args, rate);
    args
}

/// Words in FFmpeg's stderr that mean the network or the server is away, not that the station
/// sends something other than audio.
const OFFLINE_SIGNS: &[&str] = &[
    "connection refused",
    "connection reset",
    "connection timed out",
    "timed out",
    "network is unreachable",
    "no route to host",
    "failed to resolve",
    "could not resolve",
    "nodename nor servname",
    "name or service not known",
    "temporary failure in name resolution",
    "server returned 5",
    "input/output error",
    "end of file",
    "broken pipe",
];

/// `text` with every URL replaced, so FFmpeg's messages can be shown and logged.
pub(crate) fn scrub_urls(text: &str) -> String {
    text.split_whitespace()
        .map(|word| {
            if word.contains("://") {
                "<address>"
            } else {
                word
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// The error for a URL input FFmpeg could not play: offline when the network or the server is
/// away, otherwise "not sending audio". Never names the URL.
pub(crate) fn url_failure(label: &str, stderr: &str) -> anyhow::Error {
    let lower = stderr.to_ascii_lowercase();
    station_failure(
        label,
        stderr,
        OFFLINE_SIGNS.iter().any(|sign| lower.contains(sign)),
    )
}

/// A station FFmpeg could not play: offline, or "not sending audio", with FFmpeg's message
/// (URLs scrubbed) for the logs.
fn station_failure(label: &str, stderr: &str, offline: bool) -> anyhow::Error {
    let detail: String = scrub_urls(stderr).chars().take(300).collect();
    let error = if offline {
        anyhow::Error::new(Offline(OFFLINE_TEXT.into()))
    } else {
        anyhow::Error::new(StreamFailure(NOT_AUDIO.into()))
    };
    if detail.is_empty() {
        error.context(format!("streaming {label}"))
    } else {
        error.context(format!("streaming {label} (ffmpeg: {detail})"))
    }
}

impl AudioSource for FfmpegSource {
    fn spec(&self) -> StreamSpec {
        self.spec
    }

    fn read(&mut self, out: &mut Vec<f32>) -> anyhow::Result<usize> {
        let bytes = if self.pending.is_empty() {
            match self.process.next_chunk()? {
                Some(bytes) => bytes,
                None => return Ok(0),
            }
        } else {
            std::mem::take(&mut self.pending)
        };
        self.partial.extend_from_slice(&bytes);
        let frame_bytes = 4 * self.spec.channels as usize;
        let whole = self.partial.len() / frame_bytes * frame_bytes;
        out.extend(
            self.partial[..whole]
                .as_chunks::<4>()
                .0
                .iter()
                .map(|bytes| f32::from_le_bytes(*bytes)),
        );
        self.partial.drain(..whole);
        let frames = whole / frame_bytes;
        if frames == 0 {
            // A chunk smaller than one frame: keep reading rather than report the end.
            return self.read(out);
        }
        Ok(frames)
    }

    /// Restarts FFmpeg at `ms` and waits for its first output (up to the 5 s stall timeout),
    /// so the following `read` does not block on FFmpeg starting up.
    fn seek(&mut self, ms: u64) -> anyhow::Result<()> {
        match &self.origin {
            Origin::Live { .. } | Origin::Url { .. } => bail!("a live station cannot seek"),
            Origin::Input { feed: Some(_), .. } => {
                // One reader per stream: the old feeder must be gone before the new one reads.
                self.process.stop();
            }
            Origin::Input { feed: None, .. } => {}
        }
        let mut process = Process::spawn(&mut self.origin, self.spec.sample_rate, ms)?;
        // Past the end FFmpeg exits cleanly without output: the source is then finished.
        let first = process.next_chunk()?.unwrap_or_default();
        self.process = process;
        self.partial.clear();
        self.pending = first;
        Ok(())
    }

    fn duration_ms(&self) -> Option<u64> {
        None
    }
}

/// What a run reads from stdin: the reader and how to cancel it.
type StdinFeed = (Box<dyn Read + Send>, Box<dyn Fn() + Send + Sync>);

impl Process {
    /// Starts FFmpeg on `origin`, from `start_ms` on.
    fn spawn(origin: &mut Origin, rate: u32, start_ms: u64) -> anyhow::Result<Self> {
        let program = tools::ffmpeg_path().context("ffmpeg was not found")?;
        let (args, feed, stall, station): (Vec<OsString>, Option<StdinFeed>, Duration, _) =
            match origin {
                Origin::Input { input, feed } => {
                    let args = ffmpeg_args(input, rate, start_ms);
                    match input {
                        Input::File(_) => (args, None, STALL_TIMEOUT, None),
                        Input::Http(http) => {
                            let stream = feed.as_ref().context("a stream needs its buffer")?;
                            let cursor = stream.cursor();
                            let canceller = cursor.canceller();
                            let feed: StdinFeed =
                                (Box::new(cursor), Box::new(move || canceller.cancel()));
                            (args, Some(feed), STALL_TIMEOUT + http.config.stall, None)
                        }
                        Input::Live(_) => bail!(LIVE_NEEDS_THE_RADIO_OPENER),
                    }
                }
                Origin::Live { feed, label } => {
                    let live = feed.take().context("a live stream is read only once")?;
                    (
                        pipe_args(rate),
                        Some((live.reader, live.cancel)),
                        STALL_TIMEOUT + live.wait,
                        Some(Station::Pipe(label.clone())),
                    )
                }
                Origin::Url { url, label } => (
                    url_args(url, rate),
                    None,
                    STALL_TIMEOUT + URL_RW_TIMEOUT,
                    Some(Station::Url(label.clone())),
                ),
            };
        let mut command = tools::command(&program);
        command
            .args(args)
            .stdin(if feed.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command
            .spawn()
            .with_context(|| format!("starting {}", program.display()))?;
        let readers = (|| {
            let stdout = child.stdout.take().context("ffmpeg stdout")?;
            let stderr = child.stderr.take().context("ffmpeg stderr")?;
            let feeder = match feed {
                Some((reader, cancel)) => {
                    let stdin = child.stdin.take().context("ffmpeg stdin")?;
                    Some(spawn_feeder(reader, cancel, stdin)?)
                }
                None => None,
            };
            anyhow::Ok((pump_stdout(stdout)?, collect_stderr(stderr)?, feeder))
        })();
        let (chunks, stderr, feeder) = match readers {
            Ok(readers) => readers,
            Err(err) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(err);
            }
        };
        Ok(Process {
            child,
            chunks,
            stderr: Some(stderr),
            feeder,
            stall,
            station,
            finished: false,
            stopped: false,
        })
    }

    /// The stream error the feeder ran into, if any: it explains why FFmpeg's input ended.
    fn feed_error(&self) -> Option<anyhow::Error> {
        let feeder = self.feeder.as_ref()?;
        let err = feeder
            .error
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()?;
        Some(anyhow::Error::new(err))
    }

    /// The next block of output, `None` at a clean end, `Err` when FFmpeg fails or stalls.
    fn next_chunk(&mut self) -> anyhow::Result<Option<Vec<u8>>> {
        if self.finished {
            return Ok(None);
        }
        match self.chunks.recv_timeout(self.stall) {
            Ok(chunk) => Ok(Some(chunk)),
            Err(RecvTimeoutError::Timeout) => {
                let _ = self.child.kill();
                let _ = self.child.wait();
                if let Some(err) = self.feed_error() {
                    return Err(err);
                }
                match &self.station {
                    // FFmpeg's own reconnect gave up quietly: the station is away.
                    Some(Station::Url(label)) => Err(station_failure(label, "", true)),
                    // The stream kept coming, FFmpeg made nothing of it.
                    Some(Station::Pipe(label)) => Err(station_failure(label, "", false)),
                    None => bail!("ffmpeg produced no audio for {} s", self.stall.as_secs()),
                }
            }
            Err(RecvTimeoutError::Disconnected) => {
                self.finished = true;
                let status = self.child.wait().context("waiting for ffmpeg")?;
                // A stream that failed ends FFmpeg's input early, which FFmpeg may take for the
                // end of the track: the stream's error (offline) wins over a clean exit.
                if let Some(err) = self.feed_error() {
                    return Err(err);
                }
                if status.success() {
                    return Ok(None);
                }
                // stderr closes with the process, so its reader has finished or is about to.
                let stderr = self
                    .stderr
                    .take()
                    .and_then(|reader| reader.join().ok())
                    .unwrap_or_default();
                match &self.station {
                    Some(Station::Url(label)) => Err(url_failure(label, stderr.trim())),
                    Some(Station::Pipe(label)) => Err(station_failure(label, stderr.trim(), false)),
                    None => bail!("ffmpeg failed ({status}): {}", stderr.trim()),
                }
            }
        }
    }
}

impl Process {
    /// Kills FFmpeg at once and ends its feeder: the feeder's read is cancelled, and a write
    /// fails on the broken pipe. Waits (bounded) for the feeder, so no older reader of the
    /// stream is left when a new one starts.
    fn stop(&mut self) {
        if self.stopped {
            return;
        }
        self.stopped = true;
        if let Some(feeder) = &self.feeder {
            (feeder.cancel)();
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some(feeder) = &self.feeder {
            if let Err(RecvTimeoutError::Timeout) = feeder.done.recv_timeout(FEEDER_EXIT_WAIT) {
                eprintln!(
                    "the ffmpeg feeder did not end within {} s",
                    FEEDER_EXIT_WAIT.as_secs()
                );
            }
        }
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        if self.stopped {
            return;
        }
        // Dropping must not wait on the feeder (skip-spam drops sources on the engine's side):
        // cancel and kill; the feeder ends on its own right after.
        self.stopped = true;
        if let Some(feeder) = &self.feeder {
            (feeder.cancel)();
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Copies `reader` into FFmpeg's stdin until its end (stdin is then closed, which FFmpeg reads
/// as the end of its input), a read error (kept for [`Process::feed_error`]), a cancel, or a
/// broken pipe (FFmpeg is gone).
fn spawn_feeder(
    mut reader: Box<dyn Read + Send>,
    cancel: Box<dyn Fn() + Send + Sync>,
    mut stdin: ChildStdin,
) -> anyhow::Result<Feeder> {
    let (done_tx, done) = crossbeam_channel::bounded::<()>(0);
    let error: Arc<Mutex<Option<io::Error>>> = Arc::default();
    let failed = Arc::clone(&error);
    thread::Builder::new()
        .name("ffmpeg-feeder".into())
        .spawn(move || {
            let _done = done_tx;
            let mut buf = vec![0u8; CHUNK_BYTES];
            loop {
                let read = match reader.read(&mut buf) {
                    Ok(0) => return,
                    Ok(read) => read,
                    Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
                    Err(err) => {
                        *failed.lock().unwrap_or_else(|e| e.into_inner()) = Some(err);
                        return;
                    }
                };
                if stdin.write_all(&buf[..read]).is_err() {
                    return;
                }
            }
        })
        .context("spawning the ffmpeg feeder thread")?;
    Ok(Feeder {
        cancel,
        done,
        error,
    })
}

fn pump_stdout(mut stdout: ChildStdout) -> anyhow::Result<Receiver<Vec<u8>>> {
    let (tx, rx) = crossbeam_channel::bounded(4);
    thread::Builder::new()
        .name("ffmpeg-stdout".into())
        .spawn(move || loop {
            let mut chunk = vec![0u8; CHUNK_BYTES];
            match stdout.read(&mut chunk) {
                Ok(0) | Err(_) => return,
                Ok(read) => {
                    chunk.truncate(read);
                    if tx.send(chunk).is_err() {
                        return;
                    }
                }
            }
        })
        .context("spawning the ffmpeg reader thread")?;
    Ok(rx)
}

/// Collects the first few KB of stderr for error messages, and drains the rest.
fn collect_stderr(mut stderr: ChildStderr) -> anyhow::Result<JoinHandle<String>> {
    thread::Builder::new()
        .name("ffmpeg-stderr".into())
        .spawn(move || {
            let mut text = String::new();
            let mut buf = [0u8; 1024];
            while let Ok(read) = stderr.read(&mut buf) {
                if read == 0 {
                    break;
                }
                if text.len() < STDERR_LIMIT {
                    text.push_str(&String::from_utf8_lossy(&buf[..read]));
                }
            }
            text
        })
        .context("spawning the ffmpeg stderr thread")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::is_offline;
    use crate::library::tags::fixtures;

    const FFMPEG_TEST_SPEC: StreamSpec = StreamSpec {
        sample_rate: 48_000,
        channels: 2,
    };

    #[test]
    fn ffmpeg_failure_surfaces_as_an_error() {
        if tools::ffmpeg_path().is_none() {
            println!("ffmpeg not found: skipping ffmpeg_failure_surfaces_as_an_error");
            return;
        }
        let input = Input::File(fixtures().join("does-not-exist.wma"));
        let err = FfmpegSource::open(
            &input,
            StreamSpec {
                sample_rate: 48_000,
                channels: 2,
            },
        )
        .err()
        .expect("a missing file fails");
        assert!(err.to_string().contains("ffmpeg failed"), "{err:#}");
    }

    #[test]
    fn opens_a_dash_named_file_in_a_folder_with_spaces() {
        if tools::ffmpeg_path().is_none() {
            println!("ffmpeg not found: skipping the dash-named ffmpeg test");
            return;
        }
        let (_dir, path) = tools::tests::fixture_copy("opus.opus", "-x.opus");
        let mut source = FfmpegSource::open(&Input::File(path), FFMPEG_TEST_SPEC).unwrap();
        assert!(source.read(&mut Vec::new()).unwrap() > 0);
    }

    #[test]
    fn seek_waits_for_the_first_output() {
        if tools::ffmpeg_path().is_none() {
            println!("ffmpeg not found: skipping seek_waits_for_the_first_output");
            return;
        }
        let input = Input::File(fixtures().join("opus.opus"));
        let mut source = FfmpegSource::open(&input, FFMPEG_TEST_SPEC).unwrap();
        source.seek(300).unwrap();
        assert!(!source.pending.is_empty(), "the restarted FFmpeg is primed");
        source.seek(5_000).unwrap();
        assert!(source.pending.is_empty());
        assert_eq!(source.read(&mut Vec::new()).unwrap(), 0, "past the end");
    }

    #[test]
    fn url_args_name_the_agent_and_web_protocols_only() {
        let args: Vec<String> = url_args("https://radio.example/live.m3u8", 44_100)
            .into_iter()
            .map(|arg| arg.into_string().unwrap())
            .collect();
        let joined = args.join(" ");
        assert!(
            joined.contains(&format!("-user_agent {USER_AGENT}")),
            "{joined}"
        );
        assert!(
            joined.contains("-reconnect 1 -reconnect_streamed 1 -reconnect_delay_max 4"),
            "{joined}"
        );
        assert!(joined.contains("-rw_timeout 10000000"), "{joined}");
        assert!(
            joined.contains("-protocol_whitelist http,https,tls,tcp,crypto,httpproxy"),
            "{joined}"
        );
        assert!(
            joined.ends_with(
                "-i https://radio.example/live.m3u8 -vn -f f32le -ac 2 -ar 44100 pipe:1"
            ),
            "{joined}"
        );
    }

    #[test]
    fn url_failures_are_classified_and_scrubbed() {
        let refused = url_failure(
            "Demo Radio 1",
            "[tcp @ 0x1] Connection to tcp://127.0.0.1:9 failed: Connection refused\nhttp://127.0.0.1:9/live.m3u8: Connection refused",
        );
        assert!(is_offline(&refused), "{refused:#}");
        let missing = url_failure(
            "Demo Radio 1",
            "[http @ 0x1] HTTP error 404 Not Found\nhttp://127.0.0.1:9/live.m3u8: Server returned 404 Not Found",
        );
        assert!(!is_offline(&missing), "{missing:#}");
        for err in [refused, missing] {
            let text = format!("{err:#} {err:?}");
            assert!(text.contains("Demo Radio 1"), "{text}");
            assert!(!text.contains("://"), "{text}");
            assert!(!text.contains("live.m3u8"), "{text}");
        }
    }

    #[test]
    fn a_live_pipe_caps_its_probe_and_fails_as_not_audio() {
        let joined = pipe_args(48_000)
            .into_iter()
            .map(|arg| arg.into_string().unwrap())
            .collect::<Vec<_>>()
            .join(" ");
        assert!(
            joined.contains("-probesize 131072 -analyzeduration 2000000 -i pipe:0"),
            "{joined}"
        );
        let err = station_failure(
            "Demo Radio 1",
            "Invalid data found when processing input",
            false,
        );
        assert!(!is_offline(&err), "{err:#}");
        assert!(format!("{err:#}").contains(NOT_AUDIO), "{err:#}");
        assert!(
            format!("{err:#}").contains("streaming Demo Radio 1"),
            "{err:#}"
        );
    }

    #[test]
    fn open_url_refuses_anything_but_a_web_address() {
        for url in [
            "file:///etc/passwd",
            "-i",
            "pipe:0",
            "ftp://radio.example/x",
        ] {
            let err = FfmpegSource::open_url(url, "Demo Radio 1", FFMPEG_TEST_SPEC)
                .err()
                .expect("not a web address");
            assert!(format!("{err:#}").contains(NOT_AN_ADDRESS), "{err:#}");
        }
    }
}
