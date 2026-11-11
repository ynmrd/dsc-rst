use std::fmt;
use std::sync::Arc;
use std::sync::atomic::Ordering::Relaxed;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SizedSample, StreamConfig, SupportedStreamConfig};
use crossbeam_channel::{Receiver, RecvTimeoutError, Sender};
use parking_lot::Mutex;

use crate::buffers::{CaptureReader, PlaybackWriter};
use crate::pipeline::{CaptureProcessor, PlaybackRenderer, Shared};
use crate::{AudioConfig, AudioEvent, SAMPLE_RATE};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Direction {
    Input,
    Output,
}

impl fmt::Display for Direction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Direction::Input => "input",
            Direction::Output => "output",
        })
    }
}

enum Command {
    Config(AudioConfig),
    Capture(bool),
    Playback(bool),
    StreamLost { dir: Direction, generation: u64, reason: String },
    Shutdown,
}

/// Handle to the audio subsystem. Cheap to clone; all clones control the same devices.
///
/// cpal streams are not `Send` on every platform, so they are owned by a dedicated `dn-audio`
/// thread that this handle drives through a command channel.
#[derive(Clone)]
pub struct AudioEngine {
    inner: Arc<EngineInner>,
}

struct EngineInner {
    shared: Arc<Shared>,
    commands: Sender<Command>,
    thread: Mutex<Option<JoinHandle<()>>>,
}

impl Drop for EngineInner {
    fn drop(&mut self) {
        drop(self.commands.send(Command::Shutdown));
    }
}

impl AudioEngine {
    /// Creates the engine and its audio thread without opening any device.
    pub fn new(config: AudioConfig) -> Self {
        let shared = Arc::new(Shared::new(&config));
        let (tx, rx) = crossbeam_channel::unbounded();
        let thread_shared = shared.clone();
        let thread_tx = tx.clone();
        let thread =
            std::thread::Builder::new().name("dn-audio".into()).spawn(move || AudioThread::new(thread_shared, thread_tx, config).run(rx));
        let thread = match thread {
            Ok(handle) => Some(handle),
            Err(e) => {
                shared.events.emit(AudioEvent::Error(format!("failed to start audio thread: {e}")));
                None
            }
        };
        Self { inner: Arc::new(EngineInner { shared, commands: tx, thread: Mutex::new(thread) }) }
    }

    fn send(&self, cmd: Command) {
        drop(self.inner.commands.send(cmd));
    }

    /// Applies new settings; only a stream whose device changed is rebuilt.
    pub fn set_config(&self, config: AudioConfig) {
        self.inner.shared.apply_config(&config);
        self.send(Command::Config(config));
    }

    pub fn start_capture(&self) {
        self.send(Command::Capture(true));
    }

    pub fn stop_capture(&self) {
        self.send(Command::Capture(false));
    }

    pub fn start_playback(&self) {
        self.send(Command::Playback(true));
    }

    pub fn stop_playback(&self) {
        self.send(Command::Playback(false));
    }

    pub fn capture_frames(&self) -> CaptureReader {
        self.inner.shared.capture.reader()
    }

    pub fn playback_writer(&self) -> PlaybackWriter {
        PlaybackWriter::new(self.inner.shared.playback.clone())
    }

    pub fn set_ptt(&self, pressed: bool) {
        self.inner.shared.ptt_pressed.store(pressed, Relaxed);
    }

    pub fn set_muted(&self, muted: bool) {
        let shared = &self.inner.shared;
        shared.muted.store(muted, Relaxed);
        if muted {
            // Don't wait for the next capture frame to report the gate closed.
            shared.voice_active.store(false, Relaxed);
        }
    }

    pub fn set_deafened(&self, deafened: bool) {
        self.inner.shared.deafened.store(deafened, Relaxed);
    }

    pub fn input_level_db(&self) -> f32 {
        self.inner.shared.level_db.load()
    }

