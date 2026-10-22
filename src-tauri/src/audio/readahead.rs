//! Decoding ahead of playback, on a thread of its own per source.
//!
//! The decode thread keeps up to `target_ms` of audio in a queue of 4096-frame chunks. The
//! engine thread only takes chunks out of that queue, so a slow read (a file on a NAS, FFmpeg
//! restarting after a seek) never blocks it: it sees [`ReadOutcome::Starved`] instead. A seek
//! bumps an epoch and drops the queue; the thread then seeks the source, and whatever it
//! decoded for an older epoch is thrown away.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::anyhow;

use super::source::{guarded, AudioSource, StreamSpec};

/// How far ahead of playback a source is decoded (inside the 2 to 4 s window).
///
/// Memory: the queue holds at most the target plus one chunk, as interleaved f32, so about
/// `rate * channels * 4 bytes * 3 s` per `ReadAhead`: 1.06 MB at 44.1 kHz stereo, 1.15 MB
/// at 48 kHz, 4.6 MB at 192 kHz (24/192 files decode to f32 like any other). Playback holds
/// two (current and next).
pub const READ_AHEAD_MS: u32 = 3_000;
/// Frames per chunk the decode thread hands over.
const CHUNK_FRAMES: usize = 4096;
/// A source call (read or seek) running longer than this is slow I/O (a NAS, an FFmpeg
/// restart): [`ReadAhead::wait_ready`] stops waiting for it. Local reads take microseconds;
/// the margin keeps a read that was merely preempted on a busy machine from counting.
pub const SLOW_CALL: Duration = Duration::from_millis(50);

/// What a [`ReadAhead::try_read`] call found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReadOutcome {
    /// This many frames were appended.
    Frames(usize),
    /// Nothing decoded yet; the decode thread is still working on it.
    Starved,
    /// The source ended; everything decoded has been read.
    End,
}

enum Item {
    Frames(Vec<f32>),
    End,
    Error(anyhow::Error),
}

/// A piece of decoder output, tagged with the epoch (seek) it was decoded for.
struct Chunk {
    epoch: u64,
    item: Item,
}

struct State {
    epoch: u64,
    /// A seek the decode thread has not done yet.
    seek: Option<u64>,
    chunks: VecDeque<Chunk>,
    buffered_frames: u64,
    target_frames: u64,
    /// The decoder stopped (end of stream or an error) in this epoch.
    exhausted: bool,
    /// It stopped on an error (in this epoch), not at the end of the stream.
    failed: bool,
    /// The [`ReadAhead`] is gone; the thread exits.
    stop: bool,
    /// The decode thread is not running (it failed to start or stopped after a panic).
    gone: bool,
}

impl State {
    /// With no decode thread left, a read must not wait for one: report an error once.
    fn fail_if_gone(&mut self) {
        if self.gone && !self.exhausted && self.chunks.is_empty() {
            self.chunks.push_back(Chunk {
                epoch: self.epoch,
                item: Item::Error(anyhow!("the decode thread stopped")),
            });
            self.exhausted = true;
            self.failed = true;
        }
    }
}

struct Shared {
    state: Mutex<State>,
    /// Wakes the decode thread (room in the queue, a seek, a new target, stop).
    wake: Condvar,
    /// Wakes [`ReadAhead::wait_ready`] when something was queued.
    queued: Condvar,
    /// The state's epoch, for the decode thread to notice a seek between two reads.
    epoch: AtomicU64,
    /// When the running source call started, in microseconds after `base` plus one; 0 = none.
    call_started: AtomicU64,
    base: Instant,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn sync(&self, state: &State) {
        self.epoch.store(state.epoch, Ordering::Release);
    }

    /// Runs a source call, recording when it started.
    fn call<T>(&self, run: impl FnOnce() -> T) -> T {
        let started = self.base.elapsed().as_micros() as u64 + 1;
        self.call_started.store(started, Ordering::Release);
        let result = run();
        self.call_started.store(0, Ordering::Release);
        result
    }

    /// How long the running source call has taken so far.
    #[cfg(test)]
    fn call_age(&self) -> Option<Duration> {
        let started = self.call_started.load(Ordering::Acquire);
        (started != 0).then(|| {
            self.base
                .elapsed()
                .saturating_sub(Duration::from_micros(started - 1))
        })
    }

