//! Decoding with symphonia.

use std::fs::File;
use std::io::ErrorKind;

use anyhow::{anyhow, bail, Context};
use symphonia::core::codecs::audio::well_known::CODEC_ID_AAC;
use symphonia::core::codecs::audio::{AudioDecoder, AudioDecoderOptions};
use symphonia::core::codecs::CodecParameters;
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, FormatReader, SeekMode, SeekTo, TrackType};
use symphonia::core::io::{MediaSourceStream, MediaSourceStreamOptions};
use symphonia::core::meta::MetadataOptions;
use symphonia::core::units::{Time, TimeBase, Timestamp};

use symphonia::core::io::MediaSource;

use super::http::{is_stream_failure, HttpStream};
use super::source::{
    describe, guarded, AudioSource, DecoderPanic, Input, StreamSpec, LIVE_NEEDS_THE_RADIO_OPENER,
};

/// symphonia's error, with an IO error passed on as itself: symphonia's `Error` names its IO
/// error only through the deprecated `cause`, so `is_offline` would not find an `Offline` in it.
fn symphonia_error(err: SymphoniaError) -> anyhow::Error {
    match err {
        SymphoniaError::IoError(err) => anyhow::Error::new(err),
        other => anyhow::Error::new(other),
    }
}

/// An IO error from the source itself (an HTTP stream gone offline or failed), not one symphonia
/// raised over malformed data ("unexpected end of bitstream", a bogus atom length): only the
/// former is an error at open; the latter falls back to FFmpeg as before.
fn read_failed(err: &std::io::Error) -> bool {
    is_stream_failure(err)
}

/// Consecutive undecodable packets after which a stream is considered broken.
const MAX_CONSECUTIVE_ERRORS: u32 = 32;
/// A live chain link shorter than this is not followed: the stream counts as broken.
const MIN_CHAIN_MS: u64 = 500;

/// A live stream's next chain link could not be followed (it ended too soon, has no audio
/// track or a codec symphonia lacks).
#[derive(Debug)]
pub struct ChainFailed(pub String);

impl std::fmt::Display for ChainFailed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ChainFailed {}

fn chain_failed(text: &str) -> anyhow::Error {
    anyhow::Error::new(ChainFailed(text.into()))
}

pub struct SymphoniaSource {
    reader: Box<dyn FormatReader + 'static>,
    decoder: Box<dyn AudioDecoder>,
    track_id: u32,
    time_base: Option<TimeBase>,
    spec: StreamSpec,
    duration_ms: Option<u64>,
    /// Decoded samples not yet handed out (the first packet is decoded at open).
    pending: Vec<f32>,
    scratch: Vec<f32>,
    /// After a seek: packets before this timestamp are trimmed.
    seek_target: Option<Timestamp>,
    /// Encoder delay the decoder does not trim itself, dropped at the start of the stream.
    priming: usize,
    /// Frames still to drop before audio is handed out.
    skip: usize,
    ended: bool,
    /// Set after a decoder panic; the decoder state is unknown from then on.
    poisoned: bool,
    /// A live stream ([`SymphoniaSource::open_live`]): a new chain link (Ogg) is followed in
    /// place, and its spec may differ.
    live: bool,
    /// Frames handed out since the open or the last chain link.
    since_chain: u64,
}

impl SymphoniaSource {
    /// Opens `input`, failing when symphonia cannot probe it or has no decoder for it.
    pub fn open(input: &Input) -> anyhow::Result<Self> {
        Self::open_if_supported(input)?
            .ok_or_else(|| anyhow!("symphonia cannot decode {}", describe(input)))
    }

