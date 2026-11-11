//! Discord voice connections via songbird (DAVE E2EE), mixing, speaking indicators.
//!
//! [`VoiceManager`] drives a standalone songbird [`Driver`] with the session details the main
//! gateway delivers (`VOICE_STATE_UPDATE` + `VOICE_SERVER_UPDATE`). The microphone is fed from
//! [`dn_audio::AudioEngine`] and received audio is mixed back into its playback queue. All
//! songbird types stay internal to this crate.

#![forbid(unsafe_code)]

mod receive;
mod source;

use std::collections::HashMap;
use std::fmt;
use std::num::NonZeroU64;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering::SeqCst};
use std::sync::{Arc, Weak};
use std::time::Duration;

use async_trait::async_trait;
use dn_audio::{AudioEngine, PlaybackWriter, SAMPLE_RATE};
use dn_core::Id;
use parking_lot::{Mutex, RwLock};
use songbird::driver::{Bitrate, Channels, DecodeConfig, DecodeMode, SampleRate};
use songbird::events::context_data::{DisconnectKind, DisconnectReason};
use songbird::events::{CoreEvent, Event, EventContext, EventHandler};
use songbird::input::codecs::{get_codec_registry, get_probe};
use songbird::input::{Input, RawAdapter};
use songbird::tracks::{Track, TrackHandle};
use songbird::{Config, ConnectionInfo, Driver};
use tokio::sync::mpsc::UnboundedSender;
use tokio::time::MissedTickBehavior;

use receive::{SpeakingTracker, SsrcMap, TICK_SAMPLES, is_audible, mix_tick};
use source::CaptureSource;

/// Everything needed to join a voice channel, gathered from the main gateway's
/// `VOICE_STATE_UPDATE` (session id) and `VOICE_SERVER_UPDATE` (endpoint, token).
#[derive(Clone)]
pub struct VoiceConnectInfo {
    pub guild_id: Id,
    pub channel_id: Id,
    pub user_id: Id,
    pub session_id: String,
    /// Voice server host as sent by Discord (e.g. `"c-ams01.discord.media:443"`).
    pub endpoint: String,
    pub token: String,
}

impl fmt::Debug for VoiceConnectInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("VoiceConnectInfo")
            .field("guild_id", &self.guild_id)
            .field("channel_id", &self.channel_id)
            .field("user_id", &self.user_id)
            .field("session_id", &self.session_id)
            .field("endpoint", &self.endpoint)
            .field("token", &"<redacted>")
            .finish()
    }
}

impl VoiceConnectInfo {
    fn to_songbird(&self) -> Result<ConnectionInfo, VoiceError> {
        let id = |id: Id, what: &str| {
            NonZeroU64::new(id.0)
                .ok_or_else(|| VoiceError::Connection(format!("{what} id must not be zero")))
        };
        let endpoint = normalize_endpoint(&self.endpoint);
        if endpoint.is_empty() {
            return Err(VoiceError::Connection("voice endpoint is empty".into()));
        }
        if self.session_id.is_empty() || self.token.is_empty() {
            return Err(VoiceError::Connection(
                "voice session id or token is empty".into(),
            ));
        }
        Ok(ConnectionInfo {
            channel_id: id(self.channel_id, "channel")?.into(),
            endpoint,
            guild_id: id(self.guild_id, "guild")?.into(),
            session_id: self.session_id.clone(),
            token: self.token.clone(),
            user_id: id(self.user_id, "user")?.into(),
        })
    }
}

/// songbird adds the `wss://` scheme itself.
fn normalize_endpoint(endpoint: &str) -> String {
    let endpoint = endpoint.trim();
    let endpoint = endpoint.strip_prefix("wss://").unwrap_or(endpoint);
    endpoint.trim_end_matches('/').to_owned()
}

#[derive(Debug, Clone, PartialEq)]
pub enum VoiceState {
    Idle,
    Connecting,
    Connected {
        channel_id: Id,
    },
    /// Switching to a new voice server or session (a repeated [`VoiceManager::connect`]).
    Reconnecting,
    Failed(String),
}

