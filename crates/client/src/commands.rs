//! Handlers for UI commands. Network calls run in spawned tasks so the backend loop stays
//! responsive; results are applied to the shared store or sent back as `UiEvent`s.

use std::time::{Duration, Instant};

use dn_api::{ApiError, CreateForumPost, CreateMessage, FileUpload, MessagePage, SearchOutcome, SearchQuery};
use dn_cache::bus::{Command, SearchRequest, SearchSource, UiEvent};
use dn_core::{Dispatch, Id, Message, MessageDelete, Presence, PresenceUser, Status};
use dn_gateway::{GatewayCommand, Intents, PresenceUpdate};

use crate::{Backend, Internal, PendingSend, random_nonce};

const PAGE: u8 = 50;
const MAX_UPLOAD_BYTES: u64 = 10 * 1024 * 1024;
const MAX_DOWNLOAD_BYTES: u64 = 512 * 1024 * 1024;
const MEMBER_REQUEST_LIMIT: u64 = 10_000;

impl Backend {
    pub(crate) async fn handle_command(&mut self, cmd: Command) {
        match cmd {
            Command::AddBotAccount { token } => {
                let accounts = self.accounts.clone();
                let internal = self.internal.clone();
                let api = self.cfg.api_base.clone();
                tokio::spawn(async move {
                    let r = accounts.lock().await.add_bot_account(&token, &api).await.map_err(|e| e.to_string());
                    let _ = internal.send(Internal::AccountAdded(r));
                });
            }
            Command::SwitchAccount(id) => {
                let r = self.accounts.lock().await.set_active(&id);
                match r {
                    Ok(()) => {
                        self.degraded_intents = false;
                        self.start_active_session().await;
                    }
                    Err(e) => self.link.toast(e.to_string(), true),
                }
            }
            Command::RemoveAccount(id) => {
                let active = self.session.as_ref().is_some_and(|s| s.account_id == id);
                if active {
                    self.stop_session().await;
                }
                let r = self.accounts.lock().await.remove_account(&id).await;
                if let Err(e) = r {
                    self.link.toast(format!("Removing account failed: {e}"), true);
                }
                self.refresh_accounts_view().await;
                if active {
                    self.start_active_session().await;
                }
            }
            Command::Reconnect => {
                self.degraded_intents = false;
                self.start_active_session().await;
            }
            Command::BeginOAuth { client_id, client_secret, port } => self.begin_oauth(client_id, client_secret, port),
            Command::CancelOAuth => {
                if let Some(c) = self.oauth_cancel.take() {
                    let _ = c.send(());
                }
            }
            Command::UnlinkOAuth => {
                if let Some(id) = self.session.as_ref().map(|s| s.account_id.clone()) {
                    let r = self.accounts.lock().await.unlink_oauth(&id).await;
                    if let Err(e) = r {
                        self.link.toast(e.to_string(), true);
                    }
                    self.store().session.linked = None;
                    self.refresh_accounts_view().await;
                }
            }
            Command::OpenChannel { channel_id } => self.open_channel(channel_id).await,
            Command::LoadOlder { channel_id, before } => self.load_older(channel_id, before),
            Command::JumpToPresent { channel_id } => {
                self.last_fetch.remove(&channel_id);
                self.fetch_latest(channel_id);
            }
            Command::JumpToMessage { channel_id, message_id } => self.jump_to(channel_id, message_id),
            Command::SendMessage { channel_id, content, reply_to, mention_reply, files, sticker_ids } => {
                let nonce = random_nonce();
                let label = if !content.is_empty() {
                    content.clone()
                } else if !files.is_empty() {
                    format!("Uploading {} file(s)…", files.len())
                } else if !sticker_ids.is_empty() {
                    "Sending sticker…".to_string()
                } else {
                    String::new()
                };
                self.store().add_pending(channel_id, nonce.clone(), label);
                self.pending_sends
                    .insert(nonce.clone(), PendingSend { channel: channel_id, content, reply_to, mention_reply, files, sticker_ids });
                self.send_pending(nonce);
            }
            Command::RetrySend { channel_id, nonce } => {
                if self.pending_sends.contains_key(&nonce) {
                    if let Some(p) = self.store().channel_messages_mut(channel_id).pending.iter_mut().find(|p| p.nonce == nonce) {
                        p.failed = None;
                    }
                    self.send_pending(nonce);
                }
            }
            Command::DiscardPending { channel_id, nonce } => {
                self.pending_sends.remove(&nonce);
                self.store().channel_messages_mut(channel_id).remove_pending(&nonce);
            }
            Command::EditMessage { channel_id, message_id, content } => {
                self.spawn_rest(move |rest, link| async move {
                    match rest.edit_message(channel_id, message_id, &content).await {
                        Ok(m) => {
                            link.store.lock().channel_messages_mut(channel_id).replace(m);
                        }
                        Err(e) => link.toast(format!("Edit failed: {e}"), true),
                    }
                });
            }
            Command::DeleteMessage { channel_id, message_id } => {
                self.spawn_rest(move |rest, link| async move {
                    match rest.delete_message(channel_id, message_id).await {
                        Ok(()) | Err(ApiError::NotFound) => {
                            link.store.lock().apply(&Dispatch::MessageDelete(MessageDelete { id: message_id, channel_id, guild_id: None }));
                        }
                        Err(e) => link.toast(format!("Delete failed: {e}"), true),
                    }
                });
            }
            Command::React { channel_id, message_id, emoji, add } => {
                self.spawn_rest(move |rest, link| async move {
                    let r = if add {
                        rest.add_reaction(channel_id, message_id, &emoji).await
                    } else {
                        rest.remove_own_reaction(channel_id, message_id, &emoji).await
                    };
                    if let Err(e) = r {
                        link.toast(format!("Reaction failed: {e}"), true);
                    }
                });
            }
            Command::Pin { channel_id, message_id, pin } => {
                self.spawn_rest(move |rest, link| async move {
                    let r = if pin { rest.pin(channel_id, message_id).await } else { rest.unpin(channel_id, message_id).await };
                    match r {
                        Ok(()) => {
                            let mut store = link.store.lock();
                            if let Some(m) = store.channel_messages_mut(channel_id).get_mut(message_id) {
                                m.pinned = pin;
                            }
                            drop(store);
                            link.toast(if pin { "Message pinned" } else { "Message unpinned" }, false);
                        }
                        Err(e) => link.toast(format!("Pin failed: {e}"), true),
                    }
                });
            }
            Command::LoadPins { channel_id } => {
                self.spawn_rest(move |rest, link| async move {
                    match rest.pins(channel_id, None, 50).await {
                        Ok(page) => link.emit(UiEvent::Pins { channel_id, messages: page.items.into_iter().map(|p| p.message).collect() }),
                        Err(e) => {
                            link.emit(UiEvent::Pins { channel_id, messages: vec![] });
                            link.toast(format!("Couldn't load pins: {e}"), true);
                        }
                    }
                });
            }
            Command::Typing { channel_id } => {
                self.spawn_rest(move |rest, _| async move {
                    let _ = rest.trigger_typing(channel_id).await;
                });
            }
            Command::MarkRead { channel_id, message_id } => {
                self.store().mark_read(channel_id, message_id);
            }
            Command::Search(req) => self.search(req),
            Command::OpenDm { user_id } => {
                let storage = self.storage.clone();
                self.spawn_rest(move |rest, link| async move {
                    match rest.create_dm(user_id).await {
                        Ok(ch) => {
                            let id = ch.id;
                            link.store.lock().upsert_channel(ch.clone());
                            if let Some(s) = storage {
                                let _ = s.call(move |s| s.upsert_private_channel(&ch)).await;
                            }
                            link.emit(UiEvent::OpenChannel(id));
                        }
                        Err(e) => link.toast(format!("Couldn't open a DM: {e}"), true),
                    }
                });
            }
            Command::CreateThread { channel_id, message_id, name } => {
                self.spawn_rest(move |rest, link| async move {
                    match rest.start_thread_from_message(channel_id, message_id, &name).await {
                        Ok(t) => {
                            let id = t.id;
                            link.store.lock().upsert_channel(t);
                            link.emit(UiEvent::OpenChannel(id));
                        }
                        Err(e) => link.toast(format!("Couldn't create the thread: {e}"), true),
                    }
                });
            }
            Command::CreateForumPost { channel_id, title, content, tags, files, sticker_ids } => {
                if self.rest().is_none() {
                    self.link.emit(UiEvent::ForumPostResult { channel_id, result: Err("Not connected to Discord.".into()) });
                } else {
                    self.create_forum_post(channel_id, title, content, tags, files, sticker_ids);
                }
            }
            Command::DownloadTo { url, path } => {
                self.spawn_rest(move |rest, link| async move {
                    match rest.download(&url, MAX_DOWNLOAD_BYTES).await {
                        Ok(bytes) => match tokio::fs::write(&path, &bytes).await {
                            Ok(()) => link.toast(format!("Saved {}", path.display()), false),
                            Err(e) => link.toast(format!("Couldn't save the file: {e}"), true),
                        },
                        Err(e) => link.toast(format!("Download failed: {e}"), true),
                    }
                });
            }
            Command::FetchProfile { user_id, guild_id } => {
                let cached_member = guild_id.and_then(|g| self.store().member(g, user_id).cloned());
                self.spawn_rest(move |rest, link| async move {
                    let user = match rest.user(user_id).await {
                        Ok(u) => u,
                        Err(e) => {
                            tracing::debug!("profile fetch failed: {e}");
                            return;
                        }
                    };
                    let member = match (cached_member, guild_id) {
                        (Some(m), _) => Some(m),
                        (None, Some(g)) => rest.guild_member(g, user_id).await.ok(),
                        _ => None,
                    };
                    link.emit(UiEvent::Profile { user: Box::new(user), member: member.map(Box::new), guild_id });
                });
            }
            Command::RequestMembers { guild_id } => self.request_members(guild_id),
            Command::SetPresence { status, custom } => {
                self.self_status = status;
                self.self_custom = custom.clone();
                self.gateway_send(GatewayCommand::UpdatePresence(PresenceUpdate { status, activity: custom.clone(), afk: false }));
                let mut store = self.store();
                if let Some(me) = store.me() {
                    let activities = custom
                        .map(|c| {
                            vec![dn_core::Activity {
                                name: "Custom Status".into(),
                                kind: dn_core::Activity::CUSTOM,
                                state: Some(c),
                                ..Default::default()
                            }]
                        })
                        .unwrap_or_default();
                    let p = Presence { user: PresenceUser { id: me, ..Default::default() }, guild_id: None, status, activities };
                    if status == Status::Offline || status == Status::Invisible {
                        store.presences.remove(&me);
                    } else {
                        store.presences.insert(me, p);
                    }
                }
            }
            Command::JoinVoice { guild_id, channel_id } => self.join_voice(guild_id, channel_id),
            Command::LeaveVoice => self.leave_voice().await,
            Command::SetMute(m) => {
                self.store().voice.self_mute = m;
                if let Some(v) = &self.voice {
                    v.set_self_mute(m);
                }
                self.resend_voice_state();
            }
            Command::SetDeaf(d) => {
                {
                    let mut s = self.store();
                    s.voice.self_deaf = d;
                }
                if let Some(v) = &self.voice {
                    v.set_self_deaf(d);
                }
                self.resend_voice_state();
            }
            Command::SetUserVolume { user_id, volume } => {
                if let Some(v) = &self.voice {
                    v.set_user_volume(user_id, volume);
                }
                self.settings_dirty = true;
            }
            Command::PushToTalk(down) => {
                if let Some(a) = &self.audio {
                    a.set_ptt(down);
                }
            }
            Command::MicTest(on) => self.mic_test(on),
            Command::RefreshAudioDevices => self.refresh_audio_devices(),
            Command::SettingsChanged => self.settings_changed(),
            Command::FocusChanged { window_focused } => self.window_focused = window_focused,
            Command::Shutdown => {}
        }
        self.link.repaint.now();
    }