    /// `Ok(None)` when the probe fails or no decoder exists (the caller falls back to FFmpeg);
    /// `Err` when symphonia should handle the file but it does not decode.
    ///
    /// A stream (`Input::Http`) is opened here over its own download; [`open_http`] keeps the
    /// download for an FFmpeg fallback instead.
    ///
    /// [`open_http`]: super::source::open_http
    pub fn open_if_supported(input: &Input) -> anyhow::Result<Option<Self>> {
        match input {
            Input::File(path) => {
                let file =
                    File::open(path).with_context(|| format!("opening {}", path.display()))?;
                let extension = path.extension().and_then(|ext| ext.to_str());
                Self::open_media(Box::new(file), extension, &path.display().to_string())
            }
            Input::Http(http) => {
                let stream = HttpStream::open(http)?;
                Self::open_media(
                    Box::new(stream),
                    http.extension.as_deref(),
                    &describe(input),
                )
            }
            Input::Live(_) => bail!(LIVE_NEEDS_THE_RADIO_OPENER),
        }
    }

    /// Opens any media source (a file, an HTTP stream). `extension` is the container hint and
    /// `label` names the source in errors. `Ok(None)` when the probe finds nothing symphonia
    /// decodes; a read error of the source (a stream gone offline) is an `Err`.
    pub fn open_media(
        source: Box<dyn MediaSource>,
        extension: Option<&str>,
        label: &str,
    ) -> anyhow::Result<Option<Self>> {
        Self::open_with(source, extension, label, false)
    }

    /// Like [`open_media`](Self::open_media) for an endless live stream: when the stream starts
    /// a new chain link (a chained Ogg), the decoder follows it in place, at whatever spec the
    /// link has. A link that cannot be followed fails with [`ChainFailed`].
    pub fn open_live(
        source: Box<dyn MediaSource>,
        extension: Option<&str>,
        label: &str,
    ) -> anyhow::Result<Option<Self>> {
        Self::open_with(source, extension, label, true)
    }

    fn open_with(
        source: Box<dyn MediaSource>,
        extension: Option<&str>,
        label: &str,
        live: bool,
    ) -> anyhow::Result<Option<Self>> {
        let stream = MediaSourceStream::new(source, MediaSourceStreamOptions::default());
        let mut hint = Hint::new();
        if let Some(extension) = extension {
            hint.with_extension(extension);
        }

        let probed = guarded("symphonia probe", || {
            Ok(symphonia::default::get_probe().probe(
                &hint,
                stream,
                FormatOptions::default(),
                MetadataOptions::default(),
            ))
        })?;
        let reader = match probed {
            Ok(reader) => reader,
            // The source failed (not just "no format found"): FFmpeg would read the same bytes.
            Err(SymphoniaError::IoError(err)) if read_failed(&err) => {
                return Err(anyhow::Error::new(err).context(format!("reading {label}")));
            }
            Err(_) => return Ok(None),
        };
        let Some(track) = reader.default_track(TrackType::Audio) else {
            return Ok(None);
        };
        let Some(CodecParameters::Audio(params)) = &track.codec_params else {
            return Ok(None);
        };
        let codecs = symphonia::default::get_codecs();
        if codecs.get_audio_decoder(params.codec).is_none() {
            return Ok(None);
        }
        let decoder = guarded("symphonia decoder", || {
            Ok(codecs.make_audio_decoder(params, &AudioDecoderOptions::default())?)
        })
        .with_context(|| format!("creating a decoder for {label}"))?;

        // symphonia does not read MP4 edit lists yet, so AAC keeps its encoder delay. Drop the
        // 1024 frames that ffmpeg and fdk-aac encoders prime with.
        let priming = if params.codec == CODEC_ID_AAC && track.delay.is_none() {
            1024
        } else {
            0
        };
        let track_id = track.id;
        let time_base = track.time_base;
        let rate_hint = params.sample_rate;
        let duration_ms = match (time_base, track.duration, track.num_frames, rate_hint) {
            (Some(base), Some(duration), _, _) => base
                .calc_duration(duration)
                .map(|time| time.as_millis().max(0) as u64),
            (_, _, Some(frames), Some(rate)) if rate > 0 => Some(frames * 1000 / rate as u64),
            _ => None,
        };

        let mut source = SymphoniaSource {
            reader,
            decoder,
            track_id,
            time_base,
            spec: StreamSpec {
                sample_rate: rate_hint.unwrap_or(0),
                channels: 0,
            },
            duration_ms,
            pending: Vec::new(),
            scratch: Vec::new(),
            seek_target: None,
            priming,
            skip: priming,
            ended: false,
            poisoned: false,
            live,
            since_chain: 0,
        };
        // Decoding the first packet fixes the real output spec and rejects broken streams early.
        let mut first = Vec::new();
        let frames = source
            .decode_next(&mut first)
            .with_context(|| format!("decoding {label}"))?;
        if frames == 0 {
            bail!("{label} contains no decodable audio");
        }
        source.pending = first;
        Ok(Some(source))
    }