#[derive(Debug, Clone)]
pub enum VoiceEvent {
    State(VoiceState),
    Speaking {
        user_id: Id,
        speaking: bool,
    },
    /// Our own transmission started or stopped (gate, push-to-talk, mute).
    SelfSpeaking(bool),
    /// Voice round-trip time in ms. songbird does not expose its heartbeat timing, so this is
    /// currently always `None` (sent once per connection).
    Ping(Option<u32>),
    UserLeft {
        user_id: Id,
    },
    Error(String),
}

#[derive(Debug, thiserror::Error)]
pub enum VoiceError {
    #[error("voice requires a running tokio runtime")]
    NotInRuntime,
    #[error("voice connection failed: {0}")]
    Connection(String),
    #[error("audio error: {0}")]
    Audio(String),
}

/// Controls the user's voice connection. Cheap to clone; clones share one connection.
#[derive(Clone)]
pub struct VoiceManager {
    inner: Arc<Inner>,
}

struct DriverSlot {
    driver: Driver,
    track: Option<TrackHandle>,
    /// Play state last applied to `track` (tracks start paused).
    track_playing: bool,
}

struct ReceiveState {
    ssrcs: SsrcMap,
    speaking: SpeakingTracker,
    mix: Vec<f32>,
    writer: PlaybackWriter,
}

struct Inner {
    audio: AudioEngine,
    events: UnboundedSender<VoiceEvent>,
    driver: Mutex<Option<DriverSlot>>,
    state: Mutex<VoiceState>,
    /// Session we want to be in; `None` after leaving or a terminal failure.
    session: Mutex<Option<VoiceConnectInfo>>,
    /// Bumped by every connect/disconnect/failure so stale connect results are ignored.
    generation: AtomicU64,
    self_mute: AtomicBool,
    self_deaf: AtomicBool,
    bitrate_kbps: AtomicU32,
    user_volumes: RwLock<HashMap<Id, f32>>,
    rx: Mutex<ReceiveState>,
    gate_task_started: AtomicBool,
}

impl VoiceManager {
    /// Gate polling interval; one Discord voice frame.
    const GATE_POLL: Duration = Duration::from_millis(20);
    const DEFAULT_BITRATE_KBPS: u32 = 64;

