use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering::Relaxed};

use crate::buffers::{EventSink, FrameInfo, FrameQueue, SharedJitter, monitor_buffer, playback_buffer};
use crate::dsp::{
    AutoGain, AutoSensitivity, Gate, GateMode, NoiseSuppressor, Resampler, SILENCE_DB, downmix_to_mono, rms_db, soft_clip, upmix_stereo_to,
};
use crate::{AudioConfig, FRAME_SAMPLES, InputMode, SAMPLE_RATE};

pub(crate) struct AtomicF32(AtomicU32);

impl AtomicF32 {
    pub fn new(v: f32) -> Self {
        Self(AtomicU32::new(v.to_bits()))
    }
    pub fn load(&self) -> f32 {
        f32::from_bits(self.0.load(Relaxed))
    }
    pub fn store(&self, v: f32) {
        self.0.store(v.to_bits(), Relaxed);
    }
}

/// State shared between the engine handle, the audio thread and the device callbacks.
pub(crate) struct Shared {
    pub input_gain: AtomicF32,
    pub output_volume: AtomicF32,
    pub ptt_mode: AtomicBool,
    pub threshold_db: AtomicF32,
    pub auto_threshold: AtomicBool,
    pub release_delay_ms: AtomicU32,
    pub noise_suppression: AtomicBool,
    pub auto_gain: AtomicBool,

    pub ptt_pressed: AtomicBool,
    pub muted: AtomicBool,
    pub deafened: AtomicBool,
    pub monitor: AtomicBool,

    pub level_db: AtomicF32,
    pub voice_active: AtomicBool,
    pub effective_threshold_db: AtomicF32,

    pub capture: FrameQueue,
    pub playback: SharedJitter,
    pub monitor_buf: SharedJitter,
    pub events: EventSink,
}

impl Shared {
    pub fn new(config: &AudioConfig) -> Self {
        let shared = Self {
            input_gain: AtomicF32::new(1.0),
            output_volume: AtomicF32::new(1.0),
            ptt_mode: AtomicBool::new(false),
            threshold_db: AtomicF32::new(-50.0),
            auto_threshold: AtomicBool::new(false),
            release_delay_ms: AtomicU32::new(0),
            noise_suppression: AtomicBool::new(false),
            auto_gain: AtomicBool::new(false),
            ptt_pressed: AtomicBool::new(false),
            muted: AtomicBool::new(false),
            deafened: AtomicBool::new(false),
            monitor: AtomicBool::new(false),
            level_db: AtomicF32::new(SILENCE_DB),
            voice_active: AtomicBool::new(false),
            effective_threshold_db: AtomicF32::new(AutoSensitivity::new().threshold_db()),
            capture: FrameQueue::new(),
            playback: playback_buffer(),
            monitor_buf: monitor_buffer(),
            events: EventSink::new(),
        };
        shared.apply_config(config);
        shared
    }

    pub fn apply_config(&self, config: &AudioConfig) {
        self.input_gain.store(sanitize(config.input_gain, 0.0, 2.0, 1.0));
        self.output_volume.store(sanitize(config.output_volume, 0.0, 2.0, 1.0));
        match config.mode {
            InputMode::VoiceActivity { threshold_db, auto } => {
                let threshold_db = sanitize(threshold_db, SILENCE_DB, 0.0, -50.0);
                self.ptt_mode.store(false, Relaxed);
                self.threshold_db.store(threshold_db);
                self.auto_threshold.store(auto, Relaxed);
                if !auto {
                    self.effective_threshold_db.store(threshold_db);
                }
            }
            InputMode::PushToTalk { release_delay_ms } => {
                self.ptt_mode.store(true, Relaxed);
                self.release_delay_ms.store(release_delay_ms, Relaxed);
            }
        }
        self.noise_suppression.store(config.noise_suppression, Relaxed);
        self.auto_gain.store(config.auto_gain, Relaxed);
    }

    pub fn reset_meters(&self) {
        self.level_db.store(SILENCE_DB);
        self.voice_active.store(false, Relaxed);
    }
}

fn sanitize(v: f32, min: f32, max: f32, fallback: f32) -> f32 {
    if v.is_finite() { v.clamp(min, max) } else { fallback }
}

/// Short fade applied when the gate opens to avoid a click.
const FADE_IN_SAMPLES: usize = 96;

