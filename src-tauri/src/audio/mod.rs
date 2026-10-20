//! Native audio engine: sources (symphonia, FFmpeg), resampling and cpal output.
//!
//! [`Playback`] is the piece of engine logic the engine thread drives: it pulls from
//! the current source, resamples to the output spec, keeps the sink fed, moves on to a
//! pre-opened next source without a gap and reopens the output when the device is lost.
//!
//! Threading contract: `Playback` and `OutputSink` are not `Send`; build them on the engine
//! thread and pass the `Arc<dyn OutputFactory>` in. Open sources (`open_source[_with]`) off the
//! engine thread and hand them over with [`Playback::set_source`] / [`Playback::queue_next`].
//! Each source is then decoded ahead on a thread of its own ([`ReadAhead`]), so a slow read
//! or seek never blocks the engine thread.

pub mod ffmpeg;
pub mod gain;
pub mod output;
pub mod readahead;
pub mod resample;
pub mod ring;
pub mod source;
pub mod symph;

use std::sync::Arc;
use std::time::{Duration, Instant};

use crossbeam_channel::Receiver;

pub use output::{
    list_devices, CpalOutputFactory, NullSink, Output, OutputEvent, OutputFactory, OutputSink,
};
pub use readahead::{ReadAhead, ReadOutcome, READ_AHEAD_MS};
pub use resample::Resampler;
pub use source::{
    open_source, open_source_with, AudioSource, DecoderPanic, Input, StreamSpec,
    FFMPEG_DEFAULT_SPEC,
};

/// How long to wait between attempts to open an output device.
const REOPEN_BACKOFF: Duration = Duration::from_secs(1);
/// The most one `pump` waits for a decode thread that has nothing buffered yet and is not
/// inside a source call, i.e. one that is starting up or waiting for a CPU (right after a
/// start, seek or hand-over, when nothing new would be audible anyway). Such a thread always
/// makes progress; normally it takes well under a millisecond, but with every core busy (a
/// library scan) a new thread was measured waiting several hundred. The cap is a safety net,
/// not a timing. Source I/O is never waited for this long: a thread inside one read or seek
/// is given up on after [`readahead::SLOW_CALL`].
const STARVE_WAIT: Duration = Duration::from_secs(2);

/// What a [`Playback::pump`] call did. Errors from `pump` are always source (decode) errors.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PumpStatus {
    /// Still playing the same source.
    Playing,
    /// The current source ended and the queued next one took over. This fires when the next
    /// source starts being decoded, before it is audible: delay a UI track change by
    /// [`Playback::queued_ms`].
    Advanced,
    /// No source left; everything decoded has been handed to the sink.
    Finished,
    /// No output device could be opened; nothing was decoded. Retried about once a second,
    /// then playback resumes where it was.
    NoDevice,
}

pub struct Playback {
    factory: Arc<dyn OutputFactory>,
    sink: Box<dyn OutputSink>,
    events: Receiver<OutputEvent>,
    /// The sink is a [`NullSink`] and the default output must be (re)opened.
    needs_reopen: bool,
    next_reopen: Instant,
    reopen_backoff: Duration,
    output_error: Option<String>,
    /// Position to continue from once an output is back.
    resume_ms: Option<u64>,
    volume: f32,
    paused: bool,
    current: Option<ReadAhead>,
    next: Option<ReadAhead>,
    resampler: Option<Resampler>,
    decoded: Vec<f32>,
    /// Output-spec samples waiting for room in the sink.
    pending: Vec<f32>,
    pending_offset: usize,
    /// Source position of the last set_source/seek, and frames read since then.
    base_ms: u64,
    frames_read: u64,
    /// A source error hit right after a hand-over, reported on the next pump.
    deferred_error: Option<anyhow::Error>,
    /// Linear factor for the current source's samples (ReplayGain) and for `next`.
    gain: f32,
    next_gain: f32,
}

/// A finite, non-negative gain; anything else plays unchanged.
fn sane_gain(gain: f32) -> f32 {
    if gain.is_finite() && gain >= 0.0 {
        gain
    } else {
        1.0
    }
}