    /// Decodes packets until one yields audio; appends it and returns its frame count.
    fn decode_next(&mut self, out: &mut Vec<f32>) -> anyhow::Result<usize> {
        if self.poisoned {
            bail!("the decoder failed earlier");
        }
        let result = guarded("symphonia decoder", || self.decode_next_unguarded(out));
        if let Err(err) = &result {
            if err.is::<DecoderPanic>() {
                self.poisoned = true;
            }
        }
        result
    }

    fn decode_next_unguarded(&mut self, out: &mut Vec<f32>) -> anyhow::Result<usize> {
        let mut errors = 0;
        while !self.ended {
            let packet = match self.reader.next_packet() {
                Ok(Some(packet)) => packet,
                Ok(None) => {
                    self.ended = true;
                    break;
                }
                Err(SymphoniaError::IoError(err)) if err.kind() == ErrorKind::UnexpectedEof => {
                    self.ended = true;
                    break;
                }
                Err(SymphoniaError::DecodeError(msg)) => {
                    errors += 1;
                    if errors > MAX_CONSECUTIVE_ERRORS {
                        bail!("malformed stream: {msg}");
                    }
                    continue;
                }
                // The reader already read the new chain link's headers and replaced its tracks.
                Err(SymphoniaError::ResetRequired) if self.live => {
                    self.rechain()?;
                    continue;
                }
                Err(err) => return Err(symphonia_error(err)),
            };
            if packet.track_id != self.track_id {
                continue;
            }
            let buffer = match self.decoder.decode(&packet) {
                Ok(buffer) => buffer,
                Err(SymphoniaError::DecodeError(msg)) => {
                    errors += 1;
                    if errors > MAX_CONSECUTIVE_ERRORS {
                        bail!("malformed stream: {msg}");
                    }
                    continue;
                }
                Err(SymphoniaError::ResetRequired) => {
                    self.decoder.reset();
                    continue;
                }
                Err(err) => return Err(symphonia_error(err)),
            };
            errors = 0;
            let frames = buffer.frames();
            if frames == 0 {
                continue;
            }
            let spec = StreamSpec {
                sample_rate: buffer.spec().rate(),
                channels: buffer.spec().channels().count() as u16,
            };
            // A live chain link may come at another spec; a file may not change it.
            if self.spec.channels == 0 || self.live {
                self.spec = spec;
            } else if spec != self.spec {
                bail!(
                    "the stream format changed mid-stream ({:?} to {spec:?})",
                    self.spec
                );
            }
            buffer.copy_to_vec_interleaved::<f32>(&mut self.scratch);

            // After a seek, drop the frames before the requested position.
            let mut skip = 0;
            if let Some(target) = self.seek_target {
                let behind = target.get().saturating_sub(packet.pts.get());
                if behind > 0 {
                    skip = self.ts_to_frames(behind).min(frames);
                }
                if skip < frames {
                    self.seek_target = None;
                }
            }
            let primed = self.skip.min(frames - skip);
            self.skip -= primed;
            skip += primed;
            if skip == frames {
                continue;
            }
            let channels = spec.channels as usize;
            out.extend_from_slice(&self.scratch[skip * channels..frames * channels]);
            self.since_chain += (frames - skip) as u64;
            return Ok(frames - skip);
        }
        Ok(0)
    }

