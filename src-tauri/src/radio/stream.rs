//! Stations as audio sources: the radio opener the player uses, and [`RadioSource`], which keeps
//! a station playing across joints in its stream.
//!
//! A station's stream comes in through a [`LiveStream`] (a fixed ring fed by one download
//! thread) and is decoded by symphonia, with the content type or the directory's codec as the
//! container hint. When symphonia cannot decode it (Opus, a codec it lacks), FFmpeg reads the
//! same stream through stdin; the bytes the probe consumed are replayed to it first, so it sees
//! the stream from its first byte. HLS stations (the directory's flag, or an HLS playlist
//! behind the station URL) go to FFmpeg by URL.
//!
//! Joints: a reconnect continues the byte stream, which MP3 and ADTS AAC decoders resync on.
//! Ogg is chained (a new logical stream starts at every reconnect, and for many stations at
//! every song), which ends symphonia's decoder; any decoder that stops while the station is not
//! offline is replaced by a fresh stream and decoder, at most [`MAX_REOPENS`] times per minute.
//! An offline station stays offline: the error goes up at once.

use std::collections::VecDeque;
use std::io::{self, Cursor, Read, Seek, SeekFrom};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context};
use crossbeam_channel::{Receiver, RecvTimeoutError};
use symphonia::core::io::MediaSource;

use super::model::Station;
use crate::audio::ffmpeg::{FfmpegSource, LiveFeed};
use crate::audio::http::StreamFailure;
use crate::audio::live::CONNECT_TIMEOUT;
use crate::audio::source::{ffmpeg_target, AudioSource, StreamSpec};
use crate::audio::symph::SymphoniaSource;
use crate::audio::{
    is_offline, LiveConfig, LiveInput, LiveReader, LiveStream, Resampler, TitleSink,
};
use crate::library::model::{Source, Track};
use crate::player::Opener;

/// How a station is found by its track id (`radio:<uuid>`).
pub type StationLookup = Arc<dyn Fn(&str) -> Option<Station> + Send + Sync>;

/// Fresh streams and decoders a source may open within [`REOPEN_WINDOW`].
pub const MAX_REOPENS: usize = 3;
pub const REOPEN_WINDOW: Duration = Duration::from_secs(60);
/// Silence handed out at most per reopen, while the fresh stream connects.
pub const MAX_SILENCE_MS: u32 = 300;
const SILENCE_STEP_MS: u32 = 20;
/// Bytes of the probe replayed to FFmpeg at most; a probe that reads more is not replayed.
const PROBE_REPLAY_LIMIT: usize = 1024 * 1024;

pub const STATION_GONE: &str = "This station is no longer saved";

/// Opens tracks of [`Source::Radio`] as [`RadioSource`]s; every other track goes to `fallback`.
/// The station is looked up by the track id for its stream URL (`url_resolved`, else `url`),
/// codec and HLS flag. Titles from the stream go to `titles`.
pub fn radio_opener(
    lookup: StationLookup,
    config: Arc<LiveConfig>,
    titles: TitleSink,
    fallback: Opener,
) -> Opener {
    Arc::new(move |track: &Track, spec: StreamSpec| {
        if track.source != Source::Radio {
            return fallback(track, spec);
        }
        let station = lookup(&track.id)
            .ok_or_else(|| anyhow::Error::new(StreamFailure(STATION_GONE.into())))
            .with_context(|| format!("streaming {}", track.title))?;
        let stream = StationStream::new(&station, Arc::clone(&config), Arc::clone(&titles));
        let source: Box<dyn AudioSource> = Box::new(RadioSource::open(stream, spec)?);
        Ok(source)
    })
}

/// What a [`RadioSource`] plays.
#[derive(Clone)]
pub struct StationStream {
    pub url: String,
    /// The station name, for error texts.
    pub label: String,
    /// The directory's codec ("MP3", "AAC+", "OGG"), a hint only.
    pub codec: Option<String>,
    /// The directory says HLS: FFmpeg reads the URL itself.
    pub hls: bool,
    pub config: Arc<LiveConfig>,
    pub titles: TitleSink,
}

impl StationStream {
    pub fn new(station: &Station, config: Arc<LiveConfig>, titles: TitleSink) -> StationStream {
        let url = station
            .url_resolved
            .as_deref()
            .map(str::trim)
            .filter(|url| !url.is_empty())
            .unwrap_or(station.url.trim());
        StationStream {
            url: url.to_owned(),
            label: station.name.clone(),
            codec: station.codec.clone(),
            hls: station.hls,
            config,
            titles,
        }
    }
}