    /// Queues what the thread decoded for `epoch` (dropped when a seek came in since), and
    /// `end` after it. False once the thread should stop.
    fn deliver(&self, epoch: u64, samples: Vec<f32>, channels: usize, end: Option<Item>) -> bool {
        let mut state = self.lock();
        if state.stop {
            return false;
        }
        if state.epoch == epoch {
            if !samples.is_empty() {
                state.buffered_frames += (samples.len() / channels) as u64;
                state.chunks.push_back(Chunk {
                    epoch,
                    item: Item::Frames(samples),
                });
            }
            if let Some(item) = end {
                state.failed = matches!(item, Item::Error(_));
                state.chunks.push_back(Chunk { epoch, item });
                state.exhausted = true;
            }
            drop(state);
            self.queued.notify_all();
        }
        true
    }
}

/// Frames in `ms`; at least one, so a zero target still decodes a chunk at a time.
fn frames_for(ms: u32, spec: StreamSpec) -> u64 {
    (ms as u64 * spec.sample_rate as u64 / 1000).max(1)
}

/// A source decoded ahead on its own thread. Dropping it stops the thread, which drops the
/// source there (an FFmpeg child is killed on the decode thread, not the caller's).
pub struct ReadAhead {
    shared: Arc<Shared>,
    spec: StreamSpec,
    duration_ms: Option<u64>,
    channels: usize,
}