    /// Creates the manager without connecting. Call from within a tokio runtime; if that is
    /// not possible, the background tasks start on the first [`connect`](Self::connect).
    pub fn new(audio: AudioEngine, events: UnboundedSender<VoiceEvent>) -> Self {
        let writer = audio.playback_writer();
        let inner = Arc::new(Inner {
            audio,
            events,
            driver: Mutex::new(None),
            state: Mutex::new(VoiceState::Idle),
            session: Mutex::new(None),
            generation: AtomicU64::new(0),
            self_mute: AtomicBool::new(false),
            self_deaf: AtomicBool::new(false),
            bitrate_kbps: AtomicU32::new(Self::DEFAULT_BITRATE_KBPS),
            user_volumes: RwLock::new(HashMap::new()),
            rx: Mutex::new(ReceiveState {
                ssrcs: SsrcMap::default(),
                speaking: SpeakingTracker::new(SpeakingTracker::DEFAULT_HANGOVER_TICKS),
                mix: vec![0.0; TICK_SAMPLES],
                writer,
            }),
            gate_task_started: AtomicBool::new(false),
        });
        let manager = Self { inner };
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            manager.ensure_gate_task(&handle);
        }
        manager
    }

    fn ensure_gate_task(&self, handle: &tokio::runtime::Handle) {
        if !self.inner.gate_task_started.swap(true, SeqCst) {
            handle.spawn(gate_loop(Arc::downgrade(&self.inner)));
        }
    }

    /// Joins (or moves to) the voice server described by `info`.
    ///
    /// Starts microphone capture and playback. Calling it again with a new endpoint/token
    /// (e.g. after another `VOICE_SERVER_UPDATE`) reconnects; identical info while connected
    /// is a no-op.
    pub async fn connect(&self, info: VoiceConnectInfo) -> Result<(), VoiceError> {
        let handle = tokio::runtime::Handle::try_current().map_err(|_| VoiceError::NotInRuntime)?;
        let conn = info.to_songbird()?;
        let channel_id = info.channel_id;
        let inner = &self.inner;

        let generation = inner.generation.fetch_add(1, SeqCst) + 1;
        *inner.session.lock() = Some(info);
        let moving = matches!(
            *inner.state.lock(),
            VoiceState::Connected { .. } | VoiceState::Reconnecting
        );
        inner.set_state(if moving {
            VoiceState::Reconnecting
        } else {
            VoiceState::Connecting
        });
        inner.audio.start_capture();
        inner.audio.start_playback();
        self.ensure_gate_task(&handle);

        let pending = {
            let mut guard = inner.driver.lock();
            let slot = guard.get_or_insert_with(|| Inner::new_driver(inner));
            inner.ensure_track(slot);
            slot.driver.connect(conn)
        };
        let result = pending.await;

        if inner.generation.load(SeqCst) != generation {
            return result.map_err(|e| VoiceError::Connection(e.to_string()));
        }
        match result {
            Ok(()) => {
                inner.on_driver_connected(channel_id);
                Ok(())
            }
            Err(e) => {
                inner.fail(format!("could not connect to voice: {e}"));
                Err(VoiceError::Connection(e.to_string()))
            }
        }
    }

    /// Leaves the voice channel and releases the microphone and speakers.
    pub async fn disconnect(&self) {
        let inner = &self.inner;
        inner.generation.fetch_add(1, SeqCst);
        *inner.session.lock() = None;
        if let Some(slot) = inner.driver.lock().as_mut() {
            slot.driver.leave();
            slot.driver.stop();
            slot.track = None;
            slot.track_playing = false;
        }
        inner.release_audio();
        inner.clear_receive();
        inner.force_state(VoiceState::Idle);
    }

    pub fn set_self_mute(&self, muted: bool) {
        self.inner.self_mute.store(muted, SeqCst);
        self.inner.apply_mute();
    }

    /// Deafening also stops transmission, like the official client.
    pub fn set_self_deaf(&self, deafened: bool) {
        self.inner.self_deaf.store(deafened, SeqCst);
        self.inner.audio.set_deafened(deafened);
        self.inner.apply_mute();
    }

    /// Local playback volume for one user, linear `0.0..=2.0`.
    pub fn set_user_volume(&self, user: Id, volume: f32) {
        let volume = if volume.is_finite() {
            volume.clamp(0.0, 2.0)
        } else {
            1.0
        };
        self.inner.user_volumes.write().insert(user, volume);
    }

    /// Opus encoder bitrate in kbps (clamped to 8..=512).
    pub fn set_bitrate(&self, kbps: u32) {
        let kbps = kbps.clamp(8, 512);
        self.inner.bitrate_kbps.store(kbps, SeqCst);
        if let Some(slot) = self.inner.driver.lock().as_mut() {
            slot.driver.set_bitrate(bitrate(kbps));
        }
    }

    pub fn state(&self) -> VoiceState {
        self.inner.state.lock().clone()
    }

    pub fn speaking_users(&self) -> Vec<Id> {
        self.inner.rx.lock().speaking.speaking()
    }
}

fn bitrate(kbps: u32) -> Bitrate {
    Bitrate::Bits(i32::try_from(kbps * 1000).unwrap_or(i32::MAX))
}

impl Inner {
    fn emit(&self, event: VoiceEvent) {
        drop(self.events.send(event));
    }

    /// Updates the state and emits it if it changed (emitted under the lock to keep order).
    fn set_state(&self, new: VoiceState) {
        let mut state = self.state.lock();
        if *state != new {
            *state = new.clone();
            self.emit(VoiceEvent::State(new));
        }
    }

    fn force_state(&self, new: VoiceState) {
        let mut state = self.state.lock();
        *state = new.clone();
        self.emit(VoiceEvent::State(new));
    }