    /// Follows a live stream into its next chain link: a decoder for the link's audio track.
    fn rechain(&mut self) -> anyhow::Result<()> {
        let rate = u64::from(self.spec.sample_rate.max(1));
        if self.since_chain < rate * MIN_CHAIN_MS / 1000 {
            return Err(chain_failed("the stream's chain links end too soon"));
        }
        let track = self
            .reader
            .default_track(TrackType::Audio)
            .ok_or_else(|| chain_failed("the next chain link has no audio"))?;
        let Some(CodecParameters::Audio(params)) = &track.codec_params else {
            return Err(chain_failed("the next chain link has no audio"));
        };
        let params = params.clone();
        let (track_id, time_base, delay) = (track.id, track.time_base, track.delay);
        let codecs = symphonia::default::get_codecs();
        if codecs.get_audio_decoder(params.codec).is_none() {
            return Err(chain_failed(
                "the next chain link uses a codec symphonia cannot decode",
            ));
        }
        self.decoder = guarded("symphonia decoder", || {
            Ok(codecs.make_audio_decoder(&params, &AudioDecoderOptions::default())?)
        })?;
        self.track_id = track_id;
        self.time_base = time_base;
        self.priming = if params.codec == CODEC_ID_AAC && delay.is_none() {
            1024
        } else {
            0
        };
        self.skip = self.priming;
        self.seek_target = None;
        self.ended = false;
        self.since_chain = 0;
        Ok(())
    }

    fn ts_to_frames(&self, ts: i64) -> usize {
        let rate = self.spec.sample_rate as f64;
        match self
            .time_base
            .and_then(|base| base.calc_time(Timestamp::new(ts)))
        {
            Some(time) => (time.as_secs_f64() * rate).round().max(0.0) as usize,
            None => ts.max(0) as usize,
        }
    }
}

impl AudioSource for SymphoniaSource {
    fn spec(&self) -> StreamSpec {
        self.spec
    }

    fn read(&mut self, out: &mut Vec<f32>) -> anyhow::Result<usize> {
        if !self.pending.is_empty() {
            let frames = self.pending.len() / self.spec.channels as usize;
            out.append(&mut self.pending);
            return Ok(frames);
        }
        self.decode_next(out)
    }

    fn seek(&mut self, ms: u64) -> anyhow::Result<()> {
        if self.poisoned {
            bail!("the decoder failed earlier");
        }
        self.pending.clear();
        let to = SeekTo::Time {
            time: Time::from_millis(ms as i64),
            track_id: Some(self.track_id),
        };
        let reader = &mut self.reader;
        let seeked = guarded("symphonia seek", || Ok(reader.seek(SeekMode::Accurate, to)))?;
        self.decoder.reset();
        match seeked {
            Ok(seeked) => {
                self.ended = false;
                self.seek_target = Some(seeked.required_ts);
                self.skip = if ms == 0 { self.priming } else { 0 };
                Ok(())
            }
            Err(SymphoniaError::SeekError(_)) if self.duration_ms.is_some_and(|d| ms >= d) => {
                // Past the end: the source is simply finished.
                self.ended = true;
                Ok(())
            }
            Err(err) => Err(symphonia_error(err).context(format!("seeking to {ms} ms"))),
        }
    }

    fn duration_ms(&self) -> Option<u64> {
        self.duration_ms
    }
}

#[cfg(test)]
mod tests {
    use std::io::{self, Cursor, Read, Seek, SeekFrom};

    use super::*;
    use crate::audio::http::{is_offline, Offline, StreamFailure};
    use crate::library::tags::fixtures;

    /// A source over a real file's bytes whose reads fail from byte `after` on, with `error`.
    struct Failing {
        bytes: Cursor<Vec<u8>>,
        after: u64,
        error: fn() -> io::Error,
    }

    impl Failing {
        fn new(name: &str, after: u64, error: fn() -> io::Error) -> Box<Failing> {
            let bytes = std::fs::read(fixtures().join(name)).unwrap();
            Box::new(Failing {
                bytes: Cursor::new(bytes),
                after,
                error,
            })
        }
    }

