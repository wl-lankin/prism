//! Stations as audio sources: the radio opener the player uses, and [`RadioSource`], which keeps
//! a station playing across joints in its stream.
//!
//! A station's stream comes in through a [`LiveStream`] (a fixed ring fed by one download
//! thread) and is decoded by symphonia, with the content type or the directory's codec as the
//! container hint. AAC goes to FFmpeg through stdin: symphonia decodes AAC-LC only, and most AAC
//! stations send HE-AAC (AAC+), which would play muffled at its core rate. Anything else symphonia
//! cannot open goes to FFmpeg too; the bytes the probe consumed are replayed to it first, so it
//! sees the stream from its first byte. HLS stations (the directory's flag, or an HLS playlist
//! behind the station URL) go to FFmpeg by URL.
//!
//! Joints:
//! - A reconnect continues the byte stream, which MP3 and ADTS AAC decoders resync on.
//! - A chained Ogg (a new chain link at every reconnect, and for many stations at every song) is
//!   followed in place by the decoder, also at another rate or layout; the spec handed out stays
//!   the first one (a resampler adapts).
//! - Any other decoder stop on a healthy stream gets a fresh decoder on the same stream first.
//!   Only a failing stream, or a fresh decoder that stops again within a second, reconnects, at
//!   most [`MAX_REOPENS`] times per minute. Once the station has played, a failed reconnect and
//!   the cap both go up as [`Offline`], so the player holds the station.
//! - An offline station stays offline: the error goes up at once.

use std::collections::VecDeque;
use std::io::{self, Cursor, Read, Seek, SeekFrom};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context};
use crossbeam_channel::{Receiver, RecvTimeoutError};
use symphonia::core::io::MediaSource;

use super::model::Station;
use crate::audio::ffmpeg::{FfmpegSource, LiveFeed};
use crate::audio::http::{is_stream_failure, Offline, StreamFailure};
use crate::audio::live::{CONNECT_TIMEOUT, OFFLINE_TEXT};
use crate::audio::source::{ffmpeg_target, AudioSource, StreamSpec};
use crate::audio::symph::SymphoniaSource;
use crate::audio::{
    is_offline, LiveConfig, LiveInput, LiveReader, LiveStream, Resampler, TitleSink,
};
use crate::library::model::{Source, Track};
use crate::player::Opener;

/// How a station is found by its track id (`radio:<uuid>`).
pub type StationLookup = Arc<dyn Fn(&str) -> Option<Station> + Send + Sync>;
/// The title sink for a station id: a title stays bound to the station that sent it.
pub type TitleSinks = Arc<dyn Fn(&str) -> TitleSink + Send + Sync>;

/// Reconnects a source may make within [`REOPEN_WINDOW`].
pub const MAX_REOPENS: usize = 3;
pub const REOPEN_WINDOW: Duration = Duration::from_secs(60);
/// Silence handed out at most per reconnect, while the fresh stream connects.
pub const MAX_SILENCE_MS: u32 = 300;
const SILENCE_STEP_MS: u32 = 20;
/// A fresh decoder on the same stream that stops before this much audio counts as failing.
const IN_PLACE_PROOF_MS: u64 = 1_000;
/// Bytes of the probe replayed to FFmpeg at most; a probe that reads more is not replayed.
const PROBE_REPLAY_LIMIT: usize = 1024 * 1024;

pub const STATION_GONE: &str = "This station is no longer saved";

