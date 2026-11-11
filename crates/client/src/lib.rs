//! Backend orchestrator. Owns the Discord session (REST + gateway), the local database,
//! voice, notifications and accounts, and services the UI's `Command`s. All network,
//! database and audio work runs here, off the UI thread.

mod commands;
pub mod desktop;
mod voice_glue;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::{mpsc, oneshot};

use dn_api::{Auth, RestClient};
use dn_auth::{AccountManager, AccountRecord, SecretStore};
use dn_cache::bus::{BackendLink, Command, UiEvent};
use dn_cache::{AccountView, ConnStatus, LinkedUser, Store};
use dn_core::{Dispatch, Id, Message, Status, User};
use dn_gateway::{ConnectionState, FatalError, GatewayConfig, GatewayEvent, GatewayHandle, Intents, PresenceUpdate, Timings};
use dn_settings::{Paths, Settings};
use dn_storage::{Storage, StorageHandle};

pub use desktop::set_autostart;

/// Where the backend talks to. Production uses Discord; tests point it at local mocks.
pub struct BackendConfig {
    pub paths: Paths,
    pub api_base: String,
    /// Skip `GET /gateway/bot` and connect here (tests).
    pub gateway_url: Option<String>,
    pub gateway_timings: Option<Timings>,
    pub secret_store: Arc<dyn SecretStore>,
    pub notifications: bool,
    pub audio: bool,
    pub oauth_api_base: String,
    /// Shown once at startup (e.g. credential store unavailable).
    pub startup_warning: Option<String>,
}

impl BackendConfig {
    pub fn production(paths: Paths) -> Self {
        let keyring = dn_auth::KeyringStore::new("DiscordNative");
        // Never fall back to writing tokens to disk: without a working OS store they are
        // kept in memory for this session only.
        let (secret_store, startup_warning): (Arc<dyn SecretStore>, _) = match keyring.get("probe") {
            Ok(_) => (Arc::new(keyring), None),
            Err(e) => {
                tracing::error!("credential store unavailable: {e}");
                (
                    Arc::new(dn_auth::MemoryStore::default()),
                    Some(format!("The system credential store is unavailable ({e}). Tokens will be forgotten when you close the app.")),
                )
            }
        };
        BackendConfig {
            paths,
            api_base: dn_core::API_BASE.to_string(),
            gateway_url: None,
            gateway_timings: None,
            secret_store,
            notifications: true,
            audio: true,
            oauth_api_base: dn_core::API_BASE.to_string(),
            startup_warning,
        }
    }
}

/// Results of background tasks that need the backend's state.
pub(crate) enum Internal {
    StartActiveSession,
    AccountsChanged,
    AccountAdded(Result<AccountRecord, String>),
    OAuthDone(Result<String, String>),
    Linked(Option<LinkedUser>),
    SendFailed { channel: Id, nonce: String, error: String },
    NotificationClicked(Id),
    AudioEvent(String, bool),
    VoiceTimeout(u64),
    VoiceConnectFailed(String),
}

pub(crate) struct Session {
    pub account_id: String,
    pub rest: RestClient,
    pub gateway: Option<GatewayHandle>,
    pub intents: Intents,
}

pub(crate) struct PendingSend {
    pub channel: Id,
    pub content: String,
    pub reply_to: Option<Id>,
    pub mention_reply: bool,
    pub files: Vec<std::path::PathBuf>,
    pub sticker_ids: Vec<Id>,
}