    /// Run a REST call in the background if a session exists.
    fn spawn_rest<F, Fut>(&self, f: F)
    where
        F: FnOnce(dn_api::RestClient, dn_cache::bus::BackendLink) -> Fut + Send + 'static,
        Fut: std::future::Future<Output = ()> + Send + 'static,
    {
        match self.rest() {
            Some(rest) => {
                let link = self.link.clone();
                tokio::spawn(f(rest, link));
            }
            None => self.link.toast("Not connected to Discord.", true),
        }
    }

    fn begin_oauth(&mut self, client_id: String, client_secret: Option<String>, port: u16) {
        let Some(account_id) = self.session.as_ref().map(|s| s.account_id.clone()) else {
            self.link.emit(UiEvent::OAuthFinished(Err("Connect a bot account first.".into())));
            return;
        };
        let cfg = dn_auth::OAuthConfig { client_id, client_secret, redirect_port: port, ..Default::default() };
        if let Err(e) = cfg.validate() {
            self.link.emit(UiEvent::OAuthFinished(Err(e.to_string())));
            return;
        }
        let (tx, rx) = tokio::sync::oneshot::channel();
        if let Some(old) = self.oauth_cancel.replace(tx) {
            let _ = old.send(());
        }
        let accounts = self.accounts.clone();
        let internal = self.internal.clone();
        let api_base = self.cfg.oauth_api_base.clone();
        tokio::spawn(async move {
            let result = async {
                let tokens = dn_auth::oauth::sign_in(&cfg, true, rx, Duration::from_secs(300)).await?;
                let user = dn_auth::oauth::fetch_user(&tokens.access_token, &api_base).await?;
                accounts.lock().await.link_oauth(&account_id, &cfg, &tokens, &user)?;
                Ok::<_, dn_auth::AuthError>(user.display_name().to_string())
            }
            .await
            .map_err(|e| e.to_string());
            let _ = internal.send(Internal::OAuthDone(result));
        });
    }