    fn new_driver(self: &Arc<Self>) -> DriverSlot {
        let config = Config::default().decode_mode(DecodeMode::Decode(DecodeConfig::new(
            Channels::Stereo,
            SampleRate::Hz48000,
        )));
        let mut driver = Driver::new(config);
        let handler = Handler(Arc::downgrade(self));
        for event in [
            CoreEvent::VoiceTick,
            CoreEvent::SpeakingStateUpdate,
            CoreEvent::ClientDisconnect,
            CoreEvent::DriverConnect,
            CoreEvent::DriverReconnect,
            CoreEvent::DriverDisconnect,
        ] {
            driver.add_global_event(Event::Core(event), handler.clone());
        }
        driver.set_bitrate(bitrate(self.bitrate_kbps.load(SeqCst)));
        driver.mute(self.send_blocked());
        DriverSlot {
            driver,
            track: None,
            track_playing: false,
        }
    }

    /// Adds the (paused) microphone track if the driver has none.
    fn ensure_track(&self, slot: &mut DriverSlot) {
        if slot.track.is_none() {
            slot.track = Some(
                slot.driver
                    .play(Track::new(self.microphone_input()).pause()),
            );
            slot.track_playing = false;
        }
    }

    fn microphone_input(&self) -> Input {
        let raw = || -> Input {
            RawAdapter::new(
                CaptureSource::new(self.audio.capture_frames()),
                SAMPLE_RATE,
                1,
            )
            .into()
        };
        // Parse now: songbird would otherwise probe the input on its first play, dropping the
        // start of the first thing the user says.
        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            return raw();
        };
        match raw().make_playable(get_codec_registry(), get_probe(), &handle) {
            Ok(input) => input,
            Err(e) => {
                tracing::warn!(error = %e, "could not pre-parse microphone input");
                raw()
            }
        }
    }

    fn send_blocked(&self) -> bool {
        self.self_mute.load(SeqCst) || self.self_deaf.load(SeqCst)
    }

    fn apply_mute(&self) {
        let blocked = self.send_blocked();
        self.audio.set_muted(blocked);
        if let Some(slot) = self.driver.lock().as_mut() {
            slot.driver.mute(blocked);
        }
    }

    fn should_send(&self) -> bool {
        matches!(*self.state.lock(), VoiceState::Connected { .. })
            && !self.send_blocked()
            && self.audio.voice_active()
    }

    /// Plays or pauses the microphone track; pausing makes songbird send its trailing silence
    /// frames and a not-speaking update.
    fn sync_track(&self, playing: bool) {
        let mut guard = self.driver.lock();
        let Some(slot) = guard.as_mut() else { return };
        if slot.track.is_none() || slot.track_playing == playing {
            return;
        }
        let result = slot
            .track
            .as_ref()
            .map(|t| if playing { t.play() } else { t.pause() });
        if matches!(result, Some(Err(_))) {
            // The mixer dropped the track (e.g. after an input error); start a fresh one.
            tracing::warn!("microphone track ended unexpectedly; recreating it");
            slot.track = None;
            self.ensure_track(slot);
            if playing && let Some(track) = &slot.track {
                drop(track.play());
            }
        }
        slot.track_playing = playing;
    }

    fn release_audio(&self) {
        self.audio.stop_capture();
        self.audio.stop_playback();
    }

    fn clear_receive(&self) {
        let stopped = {
            let mut rx = self.rx.lock();
            rx.ssrcs.clear();
            rx.speaking.clear()
        };
        for user_id in stopped {
            self.emit(VoiceEvent::Speaking {
                user_id,
                speaking: false,
            });
        }
    }

    /// Terminal failure: stop wanting the connection, release devices, report why.
    fn fail(&self, message: String) {
        self.generation.fetch_add(1, SeqCst);
        *self.session.lock() = None;
        tracing::warn!(%message, "voice connection failed");
        self.release_audio();
        self.clear_receive();
        self.set_state(VoiceState::Failed(message.clone()));
        self.emit(VoiceEvent::Error(message));
    }

    fn on_driver_connected(&self, channel_id: Id) {
        if self.session.lock().is_none() {
            return;
        }
        let was_connected = matches!(*self.state.lock(), VoiceState::Connected { channel_id: c } if c == channel_id);
        self.set_state(VoiceState::Connected { channel_id });
        if !was_connected {
            self.emit(VoiceEvent::Ping(None));
        }
    }

    fn on_driver_disconnected(&self, kind: DisconnectKind, reason: Option<DisconnectReason>) {
        // Our own `leave()`; `disconnect()` already reported Idle.
        if reason == Some(DisconnectReason::Requested) || self.session.lock().is_none() {
            return;
        }
        self.fail(describe_disconnect(kind, reason));
    }

    fn on_speaking_update(&self, ssrc: u32, user: Option<Id>) {
        if let Some(user) = user {
            self.rx.lock().ssrcs.insert(ssrc, user);
        }
    }

    fn on_user_left(&self, user_id: Id) {
        let was_speaking = {
            let mut rx = self.rx.lock();
            rx.ssrcs.remove_user(user_id);
            rx.speaking.remove(user_id)
        };
        if was_speaking {
            self.emit(VoiceEvent::Speaking {
                user_id,
                speaking: false,
            });
        }
        self.emit(VoiceEvent::UserLeft { user_id });
    }

    /// Mixes one 20 ms tick of decoded stereo PCM (keyed by SSRC) into playback and updates
    /// speaking indicators.
    fn on_voice_tick<'a>(&self, decoded: impl IntoIterator<Item = (u32, &'a [i16])>) {
        let deafened = self.self_deaf.load(SeqCst);
        let changes = {
            let volumes = self.user_volumes.read();
            let mut rx = self.rx.lock();
            let ReceiveState {
                ssrcs,
                speaking,
                mix,
                writer,
            } = &mut *rx;
            let mut heard = Vec::new();
            let mut sources = Vec::new();
            for (ssrc, pcm) in decoded {
                let user = ssrcs.user(ssrc);
                if let Some(user) = user
                    && is_audible(pcm)
                {
                    heard.push(user);
                }
                let volume = user.and_then(|u| volumes.get(&u).copied()).unwrap_or(1.0);
                sources.push((pcm, volume));
            }
            mix_tick(mix, sources, deafened);
            writer.write(mix);
            speaking.tick(&heard)
        };
        for (user_id, speaking) in changes {
            self.emit(VoiceEvent::Speaking { user_id, speaking });
        }
    }
}