pub(crate) struct Backend {
    pub cfg: BackendConfig,
    pub link: BackendLink,
    pub accounts: Arc<tokio::sync::Mutex<AccountManager>>,
    pub storage: Option<StorageHandle>,
    pub session: Option<Session>,
    pub session_gen: u64,
    pub gw_tx: mpsc::UnboundedSender<(u64, GatewayEvent)>,
    pub internal: mpsc::UnboundedSender<Internal>,
    pub audio: Option<dn_audio::AudioEngine>,
    pub voice: Option<dn_voice::VoiceManager>,
    pub voice_pending: Option<voice_glue::PendingVoice>,
    pub voice_join_gen: u64,
    pub notifier: Option<Arc<dn_notifications::Notifier>>,
    pub window_focused: bool,
    pub oauth_cancel: Option<oneshot::Sender<()>>,
    pub pending_sends: HashMap<String, PendingSend>,
    pub persist_buffer: Vec<Message>,
    pub delete_buffer: Vec<(Id, Vec<Id>)>,
    pub settings_dirty: bool,
    pub last_fetch: HashMap<Id, Instant>,
    pub self_status: Status,
    pub self_custom: Option<String>,
    pub mic_test: bool,
    pub autostart_applied: Option<bool>,
    pub degraded_intents: bool,
}

/// Start the backend on `rt`. It runs until `Command::Shutdown` or the UI side of the bus is dropped.
pub fn start(
    rt: &tokio::runtime::Handle,
    cfg: BackendConfig,
    link: BackendLink,
    commands: mpsc::UnboundedReceiver<Command>,
) -> tokio::task::JoinHandle<()> {
    rt.spawn(async move {
        let (gw_tx, gw_rx) = mpsc::unbounded_channel();
        let (internal_tx, internal_rx) = mpsc::unbounded_channel();
        let (voice_tx, voice_rx) = mpsc::unbounded_channel();
        let mut backend = Backend::new(cfg, link, gw_tx, internal_tx, voice_tx).await;
        backend.run(commands, gw_rx, internal_rx, voice_rx).await;
    })
}

impl Backend {
    async fn new(
        cfg: BackendConfig,
        link: BackendLink,
        gw_tx: mpsc::UnboundedSender<(u64, GatewayEvent)>,
        internal: mpsc::UnboundedSender<Internal>,
        voice_tx: mpsc::UnboundedSender<dn_voice::VoiceEvent>,
    ) -> Self {
        let accounts = match AccountManager::load(cfg.paths.accounts_file(), cfg.secret_store.clone()) {
            Ok(m) => m,
            Err(e) => {
                tracing::error!("accounts unavailable: {e}");
                link.toast(format!("Could not load accounts: {e}"), true);
                // Fall back to an in-memory manager so the app stays usable this session.
                let tmp = cfg.paths.data_dir.join("accounts.unavailable.json");
                AccountManager::load(tmp, Arc::new(dn_auth::MemoryStore::default())).expect("in-memory account manager")
            }
        };
        let storage = match Storage::open(&cfg.paths.database_file()) {
            Ok(s) => Some(StorageHandle::spawn(s)),
            Err(e) => {
                tracing::error!("local database unavailable: {e}");
                link.toast(format!("Local message cache unavailable: {e}"), true);
                None
            }
        };
        let settings = link.settings.read().clone();
        if let Some(w) = &cfg.startup_warning {
            link.toast(w.clone(), true);
        }
        let audio = cfg.audio.then(|| dn_audio::AudioEngine::new(voice_glue::audio_config(&settings)));
        let voice = audio.as_ref().map(|a| dn_voice::VoiceManager::new(a.clone(), voice_tx));
        let notifier = cfg.notifications.then(|| Arc::new(dn_notifications::Notifier::new("Discord Native", None)));

        if let Some(n) = &notifier {
            let rx = n.on_activated();
            let tx = internal.clone();
            std::thread::Builder::new()
                .name("dn-notify-clicks".into())
                .spawn(move || {
                    while let Ok(c) = rx.recv() {
                        if tx.send(Internal::NotificationClicked(c)).is_err() {
                            break;
                        }
                    }
                })
                .ok();
        }
        if let Some(a) = &audio {
            let rx = a.events();
            let tx = internal.clone();
            std::thread::Builder::new()
                .name("dn-audio-events".into())
                .spawn(move || {
                    while let Ok(ev) = rx.recv() {
                        let msg = match ev {
                            dn_audio::AudioEvent::InputLost(d) => (format!("Microphone disconnected: {d}"), true),
                            dn_audio::AudioEvent::OutputLost(d) => (format!("Audio output disconnected: {d}"), true),
                            dn_audio::AudioEvent::Error(e) => (format!("Audio error: {e}"), true),
                            dn_audio::AudioEvent::InputStarted(d) => (format!("Using microphone: {d}"), false),
                            dn_audio::AudioEvent::OutputStarted(d) => (format!("Using output: {d}"), false),
                        };
                        if tx.send(Internal::AudioEvent(msg.0, msg.1)).is_err() {
                            break;
                        }
                    }
                })
                .ok();
        }
        {
            let mut store = link.store.lock();
            store.limits.inactive_channel_messages = settings.advanced.memory_messages_per_channel as usize;
        }
        Backend {
            cfg,
            link,
            accounts: Arc::new(tokio::sync::Mutex::new(accounts)),
            storage,
            session: None,
            session_gen: 0,
            gw_tx,
            internal,
            audio,
            voice,
            voice_pending: None,
            voice_join_gen: 0,
            notifier,
            window_focused: true,
            oauth_cancel: None,
            pending_sends: HashMap::new(),
            persist_buffer: vec![],
            delete_buffer: vec![],
            settings_dirty: false,
            last_fetch: HashMap::new(),
            self_status: Status::Online,
            self_custom: None,
            mic_test: false,
            autostart_applied: None,
            degraded_intents: false,
        }
    }