/// The container hint for symphonia: the content type when it names one, else the codec.
pub(crate) fn decoder_hint(
    content_type: Option<&str>,
    codec: Option<&str>,
) -> Option<&'static str> {
    let by_type = match content_type.map(str::to_ascii_lowercase).as_deref() {
        Some("audio/mpeg" | "audio/mp3" | "audio/mpeg3" | "audio/x-mpeg") => Some("mp3"),
        Some("audio/aac" | "audio/aacp" | "audio/x-aac" | "audio/x-aacp") => Some("aac"),
        Some("application/ogg" | "audio/ogg" | "audio/x-ogg" | "audio/vorbis" | "audio/opus") => {
            Some("ogg")
        }
        Some("audio/flac" | "audio/x-flac") => Some("flac"),
        _ => None,
    };
    by_type.or_else(|| {
        let codec = codec?.trim().to_ascii_lowercase();
        match codec.as_str() {
            "mp3" => Some("mp3"),
            "ogg" | "vorbis" | "opus" => Some("ogg"),
            "flac" => Some("flac"),
            _ if codec.starts_with("aac") || codec.starts_with("he-aac") => Some("aac"),
            _ => None,
        }
    })
}

/// Which decoder a source runs, for tests.
#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DecoderKind {
    Symphonia,
    FfmpegReader,
    FfmpegUrl,
}

enum Decoder {
    Symphonia(SymphoniaSource),
    /// FFmpeg on the live stream through stdin.
    FfmpegReader(FfmpegSource),
    /// FFmpeg fetching an HLS URL itself.
    FfmpegUrl(FfmpegSource),
}

impl Decoder {
    fn source(&mut self) -> &mut dyn AudioSource {
        match self {
            Decoder::Symphonia(source) => source,
            Decoder::FfmpegReader(source) | Decoder::FfmpegUrl(source) => source,
        }
    }

    fn spec(&self) -> StreamSpec {
        match self {
            Decoder::Symphonia(source) => source.spec(),
            Decoder::FfmpegReader(source) | Decoder::FfmpegUrl(source) => source.spec(),
        }
    }
}

/// A decoder and the stream it reads. The decoder is dropped first (an FFmpeg feeder is
/// cancelled before its stream closes).
struct Opened {
    decoder: Decoder,
    /// Held only: dropping it closes the connection. `None` for HLS (FFmpeg holds it).
    _stream: Option<LiveStream>,
}

/// What the probe read, for replaying it to FFmpeg.
struct Recording {
    bytes: Vec<u8>,
    on: bool,
}

impl Recording {
    fn stop(&mut self) {
        self.on = false;
        self.bytes = Vec::new();
    }
}

fn recording(shared: &Mutex<Recording>) -> MutexGuard<'_, Recording> {
    shared.lock().unwrap_or_else(|e| e.into_inner())
}

/// A live reader that records what passes through, up to [`PROBE_REPLAY_LIMIT`].
struct Tee {
    reader: LiveReader,
    recorded: Arc<Mutex<Recording>>,
}

impl Read for Tee {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let read = self.reader.read(buf)?;
        let mut recorded = recording(&self.recorded);
        if recorded.on {
            if recorded.bytes.len() + read > PROBE_REPLAY_LIMIT {
                recorded.stop();
            } else {
                recorded.bytes.extend_from_slice(&buf[..read]);
            }
        }
        Ok(read)
    }
}

impl Seek for Tee {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        self.reader.seek(pos)
    }
}

impl MediaSource for Tee {
    fn is_seekable(&self) -> bool {
        false
    }

    fn byte_len(&self) -> Option<u64> {
        None
    }
}

