//! Native audio engine: sources (symphonia, FFmpeg), resampling and cpal output.
//!
//! [`Playback`] is the piece of engine logic the engine thread (Task 8) drives: it pulls from
//! the current source, resamples to the output spec, keeps the sink fed, moves on to a
//! pre-opened next source without a gap and reopens the output when the device is lost.

pub mod ffmpeg;
pub mod output;
pub mod resample;
pub mod source;
pub mod symph;

use std::sync::Arc;

use anyhow::Context;
use crossbeam_channel::Receiver;

pub use output::{list_devices, CpalOutputFactory, Output, OutputEvent, OutputFactory, OutputSink};
pub use resample::Resampler;
pub use source::{open_source, open_source_with, AudioSource, Input, StreamSpec};

use source::guarded;

/// What a [`Playback::pump`] call did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PumpStatus {
    /// Still playing the same source.
    Playing,
    /// The current source ended and the queued next one took over.
    Advanced,
    /// No source left; everything decoded has been handed to the sink.
    Finished,
}

pub struct Playback {
    factory: Arc<dyn OutputFactory>,
    sink: Box<dyn OutputSink>,
    events: Receiver<OutputEvent>,
    /// Set when reopening after a lost device failed; retried on the next pump.
    needs_reopen: bool,
    volume: f32,
    paused: bool,
    current: Option<Box<dyn AudioSource>>,
    next: Option<Box<dyn AudioSource>>,
    resampler: Option<Resampler>,
    decoded: Vec<f32>,
    /// Output-spec samples waiting for room in the sink.
    pending: Vec<f32>,
    pending_offset: usize,
    /// Source position of the last set_source/seek, and frames read since then.
    base_ms: u64,
    frames_read: u64,
}

impl Playback {
    pub fn new(factory: Arc<dyn OutputFactory>, device: Option<String>) -> anyhow::Result<Self> {
        let sink = factory.open(device)?;
        let events = sink.errors();
        Ok(Playback {
            factory,
            sink,
            events,
            needs_reopen: false,
            volume: 1.0,
            paused: false,
            current: None,
            next: None,
            resampler: None,
            decoded: Vec::new(),
            pending: Vec::new(),
            pending_offset: 0,
            base_ms: 0,
            frames_read: 0,
        })
    }

    /// The output spec, for opening sources with [`open_source_with`].
    pub fn output_spec(&self) -> StreamSpec {
        self.sink.spec()
    }

    /// Replaces the current source immediately (drops anything queued in the sink).
    pub fn set_source(&mut self, source: Box<dyn AudioSource>) -> anyhow::Result<()> {
        self.resampler = Some(Resampler::new(source.spec(), self.sink.spec())?);
        self.current = Some(source);
        self.clear_buffers();
        self.base_ms = 0;
        Ok(())
    }

    /// Pre-opened source to continue with when the current one ends. With the same spec the
    /// sample boundary is continuous.
    pub fn queue_next(&mut self, source: Option<Box<dyn AudioSource>>) {
        self.next = source;
    }

    /// Stops playback and drops the current and next source.
    pub fn stop(&mut self) {
        self.current = None;
        self.next = None;
        self.clear_buffers();
    }

    pub fn seek(&mut self, ms: u64) -> anyhow::Result<()> {
        let Some(current) = self.current.as_mut() else {
            return Ok(());
        };
        guarded("seek", || current.seek(ms))?;
        self.clear_buffers();
        self.base_ms = ms;
        Ok(())
    }

    pub fn set_volume(&mut self, volume: f32) {
        self.volume = volume;
        self.sink.set_volume(volume);
    }

    pub fn set_paused(&mut self, paused: bool) {
        self.paused = paused;
        self.sink.set_paused(paused);
    }

    /// Approximate position in the current source of the sample being heard.
    pub fn position_ms(&self) -> u64 {
        let Some(current) = self.current.as_ref() else {
            return 0;
        };
        let source_rate = current.spec().sample_rate.max(1) as f64;
        let out = self.sink.spec();
        let buffered_samples = (self.pending.len() - self.pending_offset) + self.sink.queued();
        let buffered_out_frames = buffered_samples as f64 / out.channels.max(1) as f64;
        let buffered_source_frames =
            buffered_out_frames * source_rate / out.sample_rate.max(1) as f64;
        let played = (self.frames_read as f64 - buffered_source_frames).max(0.0);
        self.base_ms + (played * 1000.0 / source_rate) as u64
    }

    /// True once nothing is left to play, including the sink's queue.
    pub fn is_drained(&self) -> bool {
        self.current.is_none()
            && self.pending_offset >= self.pending.len()
            && self.sink.queued() == 0
    }

