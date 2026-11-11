/// Streaming sample-rate converter for interleaved audio (Catmull-Rom cubic interpolation).
///
/// Accepts arbitrarily sized chunks and carries state between calls, adding two input frames
/// of latency. When downsampling, a 4th-order Butterworth low-pass at 45% of the output rate
/// limits aliasing.
#[derive(Debug, Clone)]
pub struct Resampler {
    in_rate: u32,
    out_rate: u32,
    channels: usize,
    step: f64,
    pos: f64,
    buf: Vec<f32>,
    filters: Vec<[Biquad; 2]>,
}

impl Resampler {
    pub fn new(in_rate: u32, out_rate: u32, channels: usize) -> Self {
        let in_rate = in_rate.max(1);
        let out_rate = out_rate.max(1);
        let channels = channels.max(1);
        let filters = if in_rate > out_rate {
            let cutoff = 0.45 * f64::from(out_rate);
            let fs = f64::from(in_rate);
            // Q values of the two sections of a 4th-order Butterworth.
            (0..channels)
                .map(|_| [Biquad::lowpass(fs, cutoff, 0.541_196), Biquad::lowpass(fs, cutoff, 1.306_563)])
                .collect()
        } else {
            Vec::new()
        };
        let mut resampler = Self {
            in_rate,
            out_rate,
            channels,
            step: f64::from(in_rate) / f64::from(out_rate),
            pos: 1.0,
            buf: Vec::with_capacity(channels * 8192),
            filters,
        };
        resampler.reset();
        resampler
    }

    pub fn in_rate(&self) -> u32 {
        self.in_rate
    }

    pub fn out_rate(&self) -> u32 {
        self.out_rate
    }

    pub fn channels(&self) -> usize {
        self.channels
    }

    pub fn is_passthrough(&self) -> bool {
        self.in_rate == self.out_rate
    }

    pub fn reset(&mut self) {
        // One leading silent frame provides the "previous" sample for the first output.
        self.buf.clear();
        self.buf.resize(self.channels, 0.0);
        self.pos = 1.0;
        for f in self.filters.iter_mut().flatten() {
            f.reset();
        }
    }

    /// Converts `input` (interleaved, whole frames) and appends the result to `out`.
    pub fn process(&mut self, input: &[f32], out: &mut Vec<f32>) {
        let ch = self.channels;
        let input = &input[..input.len() - input.len() % ch];
        if self.is_passthrough() {
            out.extend_from_slice(input);
            return;
        }

        let start = self.buf.len();
        self.buf.extend_from_slice(input);
        if !self.filters.is_empty() {
            for frame in self.buf[start..].chunks_exact_mut(ch) {
                for (s, sections) in frame.iter_mut().zip(self.filters.iter_mut()) {
                    for f in sections.iter_mut() {
                        *s = f.process(*s);
                    }
                }
            }
        }

        let frames = self.buf.len() / ch;
        loop {
            let i = self.pos as usize;
            if i + 2 >= frames {
                break;
            }
            let t = (self.pos - i as f64) as f32;
            for c in 0..ch {
                let p0 = self.buf[(i - 1) * ch + c];
                let p1 = self.buf[i * ch + c];
                let p2 = self.buf[(i + 1) * ch + c];
                let p3 = self.buf[(i + 2) * ch + c];
                out.push(catmull_rom(p0, p1, p2, p3, t));
            }
            self.pos += self.step;
        }

        let keep_from = (self.pos as usize).saturating_sub(1).min(frames);
        if keep_from > 0 {
            self.buf.drain(..keep_from * ch);
            self.pos -= keep_from as f64;
        }
    }
}

#[inline]
fn catmull_rom(p0: f32, p1: f32, p2: f32, p3: f32, t: f32) -> f32 {
    p1 + 0.5
        * t
        * (p2 - p0 + t * (2.0 * p0 - 5.0 * p1 + 4.0 * p2 - p3 + t * (3.0 * (p1 - p2) + p3 - p0)))
}