    pub fn voice_active(&self) -> bool {
        self.inner.shared.voice_active.load(Relaxed)
    }

    pub fn effective_threshold_db(&self) -> f32 {
        self.inner.shared.effective_threshold_db.load()
    }

    pub fn events(&self) -> Receiver<AudioEvent> {
        self.inner.shared.events.subscribe()
    }

    /// Mic test: plays processed (gated) capture audio back locally.
    pub fn set_monitor(&self, enabled: bool) {
        let shared = &self.inner.shared;
        shared.monitor.store(enabled, Relaxed);
        if !enabled {
            shared.monitor_buf.lock().clear();
        }
    }

    /// Stops all streams and joins the audio thread. Further calls are no-ops.
    pub fn shutdown(&self) {
        self.send(Command::Shutdown);
        if let Some(handle) = self.inner.thread.lock().take() {
            drop(handle.join());
        }
    }
}

struct ActiveStream {
    // Dropping the stream closes the device.
    _stream: cpal::Stream,
    device: String,
}

struct Slot {
    wanted: bool,
    active: Option<ActiveStream>,
    generation: u64,
    retry_at: Option<Instant>,
    attempts: u32,
    last_error: Option<String>,
}

impl Slot {
    fn new() -> Self {
        Self { wanted: false, active: None, generation: 0, retry_at: None, attempts: 0, last_error: None }
    }

    fn close(&mut self) {
        self.active = None;
        self.retry_at = None;
        self.attempts = 0;
        self.last_error = None;
    }
}

fn backoff(attempt: u32) -> Duration {
    match attempt {
        0 => Duration::from_secs(1),
        1 => Duration::from_secs(2),
        _ => Duration::from_secs(5),
    }
}

struct AudioThread {
    shared: Arc<Shared>,
    commands: Sender<Command>,
    config: AudioConfig,
    host: cpal::Host,
    input: Slot,
    output: Slot,
}

impl AudioThread {
    fn new(shared: Arc<Shared>, commands: Sender<Command>, config: AudioConfig) -> Self {
        Self { shared, commands, config, host: cpal::default_host(), input: Slot::new(), output: Slot::new() }
    }

    fn run(mut self, rx: Receiver<Command>) {
        loop {
            let deadline = [self.input.retry_at, self.output.retry_at].into_iter().flatten().min();
            let timeout = deadline.map_or(Duration::from_secs(3600), |d| d.saturating_duration_since(Instant::now()));
            match rx.recv_timeout(timeout) {
                Ok(Command::Shutdown) | Err(RecvTimeoutError::Disconnected) => break,
                Ok(cmd) => self.handle(cmd),
                Err(RecvTimeoutError::Timeout) => {}
            }
            self.run_due_retries();
        }
        self.input.close();
        self.output.close();
        self.shared.reset_meters();
    }

    fn slot(&mut self, dir: Direction) -> &mut Slot {
        match dir {
            Direction::Input => &mut self.input,
            Direction::Output => &mut self.output,
        }
    }

    fn handle(&mut self, cmd: Command) {
        match cmd {
            Command::Config(config) => {
                let old = std::mem::replace(&mut self.config, config);
                if old.input_device != self.config.input_device {
                    self.reopen(Direction::Input);
                }
                if old.output_device != self.config.output_device {
                    self.reopen(Direction::Output);
                }
            }
            Command::Capture(on) => {
                if on {
                    self.shared.capture.clear();
                }
                self.set_wanted(Direction::Input, on);
                if !on {
                    self.shared.reset_meters();
                }
            }
            Command::Playback(on) => {
                if on {
                    self.shared.playback.lock().clear();
                    self.shared.monitor_buf.lock().clear();
                }
                self.set_wanted(Direction::Output, on);
            }
            Command::StreamLost { dir, generation, reason } => {
                let slot = self.slot(dir);
                if slot.generation != generation {
                    return;
                }
                if let Some(active) = slot.active.take() {
                    tracing::warn!(%dir, device = %active.device, %reason, "audio device lost");
                    slot.attempts = 0;
                    slot.retry_at = Some(Instant::now() + backoff(0));
                    let event = match dir {
                        Direction::Input => AudioEvent::InputLost(active.device),
                        Direction::Output => AudioEvent::OutputLost(active.device),
                    };
                    self.shared.events.emit(event);
                    if dir == Direction::Input {
                        self.shared.reset_meters();
                    }
                }
            }
            Command::Shutdown => {}
        }
    }