    /// Handles output events and fills the sink as far as it has room. Never blocks; call it
    /// every few milliseconds. A decode error is returned as `Err` (the engine skips the track).
    pub fn pump(&mut self) -> anyhow::Result<PumpStatus> {
        let lost = self
            .events
            .try_iter()
            .any(|event| matches!(event, OutputEvent::DeviceLost(_)));
        if lost || self.needs_reopen {
            self.reopen()?;
        }

        let mut status = PumpStatus::Playing;
        loop {
            if self.pending_offset < self.pending.len() {
                self.pending_offset += self.sink.push(&self.pending[self.pending_offset..]);
                if self.pending_offset < self.pending.len() {
                    return Ok(status);
                }
            }
            self.pending.clear();
            self.pending_offset = 0;

            let Some(current) = self.current.as_mut() else {
                return Ok(if status == PumpStatus::Advanced {
                    status
                } else {
                    PumpStatus::Finished
                });
            };
            let Some(resampler) = self.resampler.as_mut() else {
                return Ok(PumpStatus::Finished);
            };
            self.decoded.clear();
            let decoded = &mut self.decoded;
            let frames = guarded("decoding", || current.read(decoded))?;
            if frames > 0 {
                self.frames_read += frames as u64;
                resampler.process(&self.decoded, &mut self.pending)?;
                continue;
            }

            // The current source ended.
            let spec = current.spec();
            match self.next.take() {
                Some(next) => {
                    // Same spec: the resampler keeps running, so the boundary is seamless.
                    if next.spec() != spec {
                        resampler.flush(&mut self.pending)?;
                        self.resampler = Some(Resampler::new(next.spec(), self.sink.spec())?);
                    }
                    self.current = Some(next);
                    self.base_ms = 0;
                    self.frames_read = 0;
                    status = PumpStatus::Advanced;
                }
                None => {
                    resampler.flush(&mut self.pending)?;
                    self.current = None;
                }
            }
        }
    }

    /// Reopens the default output after the device was lost and resumes where playback was.
    fn reopen(&mut self) -> anyhow::Result<()> {
        let position = self.position_ms();
        self.needs_reopen = true;
        let sink = self
            .factory
            .open(None)
            .context("reopening the default output after the device was lost")?;
        self.needs_reopen = false;
        self.sink = sink;
        self.events = self.sink.errors();
        self.sink.set_volume(self.volume);
        self.sink.set_paused(self.paused);
        if let Some(current) = self.current.as_mut() {
            self.resampler = Some(Resampler::new(current.spec(), self.sink.spec())?);
            self.pending.clear();
            self.pending_offset = 0;
            guarded("seek", || current.seek(position))?;
            self.base_ms = position;
            self.frames_read = 0;
        }
        Ok(())
    }

