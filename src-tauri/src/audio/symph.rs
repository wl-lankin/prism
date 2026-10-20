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

use super::source::{guarded, AudioSource, DecoderPanic, Input, StreamSpec};

/// Consecutive undecodable packets after which a stream is considered broken.
const MAX_CONSECUTIVE_ERRORS: u32 = 32;

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
}

impl SymphoniaSource {
    /// Opens `input`, failing when symphonia cannot probe it or has no decoder for it.
    pub fn open(input: &Input) -> anyhow::Result<Self> {
        Self::open_if_supported(input)?
            .ok_or_else(|| anyhow!("symphonia cannot decode {}", describe(input)))
    }

    /// `Ok(None)` when the probe fails or no decoder exists (the caller falls back to FFmpeg);
    /// `Err` when symphonia should handle the file but it does not decode.
    pub fn open_if_supported(input: &Input) -> anyhow::Result<Option<Self>> {
        let Input::File(path) = input;
        let file = File::open(path).with_context(|| format!("opening {}", path.display()))?;
        let stream = MediaSourceStream::new(Box::new(file), MediaSourceStreamOptions::default());
        let mut hint = Hint::new();
        if let Some(extension) = path.extension().and_then(|ext| ext.to_str()) {
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
        let Ok(reader) = probed else {
            return Ok(None);
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
        .with_context(|| format!("creating a decoder for {}", path.display()))?;

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
        };
        // Decoding the first packet fixes the real output spec and rejects broken streams early.
        let mut first = Vec::new();
        let frames = source
            .decode_next(&mut first)
            .with_context(|| format!("decoding {}", path.display()))?;
        if frames == 0 {
            bail!("{} contains no decodable audio", path.display());
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
                Err(err) => return Err(err.into()),
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
                Err(err) => return Err(err.into()),
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
            if self.spec.channels == 0 {
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
            return Ok(frames - skip);
        }
        Ok(0)
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
            Err(err) => Err(anyhow!(err).context(format!("seeking to {ms} ms"))),
        }
    }

    fn duration_ms(&self) -> Option<u64> {
        self.duration_ms
    }
}

fn describe(input: &Input) -> String {
    let Input::File(path) = input;
    path.display().to_string()
}