fn describe_disconnect(kind: DisconnectKind, reason: Option<DisconnectReason>) -> String {
    let what = match kind {
        DisconnectKind::Connect => "could not connect to voice",
        DisconnectKind::Reconnect => "voice reconnection failed",
        DisconnectKind::Runtime => "disconnected from voice",
        _ => "voice connection lost",
    };
    let why = match reason {
        None => "unknown reason".to_owned(),
        Some(DisconnectReason::WsClosed(Some(code))) => {
            format!("closed by server: {code:?} ({})", code as u16)
        }
        Some(DisconnectReason::WsClosed(None)) => "connection closed".to_owned(),
        Some(DisconnectReason::TimedOut) => "timed out".to_owned(),
        Some(DisconnectReason::Io) => "network error".to_owned(),
        Some(DisconnectReason::ProtocolViolation) => "protocol error".to_owned(),
        Some(other) => format!("{other:?}"),
    };
    format!("{what}: {why}")
}

async fn gate_loop(inner: Weak<Inner>) {
    let mut interval = tokio::time::interval(VoiceManager::GATE_POLL);
    interval.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let mut sending = false;
    loop {
        interval.tick().await;
        let Some(inner) = inner.upgrade() else { break };
        let send = inner.should_send();
        inner.sync_track(send);
        if send != sending {
            sending = send;
            inner.emit(VoiceEvent::SelfSpeaking(send));
        }
    }
}

#[derive(Clone)]
struct Handler(Weak<Inner>);

