//! Converts any rate and channel layout to the output device spec.
//!
//! Channels are mapped first (mono is duplicated, more than two channels are downmixed to
//! stereo), then the rate is converted with a rubato sinc resampler. Equal rates skip
//! resampling entirely, so same-spec playback is bit-exact.

use anyhow::{anyhow, bail};
use rubato::audioadapter_buffers::direct::InterleavedSlice;
use rubato::{
    Async, FixedAsync, Resampler as _, SincInterpolationParameters, SincInterpolationType,
    WindowFunction,
};

use super::source::StreamSpec;

/// Input frames per resampler chunk.
const CHUNK_FRAMES: usize = 1024;
const SQRT_HALF: f32 = std::f32::consts::FRAC_1_SQRT_2;

pub struct Resampler {
    input: StreamSpec,
    output: StreamSpec,
    /// Channels the rate conversion runs on: 1 for a mono device, 2 otherwise.
    work_channels: usize,
    /// `None` when the rates are equal.
    inner: Option<Async<f32>>,
    /// Channel-mapped input waiting for a full chunk.
    work: Vec<f32>,
    chunk_out: Vec<f32>,
    /// Output frames of filter delay still to drop.
    delay_left: usize,
    frames_in: u64,
    frames_out: u64,
}

impl Resampler {
    pub fn new(input: StreamSpec, output: StreamSpec) -> anyhow::Result<Self> {
        if input.sample_rate == 0 || input.channels == 0 {
            bail!("invalid input spec {input:?}");
        }
        if output.sample_rate == 0 || output.channels == 0 {
            bail!("invalid output spec {output:?}");
        }
        let work_channels = if output.channels == 1 { 1 } else { 2 };
        let inner = if input.sample_rate == output.sample_rate {
            None
        } else {
            let ratio = output.sample_rate as f64 / input.sample_rate as f64;
            let params = SincInterpolationParameters::new(128, WindowFunction::BlackmanHarris2)
                .oversampling_factor(256)
                .interpolation(SincInterpolationType::Cubic);
            let resampler = Async::<f32>::new_sinc(
                ratio,
                1.0,
                &params,
                CHUNK_FRAMES,
                work_channels,
                FixedAsync::Input,
            )
            .map_err(|err| anyhow!("creating the resampler: {err}"))?;
            Some(resampler)
        };
        let mut resampler = Resampler {
            input,
            output,
            work_channels,
            inner,
            work: Vec::new(),
            chunk_out: Vec::new(),
            delay_left: 0,
            frames_in: 0,
            frames_out: 0,
        };
        resampler.reset();
        Ok(resampler)
    }

    pub fn input_spec(&self) -> StreamSpec {
        self.input
    }

    pub fn output_spec(&self) -> StreamSpec {
        self.output
    }

    /// Converts interleaved `input` and appends what is ready to `out`. Some frames stay
    /// buffered until the next call or [`flush`](Self::flush).
    pub fn process(&mut self, input: &[f32], out: &mut Vec<f32>) -> anyhow::Result<()> {
        let channels = self.input.channels as usize;
        if !input.len().is_multiple_of(channels) {
            bail!(
                "{} samples is not a whole number of {channels}-channel frames",
                input.len()
            );
        }
        self.frames_in += (input.len() / channels) as u64;
        for frame in input.chunks_exact(channels) {
            let (left, right) = to_stereo(frame);
            if self.work_channels == 1 {
                self.work.push((left + right) * 0.5);
            } else {
                self.work.extend_from_slice(&[left, right]);
            }
        }
        if self.inner.is_none() {
            let work = std::mem::take(&mut self.work);
            self.emit(&work, out, None);
            self.work = work;
            self.work.clear();
            return Ok(());
        }
        while self.work.len() / self.work_channels >= self.next_input_frames() {
            self.run_chunk(out, None)?;
        }
        Ok(())
    }

