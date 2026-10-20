//! Audio output: the `OutputSink` abstraction and its cpal implementation.
//!
//! The engine thread pushes interleaved f32 samples in the device spec into an `rtrb` ring
//! buffer (about 200 ms); the cpal callback pops them (see [`super::ring`]), applies the volume
//! with a 20 ms ramp and writes silence on under-runs instead of blocking.

use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{anyhow, bail, Context};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{ErrorKind, FromSample, SampleFormat, SizedSample, StreamConfig};
use crossbeam_channel::{Receiver, Sender};

use super::ring::{ring, Renderer, RingWriter};
use super::source::{StreamSpec, FFMPEG_DEFAULT_SPEC};

/// Ring buffer length.
const BUFFER_MS: u32 = 200;
/// Samples the callback renders per step, from a buffer allocated when the stream opens.
const BLOCK_SAMPLES: usize = 4096;

/// Something an output reports to the engine.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OutputEvent {
    /// The stream failed (device unplugged, invalidated, ...): the engine reopens the output
    /// with `None` and continues from the same position.
    DeviceLost(String),
}

/// A running output stream. Not `Send`: it is opened and owned by the engine thread.
pub trait OutputSink {
    /// The device spec every pushed sample must already be in.
    fn spec(&self) -> StreamSpec;
    /// Queues interleaved samples without blocking; returns how many were taken (whole frames).
    fn push(&mut self, samples: &[f32]) -> usize;
    /// Samples queued and not yet played.
    fn queued(&self) -> usize;
    /// Drops everything queued (for seeks and stops).
    fn clear(&mut self);
    /// Linear gain, 0.0 to 1.0; ramped over 20 ms.
    fn set_volume(&mut self, volume: f32);
    /// Ramps to silence and stops consuming samples while paused.
    fn set_paused(&mut self, paused: bool);
    /// Stream failures, as [`OutputEvent::DeviceLost`].
    fn errors(&self) -> Receiver<OutputEvent>;
}

/// Opens outputs; `None` is the system default device.
pub trait OutputFactory: Send + Sync {
    fn open(&self, device: Option<String>) -> anyhow::Result<Box<dyn OutputSink>>;
}

/// The real factory: cpal on the default host.
#[derive(Clone, Copy, Debug, Default)]
pub struct CpalOutputFactory;

impl OutputFactory for CpalOutputFactory {
    fn open(&self, device: Option<String>) -> anyhow::Result<Box<dyn OutputSink>> {
        Ok(Box::new(Output::open(device)?))
    }
}

/// Stand-in while no output device can be opened: it accepts nothing.
pub struct NullSink {
    events: Receiver<OutputEvent>,
}

impl NullSink {
    pub fn new() -> Self {
        // The sender is dropped at once: this sink never reports anything.
        let (_, events) = crossbeam_channel::bounded(0);
        NullSink { events }
    }
}

impl Default for NullSink {
    fn default() -> Self {
        Self::new()
    }
}

impl OutputSink for NullSink {
    fn spec(&self) -> StreamSpec {
        FFMPEG_DEFAULT_SPEC
    }
    fn push(&mut self, _samples: &[f32]) -> usize {
        0
    }
    fn queued(&self) -> usize {
        0
    }
    fn clear(&mut self) {}
    fn set_volume(&mut self, _volume: f32) {}
    fn set_paused(&mut self, _paused: bool) {}
    fn errors(&self) -> Receiver<OutputEvent> {
        self.events.clone()
    }
}

/// A cpal output stream fed from a ring buffer.
pub struct Output {
    _stream: cpal::Stream,
    writer: RingWriter,
    spec: StreamSpec,
    device: String,
    events: Receiver<OutputEvent>,
}