#[async_trait]
impl EventHandler for Handler {
    async fn act(&self, ctx: &EventContext<'_>) -> Option<Event> {
        let inner = self.0.upgrade()?;
        match ctx {
            EventContext::VoiceTick(tick) => inner.on_voice_tick(
                tick.speaking
                    .iter()
                    .filter_map(|(ssrc, data)| Some((*ssrc, data.decoded_voice.as_deref()?))),
            ),
            EventContext::SpeakingStateUpdate(update) => {
                inner.on_speaking_update(update.ssrc, update.user_id.map(|u| Id(u.0)));
            }
            EventContext::ClientDisconnect(left) => inner.on_user_left(Id(left.user_id.0)),
            EventContext::DriverConnect(data) | EventContext::DriverReconnect(data) => {
                inner.on_driver_connected(Id(data.channel_id.0.get()));
            }
            EventContext::DriverDisconnect(data) => {
                inner.on_driver_disconnected(data.kind, data.reason)
            }
            _ => {}
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use std::pin::pin;
    use std::task::{Context, Poll, Waker};

    use dn_audio::AudioConfig;
    use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};

    use super::*;

    fn info() -> VoiceConnectInfo {
        VoiceConnectInfo {
            guild_id: Id(1),
            channel_id: Id(2),
            user_id: Id(3),
            session_id: "session".into(),
            endpoint: "wss://example.discord.media:443/".into(),
            token: "super-secret-token".into(),
        }
    }

    fn manager() -> (VoiceManager, UnboundedReceiver<VoiceEvent>, AudioEngine) {
        let audio = AudioEngine::new(AudioConfig::default());
        let (tx, rx) = unbounded_channel();
        (VoiceManager::new(audio.clone(), tx), rx, audio)
    }

    fn drain(rx: &mut UnboundedReceiver<VoiceEvent>) -> Vec<VoiceEvent> {
        std::iter::from_fn(|| rx.try_recv().ok()).collect()
    }

    #[test]
    fn debug_redacts_token() {
        let text = format!("{:?}", info());
        assert!(!text.contains("super-secret-token"));
        assert!(text.contains("<redacted>") && text.contains("session"));
    }

    #[test]
    fn connect_info_conversion() {
        let conn = info().to_songbird().unwrap();
        assert_eq!(conn.endpoint, "example.discord.media:443");
        assert_eq!(conn.guild_id.0.get(), 1);
        assert_eq!(conn.channel_id.0.get(), 2);
        assert_eq!(conn.user_id.0.get(), 3);

        let mut bad = info();
        bad.guild_id = Id(0);
        assert!(matches!(bad.to_songbird(), Err(VoiceError::Connection(_))));
        let mut bad = info();
        bad.endpoint = "wss://".into();
        assert!(bad.to_songbird().is_err());
    }

    #[test]
    fn connect_outside_runtime_is_rejected() {
        let (vm, _rx, audio) = manager();
        let mut fut = pin!(vm.connect(info()));
        let mut cx = Context::from_waker(Waker::noop());
        assert!(matches!(
            fut.as_mut().poll(&mut cx),
            Poll::Ready(Err(VoiceError::NotInRuntime))
        ));
        assert_eq!(vm.state(), VoiceState::Idle);
        audio.shutdown();
    }

    #[tokio::test]
    async fn controls_work_without_a_connection() {
        let (vm, mut rx, audio) = manager();
        assert_eq!(vm.state(), VoiceState::Idle);
        vm.set_self_mute(true);
        vm.set_self_deaf(true);
        vm.set_self_deaf(false);
        vm.set_user_volume(Id(9), 5.0);
        vm.set_user_volume(Id(10), f32::NAN);
        vm.set_bitrate(1_000_000);
        assert_eq!(vm.inner.bitrate_kbps.load(SeqCst), 512);
        assert_eq!(vm.inner.user_volumes.read()[&Id(9)], 2.0);
        assert_eq!(vm.inner.user_volumes.read()[&Id(10)], 1.0);
        assert!(vm.speaking_users().is_empty());

        vm.disconnect().await;
        assert_eq!(vm.state(), VoiceState::Idle);
        let events = drain(&mut rx);
        assert!(
            matches!(events.as_slice(), [VoiceEvent::State(VoiceState::Idle)]),
            "{events:?}"
        );

        // Muted: the gate loop must never report transmission.
        tokio::time::sleep(Duration::from_millis(60)).await;
        assert!(
            drain(&mut rx)
                .iter()
                .all(|e| !matches!(e, VoiceEvent::SelfSpeaking(true)))
        );
        audio.shutdown();
    }

    #[tokio::test]
    async fn microphone_input_is_parsed_up_front() {
        let (vm, _rx, audio) = manager();
        assert!(vm.inner.microphone_input().is_playable());
        audio.shutdown();
    }

    #[tokio::test]
    async fn invalid_connect_info_does_not_change_state() {
        let (vm, mut rx, audio) = manager();
        let mut bad = info();
        bad.token.clear();
        assert!(matches!(
            vm.connect(bad).await,
            Err(VoiceError::Connection(_))
        ));
        assert_eq!(vm.state(), VoiceState::Idle);
        assert!(drain(&mut rx).is_empty());
        audio.shutdown();
    }

    #[tokio::test]
    async fn receive_path_maps_ssrcs_volumes_and_speaking() {
        let (vm, mut rx, audio) = manager();
        let inner = &vm.inner;
        let playback = audio.playback_writer();
        inner.on_speaking_update(111, Some(Id(7)));
        inner.on_speaking_update(222, None);
        vm.set_user_volume(Id(7), 0.5);

        let loud = vec![16_384i16; TICK_SAMPLES];
        let stranger = vec![8_192i16; TICK_SAMPLES];
        inner.on_voice_tick([(111, &loud[..]), (333, &stranger[..])]);
        assert_eq!(vm.speaking_users(), vec![Id(7)]);
        // 0.5 * 0.5 (user 7 at half volume) + 0.25 (unknown SSRC at 1.0) = 0.5.
        assert!(inner.rx.lock().mix.iter().all(|s| (s - 0.5).abs() < 1e-4));
        assert!(playback.buffered_ms() >= 19.9);

        vm.set_self_deaf(true);
        inner.on_voice_tick([(111, &loud[..])]);
        assert!(
            inner.rx.lock().mix.iter().all(|&s| s == 0.0),
            "deafened output is silent"
        );
        vm.set_self_deaf(false);

        inner.on_user_left(Id(7));
        assert!(vm.speaking_users().is_empty());
        assert_eq!(
            inner.rx.lock().ssrcs.user(111),
            None,
            "SSRC forgotten on ClientDisconnect"
        );

        let events = drain(&mut rx);
        let speaking: Vec<_> = events
            .iter()
            .filter_map(|e| match e {
                VoiceEvent::Speaking { user_id, speaking } => Some((*user_id, *speaking)),
                _ => None,
            })
            .collect();
        assert_eq!(speaking, vec![(Id(7), true), (Id(7), false)]);
        assert!(
            events
                .iter()
                .any(|e| matches!(e, VoiceEvent::UserLeft { user_id } if *user_id == Id(7)))
        );
        audio.shutdown();
    }

    /// Exercises the real songbird driver: an unreachable endpoint must end in `Failed` with
    /// an error event (songbird retries ~5 times with backoff first, so this takes seconds).
    #[tokio::test]
    async fn unreachable_server_fails_cleanly() {
        let (vm, mut rx, audio) = manager();
        let mut target = info();
        target.endpoint = "127.0.0.1:9".into();
        let result = tokio::time::timeout(Duration::from_secs(60), vm.connect(target)).await;
        assert!(
            matches!(result, Ok(Err(VoiceError::Connection(_)))),
            "{result:?}"
        );
        assert!(matches!(vm.state(), VoiceState::Failed(_)));

        tokio::time::sleep(Duration::from_millis(200)).await;
        let events = drain(&mut rx);
        assert!(
            matches!(
                events.first(),
                Some(VoiceEvent::State(VoiceState::Connecting))
            ),
            "{events:?}"
        );
        let errors = events
            .iter()
            .filter(|e| matches!(e, VoiceEvent::Error(_)))
            .count();
        assert_eq!(errors, 1, "{events:?}");

        vm.disconnect().await;
        assert_eq!(vm.state(), VoiceState::Idle);
        audio.shutdown();
    }

    #[test]
    fn disconnect_reasons_are_readable() {
        let msg = describe_disconnect(
            DisconnectKind::Runtime,
            Some(DisconnectReason::WsClosed(Some(
                songbird::model::CloseCode::Disconnected,
            ))),
        );
        assert_eq!(
            msg,
            "disconnected from voice: closed by server: Disconnected (4014)"
        );
        assert_eq!(
            describe_disconnect(DisconnectKind::Connect, None),
            "could not connect to voice: unknown reason"
        );
    }
}
