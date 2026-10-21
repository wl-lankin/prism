//! Navidrome tracks as audio sources: the opener the player uses, and [`NavidromeSource`], which
//! adds the stream quality and seeking on streams the server cannot serve in byte ranges.
//!
//! How a seek works depends on what the server sent:
//! - A byte-seekable stream (206, or 200 with a length and `Accept-Ranges: bytes`) seeks through
//!   the decoder, which reads the bytes it needs with Range requests.
//! - A stream with a length but no byte ranges is a file (the original, a file Navidrome plays
//!   directly because it already fits the transcode request, or a cached transcode) behind a
//!   server or proxy that does not serve ranges. Navidrome ignores `timeOffset` for such a file,
//!   so it seeks through the bytes: a non-seekable stream is downloaded from byte 0 and every
//!   byte stays in the spill file, so a forward seek decodes ahead, and a backward seek opens the
//!   decoder again on a fresh cursor at byte 0 of the same buffer and decodes up to the position.
//!   No byte is downloaded twice.
//! - A stream without a length and without byte ranges is a live transcode: it seeks by
//!   requesting the transcode again with `timeOffset=<s>` and dropping the remaining fraction of
//!   a second. Navidrome honors `timeOffset` only for transcodes; `format=raw` always starts at 0.
//!
//! The original quality never uses `timeOffset`, whatever the headers say.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{bail, Context};

use super::client::{StreamQuality, SubsonicClient};
use super::NavidromeService;
use crate::audio::source::{open_decoder, open_http, AudioSource, HttpInput, StreamSpec};
use crate::audio::{is_offline, HttpStream, Offline, StreamConfig};
use crate::library::model::{navidrome_raw, Source, Track};
use crate::player::Opener;
use crate::settings::SettingsStore;

/// The error text when a Navidrome track is played without a signed-in server.
pub const NOT_CONNECTED: &str = "Navidrome is not connected";

/// Opens Navidrome tracks as [`NavidromeSource`]s at the quality the settings name (read at every
/// open), spilling into `spill_dir`; every other track goes to `fallback`.
pub fn navidrome_opener(
    service: Arc<NavidromeService>,
    settings: Arc<SettingsStore>,
    spill_dir: PathBuf,
    fallback: Opener,
) -> Opener {
    Arc::new(move |track: &Track, spec: StreamSpec| {
        if track.source != Source::Navidrome {
            return fallback(track, spec);
        }
        let client = service
            .client()
            .ok_or_else(|| anyhow::Error::new(Offline(NOT_CONNECTED.into())))?;
        let config = Arc::new(StreamConfig::new(client.stream_http(), spill_dir.clone()));
        let request =
            StreamRequest::for_track(track, client, settings.get().navidrome_quality, config)?;
        let source: Box<dyn AudioSource> = Box::new(NavidromeSource::open(request, Some(spec))?);
        Ok(source)
    })
}

/// What to stream: a song at a quality, through a client and a buffer config.
pub struct StreamRequest {
    pub client: Arc<SubsonicClient>,
    /// The server's song id (without `navidrome:`).
    pub song_id: String,
    pub quality: StreamQuality,
    /// "Artist - Title", for error texts.
    pub label: String,
    /// The container hint: the track's format for the original, `mp3` for a transcode.
    pub extension: Option<String>,
    /// The track's duration, when the stream does not tell.
    pub duration_ms: Option<u64>,
    pub config: Arc<StreamConfig>,
}

impl StreamRequest {
    pub fn for_track(
        track: &Track,
        client: Arc<SubsonicClient>,
        quality: StreamQuality,
        config: Arc<StreamConfig>,
    ) -> anyhow::Result<StreamRequest> {
        let song_id = navidrome_raw(&track.id)
            .with_context(|| format!("{} is not a Navidrome track", track.id))?;
        let extension = match quality {
            StreamQuality::Original => {
                Some(track.format.trim().to_ascii_lowercase()).filter(|f| !f.is_empty())
            }
            _ => Some("mp3".to_owned()),
        };
        Ok(StreamRequest {
            client,
            song_id: song_id.to_owned(),
            quality,
            label: format!("{} - {}", track.artist, track.title),
            extension,
            duration_ms: (track.duration_ms > 0).then_some(track.duration_ms),
            config,
        })
    }

