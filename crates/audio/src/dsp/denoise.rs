use nnnoiseless::DenoiseState;

/// RNNoise-based noise suppression for 48 kHz mono audio.
pub struct NoiseSuppressor {
    state: Box<DenoiseState<'static>>,
    input: [f32; DenoiseState::FRAME_SIZE],
    output: [f32; DenoiseState::FRAME_SIZE],
    vad_probability: f32,
}

impl NoiseSuppressor {
    /// Samples per internal frame (10 ms at 48 kHz).
    pub const FRAME: usize = DenoiseState::FRAME_SIZE;
    // nnnoiseless works on i16-scaled floats.
    const SCALE: f32 = 32768.0;

    pub fn new() -> Self {
        Self {
            state: DenoiseState::new(),
            input: [0.0; Self::FRAME],
            output: [0.0; Self::FRAME],
            vad_probability: 0.0,
        }
    }

    /// Denoises `samples` in place, one [`Self::FRAME`] chunk at a time; a trailing partial chunk
    /// is left untouched, so callers should pass whole multiples of [`Self::FRAME`].
    pub fn process(&mut self, samples: &mut [f32]) {
        for chunk in samples.chunks_exact_mut(Self::FRAME) {
            for (dst, s) in self.input.iter_mut().zip(chunk.iter()) {
                *dst = (s * Self::SCALE).clamp(-Self::SCALE, Self::SCALE - 1.0);
            }
            self.vad_probability = self.state.process_frame(&mut self.output, &self.input);
            for (dst, s) in chunk.iter_mut().zip(self.output.iter()) {
                *dst = s / Self::SCALE;
            }
        }
    }

    /// Speech probability reported by the model for the last processed chunk.
    pub fn vad_probability(&self) -> f32 {
        self.vad_probability
    }
}

impl Default for NoiseSuppressor {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::super::rms_db;
    use super::super::test_signals::noise;
    use super::*;

    /// Low-passed noise, similar to fan/HVAC hum. Flat full-band white noise is a poor fit for
    /// the RNNoise model (measured reductions vary from 1 to 34 dB with seed and level), whereas
    /// shaped noise is consistently reduced by 30-50 dB.
    fn fan_noise(amplitude: f32, seed: u64) -> Vec<f32> {
        let mut y = 0.0f32;
        noise(amplitude, 48_000 * 2, seed)
            .into_iter()
            .map(|x| {
                y += 0.1 * (x * 3.0 - y);
                y
            })
            .collect()
    }

    #[test]
    fn reduces_background_noise_energy() {
        for (amplitude, seed) in [(0.01, 1), (0.05, 3), (0.1, 42)] {
            let input = fan_noise(amplitude, seed);
            let mut ns = NoiseSuppressor::new();
            let mut output = input.clone();
            for frame in output.chunks_mut(960) {
                ns.process(frame);
            }
            // Skip the first 500 ms while the model adapts.
            let skip = 48_000 / 2;
            let in_db = rms_db(&input[skip..]);
            let out_db = rms_db(&output[skip..]);
            assert!(out_db < in_db - 12.0, "in {in_db} dB, out {out_db} dB");
        }
    }

    #[test]
    fn leaves_partial_chunk_untouched() {
        let mut ns = NoiseSuppressor::new();
        let mut samples = vec![0.25; NoiseSuppressor::FRAME + 10];
        ns.process(&mut samples);
        assert!(samples[NoiseSuppressor::FRAME..].iter().all(|&s| s == 0.25));
    }
}