    impl Read for Failing {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            let pos = self.bytes.position();
            if pos >= self.after {
                return Err((self.error)());
            }
            let n = buf.len().min((self.after - pos) as usize);
            self.bytes.read(&mut buf[..n])
        }
    }

    impl Seek for Failing {
        fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
            self.bytes.seek(pos)
        }
    }

    impl MediaSource for Failing {
        fn is_seekable(&self) -> bool {
            false
        }

        fn byte_len(&self) -> Option<u64> {
            None
        }
    }

    #[test]
    fn malformed_data_during_the_probe_falls_back_to_ffmpeg() {
        // IO errors symphonia raises over malformed data, not failures of the source.
        let malformed: [fn() -> io::Error; 3] = [
            || io::Error::other("unexpected end of bitstream"),
            || io::Error::from(io::ErrorKind::OutOfMemory),
            || io::Error::new(io::ErrorKind::InvalidData, "invalid codebook"),
        ];
        for error in malformed {
            for name in ["tagged.flac", "tagged.mp3", "gap_a.wav"] {
                let opened = SymphoniaSource::open_media(Failing::new(name, 30, error), None, name)
                    .unwrap_or_else(|err| panic!("{name}: {err:#}"));
                assert!(opened.is_none(), "{name}: the FFmpeg fallback is kept");
            }
        }
    }

    /// Reads `source` to its end or first error: frames read, the specs seen, and the error.
    fn drain(source: &mut SymphoniaSource) -> (usize, Vec<StreamSpec>, Option<anyhow::Error>) {
        let mut frames = 0;
        let mut specs = vec![source.spec()];
        loop {
            match source.read(&mut Vec::new()) {
                Ok(0) => return (frames, specs, None),
                Ok(read) => frames += read,
                Err(err) => return (frames, specs, Some(err)),
            }
            if specs.last() != Some(&source.spec()) {
                specs.push(source.spec());
            }
        }
    }

    #[test]
    fn only_live_streams_follow_a_chain() {
        let path = fixtures().join("radio/chain.ogg");
        let open = |live: bool| {
            let file = Box::new(File::open(&path).unwrap());
            SymphoniaSource::open_with(file, Some("ogg"), "chain.ogg", live)
                .unwrap()
                .expect("symphonia opens Ogg Vorbis")
        };
        // A file keeps today's behavior: the second chain link is an error.
        let (frames, specs, error) = drain(&mut open(false));
        assert!(error.is_some(), "a local chained Ogg stops at the link");
        assert_eq!(specs.len(), 1);
        assert!(frames < 22_050 * 3, "{frames} frames");

        let (frames, specs, error) = drain(&mut open(true));
        assert!(error.is_none(), "{error:?}");
        let rates: Vec<u32> = specs.iter().map(|spec| spec.sample_rate).collect();
        assert_eq!(
            rates,
            vec![22_050, 44_100],
            "the second link has its own rate"
        );
        // 2 s at 22.05 kHz, then 2 s at 44.1 kHz.
        assert!(frames > 22_050 * 2 + 44_100 * 19 / 10, "{frames} frames");
    }

    #[test]
    fn a_stream_failure_during_the_probe_is_an_error() {
        let offline = SymphoniaSource::open_media(
            Failing::new("tagged.flac", 30, || {
                io::Error::new(io::ErrorKind::ConnectionAborted, Offline("gone".into()))
            }),
            Some("flac"),
            "Demo Artist 1 - Demo Song 1",
        )
        .err()
        .expect("an offline stream is not handed to FFmpeg");
        assert!(is_offline(&offline), "{offline:#}");

        let failed = SymphoniaSource::open_media(
            Failing::new("tagged.flac", 30, || {
                io::Error::other(StreamFailure("the server answered HTTP 500".into()))
            }),
            Some("flac"),
            "Demo Artist 1 - Demo Song 1",
        )
        .err()
        .expect("a failed stream is not handed to FFmpeg");
        assert!(!is_offline(&failed), "{failed:#}");
    }
}