    async fn open_channel(&mut self, channel: Id) {
        let needs_cache = self.store().channel_messages(channel).is_none_or(|cm| !cm.loaded_once);
        if needs_cache
            && let Some(storage) = self.storage.clone()
            && let Ok(Ok(cached)) = storage.call(move |s| s.load_messages(channel, None, PAGE as usize)).await
            && !cached.is_empty()
        {
            // Shown immediately; the REST page below reconciles edits/deletions.
            self.store().messages_loaded(channel, cached, None, false);
            self.link.repaint.now();
        }
        let fresh = self.last_fetch.get(&channel).is_some_and(|t| t.elapsed() < Duration::from_secs(30));
        if !fresh || needs_cache {
            self.fetch_latest(channel);
        }
    }

    fn fetch_latest(&mut self, channel: Id) {
        let can_read = self.store().permissions_in(channel).can_read_history();
        if !can_read {
            self.store().messages_loaded(channel, vec![], None, true);
            return;
        }
        self.last_fetch.insert(channel, Instant::now());
        let storage = self.storage.clone();
        self.spawn_rest(move |rest, link| async move {
            match rest.messages(channel, MessagePage::latest(PAGE)).await {
                Ok(page) => {
                    let reached_start = page.len() < PAGE as usize;
                    {
                        let mut store = link.store.lock();
                        reconcile_deleted(&mut store, channel, &page);
                        store.messages_loaded(channel, page.clone(), None, reached_start);
                    }
                    if let Some(s) = storage {
                        let _ = s.call(move |s| s.upsert_messages(&page)).await;
                    }
                }
                Err(e) => {
                    link.store.lock().messages_loaded(channel, vec![], None, false);
                    link.toast(format!("Couldn't load messages: {e}"), true);
                }
            }
            link.repaint.now();
        });
    }