    /// The input for the stream from `offset_s` (a transcode's `timeOffset`), with a fresh token.
    fn input(&self, offset_s: Option<u32>) -> HttpInput {
        HttpInput {
            url: self
                .client
                .stream_url(&self.song_id, self.quality, offset_s),
            label: self.label.clone(),
            extension: self.extension.clone(),
            config: Arc::clone(&self.config),
        }
    }
}

/// How the current stream seeks (see the module docs).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SeekBy {
    /// The decoder seeks; the stream serves byte ranges.
    Range,
    /// Through the bytes held in the buffer from 0 (forward: decode ahead; backward: reopen).
    Buffer,
    /// A new transcode request with `timeOffset`.
    TimeOffset,
}

/// A Navidrome stream at a quality. See the module docs for how it seeks.
pub struct NavidromeSource {
    request: StreamRequest,
    device: Option<StreamSpec>,
    /// The current stream's input (its URL holds the token it was opened with).
    input: HttpInput,
    /// The current stream's buffer, held idle as the source of fresh cursors.
    stream: HttpStream,
    /// `None` only after reopening the decoder failed.
    inner: Option<Box<dyn AudioSource>>,
    spec: StreamSpec,
    duration_ms: Option<u64>,
    /// Where the current stream starts in the track (a `timeOffset` reopen), in ms.
    base_ms: u64,
    /// Frames still to drop before audio is handed out (the fraction of a second a `timeOffset`
    /// reopen cannot express).
    skip: usize,
    /// A seek went to (or past) the end of a transcode: reads report the end until the next
    /// seek. Requesting a `timeOffset` there fails, which must not read as a broken track.
    ended: bool,
}

/// Subsonic durations are whole seconds, so a stream can end up to a second before the
/// library's duration says.
const DURATION_SLACK_MS: u64 = 1_000;

impl NavidromeSource {
    /// Requests the stream and opens a decoder on it (symphonia, or FFmpeg through stdin at the
    /// device rate). Returns once the decoder decoded its first audio.
    pub fn open(request: StreamRequest, device: Option<StreamSpec>) -> anyhow::Result<Self> {
        let input = request.input(None);
        let opened = open_http(&input, device)?;
        let spec = opened.source.spec();
        let duration_ms = opened.source.duration_ms().or(request.duration_ms);
        Ok(NavidromeSource {
            request,
            device,
            input,
            stream: opened.stream,
            inner: Some(opened.source),
            spec,
            duration_ms,
            base_ms: 0,
            skip: 0,
            ended: false,
        })
    }

    fn seek_by(&self) -> SeekBy {
        if self.stream.seekable() {
            SeekBy::Range
        } else if self.request.quality == StreamQuality::Original || self.stream.len().is_some() {
            SeekBy::Buffer
        } else {
            SeekBy::TimeOffset
        }
    }

    fn inner(&mut self) -> anyhow::Result<&mut Box<dyn AudioSource>> {
        self.inner
            .as_mut()
            .with_context(|| format!("{} could not be opened again", self.request.label))
    }

    fn frames_for_ms(&self, ms: u64) -> usize {
        (ms * u64::from(self.spec.sample_rate) / 1000) as usize
    }

