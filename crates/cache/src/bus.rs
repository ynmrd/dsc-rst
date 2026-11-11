//! UI ⇄ backend message bus. The UI sends `Command`s (never blocks); the backend applies
//! state changes to the shared `Store` and sends one-shot `UiEvent`s for transient results.

use std::path::PathBuf;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use dn_core::{Emoji, Id, Member, Message, Status, User};

use crate::SharedStore;

pub type SharedSettings = Arc<parking_lot::RwLock<dn_settings::Settings>>;

#[derive(Debug, Clone)]
pub enum Command {
    // Accounts
    AddBotAccount { token: String },
    BeginOAuth { client_id: String, client_secret: Option<String>, port: u16 },
    CancelOAuth,
    UnlinkOAuth,
    SwitchAccount(String),
    RemoveAccount(String),
    Reconnect,
    // Navigation and messaging
    OpenChannel { channel_id: Id },
    LoadOlder { channel_id: Id, before: Id },
    JumpToPresent { channel_id: Id },
    JumpToMessage { channel_id: Id, message_id: Id },
    SendMessage { channel_id: Id, content: String, reply_to: Option<Id>, mention_reply: bool, files: Vec<PathBuf>, sticker_ids: Vec<Id> },
    RetrySend { channel_id: Id, nonce: String },
    DiscardPending { channel_id: Id, nonce: String },
    EditMessage { channel_id: Id, message_id: Id, content: String },
    DeleteMessage { channel_id: Id, message_id: Id },
    React { channel_id: Id, message_id: Id, emoji: Emoji, add: bool },
    Pin { channel_id: Id, message_id: Id, pin: bool },
    LoadPins { channel_id: Id },
    Typing { channel_id: Id },
    MarkRead { channel_id: Id, message_id: Id },
    Search(SearchRequest),
    OpenDm { user_id: Id },
    CreateThread { channel_id: Id, message_id: Id, name: String },
    DownloadTo { url: String, path: PathBuf },
    FetchProfile { user_id: Id, guild_id: Option<Id> },
    RequestMembers { guild_id: Id },
    SetPresence { status: Status, custom: Option<String> },
    // Voice
    JoinVoice { guild_id: Id, channel_id: Id },
    LeaveVoice,
    SetMute(bool),
    SetDeaf(bool),
    SetUserVolume { user_id: Id, volume: f32 },
    PushToTalk(bool),
    MicTest(bool),
    RefreshAudioDevices,
    // App
    SettingsChanged,
    FocusChanged { window_focused: bool },
    Shutdown,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct SearchRequest {
    pub query: String,
    pub guild_id: Option<Id>,
    pub channel_id: Option<Id>,
    pub author_id: Option<Id>,
    pub offset: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchSource {
    /// Discord's server-side guild search.
    Discord,
    /// Local SQLite index of messages this client has seen.
    Local,
}

#[derive(Debug, Clone)]
pub enum UiEvent {
    Toast { text: String, error: bool },
    SearchResults { request: SearchRequest, total: u64, messages: Vec<Message>, source: SearchSource, note: Option<String> },
    Pins { channel_id: Id, messages: Vec<Message> },
    Profile { user: User, member: Option<Member>, guild_id: Option<Id> },
    OpenChannel(Id),
    OAuthFinished(Result<String, String>),
    AccountAdded(Result<String, String>),
    RequestAttention,
}

/// Repaint hook installed by the UI once the egui context exists.
#[derive(Clone, Default)]
pub struct Repaint(Arc<OnceLock<Box<dyn Fn(Duration) + Send + Sync>>>);

impl Repaint {
    pub fn install(&self, f: impl Fn(Duration) + Send + Sync + 'static) {
        let _ = self.0.set(Box::new(f));
    }
    pub fn now(&self) {
        self.after(Duration::ZERO);
    }
    pub fn after(&self, d: Duration) {
        if let Some(f) = self.0.get() {
            f(d);
        }
    }
}

impl std::fmt::Debug for Repaint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Repaint")
    }
}

/// Everything the UI needs to talk to the backend.
#[derive(Clone)]
pub struct UiLink {
    pub store: SharedStore,
    pub settings: SharedSettings,
    pub commands: tokio::sync::mpsc::UnboundedSender<Command>,
    pub events: crossbeam_channel::Receiver<UiEvent>,
    pub repaint: Repaint,
}

impl UiLink {
    pub fn send(&self, cmd: Command) {
        if self.commands.send(cmd).is_err() {
            tracing::warn!("backend is gone; command dropped");
        }
    }
}

/// Backend side of the bus.
#[derive(Clone)]
pub struct BackendLink {
    pub store: SharedStore,
    pub settings: SharedSettings,
    pub events: crossbeam_channel::Sender<UiEvent>,
    pub repaint: Repaint,
}

impl BackendLink {
    pub fn emit(&self, ev: UiEvent) {
        // Bounded: if the UI is hidden and not draining, drop rather than grow.
        let _ = self.events.try_send(ev);
        self.repaint.now();
    }

    pub fn toast(&self, text: impl Into<String>, error: bool) {
        self.emit(UiEvent::Toast { text: text.into(), error });
    }
}

/// Create both ends of the bus.
pub fn link(store: SharedStore, settings: SharedSettings) -> (UiLink, BackendLink, tokio::sync::mpsc::UnboundedReceiver<Command>) {
    let (cmd_tx, cmd_rx) = tokio::sync::mpsc::unbounded_channel();
    let (ev_tx, ev_rx) = crossbeam_channel::bounded(512);
    let repaint = Repaint::default();
    let ui = UiLink { store: store.clone(), settings: settings.clone(), commands: cmd_tx, events: ev_rx, repaint: repaint.clone() };
    let backend = BackendLink { store, settings, events: ev_tx, repaint };
    (ui, backend, cmd_rx)
}