    fn load_older(&mut self, channel: Id, before: Id) {
        let storage = self.storage.clone();
        self.spawn_rest(move |rest, link| async move {
            match rest.messages(channel, MessagePage::before(before, PAGE)).await {
                Ok(page) => {
                    let reached_start = page.len() < PAGE as usize;
                    link.store.lock().messages_loaded(channel, page.clone(), Some(before), reached_start);
                    if let Some(s) = storage {
                        let _ = s.call(move |s| s.upsert_messages(&page)).await;
                    }
                }
                Err(e) => {
                    link.store.lock().set_loading_older(channel, false);
                    link.toast(format!("Couldn't load older messages: {e}"), true);
                }
            }
            link.repaint.now();
        });
    }

    fn jump_to(&mut self, channel: Id, message: Id) {
        self.spawn_rest(move |rest, link| async move {
            match rest.messages(channel, MessagePage::around(message, PAGE)).await {
                Ok(page) => link.store.lock().messages_loaded_around(channel, page),
                Err(e) => link.toast(format!("Couldn't jump to the message: {e}"), true),
            }
            link.repaint.now();
        });
    }

    fn create_forum_post(
        &self,
        channel_id: Id,
        title: String,
        content: String,
        tags: Vec<Id>,
        files: Vec<std::path::PathBuf>,
        sticker_ids: Vec<Id>,
    ) {
        self.spawn_rest(move |rest, link| async move {
            let result = async {
                let files = read_uploads(&files).await?;
                let message = CreateMessage { content, sticker_ids, files, ..Default::default() };
                let post = CreateForumPost { name: title, message, applied_tags: tags };
                rest.create_forum_post(channel_id, &post).await.map_err(|e| e.to_string())
            }
            .await;
            match result {
                Ok(post) => {
                    let id = post.channel.id;
                    {
                        let mut store = link.store.lock();
                        store.upsert_channel(post.channel);
                        if let Some(m) = post.message {
                            store.apply(&Dispatch::MessageCreate(Box::new(m)));
                        }
                    }
                    link.emit(UiEvent::ForumPostResult { channel_id, result: Ok(id) });
                    link.emit(UiEvent::OpenChannel(id));
                }
                Err(e) => {
                    link.emit(UiEvent::ForumPostResult { channel_id, result: Err(e.clone()) });
                    link.toast(format!("Couldn't create the post: {e}"), true);
                }
            }
        });
    }