/// Capture DSP chain run on the device callback thread. Buffers are allocated up front; the
/// steady state allocates nothing unless a callback is larger than any seen before.
pub(crate) struct CaptureProcessor {
    shared: Arc<Shared>,
    channels: usize,
    mono: Vec<f32>,
    resampler: Option<Resampler>,
    resampled: Vec<f32>,
    frame: [f32; FRAME_SAMPLES],
    filled: usize,
    denoiser: NoiseSuppressor,
    agc: AutoGain,
    sensitivity: AutoSensitivity,
    gate: Gate,
    was_open: bool,
}

impl CaptureProcessor {
    pub fn new(shared: Arc<Shared>, device_rate: u32, channels: usize) -> Self {
        let channels = channels.max(1);
        let resampler = (device_rate != SAMPLE_RATE).then(|| Resampler::new(device_rate, SAMPLE_RATE, 1));
        Self {
            shared,
            channels,
            mono: Vec::with_capacity(8192),
            resampler,
            resampled: Vec::with_capacity(16384),
            frame: [0.0; FRAME_SAMPLES],
            filled: 0,
            denoiser: NoiseSuppressor::new(),
            agc: AutoGain::new(),
            sensitivity: AutoSensitivity::new(),
            gate: Gate::new(GateMode::VoiceActivity { threshold_db: -50.0 }),
            was_open: false,
        }
    }

    /// Consumes interleaved device-format samples (already converted to f32).
    pub fn process(&mut self, interleaved: &[f32]) {
        downmix_to_mono(interleaved, self.channels, &mut self.mono);
        let input = match &mut self.resampler {
            Some(rs) => {
                self.resampled.clear();
                rs.process(&self.mono, &mut self.resampled);
                std::mem::take(&mut self.resampled)
            }
            None => std::mem::take(&mut self.mono),
        };
        let mut rest = &input[..];
        while !rest.is_empty() {
            let n = (FRAME_SAMPLES - self.filled).min(rest.len());
            self.frame[self.filled..self.filled + n].copy_from_slice(&rest[..n]);
            self.filled += n;
            rest = &rest[n..];
            if self.filled == FRAME_SAMPLES {
                self.filled = 0;
                self.process_frame();
            }
        }
        if self.resampler.is_some() {
            self.resampled = input;
        } else {
            self.mono = input;
        }
    }

    fn process_frame(&mut self) {
        let s = &*self.shared;
        let frame = &mut self.frame;

        let gain = s.input_gain.load();
        if gain != 1.0 {
            frame.iter_mut().for_each(|x| *x *= gain);
        }
        if s.noise_suppression.load(Relaxed) {
            self.denoiser.process(frame);
        }

        // Level and gate use the signal before AGC so the AGC cannot open the gate on noise.
        let level = rms_db(frame);
        let auto_threshold = self.sensitivity.update(level);
        let threshold = if s.auto_threshold.load(Relaxed) { auto_threshold } else { s.threshold_db.load() };
        s.effective_threshold_db.store(threshold);

        let mode = if s.ptt_mode.load(Relaxed) {
            GateMode::PushToTalk { release_delay_ms: s.release_delay_ms.load(Relaxed) }
        } else {
            GateMode::VoiceActivity { threshold_db: threshold }
        };
        self.gate.set_mode(mode);
        let open = self.gate.process(level, s.ptt_pressed.load(Relaxed), s.muted.load(Relaxed));

        if s.auto_gain.load(Relaxed) {
            self.agc.process(frame, open && level > threshold);
        }

        match (self.was_open, open) {
            (true, true) => {}
            (false, true) => {
                for (i, x) in frame.iter_mut().take(FADE_IN_SAMPLES).enumerate() {
                    *x *= i as f32 / FADE_IN_SAMPLES as f32;
                }
            }
            (true, false) => {
                let n = frame.len() as f32;
                for (i, x) in frame.iter_mut().enumerate() {
                    *x *= 1.0 - (i as f32 + 1.0) / n;
                }
            }
            (false, false) => frame.fill(0.0),
        }
        self.was_open = open;

        s.level_db.store(level);
        s.voice_active.store(open, Relaxed);
        if s.monitor.load(Relaxed) {
            s.monitor_buf.lock().push(frame);
        }
        s.capture.push(frame, FrameInfo { voice_active: open, level_db: level });
    }
}

