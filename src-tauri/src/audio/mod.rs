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
pub mod mix;
pub mod output;
pub mod readahead;
pub mod resample;
pub mod ring;
pub mod source;
pub mod symph;

use std::sync::Arc;
use std::time::{Duration, Instant};

use crossbeam_channel::Receiver;

pub use mix::{equal_power, LevelMeter};
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
/// How long `pump` waits for the decode thread when nothing is buffered: not at all. Pause,
/// seek, next and state events must never queue behind a slow NAS read or a busy CPU; the
/// engine simply pumps again on its next tick. (Tests may set [`Playback::starve_wait`].)
const STARVE_WAIT: Duration = Duration::ZERO;

/// What a [`Playback::pump`] call did. Errors from `pump` are always source (decode) errors.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PumpStatus {
    /// Still playing the same source.
    Playing,
    /// The current source ended and the queued next one took over, or a crossfade into it
    /// started. This fires when the next source starts being decoded, before it is audible:
    /// delay a UI track change by [`Playback::queued_ms`].
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
    gain: Gain,
    next_gain: f32,
    /// How long `next` blends into the current source's end; 0 is a gapless hand-over.
    next_fade_ms: u32,
    /// The previous source playing out under the current one, while a crossfade runs.
    fade: Option<Fade>,
    /// How far ahead every source is decoded ([`READ_AHEAD_MS`], more for a crossfade).
    read_ahead_ms: u32,
    /// The current source's output-spec samples during a crossfade, before they are mixed.
    incoming: Vec<f32>,
    /// The outgoing source's decoded samples during a crossfade.
    outgoing: Vec<f32>,
    /// How long one `pump` may wait for a decode thread with nothing buffered
    /// ([`STARVE_WAIT`], zero, in production).
    starve_wait: Duration,
    /// Loudness of the samples handed to the sink (never measured in the audio callback).
    meter: LevelMeter,
    /// When each recent meter reading becomes audible: the sink plays what it holds first.
    heard: LevelTimeline,
}

/// How many recent level readings [`Playback`] keeps for [`Playback::level`]: one per pump that
/// fed the sink (about every 10 ms), well beyond the output's buffer.
const LEVEL_READINGS: usize = 64;

/// Level readings with the instant each one is heard, in a fixed ring (no allocation).
struct LevelTimeline {
    readings: [(Instant, f32); LEVEL_READINGS],
    /// Next slot to write.
    next: usize,
    len: usize,
}

impl LevelTimeline {
    fn new() -> LevelTimeline {
        LevelTimeline {
            readings: [(Instant::now(), 0.0); LEVEL_READINGS],
            next: 0,
            len: 0,
        }
    }

    fn record(&mut self, at: Instant, level: f32) {
        self.readings[self.next] = (at, level);
        self.next = (self.next + 1) % LEVEL_READINGS;
        self.len = (self.len + 1).min(LEVEL_READINGS);
    }

    /// The newest reading already heard at `now`; 0 when none is.
    fn at(&self, now: Instant) -> f32 {
        (1..=self.len)
            .map(|back| self.readings[(self.next + LEVEL_READINGS - back) % LEVEL_READINGS])
            .find(|(at, _)| *at <= now)
            .map_or(0.0, |(_, level)| level)
    }

    fn clear(&mut self) {
        self.len = 0;
    }
}

/// A finite, non-negative gain; anything else plays unchanged.
fn sane_gain(gain: f32) -> f32 {
    if gain.is_finite() && gain >= 0.0 {
        gain
    } else {
        1.0
    }
}

/// How the source queued with [`Playback::queue_next_with`] takes over.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NextOptions {
    /// Linear factor for its samples (ReplayGain).
    pub gain: f32,
    /// How long it blends into the current source's end (equal power); 0 is gapless.
    pub crossfade_ms: u32,
}

impl Default for NextOptions {
    fn default() -> Self {
        NextOptions {
            gain: 1.0,
            crossfade_ms: 0,
        }
    }
}

/// Frames in `ms` at `sample_rate`.
fn frames_in(ms: u32, sample_rate: u32) -> u64 {
    ms as u64 * sample_rate as u64 / 1000
}

/// A linear gain that moves to a new value over one ramp (as long as the output's volume
/// ramps in [`ring`]) instead of jumping, so a change does not click.
#[derive(Clone, Copy, Debug)]
struct Gain {
    value: f32,
    target: f32,
    /// Change per frame while ramping.
    step: f32,
}

impl Gain {
    fn fixed(gain: f32) -> Gain {
        Gain {
            value: gain,
            target: gain,
            step: 0.0,
        }
    }

    fn ramp_to(&mut self, target: f32, sample_rate: u32) {
        self.target = target;
        self.step = (target - self.value).abs() / ring::ramp_frames(sample_rate);
    }

    /// Scales interleaved `samples` (clamped to full scale), ramping frame by frame.
    fn apply(&mut self, samples: &mut [f32], channels: usize) {
        if self.value == self.target {
            if self.value != 1.0 {
                let gain = self.value;
                samples
                    .iter_mut()
                    .for_each(|sample| *sample = (*sample * gain).clamp(-1.0, 1.0));
            }
            return;
        }
        for frame in samples.chunks_mut(channels.max(1)) {
            self.value = ring::ramp_toward(self.value, self.target, self.step);
            let gain = self.value;
            frame
                .iter_mut()
                .for_each(|sample| *sample = (*sample * gain).clamp(-1.0, 1.0));
        }
    }
}

