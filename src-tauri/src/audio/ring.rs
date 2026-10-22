//! The ring buffer between the engine thread and the audio callback, without cpal.
//!
//! [`RingWriter`] is the engine side, [`Renderer`] the callback side. Clearing is count-based:
//! the writer records how many samples it had pushed when `clear` was called, and the renderer
//! discards exactly those (after ramping the gain down), never samples pushed afterwards.

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;

use super::source::StreamSpec;

/// Duration of a volume, pause or clear ramp.
const RAMP_MS: f32 = 20.0;

/// Frames one ramp lasts at `sample_rate`.
pub(crate) fn ramp_frames(sample_rate: u32) -> f32 {
    RAMP_MS / 1000.0 * sample_rate.max(1) as f32
}

/// `value` moved one `step` toward `target`, never past it.
pub(crate) fn ramp_toward(value: f32, target: f32, step: f32) -> f32 {
    if value < target {
        (value + step).min(target)
    } else {
        (value - step).max(target)
    }
}

/// State shared between both sides.
struct Control {
    volume: AtomicU32,
    paused: AtomicBool,
    /// Samples pushed before the last `clear`: the renderer drops everything up to here.
    discard_until: AtomicU64,
    /// Samples the renderer has taken out of the ring (played or discarded).
    popped: AtomicU64,
}

/// Creates a ring holding `capacity_frames` frames of `spec` audio.
pub fn ring(spec: StreamSpec, capacity_frames: usize) -> (RingWriter, Renderer) {
    let channels = spec.channels.max(1) as usize;
    let capacity = capacity_frames.max(1) * channels;
    let (producer, consumer) = rtrb::RingBuffer::new(capacity);
    let control = Arc::new(Control {
        volume: AtomicU32::new(1f32.to_bits()),
        paused: AtomicBool::new(false),
        discard_until: AtomicU64::new(0),
        popped: AtomicU64::new(0),
    });
    let writer = RingWriter {
        producer,
        channels,
        pushed_total: 0,
        control: Arc::clone(&control),
    };
    let renderer = Renderer {
        consumer,
        channels,
        step: 1.0 / ramp_frames(spec.sample_rate),
        gain: 0.0,
        popped_total: 0,
        control,
    };
    (writer, renderer)
}

/// Engine side of the ring.
pub struct RingWriter {
    producer: rtrb::Producer<f32>,
    channels: usize,
    pushed_total: u64,
    control: Arc<Control>,
}

impl RingWriter {
    /// Queues whole frames without blocking; returns the samples taken.
    pub fn push(&mut self, samples: &[f32]) -> usize {
        let room = self.producer.slots() / self.channels * self.channels;
        let take = (samples.len() / self.channels * self.channels).min(room);
        let (pushed, _) = self.producer.push_partial_slice(&samples[..take]);
        self.pushed_total += pushed.len() as u64;
        pushed.len()
    }

    /// Samples queued that will still be played (excludes samples pending a clear).
    pub fn queued(&self) -> usize {
        let popped = self.control.popped.load(Ordering::Acquire);
        let discarded = self.control.discard_until.load(Ordering::Acquire);
        self.pushed_total.saturating_sub(popped.max(discarded)) as usize
    }

    /// Drops everything pushed so far; samples pushed after this call are kept.
    pub fn clear(&mut self) {
        self.control
            .discard_until
            .store(self.pushed_total, Ordering::Release);
    }

    pub fn set_volume(&mut self, volume: f32) {
        let volume = if volume.is_finite() {
            volume.clamp(0.0, 1.0)
        } else {
            1.0
        };
        self.control
            .volume
            .store(volume.to_bits(), Ordering::Relaxed);
    }

    pub fn set_paused(&mut self, paused: bool) {
        self.control.paused.store(paused, Ordering::Relaxed);
    }
}

/// Callback side of the ring. Real-time safe: no allocation, no locks, never waits.
pub struct Renderer {
    consumer: rtrb::Consumer<f32>,
    channels: usize,
    step: f32,
    gain: f32,
    popped_total: u64,
    control: Arc<Control>,
}

