//! ReplayGain: the tag values turned into one linear factor per track.

/// Bounds of a ReplayGain value, in dB; tags outside them are clamped.
const MIN_GAIN_DB: f32 = -24.0;
const MAX_GAIN_DB: f32 = 12.0;

/// The linear factor for a ReplayGain value of `gain_db` (clamped to -24..+12 dB), lowered
/// where needed so that `peak * factor <= 1.0` (no clipping). Without a gain the factor is
/// 1.0; without a peak (or a nonsensical one) a positive gain is capped at 1.0, since the
/// loudness of the file is unknown.
pub fn replaygain_factor(gain_db: Option<f32>, peak: Option<f32>) -> f32 {
    let Some(db) = gain_db.filter(|db| db.is_finite()) else {
        return 1.0;
    };
    let factor = 10f32.powf(db.clamp(MIN_GAIN_DB, MAX_GAIN_DB) / 20.0);
    match peak.filter(|peak| peak.is_finite() && *peak > 0.0) {
        Some(peak) => factor.min(1.0 / peak),
        None => factor.min(1.0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replaygain_gain_clamps_to_peak() {
        let quieter = replaygain_factor(Some(-6.0), Some(0.99));
        assert!((quieter - 0.501).abs() < 0.001, "{quieter}");

        let louder = replaygain_factor(Some(6.0), Some(0.9));
        assert!(louder * 0.9 <= 1.0 + 1e-6, "{louder} clips a 0.9 peak");
        assert!(
            (louder - 1.0 / 0.9).abs() < 1e-4,
            "{louder} is clamped to the peak"
        );

        assert_eq!(replaygain_factor(None, Some(0.5)), 1.0, "no tag, no change");
        assert_eq!(replaygain_factor(Some(6.0), None), 1.0, "no peak, no boost");
        let no_peak_cut = replaygain_factor(Some(-6.0), None);
        assert!((no_peak_cut - 0.501).abs() < 0.001, "{no_peak_cut}");
        assert_eq!(
            replaygain_factor(Some(-3.0), Some(0.0)),
            replaygain_factor(Some(-3.0), None)
        );
        assert_eq!(replaygain_factor(Some(f32::NAN), None), 1.0);
    }

    #[test]
    fn huge_gain_without_a_peak_never_boosts() {
        assert!(replaygain_factor(Some(40.0), None) <= 1.0);
    }

    #[test]
    fn boost_with_a_small_peak_reaches_the_gain() {
        let factor = replaygain_factor(Some(6.0), Some(0.5));
        assert!((factor - 1.995).abs() < 0.001, "{factor}");
        assert!(factor * 0.5 <= 1.0);
    }

    #[test]
    fn cut_clamps_to_minus_24_db() {
        let clamped = replaygain_factor(Some(-40.0), None);
        let floor = replaygain_factor(Some(-24.0), None);
        assert_eq!(clamped, floor);
        assert!((floor - 0.0631).abs() < 0.001, "{floor}");
    }

    #[test]
    fn boost_clamps_to_plus_12_db() {
        let factor = replaygain_factor(Some(40.0), Some(0.01));
        assert!((factor - 3.981).abs() < 0.01, "{factor}");
    }
}