impl Playback {
    /// Opens `device` (falling back to the default output). Never fails for lack of a device:
    /// without one, playback starts on a [`NullSink`] and `pump` reports `NoDevice`.
    pub fn new(factory: Arc<dyn OutputFactory>, device: Option<String>) -> anyhow::Result<Self> {
        let mut output_error = None;
        let mut opened = factory.open(device.clone());
        if let (Err(_), Some(_)) = (&opened, &device) {
            opened = factory.open(None);
        }
        let sink = opened.unwrap_or_else(|err| {
            output_error = Some(format!("{err:#}"));
            Box::new(NullSink::new())
        });
        let events = sink.errors();
        Ok(Playback {
            factory,
            sink,
            events,
            needs_reopen: output_error.is_some(),
            next_reopen: Instant::now() + REOPEN_BACKOFF,
            reopen_backoff: REOPEN_BACKOFF,
            output_error,
            resume_ms: None,
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
            deferred_error: None,
            gain: 1.0,
            next_gain: 1.0,
        })
    }

    /// The output spec, for opening sources with [`open_source_with`].
    pub fn output_spec(&self) -> StreamSpec {
        self.sink.spec()
    }

    /// Why no output device is open, while `pump` reports `NoDevice`.
    pub fn output_error(&self) -> Option<&str> {
        self.output_error.as_deref()
    }

    /// Replaces the current source immediately (drops anything queued in the sink and the
    /// pre-opened next source) and starts decoding it ahead.
    pub fn set_source(&mut self, source: Box<dyn AudioSource>) -> anyhow::Result<()> {
        self.resampler = Some(Resampler::new(source.spec(), self.sink.spec())?);
        self.current = Some(ReadAhead::spawn(source, READ_AHEAD_MS));
        self.next = None;
        self.gain = 1.0;
        self.next_gain = 1.0;
        self.deferred_error = None;
        self.clear_buffers();
        self.base_ms = 0;
        Ok(())
    }

    /// Pre-opened source to continue with when the current one ends. With the same spec the
    /// sample boundary is continuous.
    /// The next source starts at gain 1.0; see [`Playback::set_next_gain`]. It is decoded
    /// ahead from now on, so its start is ready when the current one ends.
    pub fn queue_next(&mut self, source: Option<Box<dyn AudioSource>>) {
        self.next = source.map(|source| ReadAhead::spawn(source, READ_AHEAD_MS));
        self.next_gain = 1.0;
    }

    /// Stops playback and drops the current and next source.
    pub fn stop(&mut self) {
        self.current = None;
        self.next = None;
        self.deferred_error = None;
        self.clear_buffers();
    }

    /// Seeks the current source without waiting: the decode thread seeks while `pump` keeps
    /// returning (an FFmpeg restart can take seconds). A seek error arrives as a decode error
    /// from a later `pump`; the `Result` is kept for callers.
    pub fn seek(&mut self, ms: u64) -> anyhow::Result<()> {
        let Some(current) = self.current.as_mut() else {
            return Ok(());
        };
        current.seek(ms);
        self.clear_buffers();
        self.base_ms = ms;
        Ok(())
    }

    /// The current source has been decoded to its end (or failed): only buffered audio is
    /// left, so this is the moment to queue the next source for a gapless hand-over.
    pub fn current_exhausted(&self) -> bool {
        self.current.as_ref().is_some_and(ReadAhead::exhausted)
    }

    pub fn set_volume(&mut self, volume: f32) {
        self.volume = volume;
        self.sink.set_volume(volume);
    }

    pub fn set_paused(&mut self, paused: bool) {
        self.paused = paused;
        self.sink.set_paused(paused);
    }

    /// A linear factor applied to the current source's samples (ReplayGain), before the
    /// output volume. Every new source starts at 1.0.
    pub fn set_gain(&mut self, gain: f32) {
        self.gain = sane_gain(gain);
    }

    /// The gain for the source queued with [`Playback::queue_next`]; it takes over with it.
    pub fn set_next_gain(&mut self, gain: f32) {
        self.next_gain = sane_gain(gain);
    }

