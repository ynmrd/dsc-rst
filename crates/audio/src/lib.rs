//! Audio device management (CPAL), capture/playback and voice DSP.
//!
//! [`AudioEngine`] owns the microphone and speaker streams on a dedicated thread. Capture
//! produces gated 20 ms mono 48 kHz frames through a [`CaptureReader`]; playback consumes
//! stereo interleaved 48 kHz audio from a [`PlaybackWriter`]. Nothing here knows about Discord.

#![forbid(unsafe_code)]

mod buffers;
pub mod dsp;
mod engine;
mod pipeline;

pub use buffers::{CaptureReader, FrameInfo, PlaybackWriter};
pub use engine::AudioEngine;

use engine::{Direction, default_device, device_name, list_devices};

/// Internal processing rate for capture frames and playback audio.
pub const SAMPLE_RATE: u32 = 48_000;
/// Samples per capture frame: 20 ms of mono audio at [`SAMPLE_RATE`].
pub const FRAME_SAMPLES: usize = 960;

#[derive(Debug, Clone, PartialEq)]
pub struct DeviceInfo {
    pub name: String,
    pub is_default: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum InputMode {
    /// Transmit while the input level exceeds `threshold_db` (or an automatically estimated
    /// threshold when `auto` is set).
    VoiceActivity { threshold_db: f32, auto: bool },
    /// Transmit while the key is held, plus `release_delay_ms` after release.
    PushToTalk { release_delay_ms: u32 },
}

#[derive(Debug, Clone, PartialEq)]
pub struct AudioConfig {
    /// Device name as reported by [`input_devices`]; `None` follows the system default.
    pub input_device: Option<String>,
    /// Device name as reported by [`output_devices`]; `None` follows the system default.
    pub output_device: Option<String>,
    /// Linear microphone gain, clamped to `0.0..=2.0`.
    pub input_gain: f32,
    /// Linear output volume, clamped to `0.0..=2.0`.
    pub output_volume: f32,
    pub mode: InputMode,
    pub noise_suppression: bool,
    pub auto_gain: bool,
}

impl Default for AudioConfig {
    fn default() -> Self {
        Self {
            input_device: None,
            output_device: None,
            input_gain: 1.0,
            output_volume: 1.0,
            mode: InputMode::VoiceActivity {
                threshold_db: -50.0,
                auto: true,
            },
            noise_suppression: true,
            auto_gain: false,
        }
    }
}

#[derive(Debug, Clone)]
pub enum AudioEvent {
    /// Capture opened on the named device.
    InputStarted(String),
    /// Playback opened on the named device.
    OutputStarted(String),
    /// The named capture device disappeared; the engine keeps retrying in the background.
    InputLost(String),
    /// The named playback device disappeared; the engine keeps retrying in the background.
    OutputLost(String),
    Error(String),
}

/// Lists capture devices. Never panics; returns an empty list when no audio host is usable.
///
/// Enumeration runs on a short-lived helper thread so the caller's COM apartment (on Windows)
/// is irrelevant; it can take tens of milliseconds, so cache the result in UI code.
pub fn input_devices() -> Vec<DeviceInfo> {
    enumerate(Direction::Input)
}

/// Lists playback devices. Never panics; returns an empty list when no audio host is usable.
pub fn output_devices() -> Vec<DeviceInfo> {
    enumerate(Direction::Output)
}

fn enumerate(dir: Direction) -> Vec<DeviceInfo> {
    let worker = std::thread::Builder::new()
        .name("dn-audio-enum".into())
        .spawn(move || {
            let host = cpal::default_host();
            let default = default_device(&host, dir).and_then(|d| device_name(&d));
            let mut out: Vec<DeviceInfo> = Vec::new();
            for name in list_devices(&host, dir).iter().filter_map(device_name) {
                if !out.iter().any(|d| d.name == name) {
                    out.push(DeviceInfo {
                        is_default: default.as_deref() == Some(name.as_str()),
                        name,
                    });
                }
            }
            out
        });
    worker.ok().and_then(|h| h.join().ok()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::*;

    fn wait_for_event(
        engine: &AudioEngine,
        pred: impl Fn(&AudioEvent) -> bool,
    ) -> Option<AudioEvent> {
        let rx = engine.events();
        let deadline = Instant::now() + Duration::from_secs(10);
        while let Some(left) = deadline.checked_duration_since(Instant::now()) {
            match rx.recv_timeout(left) {
                Ok(ev) if pred(&ev) => return Some(ev),
                Ok(_) => {}
                Err(_) => return None,
            }
        }
        None
    }

    #[test]
    fn device_enumeration_never_panics() {
        for list in [input_devices(), output_devices()] {
            assert!(list.iter().filter(|d| d.is_default).count() <= 1);
            assert!(list.iter().all(|d| !d.name.is_empty()));
        }
    }

    #[test]
    fn engine_lifecycle_without_hardware() {
        let engine = AudioEngine::new(AudioConfig::default());
        assert!(!engine.voice_active());
        assert_eq!(engine.input_level_db(), dsp::SILENCE_DB);

        engine.set_config(AudioConfig {
            input_device: Some("definitely-missing-mic".into()),
            output_device: Some("definitely-missing-speaker".into()),
            mode: InputMode::VoiceActivity {
                threshold_db: -42.0,
                auto: false,
            },
            ..AudioConfig::default()
        });
        assert_eq!(engine.effective_threshold_db(), -42.0);

        engine.start_capture();
        engine.start_playback();
        // Headless machines report an error; machines with a (virtual) device start it and
        // also report that the configured name was not found.
        let ev = wait_for_event(&engine, |e| {
            matches!(
                e,
                AudioEvent::Error(_) | AudioEvent::InputStarted(_) | AudioEvent::OutputStarted(_)
            )
        });
        assert!(
            ev.is_some(),
            "engine must report the outcome of opening devices"
        );

        engine.set_ptt(true);
        engine.set_muted(true);
        assert!(!engine.voice_active());
        engine.set_deafened(true);
        engine.set_monitor(true);
        engine.set_monitor(false);
        engine.set_config(AudioConfig::default());
        engine.stop_capture();
        engine.stop_playback();
        engine.shutdown();
        engine.shutdown();
        // The handle stays usable after shutdown.
        engine.start_capture();
        let mut writer = engine.playback_writer();
        assert_eq!(writer.write(&[0.0; 4]), 4);
    }

    #[test]
    fn readers_and_writers_survive_config_changes() {
        let engine = AudioEngine::new(AudioConfig::default());
        let mut reader = engine.capture_frames();
        let mut writer = engine.playback_writer();
        engine.set_config(AudioConfig {
            output_device: Some("other".into()),
            ..AudioConfig::default()
        });
        writer.write(&[0.25; 1920]);
        assert!(writer.buffered_ms() > 19.0);
        let mut frame = [0.0; FRAME_SAMPLES];
        assert!(reader.read_frame(&mut frame).is_none());
        engine.shutdown();
    }
}