    fn set_wanted(&mut self, dir: Direction, on: bool) {
        let slot = self.slot(dir);
        slot.wanted = on;
        if on {
            if slot.active.is_none() {
                slot.attempts = 0;
                slot.last_error = None;
                self.try_open(dir);
            }
        } else {
            slot.close();
        }
    }

    fn reopen(&mut self, dir: Direction) {
        let slot = self.slot(dir);
        if !slot.wanted {
            return;
        }
        slot.close();
        self.try_open(dir);
    }

    fn run_due_retries(&mut self) {
        let now = Instant::now();
        for dir in [Direction::Input, Direction::Output] {
            let slot = self.slot(dir);
            if slot.wanted && slot.active.is_none() && slot.retry_at.is_some_and(|t| t <= now) {
                self.try_open(dir);
            }
        }
    }

    fn try_open(&mut self, dir: Direction) {
        let result = self.open(dir);
        let events = &self.shared.events;
        let slot = match dir {
            Direction::Input => &mut self.input,
            Direction::Output => &mut self.output,
        };
        match result {
            Ok(active) => {
                tracing::info!(%dir, device = %active.device, "audio stream started");
                events.emit(match dir {
                    Direction::Input => AudioEvent::InputStarted(active.device.clone()),
                    Direction::Output => AudioEvent::OutputStarted(active.device.clone()),
                });
                slot.active = Some(active);
                slot.retry_at = None;
                slot.attempts = 0;
                slot.last_error = None;
            }
            Err(message) => {
                // Report each distinct failure once instead of on every retry.
                if slot.last_error.as_deref() != Some(message.as_str()) {
                    tracing::warn!(%dir, %message, "failed to open audio device");
                    events.emit(AudioEvent::Error(message.clone()));
                    slot.last_error = Some(message);
                }
                slot.retry_at = Some(Instant::now() + backoff(slot.attempts));
                slot.attempts = slot.attempts.saturating_add(1);
            }
        }
    }

    fn open(&mut self, dir: Direction) -> Result<ActiveStream, String> {
        let wanted = match dir {
            Direction::Input => self.config.input_device.clone(),
            Direction::Output => self.config.output_device.clone(),
        };
        let (device, name) = resolve_device(&self.host, dir, wanted.as_deref())?;
        if let Some(wanted) = wanted.filter(|w| *w != name) {
            self.shared.events.emit(AudioEvent::Error(format!("{dir} device \"{wanted}\" not found, using \"{name}\"")));
        }
        let supported = pick_config(&device, dir).map_err(|e| format!("{dir} device \"{name}\": {e}"))?;

        let slot = self.slot(dir);
        slot.generation += 1;
        let generation = slot.generation;
        let on_error = error_callback(self.commands.clone(), dir, generation);
        let rate = supported.sample_rate();
        let channels = usize::from(supported.channels());
        let stream = match dir {
            Direction::Input => {
                let proc = CaptureProcessor::new(self.shared.clone(), rate, channels);
                build_input(&device, &supported, proc, on_error)
            }
            Direction::Output => {
                let renderer = PlaybackRenderer::new(self.shared.clone(), rate, channels);
                build_output(&device, &supported, renderer, on_error)
            }
        }
        .map_err(|e| format!("{dir} device \"{name}\": {e}"))?;
        stream.play().map_err(|e| format!("{dir} device \"{name}\": {e}"))?;
        tracing::debug!(%dir, %name, rate, channels, format = ?supported.sample_format(), "stream config");
        Ok(ActiveStream { _stream: stream, device: name })
    }
}

