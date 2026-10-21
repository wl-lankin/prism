//! Decoding with an external FFmpeg process, for everything symphonia cannot handle.

use std::ffi::OsString;
use std::io::{self, Read, Write};
use std::process::{Child, ChildStderr, ChildStdin, ChildStdout, Stdio};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use anyhow::{anyhow, bail, Context};
use crossbeam_channel::{Receiver, RecvTimeoutError};

use super::http::{Canceller, HttpStream};
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

/// Decodes with `ffmpeg ... -f f32le -ac 2 -ar <rate> pipe:1`. Seeking restarts FFmpeg with
/// `-ss`; the child is killed on drop.
///
/// A stream (`Input::Http`) never reaches FFmpeg as a URL: FFmpeg reads `pipe:0`, and a feeder
/// thread copies the stream buffer into its stdin from a fresh [`HttpStream::cursor`]. A seek
/// stops the old feeder and FFmpeg first (one reader per stream at a time), then starts both
/// again at byte 0 of the buffer with `-ss` after `-i` (bytes already buffered are not fetched
/// again).
pub struct FfmpegSource {
    input: Input,
    /// The stream buffer of an `Input::Http`, held idle; each FFmpeg run reads a cursor of it.
    feed: Option<HttpStream>,
    spec: StreamSpec,
    process: Process,
    /// Bytes of a sample split across two pipe reads.
    partial: Vec<u8>,
    /// Data received while checking that FFmpeg started.
    pending: Vec<u8>,
}

struct Process {
    child: Child,
    chunks: Receiver<Vec<u8>>,
    stderr: Option<JoinHandle<String>>,
    feeder: Option<Feeder>,
    /// How long FFmpeg may stay silent: [`STALL_TIMEOUT`], plus the stream's own stall timeout
    /// when it reads a stream (the feeder may wait that long for the network).
    stall: Duration,
    finished: bool,
    stopped: bool,
}

/// The thread copying a stream buffer into FFmpeg's stdin.
struct Feeder {
    canceller: Canceller,
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
            Input::File(_) => Self::start(input.clone(), None, target),
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
        Self::start(Input::Http(input.clone()), Some(stream), target)
    }

    fn start(input: Input, feed: Option<HttpStream>, target: StreamSpec) -> anyhow::Result<Self> {
        if target.channels != 2 {
            bail!(
                "FFmpeg output is always stereo, not {} channels",
                target.channels
            );
        }
        let mut process = Process::spawn(&input, feed.as_ref(), target.sample_rate, 0)?;
        let first = process
            .next_chunk()?
            .ok_or_else(|| anyhow!("ffmpeg produced no audio for {}", describe(&input)))?;
        Ok(FfmpegSource {
            input,
            feed,
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
}

/// FFmpeg's arguments for `input`, from `start_ms` on, as f32 stereo at `rate` on stdout.
///
/// A file is read as `file:<path>` with `-ss` before `-i` (a fast input seek). A stream is read
/// from `pipe:0` with `-ss` after `-i` (FFmpeg decodes and drops up to the position, since a pipe
/// cannot seek); no URL and no credential ever appear in the command line.
pub(crate) fn ffmpeg_args(input: &Input, rate: u32, start_ms: u64) -> Vec<OsString> {
    let mut args: Vec<OsString> = ["-nostdin", "-hide_banner", "-loglevel", "error"]
        .into_iter()
        .map(OsString::from)
        .collect();
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
    args.extend(
        ["-vn", "-f", "f32le", "-ac", "2", "-ar"]
            .into_iter()
            .map(OsString::from),
    );
    args.push(rate.to_string().into());
    args.push("pipe:1".into());
    args
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
        if self.feed.is_some() {
            // One reader per stream: the old feeder must be gone before the new one reads.
            self.process.stop();
        }
        let mut process =
            Process::spawn(&self.input, self.feed.as_ref(), self.spec.sample_rate, ms)?;
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

impl Process {
    /// Starts FFmpeg on `input`; `feed` is the buffer of a stream input, read through stdin.
    fn spawn(
        input: &Input,
        feed: Option<&HttpStream>,
        rate: u32,
        start_ms: u64,
    ) -> anyhow::Result<Self> {
        let program = tools::ffmpeg_path().context("ffmpeg was not found")?;
        let feed = match (input, feed) {
            (Input::File(_), _) => None,
            (Input::Http(http), feed) => Some((feed.context("a stream needs its buffer")?, http)),
            (Input::Live(_), _) => bail!(LIVE_NEEDS_THE_RADIO_OPENER),
        };
        let mut command = tools::command(&program);
        command
            .args(ffmpeg_args(input, rate, start_ms))
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
                Some((stream, _)) => {
                    let stdin = child.stdin.take().context("ffmpeg stdin")?;
                    Some(spawn_feeder(stream.cursor(), stdin)?)
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
        let stall = match feed {
            Some((_, http)) => STALL_TIMEOUT + http.config.stall,
            None => STALL_TIMEOUT,
        };
        Ok(Process {
            child,
            chunks,
            stderr: Some(stderr),
            feeder,
            stall,
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
                if let Some(err) = self.feed_error() {
                    return Err(err);
                }
                bail!("ffmpeg produced no audio for {} s", self.stall.as_secs())
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
                bail!("ffmpeg failed ({status}): {}", stderr.trim())
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
            feeder.canceller.cancel();
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
            feeder.canceller.cancel();
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Copies `stream` into FFmpeg's stdin until the end of the stream (stdin is then closed, which
/// FFmpeg reads as the end of its input), a stream error (kept for [`Process::feed_error`]), a
/// cancel, or a broken pipe (FFmpeg is gone).
fn spawn_feeder(mut stream: HttpStream, mut stdin: ChildStdin) -> anyhow::Result<Feeder> {
    let canceller = stream.canceller();
    let (done_tx, done) = crossbeam_channel::bounded::<()>(0);
    let error: Arc<Mutex<Option<io::Error>>> = Arc::default();
    let failed = Arc::clone(&error);
    thread::Builder::new()
        .name("ffmpeg-feeder".into())
        .spawn(move || {
            let _done = done_tx;
            let mut buf = vec![0u8; CHUNK_BYTES];
            loop {
                let read = match stream.read(&mut buf) {
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
        canceller,
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
}
