//! Audio output: the `OutputSink` abstraction and its cpal implementation.
//!
//! The engine thread pushes interleaved f32 samples in the device spec into an `rtrb` ring
//! buffer (about 200 ms); the cpal callback pops them, applies the volume with a 20 ms ramp
//! and writes silence on under-runs instead of blocking.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;

use anyhow::{anyhow, bail, Context};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{ErrorKind, FromSample, SampleFormat, SizedSample, StreamConfig};
use crossbeam_channel::{Receiver, Sender};

use super::source::StreamSpec;

/// Ring buffer length.
const BUFFER_MS: u32 = 200;
/// Duration of a volume (or pause) ramp.
const RAMP_MS: f32 = 20.0;

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

/// State shared between the engine side and the cpal callback.
struct Shared {
    volume: AtomicU32,
    paused: AtomicBool,
    clear: AtomicBool,
}

/// A cpal output stream fed from a ring buffer.
pub struct Output {
    _stream: cpal::Stream,
    producer: rtrb::Producer<f32>,
    capacity: usize,
    spec: StreamSpec,
    device: String,
    shared: Arc<Shared>,
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
        let channels = spec.channels as usize;
        let capacity = (spec.sample_rate * BUFFER_MS / 1000) as usize * channels;
        let (producer, consumer) = rtrb::RingBuffer::new(capacity);
        let shared = Arc::new(Shared {
            volume: AtomicU32::new(1f32.to_bits()),
            paused: AtomicBool::new(false),
            clear: AtomicBool::new(false),
        });
        let (tx, rx) = crossbeam_channel::unbounded();
        let parts = (consumer, Arc::clone(&shared), tx, spec);
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
            producer,
            capacity,
            spec,
            device: name,
            shared,
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
        let channels = self.spec.channels as usize;
        let room = self.producer.slots() / channels * channels;
        let take = (samples.len() / channels * channels).min(room);
        let (pushed, _) = self.producer.push_partial_slice(&samples[..take]);
        pushed.len()
    }

    fn queued(&self) -> usize {
        self.capacity - self.producer.slots()
    }

    fn clear(&mut self) {
        self.shared.clear.store(true, Ordering::Release);
    }

    fn set_volume(&mut self, volume: f32) {
        let volume = if volume.is_finite() {
            volume.clamp(0.0, 1.0)
        } else {
            1.0
        };
        self.shared
            .volume
            .store(volume.to_bits(), Ordering::Relaxed);
    }

    fn set_paused(&mut self, paused: bool) {
        self.shared.paused.store(paused, Ordering::Relaxed);
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

type Parts = (
    rtrb::Consumer<f32>,
    Arc<Shared>,
    Sender<OutputEvent>,
    StreamSpec,
);

fn build<T>(
    device: &cpal::Device,
    config: StreamConfig,
    (mut consumer, shared, events, spec): Parts,
) -> anyhow::Result<cpal::Stream>
where
    T: SizedSample + FromSample<f32>,
{
    let channels = spec.channels as usize;
    let step = 1.0 / (RAMP_MS / 1000.0 * spec.sample_rate as f32);
    let mut gain = 0f32;
    let mut scratch = vec![0f32; 8192];
    let callback_shared = Arc::clone(&shared);
    let lost = AtomicBool::new(false);

    let data = move |data: &mut [T], _: &cpal::OutputCallbackInfo| {
        let shared = &callback_shared;
        if shared.clear.swap(false, Ordering::AcqRel) {
            let queued = consumer.slots();
            if let Ok(chunk) = consumer.read_chunk(queued) {
                chunk.commit_all();
            }
        }
        if scratch.len() < data.len() {
            scratch.resize(data.len(), 0.0);
        }
        let buf = &mut scratch[..data.len()];
        let paused = shared.paused.load(Ordering::Relaxed);
        let target = if paused {
            0.0
        } else {
            f32::from_bits(shared.volume.load(Ordering::Relaxed))
        };

        // Paused and fully ramped down: keep the queue for resuming.
        let got = if paused && gain == 0.0 {
            0
        } else {
            consumer.pop_partial_slice(buf).0.len()
        };
        buf[got..].fill(0.0);
        for frame in buf.chunks_exact_mut(channels) {
            if gain < target {
                gain = (gain + step).min(target);
            } else if gain > target {
                gain = (gain - step).max(target);
            }
            for sample in frame {
                *sample *= gain;
            }
        }
        for (out, sample) in data.iter_mut().zip(buf.iter()) {
            *out = T::from_sample(*sample);
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
