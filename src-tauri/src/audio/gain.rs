//! ReplayGain: the tag values turned into one linear factor per track.

/// The linear factor for a ReplayGain value of `gain_db`, lowered where needed so that
/// `peak * factor <= 1.0` (no clipping). Without a gain the factor is 1.0; without a peak
/// (or a nonsensical one) only the gain counts.
pub fn replaygain_factor(gain_db: Option<f32>, peak: Option<f32>) -> f32 {
    let Some(db) = gain_db.filter(|db| db.is_finite()) else {
        return 1.0;
    };
    let factor = 10f32.powf(db / 20.0);
    match peak.filter(|peak| peak.is_finite() && *peak > 0.0) {
        Some(peak) => factor.min(1.0 / peak),
        None => factor,
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
        let no_peak = replaygain_factor(Some(6.0), None);
        assert!((no_peak - 1.9953).abs() < 0.001, "{no_peak}");
        assert_eq!(
            replaygain_factor(Some(-3.0), Some(0.0)),
            replaygain_factor(Some(-3.0), None)
        );
        assert_eq!(replaygain_factor(Some(f32::NAN), None), 1.0);
    }
}