    fn send_pending(&mut self, nonce: String) {
        let Some(p) = self.pending_sends.get(&nonce) else { return };
        let (channel, content, reply_to, mention_reply, files, sticker_ids) =
            (p.channel, p.content.clone(), p.reply_to, p.mention_reply, p.files.clone(), p.sticker_ids.clone());
        let internal = self.internal.clone();
        let n = nonce.clone();
        self.spawn_rest(move |rest, link| async move {
            let uploads = match read_uploads(&files).await {
                Ok(u) => u,
                Err(error) => {
                    let _ = internal.send(Internal::SendFailed { channel, nonce: n, error });
                    return;
                }
            };
            let msg = CreateMessage {
                content,
                nonce: Some(n.clone()),
                reply_to,
                mention_replied_user: mention_reply,
                sticker_ids,
                files: uploads,
                tts: false,
            };
            match rest.create_message(channel, &msg).await {
                Ok(mut sent) => {
                    // The gateway echo usually arrives first; applying the REST result too is
                    // idempotent (dedup by id) and covers a lagging gateway.
                    sent.nonce = Some(serde_json::Value::String(n));
                    link.store.lock().apply(&Dispatch::MessageCreate(Box::new(sent)));
                    link.repaint.now();
                }
                Err(e) => {
                    let _ = internal.send(Internal::SendFailed { channel, nonce: n, error: e.to_string() });
                }
            }
        });
    }

    fn search(&mut self, req: SearchRequest) {
        let storage = self.storage.clone();
        let has_content_intent =
            self.session.as_ref().is_some_and(|s| s.intents.contains(Intents::MESSAGE_CONTENT)) && !self.degraded_intents;
        let link = self.link.clone();
        let rest = self.rest();
        tokio::spawn(async move {
            let mut note = None;
            if let (Some(rest), Some(guild)) = (rest, req.guild_id) {
                let q = SearchQuery {
                    content: (!req.query.is_empty()).then(|| req.query.clone()),
                    channel_ids: req.channel_id.into_iter().collect(),
                    author_ids: req.author_id.into_iter().collect(),
                    limit: 25,
                    offset: req.offset,
                    ..Default::default()
                };
                for _ in 0..3 {
                    match rest.search_guild_messages(guild, &q).await {
                        Ok(SearchOutcome::Results { total, messages }) => {
                            link.emit(UiEvent::SearchResults {
                                request: Box::new(req),
                                total,
                                messages,
                                source: SearchSource::Discord,
                                note: None,
                            });
                            return;
                        }
                        Ok(SearchOutcome::Indexing { retry_after }) => {
                            note = Some("Discord is still indexing this server; showing local results.".to_string());
                            tokio::time::sleep(Duration::from_secs_f64(retry_after.clamp(0.5, 5.0))).await;
                        }
                        Err(e) => {
                            note = Some(if has_content_intent {
                                format!("Discord search failed ({e}); showing messages cached locally.")
                            } else {
                                "Discord search needs the Message Content intent; showing messages cached locally.".to_string()
                            });
                            break;
                        }
                    }
                }
            }
            let Some(storage) = storage else {
                link.emit(UiEvent::SearchResults { request: Box::new(req), total: 0, messages: vec![], source: SearchSource::Local, note });
                return;
            };
            let filter = dn_storage::SearchFilter {
                query: req.query.clone(),
                guild_id: req.guild_id,
                channel_id: req.channel_id,
                author_id: req.author_id,
                limit: 25,
                offset: req.offset as usize,
            };
            let messages: Vec<Message> = storage.call(move |s| s.search(&filter)).await.ok().and_then(|r| r.ok()).unwrap_or_default();
            let total = req.offset as u64 + messages.len() as u64 + if messages.len() == 25 { 1 } else { 0 };
            link.emit(UiEvent::SearchResults { request: Box::new(req), total, messages, source: SearchSource::Local, note });
        });
    }

