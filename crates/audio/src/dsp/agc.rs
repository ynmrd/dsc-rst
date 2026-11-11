use super::{SILENCE_DB, db_to_linear, linear_to_db, rms_db};

/// Estimates the background noise floor and derives a voice-activity threshold from it.
///
/// The floor follows drops in level quickly and rises slowly, so speech barely moves it while
/// a change of environment is picked up within seconds.
#[derive(Debug, Clone)]
pub struct AutoSensitivity {
    floor_db: f32,
}

impl AutoSensitivity {
    pub const MARGIN_DB: f32 = 12.0;
    pub const MIN_THRESHOLD_DB: f32 = -70.0;
    pub const MAX_THRESHOLD_DB: f32 = -20.0;
    const INITIAL_FLOOR_DB: f32 = -60.0;
    const RISE_DB_PER_FRAME: f32 = 0.04;
    const FALL_COEFF: f32 = 0.25;

    pub fn new() -> Self {
        Self { floor_db: Self::INITIAL_FLOOR_DB }
    }

    /// Feeds one frame level and returns the updated threshold.
    pub fn update(&mut self, level_db: f32) -> f32 {
        let level = if level_db.is_nan() { SILENCE_DB } else { level_db.max(SILENCE_DB) };
        if level < self.floor_db {
            self.floor_db += (level - self.floor_db) * Self::FALL_COEFF;
        } else {
            self.floor_db = (self.floor_db + Self::RISE_DB_PER_FRAME).min(level);
        }
        self.threshold_db()
    }

    pub fn noise_floor_db(&self) -> f32 {
        self.floor_db
    }

    pub fn threshold_db(&self) -> f32 {
        (self.floor_db + Self::MARGIN_DB).clamp(Self::MIN_THRESHOLD_DB, Self::MAX_THRESHOLD_DB)
    }
}

impl Default for AutoSensitivity {
    fn default() -> Self {
        Self::new()
    }
}

/// Slow automatic gain control toward a speech RMS target with a fast clip guard.
///
/// Gain only adapts while `speech` is reported, so background noise below the gate is never
/// pulled up; the peak guard caps output below 0 dBFS regardless of the adapted gain.
#[derive(Debug, Clone)]
pub struct AutoGain {
    gain_db: f32,
    target_db: f32,
    max_gain_db: f32,
}

impl AutoGain {
    pub const DEFAULT_TARGET_DB: f32 = -20.0;
    pub const DEFAULT_MAX_GAIN_DB: f32 = 12.0;
    const MIN_GAIN_DB: f32 = -12.0;
    const RISE_DB_PER_FRAME: f32 = 0.1;
    const FALL_COEFF: f32 = 0.3;
    const CEILING: f32 = 0.98;

    pub fn new() -> Self {
        Self { gain_db: 0.0, target_db: Self::DEFAULT_TARGET_DB, max_gain_db: Self::DEFAULT_MAX_GAIN_DB }
    }

    pub fn with_target_db(mut self, target_db: f32) -> Self {
        self.target_db = target_db;
        self
    }

    pub fn with_max_gain_db(mut self, max_gain_db: f32) -> Self {
        self.max_gain_db = max_gain_db.max(0.0);
        self
    }

    pub fn gain_db(&self) -> f32 {
        self.gain_db
    }

    pub fn reset(&mut self) {
        self.gain_db = 0.0;
    }

    /// Applies gain in place (ramped across the frame) and returns the gain now in effect.
    pub fn process(&mut self, frame: &mut [f32], speech: bool) -> f32 {
        if frame.is_empty() {
            return self.gain_db;
        }
        let start = db_to_linear(self.gain_db);
        let level = rms_db(frame);
        if speech && level > SILENCE_DB {
            let desired = (self.target_db - level).clamp(Self::MIN_GAIN_DB, self.max_gain_db);
            if desired > self.gain_db {
                self.gain_db = (self.gain_db + Self::RISE_DB_PER_FRAME).min(desired);
            } else {
                self.gain_db += (desired - self.gain_db) * Self::FALL_COEFF;
            }
        }

        let peak = frame.iter().fold(0.0f32, |m, s| m.max(s.abs()));
        let mut end = db_to_linear(self.gain_db);
        if peak * end > Self::CEILING {
            end = Self::CEILING / peak;
            self.gain_db = linear_to_db(end);
        }
        let start = if peak * start > Self::CEILING { end } else { start };

        let n = frame.len() as f32;
        for (i, s) in frame.iter_mut().enumerate() {
            let g = start + (end - start) * (i as f32 + 1.0) / n;
            *s *= g;
        }
        self.gain_db
    }
}