pub(crate) fn device_name(device: &cpal::Device) -> Option<String> {
    match device.description() {
        Ok(desc) => Some(desc.name().to_owned()),
        Err(_) => {
            let name = device.to_string();
            (!name.is_empty()).then_some(name)
        }
    }
}

pub(crate) fn default_device(host: &cpal::Host, dir: Direction) -> Option<cpal::Device> {
    match dir {
        Direction::Input => host.default_input_device(),
        Direction::Output => host.default_output_device(),
    }
}

pub(crate) fn list_devices(host: &cpal::Host, dir: Direction) -> Vec<cpal::Device> {
    let devices = match dir {
        Direction::Input => host.input_devices().map(|d| d.collect()),
        Direction::Output => host.output_devices().map(|d| d.collect()),
    };
    devices.unwrap_or_else(|e| {
        tracing::debug!(%dir, error = %e, "device enumeration failed");
        Vec::new()
    })
}

/// The configured device if present, else the system default.
fn resolve_device(host: &cpal::Host, dir: Direction, wanted: Option<&str>) -> Result<(cpal::Device, String), String> {
    if let Some(wanted) = wanted
        && let Some(device) = list_devices(host, dir).into_iter().find(|d| device_name(d).as_deref() == Some(wanted))
    {
        return Ok((device, wanted.to_owned()));
    }
    let device = default_device(host, dir).ok_or_else(|| format!("no {dir} device available"))?;
    let name = device_name(&device).unwrap_or_else(|| format!("default {dir}"));
    Ok((device, name))
}

/// The device's default config, switched to 48 kHz when the device supports it with the same
/// channel count and sample format.
fn pick_config(device: &cpal::Device, dir: Direction) -> Result<SupportedStreamConfig, cpal::Error> {
    let default = match dir {
        Direction::Input => device.default_input_config()?,
        Direction::Output => device.default_output_config()?,
    };
    if default.sample_rate() == SAMPLE_RATE {
        return Ok(default);
    }
    let ranges: Vec<_> = match dir {
        Direction::Input => device.supported_input_configs().map(|r| r.collect()),
        Direction::Output => device.supported_output_configs().map(|r| r.collect()),
    }
    .unwrap_or_default();
    let at_48k = ranges
        .into_iter()
        .filter(|r| r.channels() == default.channels() && r.sample_format() == default.sample_format())
        .find_map(|r| r.try_with_sample_rate(SAMPLE_RATE));
    Ok(at_48k.unwrap_or(default))
}

fn error_callback(commands: Sender<Command>, dir: Direction, generation: u64) -> impl FnMut(cpal::Error) + Send + 'static {
    use cpal::ErrorKind;
    move |err: cpal::Error| match err.kind() {
        ErrorKind::Xrun => tracing::trace!(%dir, "audio xrun"),
        ErrorKind::DeviceChanged => tracing::debug!(%dir, "audio route changed"),
        ErrorKind::RealtimeDenied => tracing::debug!(%dir, "realtime audio priority denied"),
        ErrorKind::DeviceNotAvailable | ErrorKind::StreamInvalidated | ErrorKind::HostUnavailable | ErrorKind::BackendError => {
            drop(commands.send(Command::StreamLost { dir, generation, reason: err.to_string() }));
        }
        _ => tracing::warn!(%dir, error = %err, "audio stream error"),
    }
}

fn unsupported_format(format: SampleFormat) -> cpal::Error {
    cpal::Error::with_message(cpal::ErrorKind::UnsupportedConfig, format!("unsupported sample format {format:?}"))
}