/// Opens tracks of [`Source::Radio`] as [`RadioSource`]s; every other track goes to `fallback`.
/// The station is looked up by the track id for its stream URL (`url_resolved`, else `url`),
/// codec and HLS flag. Titles from the stream go to `titles(<station id>)`.
pub fn radio_opener(
    lookup: StationLookup,
    config: Arc<LiveConfig>,
    titles: TitleSinks,
    fallback: Opener,
) -> Opener {
    Arc::new(move |track: &Track, spec: StreamSpec| {
        if track.source != Source::Radio {
            return fallback(track, spec);
        }
        let station = lookup(&track.id)
            .ok_or_else(|| anyhow::Error::new(StreamFailure(STATION_GONE.into())))
            .with_context(|| format!("streaming {}", track.title))?;
        let stream = StationStream::new(&station, Arc::clone(&config), titles(&station.id));
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
    /// This station's title sink.
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

/// The container hint: the content type when it names one, else the codec. `"aac"` goes to
/// FFmpeg, every other hint to symphonia.
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

/// A failure of the stream itself (offline, closed, an HTTP error while reading), not of the
/// decoder: FFmpeg would read the same failing bytes, and a fresh decoder would too.
pub(crate) fn reader_failed(err: &anyhow::Error) -> bool {
    err.chain().any(|inner| {
        inner.is::<Offline>()
            || inner
                .downcast_ref::<io::Error>()
                .is_some_and(is_stream_failure)
    })
}

/// Offline, naming the station: what goes up once a station that played cannot come back.
fn station_offline(label: &str) -> anyhow::Error {
    anyhow::Error::new(Offline(OFFLINE_TEXT.into())).context(format!("streaming {label}"))
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
    /// Dropping it closes the connection. `None` for HLS (FFmpeg holds it).
    stream: Option<LiveStream>,
}

/// What the probe read, for replaying it to FFmpeg.
struct Recording {
    on: AtomicBool,
    bytes: Mutex<Vec<u8>>,
}

impl Recording {
    fn new() -> Recording {
        Recording {
            on: AtomicBool::new(true),
            bytes: Mutex::new(Vec::new()),
        }
    }

    /// Ends the recording; reads no longer lock.
    fn take(&self) -> Vec<u8> {
        self.on.store(false, Ordering::SeqCst);
        std::mem::take(&mut *self.bytes.lock().unwrap_or_else(|e| e.into_inner()))
    }
}

/// A live reader that records what passes through, up to [`PROBE_REPLAY_LIMIT`].
struct Tee {
    reader: LiveReader,
    recorded: Arc<Recording>,
}

impl Read for Tee {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let read = self.reader.read(buf)?;
        if self.recorded.on.load(Ordering::SeqCst) {
            let mut bytes = self
                .recorded
                .bytes
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if bytes.len() + read > PROBE_REPLAY_LIMIT {
                drop(bytes);
                self.recorded.take();
            } else {
                bytes.extend_from_slice(&buf[..read]);
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

/// Connects to the station and opens a decoder on its stream (see the module docs).
fn connect(station: &StationStream, target: StreamSpec) -> anyhow::Result<Opened> {
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
    let decoder = open_decoder(&stream, station, target)?;
    Ok(Opened {
        decoder,
        stream: Some(stream),
    })
}

/// A decoder on a freshly connected stream: FFmpeg for AAC, else symphonia, falling back to
/// FFmpeg (with the probed bytes replayed) unless the stream itself failed.
fn open_decoder(
    stream: &LiveStream,
    station: &StationStream,
    target: StreamSpec,
) -> anyhow::Result<Decoder> {
    let hint = decoder_hint(stream.content_type(), station.codec.as_deref());
    if hint == Some("aac") {
        return ffmpeg_reader(stream, Vec::new(), station, target);
    }
    let recorded = Arc::new(Recording::new());
    let tee = Tee {
        reader: stream.reader(),
        recorded: Arc::clone(&recorded),
    };
    match SymphoniaSource::open_live(Box::new(tee), hint, &station.label) {
        Ok(Some(source)) => {
            recorded.take();
            return Ok(Decoder::Symphonia(source));
        }
        Ok(None) => {}
        Err(err) if reader_failed(&err) => return Err(err),
        // A codec symphonia knows but fails on (a broken first packet, an odd profile).
        Err(_) => {}
    }
    // The probe's reader is gone with it; a new one goes on after the bytes it consumed.
    ffmpeg_reader(stream, recorded.take(), station, target)
}

/// FFmpeg on a new reader of `stream`, with `head` fed first.
fn ffmpeg_reader(
    stream: &LiveStream,
    head: Vec<u8>,
    station: &StationStream,
    target: StreamSpec,
) -> anyhow::Result<Decoder> {
    let reader = stream.reader();
    let canceller = reader.canceller();
    let config = &station.config;
    let feed = LiveFeed {
        reader: Box::new(Cursor::new(head).chain(reader)),
        cancel: Box::new(move || canceller.cancel()),
        wait: config.outage + config.stall + CONNECT_TIMEOUT,
    };
    Ok(Decoder::FfmpegReader(FfmpegSource::open_reader(
        feed,
        &station.label,
        target,
    )?))
}

fn open_hls(url: &str, station: &StationStream, target: StreamSpec) -> anyhow::Result<Opened> {
    Ok(Opened {
        decoder: Decoder::FfmpegUrl(FfmpegSource::open_url(url, &station.label, target)?),
        stream: None,
    })
}

/// Waits `wait`, in short steps so a closed source ends it early. False once closed.
fn pause(wait: Duration, closed: &AtomicBool) -> bool {
    let until = Instant::now() + wait;
    while Instant::now() < until {
        if closed.load(Ordering::SeqCst) {
            return false;
        }
        thread::sleep(Duration::from_millis(20).min(until - Instant::now()));
    }
    !closed.load(Ordering::SeqCst)
}

/// Connects again after a played station's stream failed: retried with the config's backoff
/// while the outage lasts. Anything still failing then is offline (an Icecast mount answers 404
/// while its encoder reconnects).
fn reconnect_within_outage(
    station: &StationStream,
    target: StreamSpec,
    closed: &AtomicBool,
) -> anyhow::Result<Opened> {
    let config = &station.config;
    let started = Instant::now();
    for attempt in 0.. {
        if closed.load(Ordering::SeqCst) {
            break;
        }
        let err = match connect(station, target) {
            Ok(opened) => return Ok(opened),
            Err(err) => err,
        };
        let wait = config
            .backoff
            .get(attempt)
            .or(config.backoff.last())
            .copied()
            .unwrap_or(Duration::from_millis(500));
        if started.elapsed() + wait >= config.outage || !pause(wait, closed) {
            return Err(if is_offline(&err) {
                err
            } else {
                station_offline(&station.label)
            });
        }
    }
    Err(station_offline(&station.label))
}

/// A playing station. No duration, no seek. Its spec is fixed by the first decoder: a decoder
/// or chain link at another rate or channel count is resampled to it.
pub struct RadioSource {
    station: StationStream,
    spec: StreamSpec,
    /// `None` while a reconnect runs, and after an error went up.
    current: Option<Opened>,
    /// The spec `adapter` converts from (the decoder's, as of the last read).
    input_spec: StreamSpec,
    /// Converts the decoder's output to `spec` when the two differ.
    adapter: Option<Resampler>,
    scratch: Vec<f32>,
    /// Frames heard from a decoder opened in place on the same stream; `None` after a connect.
    in_place: Option<u64>,
    /// A reconnect running on a thread of its own.
    reopening: Option<Receiver<anyhow::Result<Opened>>>,
    /// Set on drop: a reconnect still running stops retrying.
    closed: Arc<AtomicBool>,
    /// Frames of silence still allowed while the current reconnect runs.
    silence_left: u64,
    /// When the recent reconnects started, within [`REOPEN_WINDOW`].
    recent: VecDeque<Instant>,
    reopens: usize,
    fresh_decoders: usize,
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

/// Why the decoder stopped.
#[derive(PartialEq, Eq)]
enum Stop {
    /// The decoder ended or failed on the bytes.
    Decoder,
    /// The stream under it failed.
    Reader,
}

impl RadioSource {
    /// Resets the station title (an old title never stays on a new station), connects and
    /// opens a decoder; returns once it decoded its first audio.
    pub fn open(station: StationStream, device: StreamSpec) -> anyhow::Result<RadioSource> {
        (station.titles)(None);
        let opened = connect(&station, ffmpeg_target(Some(device)))?;
        let spec = opened.decoder.spec();
        Ok(RadioSource {
            station,
            spec,
            current: Some(opened),
            input_spec: spec,
            adapter: None,
            scratch: Vec::new(),
            in_place: None,
            reopening: None,
            closed: Arc::default(),
            silence_left: 0,
            recent: VecDeque::new(),
            reopens: 0,
            fresh_decoders: 0,
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

    /// The current decoder's own spec (it may differ from [`AudioSource::spec`]).
    #[cfg(test)]
    pub(crate) fn decoder_spec(&self) -> Option<StreamSpec> {
        self.current.as_ref().map(|opened| opened.decoder.spec())
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

    /// Reconnects so far.
    #[cfg(test)]
    pub(crate) fn reopens(&self) -> usize {
        self.reopens
    }

    /// Fresh decoders opened on the same stream so far.
    #[cfg(test)]
    pub(crate) fn fresh_decoders(&self) -> usize {
        self.fresh_decoders
    }

    /// FFmpeg's target after a reconnect: this source's rate, so only the layout is adapted.
    fn reopen_target(&self) -> StreamSpec {
        StreamSpec {
            sample_rate: self.spec.sample_rate,
            channels: 2,
        }
    }

    fn frames_in(&self, ms: u64) -> u64 {
        ms * u64::from(self.spec.sample_rate) / 1000
    }

    fn close_current(&mut self) {
        self.current = None;
        if let Some(adapter) = self.adapter.as_mut() {
            adapter.reset();
        }
    }

    /// A fresh decoder on the same stream, without a new connection, in the old decoder's
    /// family (symphonia stays symphonia: in the middle of a chained Ogg, FFmpeg would take the
    /// chain for one endless stream). `false`: none could be opened, reconnect.
    fn fresh_in_place(&mut self) -> anyhow::Result<bool> {
        let Some(Opened {
            decoder,
            stream: Some(stream),
        }) = self.current.take()
        else {
            return Ok(false);
        };
        let symphonia = matches!(decoder, Decoder::Symphonia(_));
        // One reader at a time: the old decoder (and an FFmpeg feeder) goes first.
        drop(decoder);
        let fresh = if symphonia {
            let hint = decoder_hint(stream.content_type(), self.station.codec.as_deref());
            SymphoniaSource::open_live(Box::new(stream.reader()), hint, &self.station.label)
                .map(|source| source.map(Decoder::Symphonia))
        } else {
            ffmpeg_reader(&stream, Vec::new(), &self.station, self.reopen_target()).map(Some)
        };
        match fresh {
            Ok(Some(decoder)) => {
                self.current = Some(Opened {
                    decoder,
                    stream: Some(stream),
                });
                self.in_place = Some(0);
                self.fresh_decoders += 1;
                Ok(true)
            }
            Err(err) if is_offline(&err) => Err(err),
            _ => Ok(false),
        }
    }

    /// Starts a reconnect on a thread of its own, unless the cap is reached (then the station
    /// is offline).
    fn reconnect(&mut self) -> anyhow::Result<()> {
        // The old decoder and its connection go first: one connection per station.
        self.close_current();
        self.in_place = None;
        let now = Instant::now();
        while self
            .recent
            .front()
            .is_some_and(|at| now.duration_since(*at) >= REOPEN_WINDOW)
        {
            self.recent.pop_front();
        }
        if self.recent.len() >= MAX_REOPENS {
            return Err(station_offline(&self.station.label));
        }
        self.recent.push_back(now);
        self.reopens += 1;
        let station = self.station.clone();
        let target = self.reopen_target();
        let closed = Arc::clone(&self.closed);
        let (tx, rx) = crossbeam_channel::bounded(1);
        thread::Builder::new()
            .name("prism-radio-reconnect".into())
            .spawn(move || {
                // A source dropped meanwhile drops what was opened here (FFmpeg is reaped).
                let _ = tx.send(reconnect_within_outage(&station, target, &closed));
            })
            .with_context(|| format!("reconnecting {}", self.station.label))?;
        self.reopening = Some(rx);
        self.silence_left = self.frames_in(u64::from(MAX_SILENCE_MS));
        Ok(())
    }

    /// Waits for the reconnect: `Some(frames)` of silence handed out meanwhile, `None` once the
    /// fresh decoder is in place.
    fn wait_for_reconnect(
        &mut self,
        rx: Receiver<anyhow::Result<Opened>>,
        out: &mut Vec<f32>,
    ) -> anyhow::Result<Option<usize>> {
        let received = if self.silence_left > 0 {
            rx.recv_timeout(Duration::from_millis(SILENCE_STEP_MS.into()))
        } else {
            let config = &self.station.config;
            match rx.recv_timeout(config.outage + CONNECT_TIMEOUT + config.stall) {
                Err(RecvTimeoutError::Timeout) => return Err(station_offline(&self.station.label)),
                other => other,
            }
        };
        match received {
            Ok(opened) => {
                self.current = Some(opened?);
                Ok(None)
            }
            Err(RecvTimeoutError::Timeout) => {
                let frames = self
                    .frames_in(u64::from(SILENCE_STEP_MS))
                    .min(self.silence_left)
                    .max(1);
                self.silence_left = self.silence_left.saturating_sub(frames);
                let samples = frames as usize * usize::from(self.spec.channels);
                out.extend(std::iter::repeat_n(0.0, samples));
                self.reopening = Some(rx);
                Ok(Some(frames as usize))
            }
            Err(RecvTimeoutError::Disconnected) => Err(station_offline(&self.station.label)),
        }
    }

    /// One read of the current decoder, in the fixed spec. A decoder whose spec changed (a
    /// chain link, a fresh decoder) gets a fresh adapter.
    fn read_current(&mut self, out: &mut Vec<f32>) -> anyhow::Result<Got> {
        let Some(opened) = self.current.as_mut() else {
            return Ok(Got::Closed);
        };
        self.scratch.clear();
        if opened.decoder.source().read(&mut self.scratch)? == 0 {
            return Ok(Got::End);
        }
        let spec = opened.decoder.spec();
        let start = out.len();
        if spec != self.input_spec {
            if let Some(mut old) = self.adapter.take() {
                old.flush(out)?;
            }
            self.adapter = if spec == self.spec {
                None
            } else {
                Some(Resampler::new(spec, self.spec)?)
            };
            self.input_spec = spec;
        }
        match self.adapter.as_mut() {
            None => out.extend_from_slice(&self.scratch),
            Some(adapter) => adapter.process(&self.scratch, out)?,
        }
        Ok(
            match (out.len() - start) / usize::from(self.spec.channels) {
                0 => Got::Held,
                frames => Got::Frames(frames),
            },
        )
    }
}

impl AudioSource for RadioSource {
    fn spec(&self) -> StreamSpec {
        self.spec
    }

    fn read(&mut self, out: &mut Vec<f32>) -> anyhow::Result<usize> {
        loop {
            if let Some(rx) = self.reopening.take() {
                match self.wait_for_reconnect(rx, out)? {
                    Some(frames) => return Ok(frames),
                    None => continue,
                }
            }
            let stop = match self.read_current(out) {
                Ok(Got::Frames(frames)) => {
                    if let Some(heard) = self.in_place.as_mut() {
                        *heard += frames as u64;
                    }
                    return Ok(frames);
                }
                Ok(Got::Held) => continue,
                Ok(Got::Closed) => return Ok(0),
                Ok(Got::End) => Stop::Decoder,
                Err(err) if is_offline(&err) => {
                    self.close_current();
                    return Err(err);
                }
                Err(err) if reader_failed(&err) => Stop::Reader,
                Err(_) => Stop::Decoder,
            };
            let proven = self
                .in_place
                .is_none_or(|heard| heard >= self.frames_in(IN_PLACE_PROOF_MS));
            if stop == Stop::Decoder && proven && self.fresh_in_place()? {
                continue;
            }
            self.reconnect()?;
        }
    }

    fn seek(&mut self, _ms: u64) -> anyhow::Result<()> {
        Err(anyhow!("a live station cannot seek"))
    }

    fn duration_ms(&self) -> Option<u64> {
        None
    }
}

impl Drop for RadioSource {
    fn drop(&mut self) {
        self.closed.store(true, Ordering::SeqCst);
    }
}

#[cfg(test)]
mod tests;