/// Transposed direct-form II biquad.
#[derive(Debug, Clone, Copy)]
struct Biquad {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
    z1: f32,
    z2: f32,
}

impl Biquad {
    fn lowpass(rate: f64, cutoff: f64, q: f64) -> Self {
        let w0 = std::f64::consts::TAU * cutoff.min(rate * 0.49) / rate;
        let (sin, cos) = w0.sin_cos();
        let alpha = sin / (2.0 * q);
        let a0 = 1.0 + alpha;
        Self {
            b0: ((1.0 - cos) / 2.0 / a0) as f32,
            b1: ((1.0 - cos) / a0) as f32,
            b2: ((1.0 - cos) / 2.0 / a0) as f32,
            a1: (-2.0 * cos / a0) as f32,
            a2: ((1.0 - alpha) / a0) as f32,
            z1: 0.0,
            z2: 0.0,
        }
    }

    fn reset(&mut self) {
        self.z1 = 0.0;
        self.z2 = 0.0;
    }

    #[inline]
    fn process(&mut self, x: f32) -> f32 {
        let y = self.b0 * x + self.z1;
        self.z1 = self.b1 * x - self.a1 * y + self.z2;
        self.z2 = self.b2 * x - self.a2 * y;
        y
    }
}

#[cfg(test)]
mod tests {
    use super::super::rms_db;
    use super::super::test_signals::*;
    use super::*;

    fn run_chunked(rs: &mut Resampler, input: &[f32], chunk_frames: usize) -> Vec<f32> {
        let mut out = Vec::new();
        for chunk in input.chunks(chunk_frames * rs.channels()) {
            rs.process(chunk, &mut out);
        }
        out
    }

    #[test]
    fn upsample_44k1_to_48k_preserves_tone_and_length() {
        let input = sine(1000.0, 44_100, 0.8, 44_100);
        let mut rs = Resampler::new(44_100, 48_000, 1);
        let out = run_chunked(&mut rs, &input, 441);
        assert!((out.len() as i64 - 48_000).abs() <= 4, "len {}", out.len());
        let zc = zero_crossings(&out);
        assert!((zc as i64 - 2000).abs() <= 4, "zero crossings {zc}");
        let steady = &out[1000..out.len() - 1000];
        assert!((rms_db(steady) - rms_db(&input)).abs() < 0.2);
    }

    #[test]
    fn downsample_stereo_48k_to_44k1() {
        let left = sine(1000.0, 48_000, 0.5, 48_000);
        let right = sine(3000.0, 48_000, 0.5, 48_000);
        let input: Vec<f32> = left.iter().zip(&right).flat_map(|(l, r)| [*l, *r]).collect();
        let mut rs = Resampler::new(48_000, 44_100, 2);
        // Odd chunk sizes, as delivered by real device callbacks.
        let out = run_chunked(&mut rs, &input, 333);
        let frames = out.len() / 2;
        assert!((frames as i64 - 44_100).abs() <= 4, "frames {frames}");
        let l: Vec<f32> = out.iter().step_by(2).copied().collect();
        let r: Vec<f32> = out.iter().skip(1).step_by(2).copied().collect();
        assert!((zero_crossings(&l) as i64 - 2000).abs() <= 4);
        assert!((zero_crossings(&r) as i64 - 6000).abs() <= 6);
    }

    #[test]
    fn downsample_attenuates_content_above_new_nyquist() {
        let input = sine(30_000.0, 96_000, 0.5, 96_000);
        let mut rs = Resampler::new(96_000, 48_000, 1);
        let out = run_chunked(&mut rs, &input, 480);
        assert!(rms_db(&out[2000..]) < rms_db(&input) - 6.0);
    }

    #[test]
    fn passthrough_is_identity() {
        let input = sine(440.0, 48_000, 0.5, 4800);
        let mut rs = Resampler::new(48_000, 48_000, 1);
        assert!(rs.is_passthrough());
        assert_eq!(run_chunked(&mut rs, &input, 100), input);
    }
}