/// Playback chain run on the output callback thread: jitter buffer + monitor → resample to the
/// device rate → map to device channels → volume/deafen (last, so changes apply immediately).
pub(crate) struct PlaybackRenderer {
    shared: Arc<Shared>,
    channels: usize,
    resampler: Option<Resampler>,
    chunk: Vec<f32>,
    monitor: Vec<f32>,
    pending: Vec<f32>,
    mapped: Vec<f32>,
}

impl PlaybackRenderer {
    /// 48 kHz frames pulled per refill when resampling (10 ms).
    const RESAMPLE_CHUNK_FRAMES: usize = 480;

    pub fn new(shared: Arc<Shared>, device_rate: u32, channels: usize) -> Self {
        Self {
            shared,
            channels: channels.max(1),
            resampler: (device_rate != SAMPLE_RATE).then(|| Resampler::new(SAMPLE_RATE, device_rate, 2)),
            chunk: Vec::with_capacity(16384),
            monitor: Vec::with_capacity(8192),
            pending: Vec::with_capacity(32768),
            mapped: Vec::with_capacity(32768),
        }
    }

    /// Fills `out` (interleaved, device channel count) completely.
    pub fn render(&mut self, out: &mut [f32]) {
        let frames = out.len() / self.channels;
        while self.pending.len() / 2 < frames {
            let need = frames - self.pending.len() / 2;
            let chunk_frames = if self.resampler.is_some() { Self::RESAMPLE_CHUNK_FRAMES } else { need };
            self.pull(chunk_frames);
            match &mut self.resampler {
                Some(rs) => rs.process(&self.chunk, &mut self.pending),
                None => self.pending.extend_from_slice(&self.chunk),
            }
        }
        upmix_stereo_to(&self.pending[..frames * 2], self.channels, &mut self.mapped);
        self.pending.drain(..frames * 2);

        let s = &*self.shared;
        let (mapped, rest) = out.split_at_mut(self.mapped.len());
        rest.fill(0.0);
        if s.deafened.load(Relaxed) {
            mapped.fill(0.0);
            return;
        }
        let volume = s.output_volume.load();
        if volume == 1.0 {
            mapped.copy_from_slice(&self.mapped);
        } else {
            for (dst, src) in mapped.iter_mut().zip(&self.mapped) {
                *dst = soft_clip(src * volume);
            }
        }
    }