/// Opens the station's stream and a decoder on it (see the module docs).
fn open_decoder(station: &StationStream, device: StreamSpec) -> anyhow::Result<Opened> {
    let target = ffmpeg_target(Some(device));
    if station.hls {
        return open_hls(&station.url, station, target);
    }
    let input = LiveInput {
        url: station.url.clone(),
        label: station.label.clone(),
        codec_hint: decoder_hint(None, station.codec.as_deref()).map(str::to_owned),
        config: Arc::clone(&station.config),
        titles: Some(Arc::clone(&station.titles)),
    };
    let stream = LiveStream::open(&input)?;
    if stream.is_hls() {
        let url = stream.resolved_url().to_owned();
        drop(stream);
        return open_hls(&url, station, target);
    }
    let hint = decoder_hint(stream.content_type(), station.codec.as_deref());
    let recorded = Arc::new(Mutex::new(Recording {
        bytes: Vec::new(),
        on: true,
    }));
    let tee = Tee {
        reader: stream.reader(),
        recorded: Arc::clone(&recorded),
    };
    if let Some(source) = SymphoniaSource::open_media(Box::new(tee), hint, &station.label)? {
        recording(&recorded).stop();
        return Ok(Opened {
            decoder: Decoder::Symphonia(source),
            _stream: Some(stream),
        });
    }
    // The probe's reader is gone with it; a new one goes on after the bytes it consumed.
    let head = std::mem::take(&mut recording(&recorded).bytes);
    let reader = stream.reader();
    let canceller = reader.canceller();
    let config = &station.config;
    let feed = LiveFeed {
        reader: Box::new(Cursor::new(head).chain(reader)),
        cancel: Box::new(move || canceller.cancel()),
        wait: config.outage + config.stall + CONNECT_TIMEOUT,
    };
    let source = FfmpegSource::open_reader(feed, &station.label, target)?;
    Ok(Opened {
        decoder: Decoder::FfmpegReader(source),
        _stream: Some(stream),
    })
}

fn open_hls(url: &str, station: &StationStream, target: StreamSpec) -> anyhow::Result<Opened> {
    Ok(Opened {
        decoder: Decoder::FfmpegUrl(FfmpegSource::open_url(url, &station.label, target)?),
        _stream: None,
    })
}

/// A playing station. No duration, no seek. Its spec is fixed by the first decoder: a fresh
/// decoder at another rate or channel count is resampled to it.
pub struct RadioSource {
    station: StationStream,
    device: StreamSpec,
    spec: StreamSpec,
    /// `None` while a reopen runs, and after an error went up.
    current: Option<Opened>,
    /// Converts a fresh decoder's output to `spec` when it differs.
    adapter: Option<Resampler>,
    scratch: Vec<f32>,
    /// A fresh stream and decoder being opened on a thread of their own.
    reopening: Option<Receiver<anyhow::Result<Opened>>>,
    /// Frames of silence still allowed while the current reopen runs.
    silence_left: u64,
    /// When the recent reopens started, within [`REOPEN_WINDOW`].
    recent: VecDeque<Instant>,
    reopens: usize,
}

impl RadioSource {
    /// Resets the station title (an old title never stays on a new station), connects and
    /// opens a decoder; returns once it decoded its first audio.
    pub fn open(station: StationStream, device: StreamSpec) -> anyhow::Result<RadioSource> {
        (station.titles)(None);
        let opened = open_decoder(&station, device)?;
        let spec = opened.decoder.spec();
        Ok(RadioSource {
            station,
            device,
            spec,
            current: Some(opened),
            adapter: None,
            scratch: Vec::new(),
            reopening: None,
            silence_left: 0,
            recent: VecDeque::new(),
            reopens: 0,
        })
    }

    #[cfg(test)]
    pub(crate) fn decoder_kind(&self) -> DecoderKind {
        match self.current.as_ref().map(|opened| &opened.decoder) {
            Some(Decoder::Symphonia(_)) => DecoderKind::Symphonia,
            Some(Decoder::FfmpegReader(_)) => DecoderKind::FfmpegReader,
            Some(Decoder::FfmpegUrl(_)) => DecoderKind::FfmpegUrl,
            None => panic!("no decoder right now"),
        }
    }

    #[cfg(test)]
    pub(crate) fn ffmpeg_pid(&self) -> Option<u32> {
        match self.current.as_ref().map(|opened| &opened.decoder) {
            Some(Decoder::FfmpegReader(source) | Decoder::FfmpegUrl(source)) => {
                Some(source.child_id())
            }
            _ => None,
        }
    }

    /// Fresh streams and decoders opened so far.
    #[cfg(test)]
    pub(crate) fn reopens(&self) -> usize {
        self.reopens
    }