    /// Moves playback to `device` (`None` = the system default) and continues at the same
    /// position. A device that cannot be opened falls back to the default; without any
    /// output it continues like a lost device (`pump` reports `NoDevice` and retries). `Err`
    /// is a source error from seeking back to the position.
    pub fn set_device(&mut self, device: Option<String>) -> anyhow::Result<()> {
        let position = self.current.as_ref().map(|_| self.position_ms());
        // Release the old device before opening the next one.
        self.sink = Box::new(NullSink::new());
        self.events = self.sink.errors();
        let mut opened = self.factory.open(device.clone());
        if opened.is_err() && device.is_some() {
            opened = self.factory.open(None);
        }
        self.resume_ms = position;
        match opened {
            Ok(sink) => {
                self.install(sink);
                self.resume()
            }
            Err(err) => {
                self.output_error = Some(format!("{err:#}"));
                self.needs_reopen = true;
                self.next_reopen = Instant::now() + self.reopen_backoff;
                self.pending.clear();
                self.pending_offset = 0;
                Ok(())
            }
        }
    }

    /// Approximate position in the current source of the sample being heard.
    pub fn position_ms(&self) -> u64 {
        if let Some(ms) = self.resume_ms {
            return ms;
        }
        let Some(current) = self.current.as_ref() else {
            return 0;
        };
        let source_rate = current.spec().sample_rate.max(1) as f64;
        let buffered_source_frames =
            self.buffered_out_frames() * source_rate / self.sink.spec().sample_rate.max(1) as f64;
        let played = (self.frames_read as f64 - buffered_source_frames).max(0.0);
        self.base_ms + (played * 1000.0 / source_rate) as u64
    }

    /// How long until everything handed over so far has been heard.
    pub fn queued_ms(&self) -> u64 {
        (self.buffered_out_frames() * 1000.0 / self.sink.spec().sample_rate.max(1) as f64) as u64
    }

    /// True once nothing is left to play, including the sink's queue.
    pub fn is_drained(&self) -> bool {
        self.current.is_none()
            && self.pending_offset >= self.pending.len()
            && self.sink.queued() == 0
    }

    /// Handles output events and fills the sink as far as it has room, then returns. It does
    /// not wait for the device or for source I/O: it takes what the decode thread has
    /// buffered. With nothing buffered it waits for a decode thread that is only starting up
    /// or busy (capped at [`STARVE_WAIT`]), but at most [`readahead::SLOW_CALL`] for one inside
    /// a read or seek: a slow source (NAS, FFmpeg restart) costs at most one short wait per
    /// slow call, and `pump` returns at once while that call runs on. Call it every few
    /// milliseconds. `Err` is always a source error (the engine skips the track); a missing
    /// device is `Ok(NoDevice)`.
    pub fn pump(&mut self) -> anyhow::Result<PumpStatus> {
        if let Some(err) = self.deferred_error.take() {
            return Err(err);
        }
        let lost = self
            .events
            .try_iter()
            .any(|event| matches!(event, OutputEvent::DeviceLost(_)));
        if lost && !self.needs_reopen {
            self.resume_ms = Some(self.position_ms());
            self.sink = Box::new(NullSink::new());
            self.events = self.sink.errors();
            self.needs_reopen = true;
            self.next_reopen = Instant::now();
        }
        if self.needs_reopen {
            if Instant::now() < self.next_reopen {
                return Ok(PumpStatus::NoDevice);
            }
            if let Err(err) = self.reopen() {
                self.output_error = Some(format!("{err:#}"));
                self.next_reopen = Instant::now() + self.reopen_backoff;
                return Ok(PumpStatus::NoDevice);
            }
            self.resume()?;
        }

        let mut status = PumpStatus::Playing;
        match self.fill(&mut status) {
            Ok(status) => Ok(status),
            Err(err) if status == PumpStatus::Advanced => {
                // Report the hand-over now and the new source's error on the next pump.
                self.deferred_error = Some(err);
                Ok(PumpStatus::Advanced)
            }
            Err(err) => Err(err),
        }
    }