    fn pull(&mut self, frames: usize) {
        let s = &*self.shared;
        self.chunk.clear();
        self.chunk.resize(frames * 2, 0.0);
        s.playback.lock().pop_into(&mut self.chunk);

        if s.monitor.load(Relaxed) {
            self.monitor.clear();
            self.monitor.resize(frames, 0.0);
            s.monitor_buf.lock().pop_into(&mut self.monitor);
            for (pair, m) in self.chunk.as_chunks_mut::<2>().0.iter_mut().zip(&self.monitor) {
                pair[0] += m;
                pair[1] += m;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PlaybackWriter;
    use crate::dsp::test_signals::sine;

    fn shared(config: AudioConfig) -> Arc<Shared> {
        Arc::new(Shared::new(&config))
    }

    fn vad_config(threshold_db: f32) -> AudioConfig {
        AudioConfig { mode: InputMode::VoiceActivity { threshold_db, auto: false }, ..AudioConfig::default() }
    }

    #[test]
    fn capture_resamples_44k1_stereo_into_48k_frames() {
        let shared = shared(vad_config(-40.0));
        let mut reader = shared.capture.reader();
        let mut proc = CaptureProcessor::new(shared.clone(), 44_100, 2);
        let tone = sine(1000.0, 44_100, 0.5, 44_100 / 5);
        let stereo: Vec<f32> = tone.iter().flat_map(|s| [*s, *s]).collect();
        for chunk in stereo.chunks(441 * 2) {
            proc.process(chunk);
        }
        // 200 ms of input → 9 full frames (the resampler holds back two input samples).
        assert_eq!(reader.available_frames(), 9);
        let mut out = [0.0; FRAME_SAMPLES];
        let mut last = None;
        while let Some(info) = reader.read_frame(&mut out) {
            last = Some(info);
        }
        let info = last.unwrap();
        assert!(info.voice_active);
        assert!((info.level_db - -9.03).abs() < 0.3, "{}", info.level_db);
        assert!((rms_db(&out) - info.level_db).abs() < 0.3);
        assert!(shared.voice_active.load(Relaxed));
    }

    #[test]
    fn capture_gate_silences_quiet_input_and_mute() {
        let shared = shared(vad_config(-30.0));
        let mut reader = shared.capture.reader();
        let mut proc = CaptureProcessor::new(shared.clone(), SAMPLE_RATE, 1);
        let quiet = sine(500.0, SAMPLE_RATE, 0.01, FRAME_SAMPLES);
        proc.process(&quiet);
        let mut out = [1.0; FRAME_SAMPLES];
        let info = reader.read_frame(&mut out).unwrap();
        assert!(!info.voice_active);
        assert!(out.iter().all(|&s| s == 0.0), "closed gate must emit silence");

        shared.muted.store(true, Relaxed);
        let loud = sine(500.0, SAMPLE_RATE, 0.8, FRAME_SAMPLES);
        proc.process(&loud);
        proc.process(&loud);
        reader.read_frame(&mut out).unwrap();
        let info = reader.read_frame(&mut out).unwrap();
        assert!(!info.voice_active);
        assert!(out.iter().all(|&s| s == 0.0), "mute must emit silence");
        assert!(!shared.voice_active.load(Relaxed));
    }

    #[test]
    fn push_to_talk_follows_key() {
        let shared = shared(AudioConfig { mode: InputMode::PushToTalk { release_delay_ms: 0 }, ..AudioConfig::default() });
        let mut reader = shared.capture.reader();
        let mut proc = CaptureProcessor::new(shared.clone(), SAMPLE_RATE, 1);
        let quiet = sine(500.0, SAMPLE_RATE, 0.001, FRAME_SAMPLES);
        let mut out = [0.0; FRAME_SAMPLES];
        proc.process(&quiet);
        assert!(!reader.read_frame(&mut out).unwrap().voice_active);
        shared.ptt_pressed.store(true, Relaxed);
        proc.process(&quiet);
        assert!(reader.read_frame(&mut out).unwrap().voice_active);
        assert!(out.iter().any(|&s| s != 0.0));
    }

    #[test]
    fn auto_threshold_is_published() {
        let shared = shared(AudioConfig { mode: InputMode::VoiceActivity { threshold_db: -10.0, auto: true }, ..AudioConfig::default() });
        let mut proc = CaptureProcessor::new(shared.clone(), SAMPLE_RATE, 1);
        let silence = [0.0; FRAME_SAMPLES];
        for _ in 0..200 {
            proc.process(&silence);
        }
        assert_eq!(shared.effective_threshold_db.load(), AutoSensitivity::MIN_THRESHOLD_DB);
    }

    #[test]
    fn playback_renders_volume_resampled_and_upmixed() {
        let shared = shared(AudioConfig { output_volume: 0.5, ..AudioConfig::default() });
        let mut writer = PlaybackWriter::new(shared.playback.clone());
        let tone = sine(1000.0, SAMPLE_RATE, 0.8, 4800);
        let stereo: Vec<f32> = tone.iter().flat_map(|s| [*s, *s]).collect();
        writer.write(&stereo);

        let mut renderer = PlaybackRenderer::new(shared.clone(), 44_100, 1);
        let mut out = vec![0.0; 441];
        let mut rendered = Vec::new();
        for _ in 0..8 {
            renderer.render(&mut out);
            rendered.extend_from_slice(&out);
        }
        let steady = &rendered[500..];
        let level = rms_db(steady);
        assert!((level - (rms_db(&tone) - 6.02)).abs() < 0.3, "level {level}");

        shared.deafened.store(true, Relaxed);
        renderer.render(&mut out);
        assert!(out.iter().all(|&s| s == 0.0));
    }

    #[test]
    fn playback_underrun_is_silent_and_monitor_mixes_capture() {
        let shared = shared(vad_config(-60.0));
        let mut renderer = PlaybackRenderer::new(shared.clone(), SAMPLE_RATE, 2);
        let mut out = vec![1.0; 960];
        renderer.render(&mut out);
        assert!(out.iter().all(|&s| s == 0.0));

        shared.monitor.store(true, Relaxed);
        let mut proc = CaptureProcessor::new(shared.clone(), SAMPLE_RATE, 1);
        let tone = sine(440.0, SAMPLE_RATE, 0.5, FRAME_SAMPLES * 3);
        proc.process(&tone);
        renderer.render(&mut out);
        assert!(out.iter().any(|&s| s != 0.0), "monitor audio reaches playback");
    }
}
