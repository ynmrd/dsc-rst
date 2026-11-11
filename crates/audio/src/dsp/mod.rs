//! Allocation-free voice DSP building blocks used by the capture and playback pipelines.

mod agc;
mod denoise;
mod gate;
mod resample;

pub use agc::{AutoGain, AutoSensitivity};
pub use denoise::NoiseSuppressor;
pub use gate::{Gate, GateMode};
pub use resample::Resampler;

/// Level reported for silence; keeps meters and comparisons finite.
pub const SILENCE_DB: f32 = -100.0;

pub fn db_to_linear(db: f32) -> f32 {
    10f32.powf(db / 20.0)
}

/// Converts an amplitude to dBFS, floored at [`SILENCE_DB`].
pub fn linear_to_db(x: f32) -> f32 {
    let x = x.abs();
    if x.is_nan() || x <= 1e-5 {
        return SILENCE_DB;
    }
    (20.0 * x.log10()).max(SILENCE_DB)
}

/// RMS level in dBFS (a full-scale sine is about -3 dB).
pub fn rms_db(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return SILENCE_DB;
    }
    let sum: f64 = samples.iter().map(|&s| f64::from(s) * f64::from(s)).sum();
    linear_to_db((sum / samples.len() as f64).sqrt() as f32)
}

/// Averages interleaved channels into `out` (cleared first).
pub fn downmix_to_mono(interleaved: &[f32], channels: usize, out: &mut Vec<f32>) {
    out.clear();
    match channels {
        0 => {}
        1 => out.extend_from_slice(interleaved),
        n => {
            let scale = 1.0 / n as f32;
            out.extend(interleaved.chunks_exact(n).map(|frame| frame.iter().sum::<f32>() * scale));
        }
    }
}

/// Maps interleaved stereo onto `channels` output channels (cleared first): mono averages L/R,
/// more than two channels get L/R on the front pair and silence elsewhere.
pub fn upmix_stereo_to(interleaved_stereo: &[f32], channels: usize, out: &mut Vec<f32>) {
    out.clear();
    let frames = interleaved_stereo.as_chunks::<2>().0.iter();
    match channels {
        0 => {}
        1 => out.extend(frames.map(|f| (f[0] + f[1]) * 0.5)),
        2 => out.extend_from_slice(&interleaved_stereo[..interleaved_stereo.len() & !1]),
        n => {
            for f in frames {
                out.push(f[0]);
                out.push(f[1]);
                out.extend(std::iter::repeat_n(0.0, n - 2));
            }
        }
    }
}

/// `dst[i] += src[i] * gain` over the common length.
pub fn mix_into(dst: &mut [f32], src: &[f32], gain: f32) {
    for (d, s) in dst.iter_mut().zip(src) {
        *d += s * gain;
    }
}

/// Transparent below the knee, then a tanh curve that saturates at ±1.
pub fn soft_clip(x: f32) -> f32 {
    const KNEE: f32 = 0.8;
    if x.is_nan() {
        return 0.0;
    }
    let a = x.abs();
    if a <= KNEE {
        return x;
    }
    let over = (a - KNEE) / (1.0 - KNEE);
    (KNEE + (1.0 - KNEE) * over.tanh()).copysign(x)
}

#[cfg(test)]
pub(crate) mod test_signals {
    use std::f32::consts::TAU;

    pub fn sine(freq: f32, rate: u32, amplitude: f32, len: usize) -> Vec<f32> {
        (0..len).map(|i| amplitude * (TAU * freq * i as f32 / rate as f32).sin()).collect()
    }

    /// Deterministic uniform white noise in [-amplitude, amplitude].
    pub fn noise(amplitude: f32, len: usize, seed: u64) -> Vec<f32> {
        let mut state = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        (0..len)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                let unit = (state >> 40) as f32 / (1u64 << 24) as f32;
                amplitude * (unit * 2.0 - 1.0)
            })
            .collect()
    }

    pub fn zero_crossings(samples: &[f32]) -> usize {
        samples.windows(2).filter(|w| (w[0] < 0.0) != (w[1] < 0.0)).count()
    }
}

#[cfg(test)]
mod tests {
    use super::test_signals::*;
    use super::*;

    #[test]
    fn db_conversions_roundtrip() {
        assert!((db_to_linear(0.0) - 1.0).abs() < 1e-6);
        assert!((db_to_linear(-20.0) - 0.1).abs() < 1e-6);
        assert!((linear_to_db(0.5) - -6.0206).abs() < 1e-3);
        assert_eq!(linear_to_db(0.0), SILENCE_DB);
        assert_eq!(linear_to_db(f32::NAN), SILENCE_DB);
        for db in [-60.0, -12.5, 0.0, 6.0] {
            assert!((linear_to_db(db_to_linear(db)) - db).abs() < 1e-3);
        }
    }

    #[test]
    fn rms_of_full_scale_sine_is_minus_3_db() {
        let s = sine(1000.0, 48_000, 1.0, 48_000);
        assert!((rms_db(&s) - -3.01).abs() < 0.05, "{}", rms_db(&s));
        assert_eq!(rms_db(&[]), SILENCE_DB);
        assert_eq!(rms_db(&[0.0; 960]), SILENCE_DB);
    }

    #[test]
    fn downmix_and_upmix() {
        let mut out = Vec::new();
        downmix_to_mono(&[1.0, 0.0, 0.5, 0.5], 2, &mut out);
        assert_eq!(out, vec![0.5, 0.5]);
        downmix_to_mono(&[0.3, 0.6, 0.9], 3, &mut out);
        assert!((out[0] - 0.6).abs() < 1e-6);

        upmix_stereo_to(&[1.0, 0.0, 0.2, 0.4], 1, &mut out);
        assert_eq!(out.len(), 2);
        assert!((out[0] - 0.5).abs() < 1e-6 && (out[1] - 0.3).abs() < 1e-6);
        upmix_stereo_to(&[1.0, 0.0], 2, &mut out);
        assert_eq!(out, vec![1.0, 0.0]);
        upmix_stereo_to(&[1.0, -1.0, 0.5, 0.25], 6, &mut out);
        assert_eq!(out, vec![1.0, -1.0, 0.0, 0.0, 0.0, 0.0, 0.5, 0.25, 0.0, 0.0, 0.0, 0.0]);
    }

    #[test]
    fn mix_and_soft_clip() {
        let mut dst = vec![0.1, 0.2, 0.3];
        mix_into(&mut dst, &[1.0, 1.0], 0.5);
        assert_eq!(dst, vec![0.6, 0.7, 0.3]);

        assert_eq!(soft_clip(0.5), 0.5);
        assert_eq!(soft_clip(-0.8), -0.8);
        assert!(soft_clip(1.2) < 1.0 && soft_clip(1.2) > 0.95);
        assert!(soft_clip(10.0) <= 1.0 && soft_clip(10.0) > 0.99);
        assert!(soft_clip(-10.0) >= -1.0);
        assert_eq!(soft_clip(f32::NAN), 0.0);
        let mut prev = soft_clip(0.0);
        for i in 1..400 {
            let v = soft_clip(i as f32 * 0.01);
            assert!(v >= prev);
            prev = v;
        }
    }
}