    fn fill(&mut self, status: &mut PumpStatus) -> anyhow::Result<PumpStatus> {
        let wait_until = Instant::now() + STARVE_WAIT;
        loop {
            if self.pending_offset < self.pending.len() {
                self.pending_offset += self.sink.push(&self.pending[self.pending_offset..]);
                if self.pending_offset < self.pending.len() {
                    return Ok(*status);
                }
            }
            self.pending.clear();
            self.pending_offset = 0;

            let Some(current) = self.current.as_mut() else {
                return Ok(if *status == PumpStatus::Advanced {
                    *status
                } else {
                    PumpStatus::Finished
                });
            };
            let Some(resampler) = self.resampler.as_mut() else {
                return Ok(PumpStatus::Finished);
            };
            self.decoded.clear();
            match current.try_read(&mut self.decoded)? {
                ReadOutcome::Frames(frames) => {
                    self.frames_read += frames as u64;
                    if self.gain != 1.0 {
                        let gain = self.gain;
                        self.decoded
                            .iter_mut()
                            .for_each(|sample| *sample = (*sample * gain).clamp(-1.0, 1.0));
                    }
                    resampler.process(&self.decoded, &mut self.pending)?;
                    continue;
                }
                ReadOutcome::Starved => {
                    let now = Instant::now();
                    if now < wait_until && current.wait_ready(wait_until - now) {
                        continue;
                    }
                    // Keep playing what the sink holds; the decode thread catches up.
                    return Ok(*status);
                }
                ReadOutcome::End => {}
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
                    self.gain = std::mem::replace(&mut self.next_gain, 1.0);
                    self.base_ms = 0;
                    self.frames_read = 0;
                    *status = PumpStatus::Advanced;
                }
                None => {
                    resampler.flush(&mut self.pending)?;
                    self.current = None;
                }
            }
        }
    }

    /// Opens the default output in place of the [`NullSink`].
    fn reopen(&mut self) -> anyhow::Result<()> {
        let sink = self.factory.open(None)?;
        self.install(sink);
        Ok(())
    }

    /// Makes `sink` the output, with the current volume and pause state.
    fn install(&mut self, sink: Box<dyn OutputSink>) {
        self.needs_reopen = false;
        self.output_error = None;
        self.sink = sink;
        self.events = self.sink.errors();
        self.sink.set_volume(self.volume);
        self.sink.set_paused(self.paused);
    }

    /// Continues the current source at the remembered position on the new output. The seek
    /// does not wait (the decode thread does it). A queued next source is kept: it has not
    /// been read from, so it still starts at its beginning when the current one ends.
    fn resume(&mut self) -> anyhow::Result<()> {
        self.pending.clear();
        self.pending_offset = 0;
        let Some(current) = self.current.as_mut() else {
            self.resume_ms = None;
            return Ok(());
        };
        self.resampler = Some(Resampler::new(current.spec(), self.sink.spec())?);
        if let Some(position) = self.resume_ms.take() {
            current.seek(position);
            self.base_ms = position;
            self.frames_read = 0;
        }
        Ok(())
    }

    fn buffered_out_frames(&self) -> f64 {
        let buffered_samples = (self.pending.len() - self.pending_offset) + self.sink.queued();
        buffered_samples as f64 / self.sink.spec().channels.max(1) as f64
    }

    fn clear_buffers(&mut self) {
        self.pending.clear();
        self.pending_offset = 0;
        self.frames_read = 0;
        self.resume_ms = None;
        if let Some(resampler) = self.resampler.as_mut() {
            resampler.reset();
        }
        self.sink.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::readahead::tests::{frame_of, SlowSource};
    use crate::library::tags::fixtures;
    use crossbeam_channel::Sender;
    use std::sync::atomic::{AtomicBool, Ordering};
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
        /// While set, every open fails as if no device existed.
        fail: AtomicBool,
        /// A device name that does not exist (opening it fails).
        missing: Mutex<Option<String>>,
    }

    impl FakeFactory {
        fn new(spec: StreamSpec, capacity: usize) -> Arc<Self> {
            Arc::new(FakeFactory {
                spec,
                capacity,
                opened: Mutex::default(),
                senders: Mutex::default(),
                queues: Mutex::default(),
                fail: AtomicBool::new(false),
                missing: Mutex::default(),
            })
        }

        fn last_queue(&self) -> Arc<Mutex<Vec<f32>>> {
            Arc::clone(self.queues.lock().unwrap().last().unwrap())
        }
    }

    impl OutputFactory for FakeFactory {
        fn open(&self, device: Option<String>) -> anyhow::Result<Box<dyn OutputSink>> {
            self.opened.lock().unwrap().push(device.clone());
            if self.fail.load(Ordering::SeqCst) {
                anyhow::bail!("no default output device");
            }
            if device.is_some() && device == *self.missing.lock().unwrap() {
                anyhow::bail!("no output device with that name");
            }
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

    /// A source with the gap_*.wav spec that fails on its first read.
    struct FailingSource;

    impl AudioSource for FailingSource {
        fn spec(&self) -> StreamSpec {
            STEREO_44K
        }
        fn read(&mut self, _out: &mut Vec<f32>) -> anyhow::Result<usize> {
            anyhow::bail!("broken stream")
        }
        fn seek(&mut self, _ms: u64) -> anyhow::Result<()> {
            Ok(())
        }
        fn duration_ms(&self) -> Option<u64> {
            None
        }
    }

    /// Plays everything the fake sink holds (the test's stand-in for the audio callback).
    fn play_queue(factory: &FakeFactory) {
        factory.last_queue().lock().unwrap().clear();
    }

    #[test]
    fn no_device_reports_no_device_without_consuming_the_source() {
        let factory = FakeFactory::new(STEREO_44K, 8_820);
        factory.fail.store(true, Ordering::SeqCst);
        let mut playback = Playback::new(factory.clone(), Some("Speakers".into())).unwrap();
        assert!(playback.output_error().is_some());
        playback.set_source(wav("gap_a.wav")).unwrap();
        playback.reopen_backoff = Duration::ZERO;
        playback.next_reopen = Instant::now();
        assert_eq!(playback.pump().unwrap(), PumpStatus::NoDevice);
        assert_eq!(playback.pump().unwrap(), PumpStatus::NoDevice);
        assert_eq!(playback.frames_read, 0, "nothing was decoded");
        assert_eq!(
            *factory.opened.lock().unwrap(),
            vec![Some("Speakers".to_string()), None, None, None],
            "the requested device, then the default, retried"
        );

        factory.fail.store(false, Ordering::SeqCst);
        assert_eq!(playback.pump().unwrap(), PumpStatus::Playing);
        assert!(playback.output_error().is_none());
        let played = factory.last_queue().lock().unwrap().clone();
        let mut expected = Vec::new();
        wav("gap_a.wav").read(&mut expected).unwrap();
        assert_eq!(
            played[..1000],
            expected[..1000],
            "playback starts at the beginning"
        );
    }

    #[test]
    fn reopen_attempts_back_off() {
        let factory = FakeFactory::new(STEREO_44K, 8_820);
        factory.fail.store(true, Ordering::SeqCst);
        let mut playback = Playback::new(factory.clone(), None).unwrap();
        for _ in 0..10 {
            assert_eq!(playback.pump().unwrap(), PumpStatus::NoDevice);
        }
        assert_eq!(
            factory.opened.lock().unwrap().len(),
            1,
            "no retry within the backoff"
        );
    }

    #[test]
    fn lost_device_resumes_from_the_same_position() {
        let factory = FakeFactory::new(STEREO_44K, 8_820);
        let mut playback = Playback::new(factory.clone(), None).unwrap();
        playback.reopen_backoff = Duration::ZERO;
        playback.set_source(wav("gap_a.wav")).unwrap();
        for _ in 0..3 {
            playback.pump().unwrap();
            play_queue(&factory);
        }
        playback.pump().unwrap(); // the sink holds unplayed audio when the device goes
        let position = playback.position_ms();
        assert!(position >= 290, "{position} ms played");

        factory.fail.store(true, Ordering::SeqCst);
        let lost = factory.senders.lock().unwrap()[0].clone();
        lost.send(OutputEvent::DeviceLost("unplugged".into()))
            .unwrap();
        assert_eq!(playback.pump().unwrap(), PumpStatus::NoDevice);
        assert_eq!(playback.position_ms(), position);

        factory.fail.store(false, Ordering::SeqCst);
        assert_eq!(playback.pump().unwrap(), PumpStatus::Playing);
        assert!(playback.position_ms().abs_diff(position) <= 1);
        let played = factory.last_queue().lock().unwrap().clone();
        let mut expected = Vec::new();
        let mut source = wav("gap_a.wav");
        while source.read(&mut expected).unwrap() > 0 {}
        let offset = (position as usize * 44_100 / 1000) * 2;
        assert_eq!(
            played[..1000],
            expected[offset..offset + 1000],
            "the new output continues where the old one stopped"
        );
    }

    #[test]
    fn error_after_a_hand_over_still_reports_the_advance() {
        let factory = FakeFactory::new(STEREO_44K, usize::MAX);
        let mut playback = Playback::new(factory.clone(), None).unwrap();
        playback.set_source(wav("gap_a.wav")).unwrap();
        playback.queue_next(Some(Box::new(FailingSource)));
        assert_eq!(playback.pump().unwrap(), PumpStatus::Advanced);
        let err = playback.pump().unwrap_err();
        assert!(err.to_string().contains("broken stream"), "{err:#}");
    }

    #[test]
    fn set_source_drops_the_queued_next_source() {
        let factory = FakeFactory::new(STEREO_44K, usize::MAX);
        let mut playback = Playback::new(factory.clone(), None).unwrap();
        playback.set_source(wav("gap_a.wav")).unwrap();
        playback.queue_next(Some(wav("gap_b.wav")));
        playback.set_source(wav("gap_a.wav")).unwrap();
        assert_eq!(playback.pump().unwrap(), PumpStatus::Finished);
    }

    fn decode_all(name: &str) -> Vec<f32> {
        let mut samples = Vec::new();
        let mut source = wav(name);
        while source.read(&mut samples).unwrap() > 0 {}
        samples
    }

    #[test]
    fn gain_scales_each_source_before_the_output() {
        let factory = FakeFactory::new(STEREO_44K, usize::MAX);
        let mut playback = Playback::new(factory.clone(), None).unwrap();
        playback.set_source(wav("gap_a.wav")).unwrap();
        playback.set_gain(0.5);
        playback.queue_next(Some(wav("gap_b.wav")));
        playback.set_next_gain(0.25);
        assert_eq!(playback.pump().unwrap(), PumpStatus::Advanced);
        assert_eq!(playback.pump().unwrap(), PumpStatus::Finished);

        let a = decode_all("gap_a.wav");
        let b = decode_all("gap_b.wav");
        let played = factory.last_queue().lock().unwrap().clone();
        assert_eq!(played.len(), a.len() + b.len());
        for (heard, decoded) in played.iter().zip(&a) {
            assert!((heard - decoded * 0.5).abs() < 1e-6);
        }
        for (heard, decoded) in played[a.len()..].iter().zip(&b) {
            assert!(
                (heard - decoded * 0.25).abs() < 1e-6,
                "the next source has its own gain"
            );
        }
    }

    #[test]
    fn applied_samples_never_exceed_full_scale() {
        let factory = FakeFactory::new(STEREO_44K, usize::MAX);
        let mut playback = Playback::new(factory.clone(), None).unwrap();
        playback.set_source(wav("gap_a.wav")).unwrap();
        playback.set_gain(100.0);
        assert_eq!(playback.pump().unwrap(), PumpStatus::Finished);
        let played = factory.last_queue().lock().unwrap().clone();
        assert!(!played.is_empty());
        assert!(played.iter().all(|sample| sample.abs() <= 1.0));
        assert!(
            played.iter().any(|sample| sample.abs() == 1.0),
            "the gain did clip"
        );
    }

    #[test]
    fn a_new_source_starts_at_unity_gain() {
        let factory = FakeFactory::new(STEREO_44K, usize::MAX);
        let mut playback = Playback::new(factory.clone(), None).unwrap();
        playback.set_source(wav("gap_a.wav")).unwrap();
        playback.set_gain(0.5);
        playback.set_source(wav("gap_a.wav")).unwrap();
        playback.pump().unwrap();
        let played = factory.last_queue().lock().unwrap().clone();
        assert!(played == decode_all("gap_a.wav"));
    }

    #[test]
    fn set_device_reopens_and_keeps_the_position() {
        let factory = FakeFactory::new(STEREO_44K, 8_820);
        let mut playback = Playback::new(factory.clone(), None).unwrap();
        playback.set_source(wav("gap_a.wav")).unwrap();
        for _ in 0..3 {
            playback.pump().unwrap();
            play_queue(&factory);
        }
        playback.pump().unwrap();
        let position = playback.position_ms();
        assert!(position >= 290, "{position} ms played");

        playback.set_device(Some("USB DAC".into())).unwrap();
        assert_eq!(
            factory.opened.lock().unwrap().last().cloned(),
            Some(Some("USB DAC".to_string()))
        );
        assert!(playback.position_ms().abs_diff(position) <= 1);
        assert_eq!(playback.pump().unwrap(), PumpStatus::Playing);
        let played = factory.last_queue().lock().unwrap().clone();
        let expected = decode_all("gap_a.wav");
        let offset = (position as usize * 44_100 / 1000) * 2;
        assert_eq!(
            played[..1000],
            expected[offset..offset + 1000],
            "the new device continues where the old one was"
        );
    }

    #[test]
    fn a_missing_device_falls_back_to_the_default() {
        let factory = FakeFactory::new(STEREO_44K, 8_820);
        *factory.missing.lock().unwrap() = Some("Gone".into());
        let mut playback = Playback::new(factory.clone(), None).unwrap();
        playback.set_source(wav("gap_a.wav")).unwrap();
        playback.set_device(Some("Gone".into())).unwrap();
        assert_eq!(
            *factory.opened.lock().unwrap(),
            vec![None, Some("Gone".to_string()), None],
            "the named device, then the default"
        );
        assert!(playback.output_error().is_none());
        assert_eq!(playback.pump().unwrap(), PumpStatus::Playing);
        assert!(!factory.last_queue().lock().unwrap().is_empty());
    }

    /// Pumps and lets the fake device play what it holds until `done`, collecting the output.
    fn play_until(
        playback: &mut Playback,
        factory: &FakeFactory,
        done: impl Fn(PumpStatus) -> bool,
    ) -> (Vec<PumpStatus>, Vec<f32>) {
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut statuses: Vec<PumpStatus> = Vec::new();
        let mut played = Vec::new();
        loop {
            let status = playback.pump().unwrap();
            if statuses.last() != Some(&status) {
                statuses.push(status);
            }
            played.extend(factory.last_queue().lock().unwrap().drain(..));
            if done(status) {
                return (statuses, played);
            }
            assert!(Instant::now() < deadline, "{statuses:?}");
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    #[test]
    fn slow_source_does_not_block_pump() {
        // A 100 ms device that plays 4 times faster than real time. The source stalls for
        // 500 ms once, in the first read past the initial read-ahead (3 s): soon after the
        // start, with about 3 s (750 ms at 4x) buffered. Decoding inline, a pump would hit
        // the stall about 0.8 s in.
        const SPEED: f64 = 4.0;
        let factory = FakeFactory::new(STEREO_44K, 4_410 * 2);
        let mut playback = Playback::new(factory.clone(), None).unwrap();
        let mut source = SlowSource::new(10);
        source.stall = Some((140_000, Duration::from_millis(500)));
        playback.set_source(Box::new(source)).unwrap();
        let queue = factory.last_queue();
        // Not timed: the first pump waits for the decode thread to start (a busy machine can
        // take a while to schedule a new thread); then the read-ahead fills.
        assert_eq!(playback.pump().unwrap(), PumpStatus::Playing);
        std::thread::sleep(Duration::from_millis(300));

        let started = Instant::now();
        let mut last = started;
        let mut played: Vec<f32> = Vec::new();
        let mut slowest = Duration::ZERO;
        let mut under_runs = 0;
        while started.elapsed() < Duration::from_millis(1_200) {
            let pumped = Instant::now();
            assert_eq!(playback.pump().unwrap(), PumpStatus::Playing);
            slowest = slowest.max(pumped.elapsed());
            std::thread::sleep(Duration::from_millis(2));
            let now = Instant::now();
            let due = ((now - last).as_secs_f64() * SPEED * 44_100.0) as usize * 2;
            last = now;
            let mut queue = queue.lock().unwrap();
            if queue.is_empty() {
                under_runs += 1;
            }
            let take = due.min(queue.len());
            played.extend(queue.drain(..take));
        }
        assert!(
            slowest < Duration::from_millis(20),
            "a pump took {slowest:?}"
        );
        assert_eq!(under_runs, 0, "the device never ran dry");
        assert!(
            played.len() / 2 > 154_350,
            "playback went on past the stall point (3.5 s): {} frames",
            played.len() / 2
        );
        assert!(
            played
                .as_chunks::<2>()
                .0
                .iter()
                .enumerate()
                .all(|(index, frame)| frame_of(frame[0]) == index as u64),
            "one continuous run from the buffer"
        );
    }

    #[test]
    fn seek_does_not_block_on_slow_source() {
        let factory = FakeFactory::new(STEREO_44K, 4_410 * 2);
        let mut playback = Playback::new(factory.clone(), None).unwrap();
        let mut source = SlowSource::new(10);
        source.seek_delay = Duration::from_secs(1);
        playback.set_source(Box::new(source)).unwrap();
        playback.pump().unwrap();
        play_queue(&factory);

        let started = Instant::now();
        playback.seek(5_000).unwrap();
        assert!(
            started.elapsed() < Duration::from_millis(20),
            "seek took {:?}",
            started.elapsed()
        );
        assert_eq!(playback.position_ms(), 5_000);

        let queue = factory.last_queue();
        let deadline = Instant::now() + Duration::from_secs(3);
        let mut pumps = 0;
        let mut slowest = Duration::ZERO;
        while queue.lock().unwrap().is_empty() {
            let pumped = Instant::now();
            assert_eq!(playback.pump().unwrap(), PumpStatus::Playing);
            slowest = slowest.max(pumped.elapsed());
            pumps += 1;
            assert!(Instant::now() < deadline, "nothing played after the seek");
            std::thread::sleep(Duration::from_millis(2));
        }
        // The engine kept pumping through the 1 s seek (one sleep is 2 to 16 ms); at most one
        // pump waited, for about `SLOW_CALL`, to see that the seek is slow.
        assert!(pumps >= 10, "only {pumps} pumps during the seek");
        assert!(
            slowest < Duration::from_millis(500),
            "a pump waited for the seek: {slowest:?}"
        );
        let first = queue.lock().unwrap()[0];
        assert_eq!(frame_of(first), 220_500, "the first frames come from 5 s");
    }

    #[test]
    fn set_device_during_a_gapless_tail_keeps_the_next_source() {
        let factory = FakeFactory::new(STEREO_44K, 8_820);
        let mut playback = Playback::new(factory.clone(), None).unwrap();
        playback.set_source(wav("gap_a.wav")).unwrap();
        playback.queue_next(Some(wav("gap_b.wav")));
        let deadline = Instant::now() + Duration::from_secs(5);
        while !playback.current_exhausted() {
            assert_eq!(playback.pump().unwrap(), PumpStatus::Playing);
            assert!(Instant::now() < deadline, "gap_a was never fully decoded");
            std::thread::sleep(Duration::from_millis(1));
        }
        let position = playback.position_ms();

        playback.set_device(Some("USB DAC".into())).unwrap();
        let (statuses, played) = play_until(&mut playback, &factory, |status| {
            status == PumpStatus::Finished
        });
        assert!(
            statuses.contains(&PumpStatus::Advanced),
            "b took over: {statuses:?}"
        );
        let a = decode_all("gap_a.wav");
        let b = decode_all("gap_b.wav");
        let offset = (position as usize * 44_100 / 1000) * 2;
        let expected: Vec<f32> = [&a[offset..], &b[..]].concat();
        assert_eq!(played.len(), expected.len());
        assert!(
            played == expected,
            "a's tail on the new device, then all of b"
        );
    }

    /// Manual check on real hardware: `cargo test -- --ignored smoke_plays`.
    #[test]
    #[ignore = "needs an audio output device"]
    fn smoke_plays_tone_on_default_device() {
        let mut playback = Playback::new(Arc::new(CpalOutputFactory), None).unwrap();
        assert_eq!(
            playback.output_error(),
            None,
            "no output device could be opened"
        );
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