    /// The decoder stopped while the station is not offline: starts a fresh stream and decoder,
    /// unless the cap is reached (`false`: the stop goes up).
    fn reopen(&mut self) -> anyhow::Result<bool> {
        let now = Instant::now();
        while self
            .recent
            .front()
            .is_some_and(|at| now.duration_since(*at) >= REOPEN_WINDOW)
        {
            self.recent.pop_front();
        }
        // The old decoder and its connection go first: one connection per station.
        self.current = None;
        self.adapter = None;
        if self.recent.len() >= MAX_REOPENS {
            return Ok(false);
        }
        self.recent.push_back(now);
        self.reopens += 1;
        let station = self.station.clone();
        let device = self.device;
        let (tx, rx) = crossbeam_channel::bounded(1);
        thread::Builder::new()
            .name("prism-radio-reopen".into())
            .spawn(move || {
                // A source dropped meanwhile drops what was opened here (FFmpeg is reaped).
                let _ = tx.send(open_decoder(&station, device));
            })
            .with_context(|| format!("reopening {}", self.station.label))?;
        self.reopening = Some(rx);
        self.silence_left = u64::from(MAX_SILENCE_MS) * u64::from(self.spec.sample_rate) / 1000;
        Ok(true)
    }

    /// Waits for the reopen: `Some(frames)` of silence handed out meanwhile, `None` once the
    /// fresh decoder is in place.
    fn wait_for_reopen(
        &mut self,
        rx: Receiver<anyhow::Result<Opened>>,
        out: &mut Vec<f32>,
    ) -> anyhow::Result<Option<usize>> {
        let received = if self.silence_left > 0 {
            rx.recv_timeout(Duration::from_millis(SILENCE_STEP_MS.into()))
        } else {
            rx.recv().map_err(|_| RecvTimeoutError::Disconnected)
        };
        match received {
            Ok(opened) => {
                self.install(opened?)?;
                Ok(None)
            }
            Err(RecvTimeoutError::Timeout) => {
                let step = u64::from(SILENCE_STEP_MS) * u64::from(self.spec.sample_rate) / 1000;
                let frames = step.min(self.silence_left).max(1);
                self.silence_left = self.silence_left.saturating_sub(frames);
                let samples = frames as usize * usize::from(self.spec.channels);
                out.extend(std::iter::repeat_n(0.0, samples));
                self.reopening = Some(rx);
                Ok(Some(frames as usize))
            }
            Err(RecvTimeoutError::Disconnected) => {
                bail!("reopening {} stopped", self.station.label)
            }
        }
    }

    fn install(&mut self, opened: Opened) -> anyhow::Result<()> {
        let spec = opened.decoder.spec();
        self.adapter = if spec == self.spec {
            None
        } else {
            Some(Resampler::new(spec, self.spec)?)
        };
        self.current = Some(opened);
        Ok(())
    }

    /// One read of the current decoder, in the fixed spec.
    fn read_current(&mut self, out: &mut Vec<f32>) -> anyhow::Result<Got> {
        let Some(opened) = self.current.as_mut() else {
            return Ok(Got::Closed);
        };
        let Some(adapter) = self.adapter.as_mut() else {
            return Ok(match opened.decoder.source().read(out)? {
                0 => Got::End,
                frames => Got::Frames(frames),
            });
        };
        self.scratch.clear();
        if opened.decoder.source().read(&mut self.scratch)? == 0 {
            return Ok(Got::End);
        }
        let start = out.len();
        adapter.process(&self.scratch, out)?;
        Ok(
            match (out.len() - start) / usize::from(self.spec.channels) {
                0 => Got::Held,
                frames => Got::Frames(frames),
            },
        )
    }
}

/// What one read of the current decoder gave.
enum Got {
    Frames(usize),
    /// The adapter keeps the frames until its first chunk is full: read again.
    Held,
    /// The decoder ended.
    End,
    /// No decoder: an error went up earlier.
    Closed,
}

impl AudioSource for RadioSource {
    fn spec(&self) -> StreamSpec {
        self.spec
    }

    fn read(&mut self, out: &mut Vec<f32>) -> anyhow::Result<usize> {
        loop {
            if let Some(rx) = self.reopening.take() {
                match self.wait_for_reopen(rx, out)? {
                    Some(frames) => return Ok(frames),
                    None => continue,
                }
            }
            let error = match self.read_current(out) {
                Ok(Got::Frames(frames)) => return Ok(frames),
                Ok(Got::Held) => continue,
                Ok(Got::Closed) => return Ok(0),
                Ok(Got::End) => None,
                Err(err) if is_offline(&err) => {
                    self.current = None;
                    return Err(err);
                }
                Err(err) => Some(err),
            };
            if !self.reopen()? {
                return match error {
                    Some(err) => Err(err),
                    None => Ok(0),
                };
            }
        }
    }

    fn seek(&mut self, _ms: u64) -> anyhow::Result<()> {
        Err(anyhow!("a live station cannot seek"))
    }

    fn duration_ms(&self) -> Option<u64> {
        None
    }
}

#[cfg(test)]
mod tests;
