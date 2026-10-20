//! Mixing helpers that run on the engine thread, never in the audio callback.

/// How much audio one level reading covers.
const WINDOW_MS: u32 = 10;
/// Time constant of the fall back after a loud moment; rises follow at once.
const RELEASE_SECS: f32 = 0.150;
/// The quietest level shown above zero, in dBFS; full scale is 1.0.
const FLOOR_DB: f32 = -50.0;
/// Below this the level reads as silence.
const SILENT: f32 = 0.001;

/// Loudness of what is handed to the output, for the ring's glow: the RMS of each
/// [`WINDOW_MS`] of samples mapped from [`FLOOR_DB`]..0 dBFS to 0..1, held up at once and
/// released over [`RELEASE_SECS`].
///
/// Feed it on the engine thread with what is pushed to the sink. It keeps a running sum
/// only: `push` never allocates or locks.
pub struct LevelMeter {
    /// Samples (all channels) in one window.
    window: usize,
    /// How much of the gap to the target is left after one falling window.
    release: f32,
    sum_squares: f64,
    count: usize,
    level: f32,
}

/// RMS (linear) to 0..1: `clamp((20*log10(rms) + 50) / 50, 0, 1)`.
pub fn level_of_rms(rms: f32) -> f32 {
    if rms <= 0.0 || !rms.is_finite() {
        return 0.0;
    }
    ((20.0 * rms.log10() - FLOOR_DB) / -FLOOR_DB).clamp(0.0, 1.0)
}

impl LevelMeter {
    pub fn new(sample_rate: u32, channels: u16) -> LevelMeter {
        let frames = (sample_rate.max(1) * WINDOW_MS / 1000).max(1) as usize;
        let window_secs = frames as f32 / sample_rate.max(1) as f32;
        LevelMeter {
            window: frames * channels.max(1) as usize,
            release: (-window_secs / RELEASE_SECS).exp(),
            sum_squares: 0.0,
            count: 0,
            level: 0.0,
        }
    }

    pub fn push(&mut self, samples: &[f32]) {
        for &sample in samples {
            self.sum_squares += f64::from(sample) * f64::from(sample);
            self.count += 1;
            if self.count == self.window {
                let rms = (self.sum_squares / self.count as f64).sqrt() as f32;
                self.sum_squares = 0.0;
                self.count = 0;
                self.settle(level_of_rms(rms));
            }
        }
    }

    /// 0..=1, the smoothed loudness of what was pushed.
    pub fn level(&self) -> f32 {
        self.level
    }

    /// Back to silence (after a stop or a seek), with no window half filled.
    pub fn reset(&mut self) {
        self.sum_squares = 0.0;
        self.count = 0;
        self.level = 0.0;
    }

    fn settle(&mut self, target: f32) {
        self.level = if target >= self.level {
            target
        } else {
            target + (self.level - target) * self.release
        };
        if self.level < SILENT {
            self.level = 0.0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 48_000;

    fn sine(amplitude: f32, ms: u32) -> Vec<f32> {
        let frames = RATE * ms / 1000;
        (0..frames)
            .flat_map(|n| {
                let value = amplitude
                    * (2.0 * std::f32::consts::PI * 1_000.0 * n as f32 / RATE as f32).sin();
                [value, value]
            })
            .collect()
    }

    #[test]
    fn level_tracks_rms() {
        let mut meter = LevelMeter::new(RATE, 2);
        meter.push(&sine(0.5, 500));
        let expected = (20.0 * 0.3536_f32.log10() + 50.0) / 50.0;
        assert!(
            (meter.level() - expected).abs() <= 0.02,
            "{} for {expected}",
            meter.level()
        );

        let mut quiet = LevelMeter::new(RATE, 2);
        quiet.push(&vec![0.0; RATE as usize]);
        assert_eq!(quiet.level(), 0.0, "silence");
    }

    #[test]
    fn level_releases_over_150_ms() {
        let mut meter = LevelMeter::new(RATE, 2);
        meter.push(&sine(0.5, 200));
        let loud = meter.level();
        // One release time later about 1/e of it is left, not nothing.
        meter.push(&vec![0.0; (RATE as usize * 150 / 1000) * 2]);
        let released = meter.level();
        assert!(
            released > loud * 0.25 && released < loud * 0.5,
            "{released} after 150 ms from {loud}"
        );
        meter.push(&vec![0.0; RATE as usize * 2 * 2]);
        assert_eq!(meter.level(), 0.0, "silence in the end");
    }

    #[test]
    fn level_attacks_at_once() {
        let mut meter = LevelMeter::new(RATE, 2);
        meter.push(&vec![0.0; RATE as usize]);
        meter.push(&sine(0.5, 20));
        assert!(meter.level() > 0.75, "{}", meter.level());
    }

    #[test]
    fn full_scale_and_the_floor() {
        assert_eq!(level_of_rms(1.0), 1.0);
        assert_eq!(level_of_rms(2.0), 1.0);
        assert_eq!(level_of_rms(0.003), 0.0);
        assert_eq!(level_of_rms(f32::NAN), 0.0);
        let mut meter = LevelMeter::new(RATE, 2);
        meter.push(&sine(0.5, 100));
        meter.reset();
        assert_eq!(meter.level(), 0.0);
    }
}