impl Output {
    /// Opens `device` by name (as [`list_devices`] reports it), or the default output.
    pub fn open(device: Option<String>) -> anyhow::Result<Output> {
        let host = cpal::default_host();
        let device = match device {
            None => host
                .default_output_device()
                .context("no default output device")?,
            Some(name) => host
                .output_devices()
                .context("listing output devices")?
                .find(|candidate| device_name(candidate).as_deref() == Some(name.as_str()))
                .ok_or_else(|| anyhow!("output device {name:?} not found"))?,
        };
        let name = device_name(&device).unwrap_or_else(|| "unknown device".into());
        let supported = device
            .default_output_config()
            .with_context(|| format!("reading the output config of {name}"))?;
        let format = supported.sample_format();
        let config: StreamConfig = supported.into();
        let spec = StreamSpec {
            sample_rate: config.sample_rate,
            channels: config.channels,
        };
        let (writer, renderer) = ring(spec, (spec.sample_rate * BUFFER_MS / 1000) as usize);
        let (tx, rx) = crossbeam_channel::unbounded();
        let parts = (renderer, tx, spec);
        let stream = match format {
            SampleFormat::F32 => build::<f32>(&device, config, parts),
            SampleFormat::F64 => build::<f64>(&device, config, parts),
            SampleFormat::I16 => build::<i16>(&device, config, parts),
            SampleFormat::I32 => build::<i32>(&device, config, parts),
            SampleFormat::U16 => build::<u16>(&device, config, parts),
            SampleFormat::I8 => build::<i8>(&device, config, parts),
            SampleFormat::U8 => build::<u8>(&device, config, parts),
            other => bail!("unsupported output sample format {other}"),
        }
        .with_context(|| format!("opening {name}"))?;
        stream.play().with_context(|| format!("starting {name}"))?;
        Ok(Output {
            _stream: stream,
            writer,
            spec,
            device: name,
            events: rx,
        })
    }

    pub fn device_name(&self) -> &str {
        &self.device
    }
}

impl OutputSink for Output {
    fn spec(&self) -> StreamSpec {
        self.spec
    }
    fn push(&mut self, samples: &[f32]) -> usize {
        self.writer.push(samples)
    }
    fn queued(&self) -> usize {
        self.writer.queued()
    }
    fn clear(&mut self) {
        self.writer.clear();
    }
    fn set_volume(&mut self, volume: f32) {
        self.writer.set_volume(volume);
    }
    fn set_paused(&mut self, paused: bool) {
        self.writer.set_paused(paused);
    }
    fn errors(&self) -> Receiver<OutputEvent> {
        self.events.clone()
    }
}

/// Names of the available output devices.
pub fn list_devices() -> Vec<String> {
    cpal::default_host()
        .output_devices()
        .map(|devices| devices.filter_map(|device| device_name(&device)).collect())
        .unwrap_or_default()
}

fn device_name(device: &cpal::Device) -> Option<String> {
    device
        .description()
        .ok()
        .map(|description| description.name().to_string())
}

type Parts = (Renderer, Sender<OutputEvent>, StreamSpec);

fn build<T>(
    device: &cpal::Device,
    config: StreamConfig,
    (mut renderer, events, spec): Parts,
) -> anyhow::Result<cpal::Stream>
where
    T: SizedSample + FromSample<f32>,
{
    let channels = spec.channels.max(1) as usize;
    let block = (BLOCK_SAMPLES / channels).max(1) * channels;
    let mut scratch = vec![0f32; block];
    let lost = AtomicBool::new(false);

    let data = move |data: &mut [T], _: &cpal::OutputCallbackInfo| {
        for out in data.chunks_mut(block) {
            let buf = &mut scratch[..out.len()];
            renderer.render(buf);
            for (sample_out, sample) in out.iter_mut().zip(buf.iter()) {
                *sample_out = T::from_sample(*sample);
            }
        }
    };

    let error = move |err: cpal::Error| match err.kind() {
        // Glitches and automatic reroutes: the stream keeps running.
        ErrorKind::Xrun | ErrorKind::DeviceChanged | ErrorKind::RealtimeDenied => {}
        _ => {
            if !lost.swap(true, Ordering::AcqRel) {
                let _ = events.send(OutputEvent::DeviceLost(err.to_string()));
            }
        }
    };

    Ok(device.build_output_stream::<T, _, _>(config, data, error, None)?)
}