    /// Opens the decoder again at byte 0 of the current buffer and moves it to `ms` (decoding
    /// ahead, from the bytes already held).
    fn reopen_in_buffer(&mut self, ms: u64) -> anyhow::Result<()> {
        // One reader per stream: the old decoder (and an FFmpeg feeder) goes first.
        self.inner = None;
        let mut source = open_decoder(&self.stream, &self.input, self.device)?;
        if source.spec() != self.spec {
            bail!(
                "{} changed its format when opened again ({:?} to {:?})",
                self.request.label,
                self.spec,
                source.spec()
            );
        }
        self.skip = 0;
        if ms > 0 {
            if let Err(err) = source.seek(ms) {
                if is_offline(&err) {
                    return Err(err);
                }
                // The decoder cannot even seek forward here: drop the frames up to `ms`. The
                // failed decoder goes first (one reader per stream).
                drop(source);
                source = open_decoder(&self.stream, &self.input, self.device)?;
                self.skip = self.frames_for_ms(ms);
            }
        }
        self.inner = Some(source);
        Ok(())
    }

    /// Requests the transcode again from `ms / 1000` s and drops the remaining `ms % 1000` worth
    /// of frames. At or past the known end, the track just ends: nothing is requested, and a
    /// request in the last [`DURATION_SLACK_MS`] that finds no audio (other than offline) ends
    /// it too.
    fn reopen_at_offset(&mut self, ms: u64) -> anyhow::Result<()> {
        if self.duration_ms.is_some_and(|duration| ms >= duration) {
            self.ended = true;
            return Ok(());
        }
        let seconds = u32::try_from(ms / 1000).unwrap_or(u32::MAX);
        let input = self.request.input((seconds > 0).then_some(seconds));
        let opened = match open_http(&input, self.device) {
            Ok(opened) => opened,
            Err(err)
                if !is_offline(&err)
                    && self
                        .duration_ms
                        .is_some_and(|duration| ms + DURATION_SLACK_MS >= duration) =>
            {
                self.ended = true;
                return Ok(());
            }
            Err(err) => return Err(err),
        };
        if opened.source.spec() != self.spec {
            bail!(
                "{} changed its format after seeking ({:?} to {:?})",
                self.request.label,
                self.spec,
                opened.source.spec()
            );
        }
        // The new stream replaces the old one (its download stops with its last reader).
        self.inner = Some(opened.source);
        self.stream = opened.stream;
        self.input = input;
        self.base_ms = u64::from(seconds) * 1000;
        self.skip = self.frames_for_ms(ms - self.base_ms);
        Ok(())
    }
}

impl AudioSource for NavidromeSource {
    fn spec(&self) -> StreamSpec {
        self.spec
    }

    fn read(&mut self, out: &mut Vec<f32>) -> anyhow::Result<usize> {
        if self.ended {
            return Ok(0);
        }
        let channels = usize::from(self.spec.channels);
        loop {
            let start = out.len();
            let frames = self.inner()?.read(out)?;
            if frames == 0 || self.skip == 0 {
                return Ok(frames);
            }
            let dropped = self.skip.min(frames);
            self.skip -= dropped;
            out.drain(start..start + dropped * channels);
            if dropped < frames {
                return Ok(frames - dropped);
            }
        }
    }

    fn seek(&mut self, ms: u64) -> anyhow::Result<()> {
        self.ended = false;
        match self.seek_by() {
            SeekBy::Range if ms >= self.base_ms => {
                self.skip = 0;
                let base = self.base_ms;
                self.inner()?.seek(ms - base)
            }
            // Before the start of a `timeOffset` stream that turned out byte-seekable.
            SeekBy::Range => self.reopen_at_offset(ms),
            SeekBy::Buffer => {
                self.skip = 0;
                let base = self.base_ms;
                if ms < base {
                    return self.reopen_at_offset(ms);
                }
                match self.inner()?.seek(ms - base) {
                    Ok(()) => Ok(()),
                    Err(err) if is_offline(&err) => Err(err),
                    // A decoder on an unseekable stream seeks forward only.
                    Err(_) => self.reopen_in_buffer(ms - base),
                }
            }
            SeekBy::TimeOffset => self.reopen_at_offset(ms),
        }
    }

    fn duration_ms(&self) -> Option<u64> {
        self.duration_ms
    }
}

#[cfg(test)]
mod tests;