    fn request_members(&mut self, guild: Id) {
        let Some(intents) = self.session.as_ref().map(|s| s.intents) else { return };
        let count = {
            let mut store = self.store();
            let Some(gs) = store.guilds.get_mut(&guild) else { return };
            if gs.members_requested {
                return;
            }
            gs.members_requested = true;
            gs.guild.member_count.unwrap_or(0)
        };
        if self.degraded_intents || !intents.contains(Intents::GUILD_MEMBERS) || count > MEMBER_REQUEST_LIMIT {
            return;
        }
        self.gateway_send(GatewayCommand::RequestGuildMembers {
            guild_id: guild,
            query: Some(String::new()),
            limit: 0,
            presences: intents.contains(Intents::GUILD_PRESENCES),
            user_ids: vec![],
            nonce: None,
        });
    }

    fn mic_test(&mut self, on: bool) {
        self.mic_test = on;
        let in_voice = self.voice_pending.is_some();
        let Some(a) = &self.audio else { return };
        a.set_monitor(on);
        if on {
            a.start_capture();
            a.start_playback();
        } else if !in_voice {
            a.stop_capture();
            a.stop_playback();
            self.store().voice.input_level_db = -100.0;
        }
    }

    fn settings_changed(&mut self) {
        self.settings_dirty = true;
        let s = self.link.settings.read().clone();
        if let Some(a) = &self.audio {
            a.set_config(crate::voice_glue::audio_config(&s));
        }
        if let Some(v) = &self.voice {
            v.set_bitrate(s.voice.bitrate_kbps);
            for (user, vol) in &s.voice.user_volumes {
                if let Ok(id) = user.parse::<u64>() {
                    v.set_user_volume(Id(id), *vol);
                }
            }
        }
        self.store().limits.inactive_channel_messages = s.advanced.memory_messages_per_channel as usize;
        self.apply_autostart(s.behavior.start_on_boot);
    }
}

/// Reads attachments from disk, enforcing the bot upload limit.
async fn read_uploads(files: &[std::path::PathBuf]) -> Result<Vec<FileUpload>, String> {
    let mut uploads = Vec::with_capacity(files.len());
    let mut total = 0u64;
    for path in files {
        let bytes = tokio::fs::read(path).await.map_err(|e| format!("can't read {}: {e}", path.display()))?;
        total += bytes.len() as u64;
        if total > MAX_UPLOAD_BYTES {
            return Err("files exceed the 10 MB bot upload limit".into());
        }
        let filename = path.file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_else(|| "file".into());
        uploads.push(FileUpload { filename, bytes, content_type: None, spoiler: false });
    }
    Ok(uploads)
}

/// Remove cached messages inside the fetched page's id range that Discord no longer returns
/// (deleted while we were offline).
fn reconcile_deleted(store: &mut dn_cache::Store, channel: Id, page: &[Message]) {
    let (Some(min), Some(max)) = (page.iter().map(|m| m.id).min(), page.iter().map(|m| m.id).max()) else { return };
    let present: std::collections::HashSet<Id> = page.iter().map(|m| m.id).collect();
    let stale: Vec<Id> = match store.channel_messages(channel) {
        Some(cm) => cm.messages.iter().map(|m| m.id).filter(|id| *id >= min && *id <= max && !present.contains(id)).collect(),
        None => return,
    };
    if !stale.is_empty() {
        store.channel_messages_mut(channel).remove(&stale);
    }
}