fn build_input<E>(
    device: &cpal::Device,
    supported: &SupportedStreamConfig,
    proc: CaptureProcessor,
    on_error: E,
) -> Result<cpal::Stream, cpal::Error>
where
    E: FnMut(cpal::Error) + Send + 'static,
{
    let config = supported.config();
    match supported.sample_format() {
        SampleFormat::F32 => input_stream::<f32, E>(device, config, proc, on_error),
        SampleFormat::F64 => input_stream::<f64, E>(device, config, proc, on_error),
        SampleFormat::I16 => input_stream::<i16, E>(device, config, proc, on_error),
        SampleFormat::I32 => input_stream::<i32, E>(device, config, proc, on_error),
        SampleFormat::I8 => input_stream::<i8, E>(device, config, proc, on_error),
        SampleFormat::U16 => input_stream::<u16, E>(device, config, proc, on_error),
        SampleFormat::U32 => input_stream::<u32, E>(device, config, proc, on_error),
        SampleFormat::U8 => input_stream::<u8, E>(device, config, proc, on_error),
        other => Err(unsupported_format(other)),
    }
}

fn input_stream<T, E>(
    device: &cpal::Device,
    config: StreamConfig,
    mut proc: CaptureProcessor,
    on_error: E,
) -> Result<cpal::Stream, cpal::Error>
where
    T: SizedSample,
    f32: FromSample<T>,
    E: FnMut(cpal::Error) + Send + 'static,
{
    let mut converted: Vec<f32> = Vec::with_capacity(16384);
    device.build_input_stream::<T, _, _>(
        config,
        move |data: &[T], _: &cpal::InputCallbackInfo| {
            converted.clear();
            converted.extend(data.iter().map(|s| s.to_sample::<f32>()));
            proc.process(&converted);
        },
        on_error,
        None,
    )
}

fn build_output<E>(
    device: &cpal::Device,
    supported: &SupportedStreamConfig,
    renderer: PlaybackRenderer,
    on_error: E,
) -> Result<cpal::Stream, cpal::Error>
where
    E: FnMut(cpal::Error) + Send + 'static,
{
    let config = supported.config();
    match supported.sample_format() {
        SampleFormat::F32 => output_stream::<f32, E>(device, config, renderer, on_error),
        SampleFormat::F64 => output_stream::<f64, E>(device, config, renderer, on_error),
        SampleFormat::I16 => output_stream::<i16, E>(device, config, renderer, on_error),
        SampleFormat::I32 => output_stream::<i32, E>(device, config, renderer, on_error),
        SampleFormat::I8 => output_stream::<i8, E>(device, config, renderer, on_error),
        SampleFormat::U16 => output_stream::<u16, E>(device, config, renderer, on_error),
        SampleFormat::U32 => output_stream::<u32, E>(device, config, renderer, on_error),
        SampleFormat::U8 => output_stream::<u8, E>(device, config, renderer, on_error),
        other => Err(unsupported_format(other)),
    }
}

fn output_stream<T, E>(
    device: &cpal::Device,
    config: StreamConfig,
    mut renderer: PlaybackRenderer,
    on_error: E,
) -> Result<cpal::Stream, cpal::Error>
where
    T: SizedSample + FromSample<f32>,
    E: FnMut(cpal::Error) + Send + 'static,
{
    let mut rendered: Vec<f32> = Vec::with_capacity(16384);
    device.build_output_stream::<T, _, _>(
        config,
        move |data: &mut [T], _: &cpal::OutputCallbackInfo| {
            rendered.clear();
            rendered.resize(data.len(), 0.0);
            renderer.render(&mut rendered);
            for (dst, src) in data.iter_mut().zip(&rendered) {
                *dst = T::from_sample(*src);
            }
        },
        on_error,
        None,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_schedule_caps_at_five_seconds() {
        let delays: Vec<u64> = (0..5).map(|a| backoff(a).as_secs()).collect();
        assert_eq!(delays, vec![1, 2, 5, 5, 5]);
    }
}