impl Renderer {
    /// Fills `out` (whole frames) with queued audio, silence on under-run.
    pub fn render(&mut self, out: &mut [f32]) {
        let ch = self.channels;
        let frames = out.len() / ch;
        let mut frame = 0;
        while frame < frames {
            let discard_until = self.control.discard_until.load(Ordering::Acquire);
            let stale = discard_until.saturating_sub(self.popped_total) as usize;
            if stale > 0 && self.gain == 0.0 {
                // Faded out: cut the stale audio.
                let drop = stale.min(self.consumer.slots());
                if let Ok(chunk) = self.consumer.read_chunk(drop) {
                    chunk.commit_all();
                }
                self.popped_total += drop as u64;
                self.publish();
                if drop > 0 {
                    continue;
                }
            }
            let stale = discard_until.saturating_sub(self.popped_total) as usize;
            let paused = self.control.paused.load(Ordering::Relaxed);
            if paused && self.gain == 0.0 {
                // Fully ramped down: keep the queue for resuming.
                out[frame * ch..].fill(0.0);
                return;
            }
            let target = if paused || stale > 0 {
                0.0
            } else {
                f32::from_bits(self.control.volume.load(Ordering::Relaxed))
            };
            // While fading out for a cut, never read past the stale samples.
            let mut span = frames - frame;
            if stale > 0 {
                // ...and stop where the gain reaches zero, so the cut happens right there.
                let to_silence = (self.gain / self.step).ceil().max(1.0) as usize;
                span = span.min(stale / ch).min(to_silence);
            }
            if span == 0 {
                out[frame * ch..].fill(0.0);
                return;
            }
            let region = &mut out[frame * ch..(frame + span) * ch];
            let got = self.consumer.pop_partial_slice(region).0.len();
            self.popped_total += got as u64;
            region[got..].fill(0.0);
            for samples in region.chunks_exact_mut(ch) {
                self.gain = ramp_toward(self.gain, target, self.step);
                for sample in samples {
                    *sample *= self.gain;
                }
            }
            self.publish();
            frame += span;
        }
    }

    fn publish(&self) {
        self.control
            .popped
            .store(self.popped_total, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::output::BUFFER_MS;

    const SPEC: StreamSpec = StreamSpec {
        sample_rate: 48_000,
        channels: 2,
    };
    /// The real output's ring at this spec.
    const RING_FRAMES: usize = (SPEC.sample_rate * BUFFER_MS / 1000) as usize;
    /// Frames in one 20 ms ramp at 48 kHz.
    const RAMP_FRAMES: usize = 960;

    fn render_all(renderer: &mut Renderer, frames: usize) -> Vec<f32> {
        let mut out = vec![9.0; frames * 2];
        for block in out.chunks_mut(256) {
            renderer.render(block);
        }
        out
    }

    #[test]
    fn clear_before_playback_drops_only_the_old_samples() {
        let (mut writer, mut renderer) = ring(SPEC, RING_FRAMES);
        let a = vec![1.0; 2_000];
        let b = vec![-1.0; 3_000];
        assert_eq!(writer.push(&a), a.len());
        writer.clear();
        assert_eq!(writer.queued(), 0);
        assert_eq!(writer.push(&b), b.len());
        assert_eq!(writer.queued(), b.len());

        let out = render_all(&mut renderer, 4_000);
        assert!(out.iter().all(|s| *s <= 0.0), "nothing from A is played");
        assert_eq!(
            out.iter().filter(|s| **s < 0.0).count(),
            b.len(),
            "all of B is played"
        );
        assert_eq!(writer.queued(), 0);
    }

    #[test]
    fn clear_while_playing_fades_out_then_plays_all_new_samples() {
        let (mut writer, mut renderer) = ring(SPEC, RING_FRAMES);
        writer.push(&vec![1.0; RING_FRAMES * 2]);
        render_all(&mut renderer, 2_000); // A is audible at full gain now
        writer.clear();
        let b = vec![-1.0; 4_000];
        writer.push(&b);

        let out = render_all(&mut renderer, 6_000);
        let first_b = out.iter().position(|s| *s < 0.0).expect("B is played");
        let fade = &out[..first_b];
        assert!(fade.len() <= RAMP_FRAMES * 2 + 2, "A fades within one ramp");
        assert!(
            fade.windows(2).all(|w| w[1] <= w[0] + 1e-6),
            "A only fades down"
        );
        assert!(fade.iter().all(|s| *s >= 0.0));
        assert_eq!(out.iter().filter(|s| **s < 0.0).count(), b.len());
        // B starts from silence and ramps up (no click).
        assert!(out[first_b].abs() < 0.01, "B ramps in: {}", out[first_b]);
    }

    #[test]
    fn under_run_writes_silence() {
        let (mut writer, mut renderer) = ring(SPEC, 100);
        writer.push(&[0.5; 20]);
        let out = render_all(&mut renderer, 100);
        assert!(out[20..].iter().all(|s| *s == 0.0));
    }

    #[test]
    fn paused_ramps_down_and_keeps_the_queue() {
        let (mut writer, mut renderer) = ring(SPEC, RING_FRAMES);
        writer.push(&vec![1.0; RING_FRAMES * 2]);
        render_all(&mut renderer, 2_000);
        writer.set_paused(true);
        render_all(&mut renderer, 2_000);
        let queued = writer.queued();
        let out = render_all(&mut renderer, 1_000);
        assert!(out.iter().all(|s| *s == 0.0));
        assert_eq!(writer.queued(), queued, "nothing is consumed while paused");
    }
}