    /// Emits the buffered tail at the end of a stream, then resets for the next one.
    pub fn flush(&mut self, out: &mut Vec<f32>) -> anyhow::Result<()> {
        if self.inner.is_some() {
            let ratio = self.output.sample_rate as f64 / self.input.sample_rate as f64;
            let expected = (self.frames_in as f64 * ratio).round() as u64;
            // Pad with silence until the filter delay has pushed every real frame out.
            let mut guard = 0;
            while self.frames_out < expected && guard < 64 {
                let need = self.next_input_frames() * self.work_channels;
                if self.work.len() < need {
                    self.work.resize(need, 0.0);
                }
                self.run_chunk(out, Some(expected))?;
                guard += 1;
            }
        }
        self.reset();
        Ok(())
    }

    /// Drops buffered audio and filter state (for seeks and track changes).
    pub fn reset(&mut self) {
        self.work.clear();
        self.frames_in = 0;
        self.frames_out = 0;
        self.delay_left = match &mut self.inner {
            Some(inner) => {
                inner.reset();
                inner.output_delay()
            }
            None => 0,
        };
    }

    fn next_input_frames(&self) -> usize {
        self.inner
            .as_ref()
            .map_or(1, |inner| inner.input_frames_next())
    }

    fn run_chunk(&mut self, out: &mut Vec<f32>, limit: Option<u64>) -> anyhow::Result<()> {
        let Some(inner) = self.inner.as_mut() else {
            return Ok(());
        };
        let channels = self.work_channels;
        let out_frames = inner.output_frames_max();
        self.chunk_out.resize(out_frames * channels, 0.0);
        let in_frames = self.work.len() / channels;
        let input = InterleavedSlice::new(&self.work[..], channels, in_frames)
            .map_err(|err| anyhow!("resampler input: {err}"))?;
        let mut output = InterleavedSlice::new_mut(&mut self.chunk_out[..], channels, out_frames)
            .map_err(|err| anyhow!("resampler output: {err}"))?;
        let (used, written) = inner
            .process_into_buffer(&input, &mut output, None)
            .map_err(|err| anyhow!("resampling: {err}"))?;
        let chunk = std::mem::take(&mut self.chunk_out);
        self.emit(&chunk[..written * channels], out, limit);
        self.chunk_out = chunk;
        self.work.drain(..used * channels);
        Ok(())
    }

    /// Appends work-channel samples to `out` in the output layout, dropping the filter delay
    /// and anything past `limit` output frames.
    fn emit(&mut self, samples: &[f32], out: &mut Vec<f32>, limit: Option<u64>) {
        let channels = self.work_channels;
        let mut frames = samples.chunks_exact(channels);
        let skip = self.delay_left.min(frames.len());
        self.delay_left -= skip;
        let frames = frames.by_ref().skip(skip);
        let out_channels = self.output.channels as usize;
        for frame in frames {
            if limit.is_some_and(|limit| self.frames_out >= limit) {
                break;
            }
            self.frames_out += 1;
            match (channels, out_channels) {
                (1, _) => out.push(frame[0]),
                (_, 2) => out.extend_from_slice(frame),
                _ => {
                    out.extend_from_slice(frame);
                    out.extend(std::iter::repeat_n(0.0, out_channels - 2));
                }
            }
        }
    }
}