impl ReadAhead {
    /// Starts decoding `source` on a new thread, up to `target_ms` ahead.
    pub fn spawn(source: Box<dyn AudioSource>, target_ms: u32) -> ReadAhead {
        let spec = source.spec();
        let duration_ms = source.duration_ms();
        let channels = spec.channels.max(1) as usize;
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                epoch: 0,
                seek: None,
                chunks: VecDeque::new(),
                buffered_frames: 0,
                target_frames: frames_for(target_ms, spec),
                exhausted: false,
                failed: false,
                stop: false,
                gone: false,
            }),
            wake: Condvar::new(),
            queued: Condvar::new(),
            epoch: AtomicU64::new(0),
            call_started: AtomicU64::new(0),
            base: Instant::now(),
        });
        let worker = Arc::clone(&shared);
        let spawned = thread::Builder::new()
            .name("prism-readahead".into())
            .spawn(move || {
                // Source calls are guarded one by one; this catches a panic anywhere else.
                if let Err(err) = guarded("the decode thread", || {
                    decode(source, &worker, channels);
                    Ok(())
                }) {
                    let epoch = worker.epoch.load(Ordering::Acquire);
                    worker.deliver(epoch, Vec::new(), channels, Some(Item::Error(err)));
                }
                // The source is dropped by now (an FFmpeg child killed and reaped).
                worker.lock().gone = true;
                worker.queued.notify_all();
            });
        if let Err(err) = spawned {
            shared.deliver(
                0,
                Vec::new(),
                channels,
                Some(Item::Error(anyhow!(
                    "starting the decode thread failed: {err}"
                ))),
            );
            shared.lock().gone = true;
        }
        ReadAhead {
            shared,
            spec,
            duration_ms,
            channels,
        }
    }

    pub fn spec(&self) -> StreamSpec {
        self.spec
    }

    pub fn duration_ms(&self) -> Option<u64> {
        self.duration_ms
    }

    /// Appends the next decoded chunk to `out` without waiting. A decode error is returned
    /// after every frame decoded before it; after the error the source reads as ended.
    pub fn try_read(&mut self, out: &mut Vec<f32>) -> anyhow::Result<ReadOutcome> {
        let mut state = self.shared.lock();
        while let Some(chunk) = state.chunks.pop_front() {
            if chunk.epoch != state.epoch {
                continue;
            }
            let epoch = chunk.epoch;
            let outcome = match chunk.item {
                Item::Frames(samples) => {
                    let frames = samples.len() / self.channels;
                    state.buffered_frames = state.buffered_frames.saturating_sub(frames as u64);
                    out.extend_from_slice(&samples);
                    Ok(ReadOutcome::Frames(frames))
                }
                Item::End => {
                    // The end stays queued: every later read reports it too.
                    state.chunks.push_front(Chunk {
                        epoch,
                        item: Item::End,
                    });
                    Ok(ReadOutcome::End)
                }
                Item::Error(err) => {
                    state.chunks.push_front(Chunk {
                        epoch,
                        item: Item::End,
                    });
                    Err(err)
                }
            };
            drop(state);
            self.shared.wake.notify_one();
            return outcome;
        }
        Ok(ReadOutcome::Starved)
    }

    /// Waits up to `limit` for something to read while the decode thread is making progress,
    /// but gives up on a source call that has run longer than [`SLOW_CALL`]: that is a slow
    /// source (a NAS, an FFmpeg restart), not a busy thread. A chunk wakes it at once; a
    /// timed check can overshoot by a timer tick (about 15 ms on Windows). True when a
    /// `try_read` will not be starved. Tests only: the engine never waits (its `pump` returns
    /// when the decode thread has nothing ready).
    #[cfg(test)]
    pub(crate) fn wait_ready(&self, limit: Duration) -> bool {
        let deadline = Instant::now() + limit;
        let mut state = self.shared.lock();
        loop {
            if !state.chunks.is_empty() {
                return true;
            }
            let now = Instant::now();
            if state.stop || state.gone || now >= deadline {
                return false;
            }
            let age = self.shared.call_age().unwrap_or_default();
            if age >= SLOW_CALL {
                return false;
            }
            let check = (deadline - now).min(SLOW_CALL - age);
            state = self
                .shared
                .queued
                .wait_timeout(state, check)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }

    /// Moves to `ms` without waiting: drops the buffered audio, and the decode thread seeks
    /// the source. Reads are starved until it has decoded from the new position; a seek
    /// error arrives through [`try_read`](Self::try_read).
    ///
    /// Seeking back from an exhausted source clears [`exhausted`](Self::exhausted): the
    /// decoder runs again from `ms` and reports the end once more when it gets there.
    pub fn seek(&mut self, ms: u64) {
        let mut state = self.shared.lock();
        state.epoch += 1;
        state.seek = Some(ms);
        state.chunks.clear();
        state.buffered_frames = 0;
        state.exhausted = false;
        state.failed = false;
        state.fail_if_gone();
        self.shared.sync(&state);
        drop(state);
        self.shared.wake.notify_all();
    }

    /// The decoder reached the end of the source (or failed): nothing more will be decoded
    /// until a seek, and what is left to play is [`buffered_frames`](Self::buffered_frames).
    pub fn exhausted(&self) -> bool {
        self.shared.lock().exhausted
    }

    /// The decoder stopped on an error (reported through [`try_read`](Self::try_read) after
    /// the frames before it), not at the end of the source. Cleared by a seek.
    pub fn failed(&self) -> bool {
        self.shared.lock().failed
    }

    /// Decoded frames waiting to be read.
    pub fn buffered_frames(&self) -> u64 {
        self.shared.lock().buffered_frames
    }

    /// How far ahead to decode from now on.
    pub fn set_target_ms(&mut self, ms: u32) {
        self.shared.lock().target_frames = frames_for(ms, self.spec);
        self.shared.wake.notify_all();
    }

    /// Tells the decode thread to stop, without waiting. A chunk in progress is abandoned
    /// after the current source call; the thread then drops the source and exits.
    pub fn stop(&self) {
        let mut state = self.shared.lock();
        state.stop = true;
        state.epoch += 1;
        state.chunks.clear();
        self.shared.sync(&state);
        drop(state);
        self.shared.wake.notify_all();
    }

    /// After [`stop`](Self::stop): waits until `deadline` for the decode thread to have
    /// dropped the source and exited (for a clean shutdown: an FFmpeg child is killed and
    /// reaped there). False when a source call is still running at the deadline.
    pub fn wait_stopped(&self, deadline: Instant) -> bool {
        let mut state = self.shared.lock();
        loop {
            if state.gone {
                return true;
            }
            let now = Instant::now();
            if now >= deadline {
                return false;
            }
            state = self
                .shared
                .queued
                .wait_timeout(state, deadline - now)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }
}

impl Drop for ReadAhead {
    /// Stops the thread without waiting; the source is dropped on the decode thread.
    fn drop(&mut self) {
        self.stop();
    }
}

/// The decode thread: fills the queue up to the target, seeks when asked, and returns (which
/// drops the source) once the [`ReadAhead`] is gone.
fn decode(mut source: Box<dyn AudioSource>, shared: &Shared, channels: usize) {
    loop {
        let (epoch, seek) = {
            let mut state = shared.lock();
            loop {
                if state.stop {
                    return;
                }
                let wanted = !state.exhausted && state.buffered_frames < state.target_frames;
                if state.seek.is_some() || wanted {
                    break;
                }
                state = shared
                    .wake
                    .wait(state)
                    .unwrap_or_else(PoisonError::into_inner);
            }
            (state.epoch, state.seek.take())
        };

        if let Some(ms) = seek {
            if let Err(err) = shared.call(|| guarded("seek", || source.seek(ms))) {
                if !shared.deliver(epoch, Vec::new(), channels, Some(Item::Error(err))) {
                    return;
                }
            }
            continue;
        }

        let mut samples = Vec::with_capacity(CHUNK_FRAMES * channels);
        let mut end = None;
        while samples.len() / channels < CHUNK_FRAMES
            && shared.epoch.load(Ordering::Acquire) == epoch
        {
            match shared.call(|| guarded("decoding", || source.read(&mut samples))) {
                Ok(0) => end = Some(Item::End),
                Ok(_) => continue,
                Err(err) => end = Some(Item::Error(err)),
            }
            break;
        }
        if !shared.deliver(epoch, samples, channels, end) {
            return;
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::Instant;

    pub(crate) const SPEC: StreamSpec = StreamSpec {
        sample_rate: 44_100,
        channels: 2,
    };
    /// Frames one `read` of a [`SlowSource`] returns.
    const READ_FRAMES: u64 = 1024;

    /// The sample value a [`SlowSource`] writes for `frame`: exact in f32 below 2^24 frames.
    pub(crate) fn sample(frame: u64) -> f32 {
        frame as f32 / 16_777_216.0
    }

    /// The frame a [`SlowSource`] sample came from.
    pub(crate) fn frame_of(sample: f32) -> u64 {
        (sample as f64 * 16_777_216.0).round() as u64
    }

    /// A stereo 44.1 kHz source whose samples encode their frame index. Every `read` and
    /// `seek` sleeps a configurable time, like a file on a slow NAS.
    pub(crate) struct SlowSource {
        pub position: u64,
        pub total: u64,
        pub read_delay: Duration,
        pub seek_delay: Duration,
        /// Sleeps this long once, in the first read that starts at or after the frame.
        pub stall: Option<(u64, Duration)>,
        /// The first read that starts at or after this frame fails.
        pub fail_at: Option<u64>,
        /// Counts drops of the source.
        pub drops: Arc<AtomicUsize>,
        /// Counts `read` calls as they start.
        pub reads: Arc<AtomicUsize>,
        /// Set for the whole of a `read` / `seek` / the one-off stall, so a test can see
        /// what was still running when a call returned.
        pub in_read: Arc<AtomicBool>,
        pub in_seek: Arc<AtomicBool>,
        pub stalling: Arc<AtomicBool>,
        /// Counts finished `seek` calls.
        pub seeks_done: Arc<AtomicUsize>,
    }

    impl SlowSource {
        pub(crate) fn new(seconds: u64) -> SlowSource {
            SlowSource {
                position: 0,
                total: seconds * SPEC.sample_rate as u64,
                read_delay: Duration::ZERO,
                seek_delay: Duration::ZERO,
                stall: None,
                fail_at: None,
                drops: Arc::new(AtomicUsize::new(0)),
                reads: Arc::new(AtomicUsize::new(0)),
                in_read: Arc::new(AtomicBool::new(false)),
                in_seek: Arc::new(AtomicBool::new(false)),
                stalling: Arc::new(AtomicBool::new(false)),
                seeks_done: Arc::new(AtomicUsize::new(0)),
            }
        }

        fn read_inner(&mut self, out: &mut Vec<f32>) -> anyhow::Result<usize> {
            if self.fail_at.is_some_and(|frame| self.position >= frame) {
                anyhow::bail!("read failed at frame {}", self.position);
            }
            if let Some((frame, delay)) = self.stall {
                if self.position >= frame {
                    self.stall = None;
                    self.stalling.store(true, Ordering::SeqCst);
                    std::thread::sleep(delay);
                    self.stalling.store(false, Ordering::SeqCst);
                }
            }
            std::thread::sleep(self.read_delay);
            let frames = READ_FRAMES.min(self.total - self.position.min(self.total));
            for frame in self.position..self.position + frames {
                out.extend_from_slice(&[sample(frame), sample(frame)]);
            }
            self.position += frames;
            Ok(frames as usize)
        }
    }

    impl AudioSource for SlowSource {
        fn spec(&self) -> StreamSpec {
            SPEC
        }
        fn read(&mut self, out: &mut Vec<f32>) -> anyhow::Result<usize> {
            self.reads.fetch_add(1, Ordering::SeqCst);
            self.in_read.store(true, Ordering::SeqCst);
            let result = self.read_inner(out);
            self.in_read.store(false, Ordering::SeqCst);
            result
        }
        fn seek(&mut self, ms: u64) -> anyhow::Result<()> {
            self.in_seek.store(true, Ordering::SeqCst);
            std::thread::sleep(self.seek_delay);
            self.position = ms * SPEC.sample_rate as u64 / 1000;
            self.seeks_done.fetch_add(1, Ordering::SeqCst);
            self.in_seek.store(false, Ordering::SeqCst);
            Ok(())
        }
        fn duration_ms(&self) -> Option<u64> {
            Some(self.total * 1000 / SPEC.sample_rate as u64)
        }
    }

    impl Drop for SlowSource {
        fn drop(&mut self) {
            self.drops.fetch_add(1, Ordering::SeqCst);
        }
    }

    /// `try_read` until it is not starved, or panics after `timeout`.
    #[track_caller]
    fn read_within(
        reader: &mut ReadAhead,
        out: &mut Vec<f32>,
        timeout: Duration,
    ) -> anyhow::Result<ReadOutcome> {
        let deadline = Instant::now() + timeout;
        loop {
            match reader.try_read(out)? {
                ReadOutcome::Starved => {
                    assert!(Instant::now() < deadline, "starved for {timeout:?}");
                    reader.wait_ready(Duration::from_millis(5));
                }
                outcome => return Ok(outcome),
            }
        }
    }

    #[track_caller]
    fn wait_until(timeout: Duration, mut done: impl FnMut() -> bool) {
        let deadline = Instant::now() + timeout;
        while !done() {
            assert!(Instant::now() < deadline, "not within {timeout:?}");
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    const CHUNK: u64 = 4096;
    /// How long a test waits for something that normally takes milliseconds: only a
    /// liveness limit, generous for a machine where every core is busy (thread starts of
    /// several hundred milliseconds were measured under the full suite).
    const LIVENESS: Duration = Duration::from_secs(10);

    #[test]
    fn buffers_up_to_target() {
        let reader = ReadAhead::spawn(Box::new(SlowSource::new(10)), READ_AHEAD_MS);
        let target = READ_AHEAD_MS as u64 * SPEC.sample_rate as u64 / 1000;
        // Normally full within milliseconds; the bound allows for a busy machine.
        wait_until(LIVENESS, || reader.buffered_frames() >= target);
        std::thread::sleep(Duration::from_millis(200));
        let buffered = reader.buffered_frames();
        assert!(
            buffered >= target && buffered <= target + CHUNK,
            "{buffered} frames buffered for a target of {target}"
        );
        assert!(!reader.exhausted(), "it stopped at the target, not the end");
    }

    #[test]
    fn reading_refills_and_a_new_target_applies() {
        let mut reader = ReadAhead::spawn(Box::new(SlowSource::new(10)), 500);
        let target = 500 * SPEC.sample_rate as u64 / 1000;
        wait_until(LIVENESS, || reader.buffered_frames() >= target);
        let mut out = Vec::new();
        let outcome = read_within(&mut reader, &mut out, LIVENESS).unwrap();
        assert_eq!(outcome, ReadOutcome::Frames(CHUNK as usize));
        assert_eq!(frame_of(out[0]), 0);
        wait_until(LIVENESS, || reader.buffered_frames() >= target);

        reader.set_target_ms(1_000);
        let bigger = 1_000 * SPEC.sample_rate as u64 / 1000;
        wait_until(LIVENESS, || reader.buffered_frames() >= bigger);
    }

    #[test]
    fn frames_arrive_in_order_then_end_and_exhausted() {
        let mut source = SlowSource::new(1);
        source.total = 10_000;
        let mut reader = ReadAhead::spawn(Box::new(source), READ_AHEAD_MS);
        assert_eq!(reader.spec(), SPEC);
        assert_eq!(reader.duration_ms(), Some(226));
        wait_until(LIVENESS, || reader.exhausted());
        assert_eq!(
            reader.buffered_frames(),
            10_000,
            "all of it is still buffered"
        );
        let mut out = Vec::new();
        loop {
            match read_within(&mut reader, &mut out, LIVENESS).unwrap() {
                ReadOutcome::Frames(_) => {}
                ReadOutcome::End => break,
                ReadOutcome::Starved => unreachable!(),
            }
        }
        assert_eq!(out.len(), 20_000);
        assert!(out
            .as_chunks::<2>()
            .0
            .iter()
            .enumerate()
            .all(|(index, frame)| frame_of(frame[0]) == index as u64));
        assert_eq!(
            reader.try_read(&mut out).unwrap(),
            ReadOutcome::End,
            "stays ended"
        );
    }

    #[test]
    fn error_after_frames_is_delivered_last() {
        let mut source = SlowSource::new(10);
        source.fail_at = Some(10_000);
        let mut reader = ReadAhead::spawn(Box::new(source), READ_AHEAD_MS);
        let mut out = Vec::new();
        let err = loop {
            match read_within(&mut reader, &mut out, LIVENESS) {
                Ok(ReadOutcome::Frames(_)) => {}
                Ok(outcome) => panic!("{outcome:?} before the error"),
                Err(err) => break err,
            }
        };
        assert!(err.to_string().contains("read failed"), "{err:#}");
        // Reads start at multiples of 1024 frames; the one at 10 240 fails.
        assert_eq!(
            out.len() / 2,
            10_240,
            "every frame decoded before the error"
        );
        assert_eq!(frame_of(out[out.len() - 1]), 10_239);
        assert!(reader.exhausted());
        assert_eq!(reader.try_read(&mut out).unwrap(), ReadOutcome::End);
    }

    #[test]
    fn a_decoder_panic_is_an_error() {
        struct Panics;
        impl AudioSource for Panics {
            fn spec(&self) -> StreamSpec {
                SPEC
            }
            fn read(&mut self, _out: &mut Vec<f32>) -> anyhow::Result<usize> {
                panic!("hostile file")
            }
            fn seek(&mut self, _ms: u64) -> anyhow::Result<()> {
                Ok(())
            }
            fn duration_ms(&self) -> Option<u64> {
                None
            }
        }
        let mut reader = ReadAhead::spawn(Box::new(Panics), READ_AHEAD_MS);
        let err = read_within(&mut reader, &mut Vec::new(), LIVENESS).unwrap_err();
        assert!(
            err.downcast_ref::<crate::audio::DecoderPanic>().is_some(),
            "{err:#}"
        );
    }

    #[test]
    fn seek_never_blocks_and_restarts_at_the_target() {
        let mut source = SlowSource::new(10);
        source.seek_delay = Duration::from_millis(300);
        let seeks_done = Arc::clone(&source.seeks_done);
        let mut reader = ReadAhead::spawn(Box::new(source), READ_AHEAD_MS);
        wait_until(LIVENESS, || reader.buffered_frames() > 0);
        reader.seek(2_000);
        assert_eq!(
            seeks_done.load(Ordering::SeqCst),
            0,
            "seek returned before the 300 ms source seek finished"
        );
        assert_eq!(reader.buffered_frames(), 0, "the old chunks are gone");
        assert!(!reader.exhausted());
        let mut out = Vec::new();
        read_within(&mut reader, &mut out, LIVENESS).unwrap();
        assert_eq!(frame_of(out[0]), 88_200, "2 s at 44.1 kHz");
    }

    #[test]
    fn wait_ready_gives_up_on_a_slow_call() {
        let mut source = SlowSource::new(10);
        source.read_delay = Duration::from_secs(1);
        let in_read = Arc::clone(&source.in_read);
        let reader = ReadAhead::spawn(Box::new(source), READ_AHEAD_MS);
        wait_until(LIVENESS, || in_read.load(Ordering::SeqCst));
        std::thread::sleep(SLOW_CALL * 2); // the first read has been running a while
        assert!(!reader.wait_ready(Duration::from_secs(5)));
        assert!(
            in_read.load(Ordering::SeqCst),
            "wait_ready gave up while the 1 s read was still running"
        );
    }

    #[test]
    fn a_working_thread_is_waited_for() {
        // Every read sleeps (one timer tick on Windows): slow overall, never one slow call.
        let mut source = SlowSource::new(10);
        source.read_delay = Duration::from_millis(1);
        let mut reader = ReadAhead::spawn(Box::new(source), READ_AHEAD_MS);
        assert!(reader.wait_ready(Duration::from_secs(5)));
        let mut out = Vec::new();
        assert_eq!(
            reader.try_read(&mut out).unwrap(),
            ReadOutcome::Frames(CHUNK as usize)
        );
    }

    #[test]
    fn a_seek_without_a_decode_thread_fails_instead_of_starving() {
        let mut reader = ReadAhead::spawn(Box::new(SlowSource::new(1)), READ_AHEAD_MS);
        // As if the thread had died: it exits and marks itself gone.
        reader.shared.lock().stop = true;
        reader.shared.wake.notify_all();
        wait_until(LIVENESS, || reader.shared.lock().gone);
        reader.shared.lock().stop = false;

        reader.seek(500);
        // No waiting at all: the error is queued by the seek itself.
        let err = reader.try_read(&mut Vec::new()).unwrap_err();
        assert!(err.to_string().contains("decode thread stopped"), "{err:#}");
        assert!(reader.exhausted());
    }

    #[test]
    fn drop_stops_the_thread() {
        let mut source = SlowSource::new(10);
        source.read_delay = Duration::from_millis(200);
        let drops = Arc::clone(&source.drops);
        let in_read = Arc::clone(&source.in_read);
        let reader = ReadAhead::spawn(Box::new(source), READ_AHEAD_MS);
        wait_until(LIVENESS, || in_read.load(Ordering::SeqCst));
        drop(reader);
        assert_eq!(
            drops.load(Ordering::SeqCst),
            0,
            "drop did not wait for the running read"
        );
        // Normally right after the 200 ms read; the bound allows for a busy machine.
        wait_until(LIVENESS, || drops.load(Ordering::SeqCst) == 1);
    }

    #[test]
    fn wait_stopped_waits_for_the_source_to_be_dropped() {
        let mut source = SlowSource::new(10);
        source.read_delay = Duration::from_millis(5);
        let drops = Arc::clone(&source.drops);
        let reader = ReadAhead::spawn(Box::new(source), READ_AHEAD_MS);
        reader.stop();
        assert!(reader.wait_stopped(Instant::now() + Duration::from_secs(5)));
        assert_eq!(
            drops.load(Ordering::SeqCst),
            1,
            "dropped before it returned"
        );

        // A read that outlasts the deadline: it gives up and leaves the thread to finish.
        let mut source = SlowSource::new(10);
        source.read_delay = Duration::from_secs(1);
        let drops = Arc::clone(&source.drops);
        let in_read = Arc::clone(&source.in_read);
        let reader = ReadAhead::spawn(Box::new(source), READ_AHEAD_MS);
        wait_until(LIVENESS, || in_read.load(Ordering::SeqCst));
        reader.stop();
        assert!(!reader.wait_stopped(Instant::now() + Duration::from_millis(50)));
        assert!(in_read.load(Ordering::SeqCst), "the read was still running");
        drop(reader);
        wait_until(LIVENESS, || drops.load(Ordering::SeqCst) == 1);
    }
}