/// The previous source playing out under the next one. It starts only once the previous
/// source has been decoded to its end, so reading the rest of it never waits.
struct Fade {
    outgoing: ReadAhead,
    /// The outgoing source's own resampler: both sources are converted separately.
    resampler: Resampler,
    gain: f32,
    /// Output-spec samples of the outgoing source not mixed yet, from `offset`.
    buffer: Vec<f32>,
    offset: usize,
    /// Nothing more comes from the outgoing source.
    ended: bool,
    /// Output frames mixed so far, of `length`.
    position: u64,
    length: u64,
}

impl Fade {
    /// Buffers at least one more outgoing frame of `channels` samples; false once the
    /// outgoing source has nothing left.
    fn ensure(&mut self, channels: usize, scratch: &mut Vec<f32>) -> bool {
        while self.buffer.len() - self.offset < channels {
            if self.ended {
                return false;
            }
            self.buffer.drain(..self.offset);
            self.offset = 0;
            scratch.clear();
            match self.outgoing.try_read(scratch) {
                Ok(ReadOutcome::Frames(_)) => {
                    let gain = self.gain;
                    if gain != 1.0 {
                        scratch
                            .iter_mut()
                            .for_each(|sample| *sample = (*sample * gain).clamp(-1.0, 1.0));
                    }
                    if self.resampler.process(scratch, &mut self.buffer).is_err() {
                        self.ended = true;
                    }
                }
                // The previous track is almost over: an error at its very end only ends it.
                // An exhausted source is never starved; if it were, it ends here too.
                Ok(ReadOutcome::End | ReadOutcome::Starved) | Err(_) => {
                    let _ = self.resampler.flush(&mut self.buffer);
                    self.ended = true;
                }
            }
        }
        true
    }

    fn done(&self) -> bool {
        self.position >= self.length
    }

    /// Mixes `incoming` (output spec) with the outgoing source into `out`: equal power over
    /// the fade, then the incoming samples alone. Clamped to full scale.
    fn mix(
        &mut self,
        incoming: &[f32],
        channels: usize,
        scratch: &mut Vec<f32>,
        out: &mut Vec<f32>,
    ) {
        let channels = channels.max(1);
        for frame in incoming.chunks(channels) {
            if self.done() {
                out.extend_from_slice(frame);
                continue;
            }
            let (fade_out, fade_in) = equal_power(self.position as f32 / self.length as f32);
            let old = self.ensure(channels, scratch);
            for (channel, &sample) in frame.iter().enumerate() {
                let previous = if old {
                    self.buffer[self.offset + channel]
                } else {
                    0.0
                };
                out.push((previous * fade_out + sample * fade_in).clamp(-1.0, 1.0));
            }
            if old {
                self.offset += channels;
            }
            self.position += 1;
        }
    }