    fn clear_buffers(&mut self) {
        self.pending.clear();
        self.pending_offset = 0;
        self.frames_read = 0;
        if let Some(resampler) = self.resampler.as_mut() {
            resampler.reset();
        }
        self.sink.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::tags::fixtures;
    use crossbeam_channel::Sender;
    use std::sync::Mutex;

    /// An output that accepts up to `capacity` samples and plays nothing.
    struct FakeSink {
        spec: StreamSpec,
        queued: Arc<Mutex<Vec<f32>>>,
        capacity: usize,
        events: Receiver<OutputEvent>,
    }

    impl OutputSink for FakeSink {
        fn spec(&self) -> StreamSpec {
            self.spec
        }
        fn push(&mut self, samples: &[f32]) -> usize {
            let mut queued = self.queued.lock().unwrap();
            let taken = samples.len().min(self.capacity - queued.len());
            queued.extend_from_slice(&samples[..taken]);
            taken
        }
        fn queued(&self) -> usize {
            self.queued.lock().unwrap().len()
        }
        fn clear(&mut self) {
            self.queued.lock().unwrap().clear();
        }
        fn set_volume(&mut self, _volume: f32) {}
        fn set_paused(&mut self, _paused: bool) {}
        fn errors(&self) -> Receiver<OutputEvent> {
            self.events.clone()
        }
    }

    struct FakeFactory {
        spec: StreamSpec,
        capacity: usize,
        opened: Mutex<Vec<Option<String>>>,
        senders: Mutex<Vec<Sender<OutputEvent>>>,
        /// What each opened sink holds; the test plays it by draining.
        queues: Mutex<Vec<Arc<Mutex<Vec<f32>>>>>,
    }

    impl FakeFactory {
        fn new(spec: StreamSpec, capacity: usize) -> Arc<Self> {
            Arc::new(FakeFactory {
                spec,
                capacity,
                opened: Mutex::default(),
                senders: Mutex::default(),
                queues: Mutex::default(),
            })
        }

        fn last_queue(&self) -> Arc<Mutex<Vec<f32>>> {
            Arc::clone(self.queues.lock().unwrap().last().unwrap())
        }
    }

    impl OutputFactory for FakeFactory {
        fn open(&self, device: Option<String>) -> anyhow::Result<Box<dyn OutputSink>> {
            self.opened.lock().unwrap().push(device);
            let (tx, rx) = crossbeam_channel::unbounded();
            self.senders.lock().unwrap().push(tx);
            let queued = Arc::new(Mutex::new(Vec::new()));
            self.queues.lock().unwrap().push(Arc::clone(&queued));
            Ok(Box::new(FakeSink {
                spec: self.spec,
                queued,
                capacity: self.capacity,
                events: rx,
            }))
        }
    }

    fn wav(name: &str) -> Box<dyn AudioSource> {
        open_source(&Input::File(fixtures().join(name))).unwrap()
    }

    const STEREO_48K: StreamSpec = StreamSpec {
        sample_rate: 48_000,
        channels: 2,
    };
    const STEREO_44K: StreamSpec = StreamSpec {
        sample_rate: 44_100,
        channels: 2,
    };

    #[test]
    fn device_lost_triggers_reopen() {
        let factory = FakeFactory::new(STEREO_48K, 9_600);
        let mut playback = Playback::new(factory.clone(), Some("Speakers".into())).unwrap();
        playback.set_source(wav("gap_a.wav")).unwrap();
        assert_eq!(playback.pump().unwrap(), PumpStatus::Playing);

        let first = factory.senders.lock().unwrap()[0].clone();
        first
            .send(OutputEvent::DeviceLost("unplugged".into()))
            .unwrap();
        assert_eq!(playback.pump().unwrap(), PumpStatus::Playing);

        assert_eq!(
            *factory.opened.lock().unwrap(),
            vec![Some("Speakers".to_string()), None],
            "a lost device is reopened on the default output"
        );
        assert!(
            !factory.last_queue().lock().unwrap().is_empty(),
            "the new sink is fed"
        );
    }

    #[test]
    fn same_spec_tracks_join_without_a_gap() {
        // Unlimited sink at the files' own spec: the output is exactly what was decoded.
        let factory = FakeFactory::new(STEREO_44K, usize::MAX);
        let mut playback = Playback::new(factory.clone(), None).unwrap();
        playback.set_source(wav("gap_a.wav")).unwrap();
        playback.queue_next(Some(wav("gap_b.wav")));
        // The first pump decodes everything and reports the advance; the next one the end.
        assert_eq!(playback.pump().unwrap(), PumpStatus::Advanced);
        assert_eq!(playback.pump().unwrap(), PumpStatus::Finished);

        let mut expected = Vec::new();
        for name in ["gap_a.wav", "gap_b.wav"] {
            let mut source = wav(name);
            while source.read(&mut expected).unwrap() > 0 {}
        }
        let played = factory.last_queue().lock().unwrap().clone();
        assert_eq!(played.len(), expected.len());
        assert!(played == expected, "the boundary is sample-continuous");
    }

    #[test]
    fn source_end_reports_advance_then_finish() {
        let factory = FakeFactory::new(STEREO_48K, 4_800);
        let mut playback = Playback::new(factory.clone(), None).unwrap();
        playback.set_source(wav("gap_a.wav")).unwrap();
        playback.queue_next(Some(wav("tagged.flac")));
        let mut statuses = Vec::new();
        for _ in 0..1000 {
            let status = playback.pump().unwrap();
            if statuses.last() != Some(&status) {
                statuses.push(status);
            }
            if status == PumpStatus::Finished {
                break;
            }
            factory.last_queue().lock().unwrap().clear(); // "play" everything queued
        }
        assert_eq!(
            statuses,
            vec![
                PumpStatus::Playing,
                PumpStatus::Advanced,
                PumpStatus::Playing,
                PumpStatus::Finished
            ]
        );
    }

    #[test]
    fn seek_moves_the_position() {
        let factory = FakeFactory::new(STEREO_44K, usize::MAX);
        let mut playback = Playback::new(factory.clone(), None).unwrap();
        playback.set_source(wav("gap_a.wav")).unwrap();
        playback.seek(600).unwrap();
        assert_eq!(playback.position_ms(), 600);
        playback.pump().unwrap();
        let frames = factory.last_queue().lock().unwrap().len() / 2;
        assert!(
            frames.abs_diff(44_100 * 4 / 10) <= 441,
            "{frames} frames after a 600 ms seek"
        );
    }

    /// Manual check on real hardware: `cargo test -- --ignored smoke_plays`.
    #[test]
    #[ignore = "needs an audio output device"]
    fn smoke_plays_tone_on_default_device() {
        let mut playback = Playback::new(Arc::new(CpalOutputFactory), None).unwrap();
        println!("output spec: {:?}", playback.output_spec());
        playback.set_source(wav("tagged.flac")).unwrap();
        let started = std::time::Instant::now();
        while started.elapsed() < std::time::Duration::from_millis(1500) {
            if playback.pump().unwrap() == PumpStatus::Finished && playback.is_drained() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        println!("played to {} ms", playback.position_ms());
    }
}