    async fn run(
        &mut self,
        mut commands: mpsc::UnboundedReceiver<Command>,
        mut gw_rx: mpsc::UnboundedReceiver<(u64, GatewayEvent)>,
        mut internal_rx: mpsc::UnboundedReceiver<Internal>,
        mut voice_rx: mpsc::UnboundedReceiver<dn_voice::VoiceEvent>,
    ) {
        self.refresh_accounts_view().await;
        self.refresh_audio_devices();
        if self.link.settings.read().behavior.start_on_boot {
            self.apply_autostart(true);
        }
        let _ = self.internal.send(Internal::StartActiveSession);

        let mut flush = tokio::time::interval(Duration::from_secs(1));
        flush.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut prune = tokio::time::interval(Duration::from_secs(600));
        prune.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut level = tokio::time::interval(Duration::from_millis(50));
        level.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        loop {
            let level_active = self.mic_test || self.voice_pending.is_some();
            let has_work = !self.persist_buffer.is_empty() || !self.delete_buffer.is_empty() || self.settings_dirty;
            tokio::select! {
                cmd = commands.recv() => match cmd {
                    Some(Command::Shutdown) | None => break,
                    Some(cmd) => self.handle_command(cmd).await,
                },
                Some((generation, ev)) = gw_rx.recv() => {
                    if generation == self.session_gen {
                        self.handle_gateway(ev).await;
                    }
                }
                Some(msg) = internal_rx.recv() => self.handle_internal(msg).await,
                Some(ev) = voice_rx.recv() => self.handle_voice_event(ev),
                _ = flush.tick(), if has_work => self.flush().await,
                _ = prune.tick() => self.prune(),
                _ = level.tick(), if level_active => self.update_level(),
            }
            self.persist_read_states();
        }
        self.shutdown().await;
    }

    async fn shutdown(&mut self) {
        tracing::info!("backend shutting down");
        self.flush().await;
        self.save_settings();
        if let Some(v) = &self.voice {
            v.disconnect().await;
        }
        self.stop_session().await;
        if let Some(a) = &self.audio {
            a.shutdown();
        }
        if let Some(s) = &self.storage {
            s.shutdown();
        }
    }