    /// The incoming source ended inside the fade: the rest of the outgoing one fades out alone.
    fn play_out(&mut self, channels: usize, scratch: &mut Vec<f32>, out: &mut Vec<f32>) {
        let channels = channels.max(1);
        while !self.done() && self.ensure(channels, scratch) {
            let (fade_out, _) = equal_power(self.position as f32 / self.length as f32);
            let frame = &self.buffer[self.offset..self.offset + channels];
            out.extend(frame.iter().map(|sample| sample * fade_out));
            self.offset += channels;
            self.position += 1;
        }
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
        let spec = sink.spec();
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
            gain: Gain::fixed(1.0),
            next_gain: 1.0,
            next_fade_ms: 0,
            fade: None,
            read_ahead_ms: READ_AHEAD_MS,
            incoming: Vec::new(),
            outgoing: Vec::new(),
            starve_wait: STARVE_WAIT,
            meter: LevelMeter::new(spec.sample_rate, spec.channels),
            heard: LevelTimeline::new(),
        })
    }

    /// 0..=1, the loudness of what is being heard now (see [`LevelMeter`]). Measured as the
    /// samples go to the sink and delayed by what the sink still holds, so it follows the
    /// music, not the decoder.
    pub fn level(&self) -> f32 {
        if self.paused {
            return 0.0;
        }
        self.heard.at(Instant::now())
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
        self.current = Some(ReadAhead::spawn(source, self.read_ahead_ms));
        self.next = None;
        self.gain = Gain::fixed(1.0);
        self.next_gain = 1.0;
        self.next_fade_ms = 0;
        self.deferred_error = None;
        self.clear_buffers();
        self.base_ms = 0;
        Ok(())
    }

    /// Pre-opened source to continue with when the current one ends. With the same spec the
    /// sample boundary is continuous.
    /// The next source starts at gain 1.0; see [`Playback::set_next_gain`]. It is decoded
    /// ahead from now on, so its start is ready when the current one ends. Same as
    /// [`queue_next_with`](Self::queue_next_with) without a crossfade.
    pub fn queue_next(&mut self, source: Option<Box<dyn AudioSource>>) {
        self.queue_next_with(source, NextOptions::default());
    }

    /// Like [`queue_next`](Self::queue_next), with the next source's gain and crossfade.
    ///
    /// A crossfade starts once the current source has been decoded to its end and at most
    /// `crossfade_ms` of it is left; it lasts `min(crossfade_ms, what is left)`. Both sources
    /// are resampled separately and mixed with [`equal_power`], each with its own gain, and
    /// `pump` reports `Advanced` when the fade starts.
    pub fn queue_next_with(&mut self, source: Option<Box<dyn AudioSource>>, options: NextOptions) {
        self.next = source.map(|source| ReadAhead::spawn(source, self.read_ahead_ms));
        self.next_gain = sane_gain(options.gain);
        self.next_fade_ms = options.crossfade_ms;
    }

    /// Changes the crossfade into the queued next source (one that has not started yet).
    pub fn set_next_crossfade(&mut self, crossfade_ms: u32) {
        self.next_fade_ms = crossfade_ms;
    }

    /// How far ahead the current, the next and every later source are decoded. A crossfade
    /// needs more than its own length buffered when a source has been decoded to its end.
    pub fn set_read_ahead_ms(&mut self, ms: u32) {
        self.read_ahead_ms = ms;
        for reader in self.current.iter_mut().chain(self.next.iter_mut()) {
            reader.set_target_ms(ms);
        }
    }

    /// Stops playback and drops the current and next source (their decode threads stop on
    /// their own, without being waited for).
    pub fn stop(&mut self) {
        self.current = None;
        self.next = None;
        self.deferred_error = None;
        self.clear_buffers();
    }

    /// Drops the current and next source but keeps what was already handed to the output,
    /// so a previous track's tail plays out (after a pre-opened source failed).
    pub fn drop_sources(&mut self) {
        self.current = None;
        self.next = None;
        self.fade = None;
        self.deferred_error = None;
    }

    /// Like [`stop`](Self::stop), for quitting: waits up to `limit` in total for the decode
    /// threads to drop their sources, so FFmpeg children are killed before the process exits.
    /// A source stuck in a read longer than that is left to its thread.
    pub fn shutdown(&mut self, limit: Duration) {
        let deadline = Instant::now() + limit;
        let readers: Vec<ReadAhead> = self
            .current
            .take()
            .into_iter()
            .chain(self.next.take())
            .chain(self.fade.take().map(|fade| fade.outgoing))
            .collect();
        readers.iter().for_each(ReadAhead::stop);
        for reader in &readers {
            reader.wait_stopped(deadline);
        }
        self.stop();
    }

    /// Seeks the current source without waiting: the decode thread seeks while `pump` keeps
    /// returning (an FFmpeg restart can take seconds). A seek error arrives as a decode error
    /// from a later `pump`; the `Result` is kept for callers.
    ///
    /// Seeking back from the decoded end clears [`current_exhausted`](Self::current_exhausted);
    /// a next source already queued stays queued (it has not been read from) and takes over
    /// when the current one ends again.
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
    /// output volume, at once (for a source that is just starting). Every new source starts
    /// at 1.0.
    pub fn set_gain(&mut self, gain: f32) {
        self.gain = Gain::fixed(sane_gain(gain));
    }

    /// Like [`set_gain`](Self::set_gain) for a source already playing (a ReplayGain mode
    /// change): the factor moves there over 20 ms, as the output's volume does, so it does
    /// not click.
    pub fn ramp_gain(&mut self, gain: f32) {
        let gain = sane_gain(gain);
        match &self.current {
            Some(current) => self.gain.ramp_to(gain, current.spec().sample_rate),
            None => self.gain = Gain::fixed(gain),
        }
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

    /// Handles output events and fills the sink as far as it has room, then returns. It never
    /// waits for the device, the source or the decode thread: it takes what has been decoded
    /// ahead, and with nothing buffered (just after a start, seek or resume, or a source that
    /// cannot keep up) it returns `Playing` and the sink plays what it holds. Call it every few
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
        let filled = self.fill(&mut status);
        // What was just measured is the end of what the sink holds (samples still pending
        // were not measured yet): it is heard once the sink has played the rest.
        let spec = self.sink.spec();
        let in_sink = self.sink.queued() as f64 / spec.channels.max(1) as f64;
        let heard_at =
            Instant::now() + Duration::from_secs_f64(in_sink / spec.sample_rate.max(1) as f64);
        self.heard.record(heard_at, self.meter.level());
        match filled {
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
        let wait_until = Instant::now() + self.starve_wait;
        loop {
            if self.pending_offset < self.pending.len() {
                let from = self.pending_offset;
                self.pending_offset += self.sink.push(&self.pending[from..]);
                self.meter.push(&self.pending[from..self.pending_offset]);
                if self.pending_offset < self.pending.len() {
                    return Ok(*status);
                }
            }
            self.pending.clear();
            self.pending_offset = 0;

            if self.fade_due() {
                self.start_fade()?;
                *status = PumpStatus::Advanced;
                continue;
            }
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
            let channels = self.sink.spec().channels as usize;
            self.decoded.clear();
            match current.try_read(&mut self.decoded)? {
                ReadOutcome::Frames(frames) => {
                    self.frames_read += frames as u64;
                    self.gain
                        .apply(&mut self.decoded, current.spec().channels as usize);
                    match self.fade.as_mut() {
                        None => resampler.process(&self.decoded, &mut self.pending)?,
                        Some(fade) => {
                            self.incoming.clear();
                            resampler.process(&self.decoded, &mut self.incoming)?;
                            fade.mix(
                                &self.incoming,
                                channels,
                                &mut self.outgoing,
                                &mut self.pending,
                            );
                            if fade.done() {
                                self.fade = None;
                            }
                        }
                    }
                    continue;
                }
                ReadOutcome::Starved => {
                    let now = Instant::now();
                    // Production never waits here (`starve_wait` is zero).
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
            if let Some(mut fade) = self.fade.take() {
                // It ended inside the fade (a track shorter than the fade): its tail goes in
                // the mix and the rest of the previous one fades out alone.
                self.incoming.clear();
                resampler.flush(&mut self.incoming)?;
                fade.mix(
                    &self.incoming,
                    channels,
                    &mut self.outgoing,
                    &mut self.pending,
                );
                fade.play_out(channels, &mut self.outgoing, &mut self.pending);
            }
            match self.next.take() {
                Some(next) => {
                    // Same spec: the resampler keeps running, so the boundary is seamless.
                    if next.spec() != spec {
                        resampler.flush(&mut self.pending)?;
                        self.resampler = Some(Resampler::new(next.spec(), self.sink.spec())?);
                    }
                    self.current = Some(next);
                    self.gain = Gain::fixed(std::mem::replace(&mut self.next_gain, 1.0));
                    self.next_fade_ms = 0;
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

    /// A crossfade into the queued next source is due: the current one has been decoded to
    /// its end and no more than the fade's length of it is left.
    fn fade_due(&self) -> bool {
        if self.fade.is_some()
            || self.next_fade_ms == 0
            || self.next.is_none()
            || self.resampler.is_none()
        {
            return false;
        }
        let Some(current) = &self.current else {
            return false;
        };
        if !current.exhausted() {
            return false;
        }
        let left = current.buffered_frames();
        left > 0 && left <= frames_in(self.next_fade_ms, current.spec().sample_rate)
    }

    /// The queued next source becomes the current one and the current one plays out under
    /// it for `min(crossfade, what is left)`.
    fn start_fade(&mut self) -> anyhow::Result<()> {
        let output = self.sink.spec();
        let Some(next_spec) = self.next.as_ref().map(ReadAhead::spec) else {
            return Ok(());
        };
        // Each source has a resampler of its own; the outgoing one keeps its state.
        let incoming = Resampler::new(next_spec, output)?;
        let (Some(outgoing), Some(next), Some(resampler)) = (
            self.current.take(),
            self.next.take(),
            self.resampler.replace(incoming),
        ) else {
            return Ok(());
        };
        let left = outgoing.buffered_frames() as f64 * output.sample_rate as f64
            / outgoing.spec().sample_rate.max(1) as f64;
        let length = frames_in(self.next_fade_ms, output.sample_rate)
            .min(left as u64)
            .max(1);
        self.fade = Some(Fade {
            outgoing,
            resampler,
            gain: self.gain.target,
            buffer: Vec::new(),
            offset: 0,
            ended: false,
            position: 0,
            length,
        });
        self.current = Some(next);
        self.gain = Gain::fixed(std::mem::replace(&mut self.next_gain, 1.0));
        self.next_fade_ms = 0;
        self.base_ms = 0;
        self.frames_read = 0;
        Ok(())
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
        let spec = self.sink.spec();
        self.meter = LevelMeter::new(spec.sample_rate, spec.channels);
        self.heard.clear();
        self.sink.set_volume(self.volume);
        self.sink.set_paused(self.paused);
    }

    /// Continues the current source at the remembered position on the new output. The seek
    /// does not wait (the decode thread does it). A queued next source is kept: it has not
    /// been read from, so it still starts at its beginning when the current one ends.
    fn resume(&mut self) -> anyhow::Result<()> {
        self.pending.clear();
        self.pending_offset = 0;
        // A crossfade in progress is cut: the new output continues with the incoming track.
        self.fade = None;
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
        // A seek, a stop or a new source cuts a crossfade in progress.
        self.fade = None;
        self.pending.clear();
        self.pending_offset = 0;
        self.frames_read = 0;
        self.resume_ms = None;
        if let Some(resampler) = self.resampler.as_mut() {
            resampler.reset();
        }
        self.sink.clear();
        self.meter.reset();
        self.heard.clear();
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
        assert_eq!(pump_settled(&mut playback).unwrap(), PumpStatus::Playing);

        let first = factory.senders.lock().unwrap()[0].clone();
        first
            .send(OutputEvent::DeviceLost("unplugged".into()))
            .unwrap();
        assert_eq!(pump_settled(&mut playback).unwrap(), PumpStatus::Playing);

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
        assert_eq!(pump_settled(&mut playback).unwrap(), PumpStatus::Advanced);
        assert_eq!(pump_settled(&mut playback).unwrap(), PumpStatus::Finished);

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
            let status = pump_settled(&mut playback).unwrap();
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
        pump_settled(&mut playback).unwrap();
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
        assert_eq!(pump_settled(&mut playback).unwrap(), PumpStatus::NoDevice);
        assert_eq!(pump_settled(&mut playback).unwrap(), PumpStatus::NoDevice);
        assert_eq!(playback.frames_read, 0, "nothing was decoded");
        assert_eq!(
            *factory.opened.lock().unwrap(),
            vec![Some("Speakers".to_string()), None, None, None],
            "the requested device, then the default, retried"
        );

        factory.fail.store(false, Ordering::SeqCst);
        assert_eq!(pump_settled(&mut playback).unwrap(), PumpStatus::Playing);
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
            assert_eq!(pump_settled(&mut playback).unwrap(), PumpStatus::NoDevice);
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
            pump_settled(&mut playback).unwrap();
            play_queue(&factory);
        }
        pump_settled(&mut playback).unwrap(); // the sink holds unplayed audio when the device goes
        let position = playback.position_ms();
        assert!(position >= 290, "{position} ms played");

        factory.fail.store(true, Ordering::SeqCst);
        let lost = factory.senders.lock().unwrap()[0].clone();
        lost.send(OutputEvent::DeviceLost("unplugged".into()))
            .unwrap();
        assert_eq!(pump_settled(&mut playback).unwrap(), PumpStatus::NoDevice);
        assert_eq!(playback.position_ms(), position);

        factory.fail.store(false, Ordering::SeqCst);
        assert_eq!(pump_settled(&mut playback).unwrap(), PumpStatus::Playing);
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
        assert_eq!(pump_settled(&mut playback).unwrap(), PumpStatus::Advanced);
        let err = pump_settled(&mut playback).unwrap_err();
        assert!(err.to_string().contains("broken stream"), "{err:#}");
    }

    #[test]
    fn set_source_drops_the_queued_next_source() {
        let factory = FakeFactory::new(STEREO_44K, usize::MAX);
        let mut playback = Playback::new(factory.clone(), None).unwrap();
        playback.set_source(wav("gap_a.wav")).unwrap();
        playback.queue_next(Some(wav("gap_b.wav")));
        playback.set_source(wav("gap_a.wav")).unwrap();
        assert_eq!(pump_settled(&mut playback).unwrap(), PumpStatus::Finished);
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
        assert_eq!(pump_settled(&mut playback).unwrap(), PumpStatus::Advanced);
        assert_eq!(pump_settled(&mut playback).unwrap(), PumpStatus::Finished);

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
        assert_eq!(pump_settled(&mut playback).unwrap(), PumpStatus::Finished);
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
        pump_settled(&mut playback).unwrap();
        let played = factory.last_queue().lock().unwrap().clone();
        assert!(played == decode_all("gap_a.wav"));
    }

    #[test]
    fn set_device_reopens_and_keeps_the_position() {
        let factory = FakeFactory::new(STEREO_44K, 8_820);
        let mut playback = Playback::new(factory.clone(), None).unwrap();
        playback.set_source(wav("gap_a.wav")).unwrap();
        for _ in 0..3 {
            pump_settled(&mut playback).unwrap();
            play_queue(&factory);
        }
        pump_settled(&mut playback).unwrap();
        let position = playback.position_ms();
        assert!(position >= 290, "{position} ms played");

        playback.set_device(Some("USB DAC".into())).unwrap();
        assert_eq!(
            factory.opened.lock().unwrap().last().cloned(),
            Some(Some("USB DAC".to_string()))
        );
        assert!(playback.position_ms().abs_diff(position) <= 1);
        assert_eq!(pump_settled(&mut playback).unwrap(), PumpStatus::Playing);
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
        assert_eq!(pump_settled(&mut playback).unwrap(), PumpStatus::Playing);
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

    /// The old synchronous view of `pump`, for tests: pumps, and while the result is
    /// `Playing` only because the decode thread has not caught up (the sink still has room
    /// and the current source is not done), waits for it and pumps again.
    fn pump_settled(playback: &mut Playback) -> anyhow::Result<PumpStatus> {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let status = playback.pump()?;
            let sink_full = playback.pending_offset < playback.pending.len();
            if status != PumpStatus::Playing || sink_full {
                return Ok(status);
            }
            let Some(current) = playback.current.as_ref() else {
                return Ok(status);
            };
            assert!(
                Instant::now() < deadline,
                "the decode thread never caught up"
            );
            current.wait_ready(Duration::from_millis(100));
        }
    }

    /// Polls `done` every 2 ms; panics after `timeout`.
    #[track_caller]
    fn wait_until(timeout: Duration, mut done: impl FnMut() -> bool) {
        let deadline = Instant::now() + timeout;
        while !done() {
            assert!(Instant::now() < deadline, "not within {timeout:?}");
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    /// The one coarse wall-clock bound in the non-blocking tests: far below the 500 ms stall
    /// and the 1 s seek they run against, far above any scheduling hiccup.
    const COARSE_PUMP_BOUND: Duration = Duration::from_millis(150);

    #[test]
    fn slow_source_does_not_block_pump() {
        // A 100 ms device that plays 10 ms per step. The source stalls for 500 ms once, in
        // the first read past the initial read-ahead (3 s, 132 reads of 1024 frames), so
        // soon after playback starts, with about 3 s buffered.
        const STEP: usize = 441 * 2;
        let factory = FakeFactory::new(STEREO_44K, 4_410 * 2);
        let mut playback = Playback::new(factory.clone(), None).unwrap();
        let mut source = SlowSource::new(10);
        source.stall = Some((140_000, Duration::from_millis(500)));
        let stalling = Arc::clone(&source.stalling);
        let reads = Arc::clone(&source.reads);
        playback.set_source(Box::new(source)).unwrap();
        let queue = factory.last_queue();
        pump_settled(&mut playback).unwrap();
        wait_until(Duration::from_secs(10), || {
            reads.load(Ordering::SeqCst) >= 132
        });

        let deadline = Instant::now() + Duration::from_secs(20);
        let mut played: Vec<f32> = Vec::new();
        let mut slowest = Duration::ZERO;
        let mut pumps_during_stall = 0;
        let mut stall_seen = false;
        let mut under_runs = 0;
        loop {
            let stalled_before = stalling.load(Ordering::SeqCst);
            let pumped = Instant::now();
            assert_eq!(playback.pump().unwrap(), PumpStatus::Playing);
            slowest = slowest.max(pumped.elapsed());
            if stalled_before && stalling.load(Ordering::SeqCst) {
                // This pump started and returned while the source was stuck in its read.
                pumps_during_stall += 1;
            }
            stall_seen |= stalled_before;
            let mut queue = queue.lock().unwrap();
            let past_stall = stall_seen && !stalled_before;
            // Up to the end of the stall the buffer must carry the device; afterwards the
            // test only fast-forwards to the stall point.
            if queue.is_empty() && !past_stall {
                under_runs += 1;
            }
            let step = if past_stall { STEP * 10 } else { STEP };
            let take = step.min(queue.len());
            played.extend(queue.drain(..take));
            drop(queue);
            if stall_seen && !stalling.load(Ordering::SeqCst) && played.len() / 2 > 150_000 {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "{} frames played",
                played.len() / 2
            );
            // At most 100 steps (1 s of audio) fit in the 500 ms stall: well inside the buffer.
            std::thread::sleep(Duration::from_millis(5));
        }
        // A pump that waited for the read could not start and return inside the stall.
        assert!(
            pumps_during_stall >= 3,
            "only {pumps_during_stall} pumps ran during the 500 ms stall"
        );
        assert_eq!(under_runs, 0, "the device never ran dry");
        assert!(slowest < COARSE_PUMP_BOUND, "a pump took {slowest:?}");
        assert!(
            played
                .as_chunks::<2>()
                .0
                .iter()
                .enumerate()
                .all(|(index, frame)| frame_of(frame[0]) == index as u64),
            "one continuous run from the buffer, on past the stall point"
        );
    }

    #[test]
    fn seek_does_not_block_on_slow_source() {
        let factory = FakeFactory::new(STEREO_44K, 4_410 * 2);
        let mut playback = Playback::new(factory.clone(), None).unwrap();
        let mut source = SlowSource::new(10);
        source.seek_delay = Duration::from_secs(1);
        let in_seek = Arc::clone(&source.in_seek);
        let seeks_done = Arc::clone(&source.seeks_done);
        playback.set_source(Box::new(source)).unwrap();
        pump_settled(&mut playback).unwrap();
        play_queue(&factory);

        playback.seek(5_000).unwrap();
        assert_eq!(
            seeks_done.load(Ordering::SeqCst),
            0,
            "Playback::seek returned before the 1 s source seek finished"
        );
        assert_eq!(playback.position_ms(), 5_000);

        let queue = factory.last_queue();
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut pumps_during_seek = 0;
        let mut slowest = Duration::ZERO;
        while queue.lock().unwrap().is_empty() {
            let seeking_before = in_seek.load(Ordering::SeqCst);
            let pumped = Instant::now();
            assert_eq!(playback.pump().unwrap(), PumpStatus::Playing);
            slowest = slowest.max(pumped.elapsed());
            if seeking_before && in_seek.load(Ordering::SeqCst) {
                pumps_during_seek += 1;
            }
            assert!(Instant::now() < deadline, "nothing played after the seek");
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(
            pumps_during_seek >= 3,
            "only {pumps_during_seek} pumps ran during the 1 s seek"
        );
        assert!(slowest < COARSE_PUMP_BOUND, "a pump took {slowest:?}");
        let first = queue.lock().unwrap()[0];
        assert_eq!(frame_of(first), 220_500, "the first frames come from 5 s");
    }

    #[test]
    fn shutdown_waits_for_the_decode_threads_to_drop_their_sources() {
        let factory = FakeFactory::new(STEREO_44K, 8_820);
        let mut playback = Playback::new(factory.clone(), None).unwrap();
        let mut current = SlowSource::new(10);
        current.read_delay = Duration::from_millis(1);
        let mut next = SlowSource::new(10);
        next.read_delay = Duration::from_millis(1);
        let drops = [Arc::clone(&current.drops), Arc::clone(&next.drops)];
        playback.set_source(Box::new(current)).unwrap();
        playback.queue_next(Some(Box::new(next)));
        playback.shutdown(Duration::from_secs(5));
        assert!(
            drops.iter().all(|count| count.load(Ordering::SeqCst) == 1),
            "both sources were dropped before shutdown returned"
        );
        assert!(playback.is_drained());

        // A read stuck past the limit: shutdown gives up on it instead of hanging.
        let mut playback = Playback::new(factory.clone(), None).unwrap();
        let mut stuck = SlowSource::new(10);
        stuck.read_delay = Duration::from_secs(2);
        let in_read = Arc::clone(&stuck.in_read);
        playback.set_source(Box::new(stuck)).unwrap();
        wait_until(Duration::from_secs(10), || in_read.load(Ordering::SeqCst));
        playback.shutdown(Duration::from_millis(50));
        assert!(
            in_read.load(Ordering::SeqCst),
            "returned while the read still ran"
        );
    }

    #[test]
    fn set_device_during_a_gapless_tail_keeps_the_next_source() {
        let factory = FakeFactory::new(STEREO_44K, 8_820);
        let mut playback = Playback::new(factory.clone(), None).unwrap();
        playback.set_source(wav("gap_a.wav")).unwrap();
        playback.queue_next(Some(wav("gap_b.wav")));
        let deadline = Instant::now() + Duration::from_secs(5);
        while !playback.current_exhausted() {
            assert_eq!(pump_settled(&mut playback).unwrap(), PumpStatus::Playing);
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

    /// Endless constant samples.
    struct Constant(f32);

    impl AudioSource for Constant {
        fn spec(&self) -> StreamSpec {
            STEREO_44K
        }
        fn read(&mut self, out: &mut Vec<f32>) -> anyhow::Result<usize> {
            out.extend(std::iter::repeat_n(self.0, 1024 * 2));
            Ok(1024)
        }
        fn seek(&mut self, _ms: u64) -> anyhow::Result<()> {
            Ok(())
        }
        fn duration_ms(&self) -> Option<u64> {
            None
        }
    }

    #[test]
    fn level_follows_what_is_heard() {
        // A 100 ms sink that is never played: what it holds is still ahead of the listener.
        let factory = FakeFactory::new(STEREO_44K, 8_820);
        let mut playback = Playback::new(factory.clone(), None).unwrap();
        playback.set_source(Box::new(Constant(0.5))).unwrap();
        pump_settled(&mut playback).unwrap();
        assert_eq!(playback.level(), 0.0, "nothing heard yet");

        std::thread::sleep(Duration::from_millis(130));
        let expected = (20.0 * 0.5_f32.log10() + 50.0) / 50.0;
        assert!(
            (playback.level() - expected).abs() <= 0.02,
            "{} once the sink has played it",
            playback.level()
        );

        playback.set_paused(true);
        assert_eq!(playback.level(), 0.0, "paused");
        playback.set_paused(false);
        playback.seek(0).unwrap();
        assert_eq!(playback.level(), 0.0, "a seek starts over");
    }

    /// `ms` of a constant `value` at `spec`, read 1024 frames at a time.
    struct Dc {
        value: f32,
        spec: StreamSpec,
        frames_left: u64,
    }

    fn dc(value: f32, spec: StreamSpec, ms: u32) -> Box<dyn AudioSource> {
        Box::new(Dc {
            value,
            spec,
            frames_left: frames_in(ms, spec.sample_rate),
        })
    }

    impl AudioSource for Dc {
        fn spec(&self) -> StreamSpec {
            self.spec
        }
        fn read(&mut self, out: &mut Vec<f32>) -> anyhow::Result<usize> {
            let frames = self.frames_left.min(1024) as usize;
            out.extend(std::iter::repeat_n(
                self.value,
                frames * self.spec.channels as usize,
            ));
            self.frames_left -= frames as u64;
            Ok(frames)
        }
        fn seek(&mut self, _ms: u64) -> anyhow::Result<()> {
            Ok(())
        }
        fn duration_ms(&self) -> Option<u64> {
            None
        }
    }

    /// Frames the read-ahead hands over at a time: the granularity of a fade's start.
    const BLOCK_FRAMES: usize = 4096;

    /// Waits until the current source is decoded to its end without reading any of it, so a
    /// fade starts exactly where the rules say, whatever the decode thread's timing.
    fn wait_exhausted(playback: &Playback) {
        wait_until(Duration::from_secs(10), || playback.current_exhausted());
    }

    fn fade_options(crossfade_ms: u32) -> NextOptions {
        NextOptions {
            gain: 1.0,
            crossfade_ms,
        }
    }

    /// Left channel of interleaved stereo samples.
    fn left(samples: &[f32]) -> Vec<f32> {
        samples.iter().step_by(2).copied().collect()
    }

    #[test]
    fn crossfade_overlaps_tracks() {
        let factory = FakeFactory::new(STEREO_48K, usize::MAX);
        let mut playback = Playback::new(factory.clone(), None).unwrap();
        playback.set_source(dc(1.0, STEREO_48K, 3_000)).unwrap();
        playback.queue_next_with(Some(dc(0.5, STEREO_48K, 3_000)), fade_options(1_000));
        wait_exhausted(&playback);
        let (statuses, played) = play_until(&mut playback, &factory, |status| {
            status == PumpStatus::Finished
        });
        assert!(statuses.contains(&PumpStatus::Advanced), "{statuses:?}");

        let frames = left(&played);
        assert!(
            frames.len().abs_diff(240_000) <= BLOCK_FRAMES,
            "{} frames for 3 s + 3 s with a 1 s overlap",
            frames.len()
        );
        // All of the second track is played; the fade is the start of it.
        let fade_start = frames.len() - 144_000;
        let fade_end = 144_000;
        assert!(fade_end - fade_start > 48_000 - BLOCK_FRAMES);
        assert!(frames[..fade_start].iter().all(|s| *s == 1.0), "a alone");
        let window = &frames[fade_start..fade_end];
        assert!(
            window.iter().all(|s| *s <= 1.0 && *s >= 0.5 - 1e-6),
            "no sample above full scale"
        );
        assert!(
            window.windows(2).all(|w| w[1] <= w[0] + 1e-6),
            "the window only moves from a toward b"
        );
        assert!((window[0] - 1.0).abs() < 1e-3, "{}", window[0]);
        let last = window[window.len() - 1];
        assert!((last - 0.5).abs() < 0.01, "{last}");
        assert!(frames[fade_end..].iter().all(|s| *s == 0.5), "b alone");
    }

    #[test]
    fn short_track_shortens_the_fade() {
        let factory = FakeFactory::new(STEREO_48K, usize::MAX);
        let mut playback = Playback::new(factory.clone(), None).unwrap();
        playback.set_source(dc(1.0, STEREO_48K, 500)).unwrap();
        playback.queue_next_with(Some(dc(0.5, STEREO_48K, 3_000)), fade_options(4_000));
        wait_exhausted(&playback);
        let (statuses, played) = play_until(&mut playback, &factory, |status| {
            status == PumpStatus::Finished
        });
        assert!(statuses.contains(&PumpStatus::Advanced), "{statuses:?}");
        let frames = left(&played);
        assert_eq!(
            frames.len(),
            144_000,
            "3.5 s of tracks, 0.5 s of it overlapped"
        );
        assert!(frames.iter().all(|s| *s <= 1.0 && *s >= 0.5 - 1e-6));
        assert!(
            frames[24_000..].iter().all(|s| *s == 0.5),
            "the fade took 0.5 s"
        );
    }

    #[test]
    fn next_shorter_than_the_fade_lets_the_previous_one_fade_out() {
        let factory = FakeFactory::new(STEREO_48K, usize::MAX);
        let mut playback = Playback::new(factory.clone(), None).unwrap();
        playback.set_source(dc(1.0, STEREO_48K, 2_000)).unwrap();
        playback.queue_next_with(Some(dc(0.5, STEREO_48K, 500)), fade_options(2_000));
        wait_exhausted(&playback);
        let (_, played) = play_until(&mut playback, &factory, |status| {
            status == PumpStatus::Finished
        });
        let frames = left(&played);
        assert_eq!(frames.len(), 96_000, "the previous track's length");
        assert!(frames.iter().all(|s| (0.0..=1.0).contains(s)));
        assert!(frames[95_999] < 0.01, "faded out: {}", frames[95_999]);
    }

    #[test]
    fn crossfade_zero_is_gapless() {
        let factory = FakeFactory::new(STEREO_44K, usize::MAX);
        let mut playback = Playback::new(factory.clone(), None).unwrap();
        playback.set_source(wav("gap_a.wav")).unwrap();
        playback.queue_next_with(Some(wav("gap_b.wav")), fade_options(0));
        assert_eq!(pump_settled(&mut playback).unwrap(), PumpStatus::Advanced);
        assert_eq!(pump_settled(&mut playback).unwrap(), PumpStatus::Finished);
        let expected: Vec<f32> = [decode_all("gap_a.wav"), decode_all("gap_b.wav")].concat();
        let played = factory.last_queue().lock().unwrap().clone();
        assert_eq!(played.len(), expected.len());
        assert!(played == expected, "the boundary is sample-continuous");
    }

    #[test]
    fn crossfade_keeps_each_sources_gain() {
        let factory = FakeFactory::new(STEREO_48K, usize::MAX);
        let mut playback = Playback::new(factory.clone(), None).unwrap();
        playback.set_source(dc(1.0, STEREO_48K, 2_000)).unwrap();
        playback.set_gain(0.5);
        playback.queue_next_with(
            Some(dc(1.0, STEREO_48K, 2_000)),
            NextOptions {
                gain: 0.25,
                crossfade_ms: 500,
            },
        );
        wait_exhausted(&playback);
        let (_, played) = play_until(&mut playback, &factory, |status| {
            status == PumpStatus::Finished
        });
        let frames = left(&played);
        let fade_start = frames.len() - 96_000;
        let length = 96_000 - fade_start;
        assert!(frames[..fade_start].iter().all(|s| *s == 0.5));
        for (position, sample) in frames[fade_start..96_000].iter().enumerate() {
            let (a, b) = equal_power(position as f32 / length as f32);
            assert!(
                (sample - (0.5 * a + 0.25 * b)).abs() < 1e-5,
                "at {position}"
            );
        }
        assert!(frames[96_000..].iter().all(|s| *s == 0.25));
    }

    #[test]
    fn crossfade_resamples_each_source_to_the_output() {
        let factory = FakeFactory::new(STEREO_48K, usize::MAX);
        let mut playback = Playback::new(factory.clone(), None).unwrap();
        playback.set_source(dc(0.5, STEREO_48K, 3_000)).unwrap();
        playback.queue_next_with(Some(dc(0.5, STEREO_44K, 3_000)), fade_options(1_000));
        wait_exhausted(&playback);
        let (_, played) = play_until(&mut playback, &factory, |status| {
            status == PumpStatus::Finished
        });
        let frames = left(&played);
        assert!(
            frames.len().abs_diff(240_000) <= BLOCK_FRAMES + 64,
            "{} frames: the 44.1 kHz track at 48 kHz",
            frames.len()
        );
        // Past the fade (and the resampler's edges), the second track at its own level.
        let middle = frames[150_000..200_000].iter();
        assert!(middle.clone().all(|s| (s - 0.5).abs() < 0.01));
    }

    #[test]
    fn level_follows_the_mix_through_a_crossfade() {
        // Two uncorrelated tracks keep their loudness in an equal-power fade; two copies of
        // the same DC add up to +3 dB in its middle, which only the mixed output shows.
        let factory = FakeFactory::new(STEREO_48K, 4_800);
        let mut playback = Playback::new(factory.clone(), None).unwrap();
        playback.set_source(dc(0.5, STEREO_48K, 3_000)).unwrap();
        playback.queue_next_with(Some(dc(0.5, STEREO_48K, 3_000)), fade_options(1_000));
        wait_exhausted(&playback);
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut loudest: f32 = 0.0;
        while playback.pump().unwrap() != PumpStatus::Finished {
            loudest = loudest.max(playback.meter.level());
            play_queue(&factory);
            assert!(Instant::now() < deadline, "never finished");
            std::thread::sleep(Duration::from_millis(1));
        }
        let mixed = mix::level_of_rms(0.5 * std::f32::consts::SQRT_2);
        assert!((loudest - mixed).abs() <= 0.02, "{loudest} for {mixed}");
        assert!(loudest > mix::level_of_rms(0.5) + 0.04);
    }

    #[test]
    fn a_replay_gain_change_ramps_over_20_ms() {
        let factory = FakeFactory::new(STEREO_48K, 48_000);
        let mut playback = Playback::new(factory.clone(), None).unwrap();
        playback.set_source(dc(0.5, STEREO_48K, 2_000)).unwrap();
        wait_exhausted(&playback);
        pump_settled(&mut playback).unwrap();
        let mut played = factory
            .last_queue()
            .lock()
            .unwrap()
            .drain(..)
            .collect::<Vec<_>>();
        playback.ramp_gain(0.5);
        let (_, rest) = play_until(&mut playback, &factory, |status| {
            status == PumpStatus::Finished
        });
        played.extend(rest);
        let frames = left(&played);
        assert_eq!(frames.len(), 96_000);
        let start = frames
            .iter()
            .position(|s| *s < 0.5)
            .expect("the gain changed");
        let ramp = frames[start..]
            .iter()
            .take_while(|s| **s > 0.25 + 1e-6)
            .count();
        // 20 ms at 48 kHz is 960 frames.
        assert!((955..=961).contains(&ramp), "{ramp} frames");
        assert!(frames[start - 1..].windows(2).all(|w| w[1] <= w[0]));
        assert!(frames[start + ramp..]
            .iter()
            .all(|s| (s - 0.25).abs() < 1e-6));
    }

    #[test]
    fn a_seek_during_a_fade_cuts_the_previous_track() {
        let factory = FakeFactory::new(STEREO_48K, 4_800);
        let mut playback = Playback::new(factory.clone(), None).unwrap();
        playback.set_source(dc(1.0, STEREO_48K, 1_000)).unwrap();
        playback.queue_next_with(Some(dc(0.5, STEREO_48K, 3_000)), fade_options(1_000));
        wait_exhausted(&playback);
        assert_eq!(playback.pump().unwrap(), PumpStatus::Advanced);
        assert!(playback.fade.is_some());
        playback.seek(0).unwrap();
        assert!(playback.fade.is_none());
        play_queue(&factory);
        let (_, played) = play_until(&mut playback, &factory, |status| {
            status == PumpStatus::Finished
        });
        assert!(played.iter().all(|s| *s == 0.5), "only the new track");
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