impl Default for AutoGain {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_signals::*;
    use super::*;
    use crate::FRAME_SAMPLES;

    #[test]
    fn auto_sensitivity_converges_to_noise_floor_plus_margin() {
        let mut sens = AutoSensitivity::new();
        let noise = noise(0.0055, FRAME_SAMPLES * 1500, 7);
        let noise_db = rms_db(&noise);
        let speech = sine(220.0, 48_000, 0.3, FRAME_SAMPLES);
        let mut threshold = 0.0;
        for (i, frame) in noise.as_chunks::<FRAME_SAMPLES>().0.iter().enumerate() {
            // A 400 ms burst of "speech" every 2 s.
            let level = if i % 100 < 20 { rms_db(&speech) } else { rms_db(frame) };
            threshold = sens.update(level);
        }
        let expected = noise_db + AutoSensitivity::MARGIN_DB;
        assert!((threshold - expected).abs() < 3.0, "threshold {threshold} vs expected {expected} (noise {noise_db})");
    }

    #[test]
    fn auto_sensitivity_threshold_is_clamped() {
        let mut sens = AutoSensitivity::new();
        for _ in 0..500 {
            sens.update(SILENCE_DB);
        }
        assert_eq!(sens.threshold_db(), AutoSensitivity::MIN_THRESHOLD_DB);
        for _ in 0..5000 {
            sens.update(-3.0);
        }
        assert_eq!(sens.threshold_db(), AutoSensitivity::MAX_THRESHOLD_DB);
    }

    #[test]
    fn agc_raises_quiet_speech_within_limit() {
        let mut agc = AutoGain::new();
        let quiet = sine(300.0, 48_000, 0.0141, FRAME_SAMPLES * 400);
        let in_db = rms_db(&quiet);
        let mut last = Vec::new();
        for chunk in quiet.as_chunks::<FRAME_SAMPLES>().0 {
            let mut frame = chunk.to_vec();
            agc.process(&mut frame, true);
            assert!(frame.iter().all(|s| s.abs() <= 1.0));
            last = frame;
        }
        let out_db = rms_db(&last);
        assert!(out_db > in_db + 10.0, "in {in_db} out {out_db}");
        assert!(out_db <= in_db + AutoGain::DEFAULT_MAX_GAIN_DB + 0.1);
    }

    #[test]
    fn agc_does_not_adapt_without_speech() {
        let mut agc = AutoGain::new();
        let quiet = sine(300.0, 48_000, 0.001, FRAME_SAMPLES);
        for _ in 0..200 {
            let mut frame = quiet.clone();
            agc.process(&mut frame, false);
            assert_eq!(frame, quiet);
        }
        assert_eq!(agc.gain_db(), 0.0);
    }

    #[test]
    fn agc_never_exceeds_full_scale() {
        let mut agc = AutoGain::new();
        // Quiet input first so gain climbs to the maximum, then a sudden full-scale burst.
        let quiet = sine(300.0, 48_000, 0.01, FRAME_SAMPLES);
        for _ in 0..300 {
            agc.process(&mut quiet.clone(), true);
        }
        assert!(agc.gain_db() > 11.0);
        let loud = sine(300.0, 48_000, 1.0, FRAME_SAMPLES);
        for _ in 0..20 {
            let mut frame = loud.clone();
            agc.process(&mut frame, true);
            let peak = frame.iter().fold(0.0f32, |m, s| m.max(s.abs()));
            assert!(peak <= 1.0, "peak {peak}");
        }
        assert!(agc.gain_db() < 0.0);
    }
}