/// Maps one frame of any layout to stereo. Layouts follow the WAVE channel order.
fn to_stereo(frame: &[f32]) -> (f32, f32) {
    // (left weight, right weight) per channel.
    const C: (f32, f32) = (SQRT_HALF, SQRT_HALF);
    const L: (f32, f32) = (1.0, 0.0);
    const R: (f32, f32) = (0.0, 1.0);
    const LS: (f32, f32) = (SQRT_HALF, 0.0);
    const RS: (f32, f32) = (0.0, SQRT_HALF);
    const LFE: (f32, f32) = (0.0, 0.0);
    let weights: &[(f32, f32)] = match frame.len() {
        1 => return (frame[0], frame[0]),
        2 => return (frame[0], frame[1]),
        3 => &[L, R, C],
        4 => &[L, R, LS, RS],
        5 => &[L, R, C, LS, RS],
        6 => &[L, R, C, LFE, LS, RS],
        7 => &[L, R, C, LFE, C, LS, RS],
        8 => &[L, R, C, LFE, LS, RS, LS, RS],
        _ => &[],
    };
    let weight = |i: usize| -> (f32, f32) {
        weights
            .get(i)
            .copied()
            .unwrap_or(if i.is_multiple_of(2) { L } else { R })
    };
    let (mut left, mut right, mut left_sum, mut right_sum) = (0.0, 0.0, 0.0, 0.0);
    for (i, sample) in frame.iter().enumerate() {
        let (wl, wr) = weight(i);
        left += sample * wl;
        right += sample * wr;
        left_sum += wl;
        right_sum += wr;
    }
    (left / left_sum.max(1.0), right / right_sum.max(1.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(rate: u32, channels: u16, frames: usize) -> Vec<f32> {
        (0..frames)
            .flat_map(|i| {
                let value = (i as f32 * 440.0 * std::f32::consts::TAU / rate as f32).sin() * 0.5;
                std::iter::repeat_n(value, channels as usize)
            })
            .collect()
    }

    /// Feeds `input` in uneven blocks (as a decoder would) and flushes.
    fn run(resampler: &mut Resampler, input: &[f32], channels: usize) -> Vec<f32> {
        let mut out = Vec::new();
        for block in input.chunks(577 * channels) {
            resampler.process(block, &mut out).unwrap();
        }
        resampler.flush(&mut out).unwrap();
        out
    }

    #[test]
    fn resampler_converts_rate() {
        let input_spec = StreamSpec {
            sample_rate: 44_100,
            channels: 2,
        };
        let output_spec = StreamSpec {
            sample_rate: 48_000,
            channels: 2,
        };
        let mut resampler = Resampler::new(input_spec, output_spec).unwrap();
        let out = run(&mut resampler, &sine(44_100, 2, 44_100), 2);
        assert_eq!(out.len() % 2, 0);
        let frames = out.len() / 2;
        assert!(frames.abs_diff(48_000) <= 64, "{frames} frames out");
        // The signal survives: no silent output and no clipping.
        let peak = out.iter().fold(0f32, |peak, s| peak.max(s.abs()));
        assert!((0.4..=0.6).contains(&peak), "peak {peak}");
    }

    #[test]
    fn mono_is_duplicated_and_surround_is_downmixed() {
        let stereo = StreamSpec {
            sample_rate: 48_000,
            channels: 2,
        };
        let mut mono = Resampler::new(
            StreamSpec {
                sample_rate: 48_000,
                channels: 1,
            },
            stereo,
        )
        .unwrap();
        let out = run(&mut mono, &[0.25, -0.5], 1);
        assert_eq!(out, vec![0.25, 0.25, -0.5, -0.5]);

        let mut surround = Resampler::new(
            StreamSpec {
                sample_rate: 48_000,
                channels: 6,
            },
            stereo,
        )
        .unwrap();
        // FL FR FC LFE BL BR: only the left side carries signal.
        let out = run(&mut surround, &[0.5, 0.0, 0.0, 0.0, 0.5, 0.0], 6);
        assert_eq!(out.len(), 2);
        assert!(out[0] > 0.2 && out[1].abs() < 1e-6, "{out:?}");
    }

    #[test]
    fn same_spec_passes_samples_through_unchanged() {
        let spec = StreamSpec {
            sample_rate: 44_100,
            channels: 2,
        };
        let mut resampler = Resampler::new(spec, spec).unwrap();
        let input = sine(44_100, 2, 1000);
        assert_eq!(run(&mut resampler, &input, 2), input);
    }
}