    pub(crate) fn store(&self) -> parking_lot::MutexGuard<'_, Store> {
        self.link.store.lock()
    }

    pub(crate) fn rest(&self) -> Option<RestClient> {
        self.session.as_ref().map(|s| s.rest.clone())
    }

    pub(crate) fn gateway_send(&self, cmd: dn_gateway::GatewayCommand) -> bool {
        self.session.as_ref().and_then(|s| s.gateway.as_ref()).is_some_and(|g| g.send(cmd))
    }

    async fn handle_internal(&mut self, msg: Internal) {
        match msg {
            Internal::StartActiveSession => self.start_active_session().await,
            Internal::AccountsChanged => self.refresh_accounts_view().await,
            Internal::AccountAdded(r) => match r {
                Ok(rec) => {
                    let name = rec.bot_user.global_name.clone().unwrap_or_else(|| rec.bot_user.username.clone());
                    self.link.emit(UiEvent::AccountAdded(Ok(name)));
                    self.refresh_accounts_view().await;
                    self.start_active_session().await;
                }
                Err(e) => self.link.emit(UiEvent::AccountAdded(Err(e))),
            },
            Internal::OAuthDone(r) => {
                self.oauth_cancel = None;
                if r.is_ok() {
                    self.refresh_accounts_view().await;
                    self.load_linked_identity();
                }
                self.link.emit(UiEvent::OAuthFinished(r));
            }
            Internal::Linked(l) => {
                self.store().session.linked = l;
                self.link.repaint.now();
            }
            Internal::SendFailed { channel, nonce, error } => {
                self.store().channel_messages_mut(channel).fail_pending(&nonce, error);
                self.link.repaint.now();
            }
            Internal::NotificationClicked(c) => self.link.emit(UiEvent::OpenChannel(c)),
            Internal::AudioEvent(text, error) => {
                if error {
                    tracing::warn!("{text}");
                    self.link.toast(text, true);
                } else {
                    tracing::info!("{text}");
                }
                self.refresh_audio_devices();
            }
            Internal::VoiceTimeout(join_gen) => {
                if join_gen == self.voice_join_gen {
                    let still_connecting = matches!(self.store().voice.conn, dn_cache::VoiceConn::Connecting { .. });
                    if still_connecting {
                        self.voice_failed(
                            "Timed out waiting for Discord's voice server. Check that the bot has Connect permission.".into(),
                        )
                        .await;
                    }
                }
            }
            Internal::VoiceConnectFailed(e) => self.voice_failed(e).await,
        }
    }

    // ---------- accounts & sessions ----------

    pub(crate) async fn refresh_accounts_view(&mut self) {
        let mgr = self.accounts.lock().await;
        let active = mgr.active().map(|a| a.id.clone());
        let views: Vec<AccountView> = mgr
            .accounts()
            .iter()
            .map(|a| {
                let u = User { id: a.bot_user.id, avatar: a.bot_user.avatar.clone(), ..Default::default() };
                AccountView {
                    id: a.id.clone(),
                    label: a.label.clone(),
                    bot_name: a.bot_user.global_name.clone().unwrap_or_else(|| a.bot_user.username.clone()),
                    bot_avatar_url: u.avatar_url(64),
                    oauth_user: a.oauth.as_ref().map(|o| o.user.global_name.clone().unwrap_or_else(|| o.user.username.clone())),
                    oauth_expired: a.oauth.as_ref().is_some_and(|o| o.expired),
                    credentials_invalid: a.credentials_invalid,
                    active: Some(&a.id) == active.as_ref(),
                }
            })
            .collect();
        let no_usable = active.is_none() || views.iter().any(|v| v.active && v.credentials_invalid);
        drop(mgr);
        let mut store = self.store();
        store.session.accounts = views;
        store.session.needs_onboarding = no_usable;
        drop(store);
        self.link.repaint.now();
    }

    pub(crate) async fn stop_session(&mut self) {
        self.flush().await;
        if self.voice_pending.take().is_some()
            && let Some(v) = &self.voice
        {
            v.disconnect().await;
        }
        if let Some(s) = self.session.take()
            && let Some(g) = s.gateway
        {
            // Bounded: a hung socket must not block account switching or exit.
            let _ = tokio::time::timeout(Duration::from_secs(3), g.shutdown()).await;
        }
        self.session_gen += 1;
        self.pending_sends.clear();
        self.last_fetch.clear();
        let mut store = self.store();
        store.reset_session_data();
        store.session.conn = ConnStatus::Offline;
    }

    pub(crate) async fn start_active_session(&mut self) {
        self.stop_session().await;
        let (account_id, token) = {
            let mgr = self.accounts.lock().await;
            let Some(active) = mgr.active().filter(|a| !a.credentials_invalid).cloned() else {
                drop(mgr);
                self.refresh_accounts_view().await;
                return;
            };
            let token = mgr.bot_token(&active.id);
            (active.id, token)
        };
        let token = match token {
            Ok(Some(t)) => t,
            Ok(None) => {
                self.link.toast("The saved bot token is missing from the credential store. Please add the account again.", true);
                let _ = self.accounts.lock().await.mark_credentials_invalid(&account_id, true);
                self.refresh_accounts_view().await;
                return;
            }
            Err(e) => {
                self.link.toast(format!("Could not read the credential store: {e}"), true);
                return;
            }
        };
        let rest = match RestClient::with_base_url(Auth::Bot(token.clone()), &self.cfg.api_base) {
            Ok(r) => r,
            Err(e) => {
                self.link.toast(format!("Invalid API configuration: {e}"), true);
                return;
            }
        };
        self.refresh_accounts_view().await;
        self.restore_local_state().await;

        let settings = self.link.settings.read().clone();
        let mut intents = Intents::default_client();
        if !settings.advanced.intent_members {
            intents = intents & !Intents::GUILD_MEMBERS;
        }
        if !settings.advanced.intent_presences {
            intents = intents & !Intents::GUILD_PRESENCES;
        }
        if !settings.advanced.intent_message_content {
            intents = intents & !Intents::MESSAGE_CONTENT;
        }
        if self.degraded_intents {
            intents = intents.without_privileged();
        }
        let url = match &self.cfg.gateway_url {
            Some(u) => Some(u.clone()),
            None => match rest.gateway_bot().await {
                Ok(g) => {
                    if g.session_start_limit.remaining == 0 {
                        let wait = g.session_start_limit.reset_after;
                        self.link.toast(
                            format!("Discord's daily connection limit for this bot is used up; it resets in {} min.", wait / 60_000 + 1),
                            true,
                        );
                    }
                    Some(g.url)
                }
                Err(dn_api::ApiError::Unauthorized) => {
                    self.credentials_rejected(&account_id).await;
                    return;
                }
                Err(e) => {
                    tracing::warn!("GET /gateway/bot failed: {e}; using the default gateway URL");
                    None
                }
            },
        };
        let mut gw = GatewayConfig::new(token, intents);
        gw.url = url;
        gw.compress = settings.advanced.gateway_compression;
        gw.presence = Some(PresenceUpdate { status: self.self_status, activity: self.self_custom.clone(), afk: false });
        if let Some(t) = self.cfg.gateway_timings {
            gw.timings = t;
        }
        self.session_gen += 1;
        let generation = self.session_gen;
        let (tx, mut rx) = mpsc::unbounded_channel();
        let handle = dn_gateway::spawn(gw, tx);
        let fwd = self.gw_tx.clone();
        tokio::spawn(async move {
            while let Some(ev) = rx.recv().await {
                if fwd.send((generation, ev)).is_err() {
                    break;
                }
            }
        });
        self.session = Some(Session { account_id, rest, gateway: Some(handle), intents });
        self.store().session.conn = ConnStatus::Connecting;
        self.link.repaint.now();
        self.load_linked_identity();
    }

    /// Restore DMs and read states persisted from previous runs (bots can't list DMs).
    async fn restore_local_state(&mut self) {
        let Some(storage) = &self.storage else { return };
        let res = storage.call(|s| (s.private_channels(), s.read_states())).await;
        if let Ok((Ok(dms), Ok(reads))) = res {
            let mut store = self.store();
            store.load_read_states(reads);
            for c in dms {
                store.upsert_channel(c);
            }
            store.take_dirty_read_states();
        }
    }

    /// Fetch the OAuth-linked identity (profile + servers) in the background.
    pub(crate) fn load_linked_identity(&self) {
        let Some(account_id) = self.session.as_ref().map(|s| s.account_id.clone()) else { return };
        let accounts = self.accounts.clone();
        let internal = self.internal.clone();
        let api_base = self.cfg.oauth_api_base.clone();
        let link = self.link.clone();
        tokio::spawn(async move {
            let tokens = accounts.lock().await.oauth_tokens(&account_id).await;
            let tokens = match tokens {
                Ok(Some(t)) => t,
                Ok(None) => {
                    let _ = internal.send(Internal::Linked(None));
                    return;
                }
                Err(e) => {
                    link.toast(format!("Discord sign-in: {e}"), true);
                    let _ = internal.send(Internal::Linked(None));
                    let _ = internal.send(Internal::AccountsChanged);
                    return;
                }
            };
            let Ok(rest) = RestClient::with_base_url(Auth::Bearer(tokens.access_token.clone()), &api_base) else { return };
            match rest.current_user().await {
                Ok(user) => {
                    let guilds = rest.current_user_guilds().await.unwrap_or_default();
                    let _ = internal.send(Internal::Linked(Some(LinkedUser { user, guilds })));
                }
                Err(e) => tracing::warn!("fetching linked identity failed: {e}"),
            }
        });
    }

    async fn credentials_rejected(&mut self, account_id: &str) {
        let _ = self.accounts.lock().await.mark_credentials_invalid(account_id, true);
        {
            let mut store = self.store();
            store.session.conn =
                ConnStatus::Fatal("Discord rejected the bot token. Reset it in the Developer Portal and add the account again.".into());
        }
        self.refresh_accounts_view().await;
        self.link.toast("Discord rejected the bot token.", true);
    }

    // ---------- gateway ----------

    async fn handle_gateway(&mut self, ev: GatewayEvent) {
        match ev {
            GatewayEvent::State(s) => {
                let conn = match s {
                    ConnectionState::Connecting | ConnectionState::Identifying | ConnectionState::Resuming => ConnStatus::Connecting,
                    ConnectionState::Connected => ConnStatus::Connected,
                    ConnectionState::Reconnecting { attempt, delay } => ConnStatus::Reconnecting { attempt, delay_secs: delay.as_secs() },
                    ConnectionState::Disconnected => ConnStatus::Offline,
                };
                let mut store = self.store();
                if !matches!(store.session.conn, ConnStatus::Fatal(_)) || conn == ConnStatus::Connected {
                    store.session.conn = conn;
                }
                store.session.intents_degraded = self.degraded_intents;
            }
            GatewayEvent::Latency(d) => self.store().session.latency_ms = Some(d.as_millis() as u32),
            GatewayEvent::Dispatch(d) => self.on_dispatch(*d).await,
            GatewayEvent::Fatal(f) => {
                tracing::error!("gateway fatal: {f:?}");
                match f {
                    FatalError::DisallowedIntents if !self.degraded_intents => {
                        self.degraded_intents = true;
                        self.link.toast(
                            "Discord refused the privileged intents (Message Content / Members / Presence). Reconnecting without them — enable them in the Developer Portal for full functionality.",
                            true,
                        );
                        let _ = self.internal.send(Internal::StartActiveSession);
                    }
                    FatalError::AuthenticationFailed => {
                        if let Some(id) = self.session.as_ref().map(|s| s.account_id.clone()) {
                            self.credentials_rejected(&id).await;
                        }
                    }
                    other => {
                        let text = format!("Gateway error: {other}");
                        self.store().session.conn = ConnStatus::Fatal(text.clone());
                        self.link.toast(text, true);
                    }
                }
            }
            GatewayEvent::Warning(w) => tracing::warn!("gateway: {w}"),
        }
        self.link.repaint.now();
    }

    async fn on_dispatch(&mut self, d: Dispatch) {
        // Apply to the store first so side effects see the updated state.
        let me = {
            let mut store = self.store();
            store.apply(&d);
            store.me()
        };
        match &d {
            Dispatch::Ready(r) => {
                tracing::info!(user = %r.user.tag(), guilds = r.guilds.len(), "gateway ready");
                if let Some(s) = &self.session {
                    let name = r.user.username.clone();
                    let id = s.account_id.clone();
                    let mut mgr = self.accounts.lock().await;
                    if mgr.account(&id).is_some_and(|a| a.label.is_empty()) {
                        let _ = mgr.update_label(&id, &name);
                    }
                }
            }
            Dispatch::MessageCreate(m) => {
                self.persist_buffer.push((**m).clone());
                if m.guild_id.is_none() {
                    let ch = self.store().channels.get(&m.channel_id).cloned();
                    if let (Some(ch), Some(storage)) = (ch, &self.storage) {
                        let _ = storage.call(move |s| s.upsert_private_channel(&ch)).await;
                    }
                }
                if Some(m.author.id) != me {
                    self.maybe_notify(m);
                }
            }
            Dispatch::MessageUpdate(u) => {
                let updated = self.store().channel_messages(u.channel_id).and_then(|cm| cm.get(u.id).cloned());
                if let Some(m) = updated {
                    self.persist_buffer.push(m);
                }
            }
            Dispatch::MessageDelete(e) => self.delete_buffer.push((e.channel_id, vec![e.id])),
            Dispatch::MessageDeleteBulk(e) => self.delete_buffer.push((e.channel_id, e.ids.clone())),
            Dispatch::ChannelCreate(c) if c.kind.is_private() => {
                let ch = (**c).clone();
                if let Some(storage) = &self.storage {
                    let _ = storage.call(move |s| s.upsert_private_channel(&ch)).await;
                }
            }
            Dispatch::VoiceStateUpdate(v) if Some(v.user_id) == me => self.on_own_voice_state(v).await,
            Dispatch::VoiceServerUpdate(v) => self.on_voice_server(v),
            _ => {}
        }
    }

    fn maybe_notify(&self, m: &Message) {
        let Some(notifier) = &self.notifier else { return };
        let settings = self.link.settings.read().clone();
        if !settings.notifications.enabled {
            return;
        }
        let (ctx_owned, author, location, mention) = {
            let store = self.store();
            let Some(me) = store.me() else { return };
            let is_private = m.guild_id.is_none();
            let level = if is_private {
                settings.notifications.dm_level
            } else {
                settings.notify_level_for(m.guild_id.map(|g| g.0), m.channel_id.0)
            };
            let roles = m.guild_id.and_then(|g| store.member(g, me)).map(|mm| mm.roles.clone()).unwrap_or_default();
            let viewing = self.window_focused && store.active_channel == Some(m.channel_id);
            (
                (me, roles, map_level(level), settings.is_muted(m.guild_id.map(|g| g.0), m.channel_id.0), is_private, viewing),
                store.display_name(m.guild_id, m.author.id),
                store.location_label(m.channel_id),
                store.is_mention(m),
            )
        };
        let (me, roles, level, muted, is_private, viewing) = ctx_owned;
        let ctx = dn_notifications::NotifyContext {
            self_user_id: me,
            self_role_ids: &roles,
            level,
            muted,
            suppress_everyone: settings.notifications.suppress_everyone,
            is_private,
            viewing_channel_focused: viewing,
            dnd: self.self_status == Status::Dnd,
        };
        if dn_notifications::should_notify(m, &ctx) {
            let n = dn_notifications::build_notification(m, &author, &location, settings.notifications.show_preview);
            if let Err(e) = notifier.show(&n) {
                tracing::debug!("notification not shown: {e}");
            }
            if mention || is_private {
                self.link.emit(UiEvent::RequestAttention);
            }
        }
    }

    // ---------- persistence ----------

    pub(crate) async fn flush(&mut self) {
        if self.settings_dirty {
            self.save_settings();
        }
        let Some(storage) = self.storage.clone() else {
            self.persist_buffer.clear();
            self.delete_buffer.clear();
            return;
        };
        if !self.persist_buffer.is_empty() {
            let msgs = std::mem::take(&mut self.persist_buffer);
            match storage.call(move |s| s.upsert_messages(&msgs)).await {
                Ok(Err(e)) | Err(e) => tracing::warn!("persisting messages failed: {e}"),
                _ => {}
            }
        }
        for (channel, ids) in std::mem::take(&mut self.delete_buffer) {
            let _ = storage.call(move |s| s.delete_messages(channel, &ids)).await;
        }
    }

    fn persist_read_states(&mut self) {
        let dirty = self.store().take_dirty_read_states();
        if dirty.is_empty() {
            return;
        }
        if let Some(storage) = self.storage.clone() {
            tokio::spawn(async move {
                let _ = storage
                    .call(move |s| {
                        for (c, m) in dirty {
                            let _ = s.set_read_state(c, m);
                        }
                    })
                    .await;
            });
        }
    }

    fn prune(&self) {
        let Some(storage) = self.storage.clone() else { return };
        let max = self.link.settings.read().advanced.cache_max_messages as usize;
        tokio::spawn(async move {
            match storage.call(move |s| s.prune(max, 20_000)).await {
                Ok(Ok(n)) if n > 0 => tracing::info!("pruned {n} cached messages"),
                Ok(Err(e)) => tracing::warn!("prune failed: {e}"),
                _ => {}
            }
        });
    }

    pub(crate) fn save_settings(&mut self) {
        self.settings_dirty = false;
        let s: Settings = self.link.settings.read().clone();
        if let Err(e) = s.save(&self.cfg.paths.settings_file()) {
            tracing::warn!("saving settings failed: {e}");
        }
    }

    pub(crate) fn apply_autostart(&mut self, enabled: bool) {
        if self.autostart_applied == Some(enabled) {
            return;
        }
        match desktop::set_autostart(enabled) {
            Ok(()) => self.autostart_applied = Some(enabled),
            Err(e) => self.link.toast(format!("Could not change start-on-boot: {e}"), true),
        }
    }

    pub(crate) fn refresh_audio_devices(&self) {
        if !self.cfg.audio {
            return;
        }
        let inputs = dn_audio::input_devices().into_iter().map(|d| d.name).collect();
        let outputs = dn_audio::output_devices().into_iter().map(|d| d.name).collect();
        let mut store = self.store();
        store.session.audio_inputs = inputs;
        store.session.audio_outputs = outputs;
    }

    fn update_level(&self) {
        if let Some(a) = &self.audio {
            let level = a.input_level_db();
            self.store().voice.input_level_db = level;
            self.link.repaint.now();
        }
    }
}

fn map_level(l: dn_settings::NotifyLevel) -> dn_notifications::NotifyLevel {
    match l {
        dn_settings::NotifyLevel::All => dn_notifications::NotifyLevel::All,
        dn_settings::NotifyLevel::Mentions => dn_notifications::NotifyLevel::Mentions,
        dn_settings::NotifyLevel::Nothing => dn_notifications::NotifyLevel::Nothing,
    }
}

pub(crate) fn random_nonce() -> String {
    // Discord nonces are at most 25 chars; a u64 fits.
    rand::random::<u64>().to_string()
}
