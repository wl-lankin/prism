//! Decoding with an external FFmpeg process, for everything symphonia cannot handle.

use std::io::Read;
use std::process::{Child, ChildStderr, ChildStdout, Stdio};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use anyhow::{anyhow, bail, Context};
use crossbeam_channel::{Receiver, RecvTimeoutError};

use super::source::{describe, AudioSource, Input, StreamSpec};
use crate::tools;

/// How long FFmpeg may stay silent before it counts as hung.
const STALL_TIMEOUT: Duration = Duration::from_secs(5);
const CHUNK_BYTES: usize = 64 * 1024;
const STDERR_LIMIT: usize = 4096;

/// Decodes with `ffmpeg ... -f f32le -ac 2 -ar <rate> pipe:1`. Seeking restarts FFmpeg with
/// `-ss`; the child is killed on drop.
pub struct FfmpegSource {
    input: Input,
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
    finished: bool,
}

impl FfmpegSource {
    /// Starts FFmpeg and waits for its first output, so a file FFmpeg cannot read fails here.
    pub fn open(input: &Input, target: StreamSpec) -> anyhow::Result<Self> {
        if target.channels != 2 {
            bail!(
                "FFmpeg output is always stereo, not {} channels",
                target.channels
            );
        }
        let mut process = Process::spawn(input, target.sample_rate, 0)?;
        let first = process
            .next_chunk()?
            .ok_or_else(|| anyhow!("ffmpeg produced no audio for {}", describe(input)))?;
        Ok(FfmpegSource {
            input: input.clone(),
            spec: target,
            process,
            partial: Vec::new(),
            pending: first,
        })
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
        let mut process = Process::spawn(&self.input, self.spec.sample_rate, ms)?;
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
    fn spawn(input: &Input, rate: u32, start_ms: u64) -> anyhow::Result<Self> {
        let program = tools::ffmpeg_path().context("ffmpeg was not found")?;
        let path = match input {
            Input::File(path) => path,
            Input::Http(_) => bail!(
                "{} is a stream; streams are not playable yet",
                describe(input)
            ),
        };
        let mut command = tools::command(&program);
        command.args(["-nostdin", "-hide_banner", "-loglevel", "error"]);
        if start_ms > 0 {
            command
                .arg("-ss")
                .arg(format!("{}.{:03}", start_ms / 1000, start_ms % 1000));
        }
        command
            .arg("-i")
            .arg(tools::file_input(path))
            .args(["-vn", "-f", "f32le", "-ac", "2", "-ar"])
            .arg(rate.to_string())
            .arg("pipe:1")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command
            .spawn()
            .with_context(|| format!("starting {}", program.display()))?;
        let readers = (|| {
            let stdout = child.stdout.take().context("ffmpeg stdout")?;
            let stderr = child.stderr.take().context("ffmpeg stderr")?;
            anyhow::Ok((pump_stdout(stdout)?, collect_stderr(stderr)?))
        })();
        let (chunks, stderr) = match readers {
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
            finished: false,
        })
    }

    /// The next block of output, `None` at a clean end, `Err` when FFmpeg fails or stalls.
    fn next_chunk(&mut self) -> anyhow::Result<Option<Vec<u8>>> {
        if self.finished {
            return Ok(None);
        }
        match self.chunks.recv_timeout(STALL_TIMEOUT) {
            Ok(chunk) => Ok(Some(chunk)),
            Err(RecvTimeoutError::Timeout) => {
                let _ = self.child.kill();
                bail!("ffmpeg produced no audio for {} s", STALL_TIMEOUT.as_secs())
            }
            Err(RecvTimeoutError::Disconnected) => {
                self.finished = true;
                let status = self.child.wait().context("waiting for ffmpeg")?;
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

impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
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
