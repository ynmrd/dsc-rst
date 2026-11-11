//! Client state store. The backend applies gateway dispatches and REST results; the UI
//! reads it each frame. It is shared as `Arc<Mutex<Store>>` so state stays current even
//! while the window is hidden (egui does not run frames for hidden windows).

pub mod bus;
pub mod messages;
mod views;

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};

use dn_core::permissions::{self, Permissions};
use dn_core::*;

pub use messages::{ChannelMessages, PendingMessage};
pub use views::{ChannelGroup, MemberGroup, MemberRow};

pub type SharedStore = Arc<parking_lot::Mutex<Store>>;

pub const TYPING_TTL: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, PartialEq, Default)]
pub enum ConnStatus {
    #[default]
    Offline,
    Connecting,
    Connected,
    Reconnecting {
        attempt: u32,
        delay_secs: u64,
    },
    Fatal(String),
}

#[derive(Debug, Clone, PartialEq, Default)]
pub enum VoiceConn {
    #[default]
    Idle,
    Connecting {
        guild_id: Id,
        channel_id: Id,
    },
    Connected {
        guild_id: Id,
        channel_id: Id,
    },
    Reconnecting {
        guild_id: Id,
        channel_id: Id,
    },
    Failed(String),
}

impl VoiceConn {
    pub fn channel(&self) -> Option<(Id, Id)> {
        match *self {
            VoiceConn::Connecting { guild_id, channel_id }
            | VoiceConn::Connected { guild_id, channel_id }
            | VoiceConn::Reconnecting { guild_id, channel_id } => Some((guild_id, channel_id)),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct VoiceStatus {
    pub conn: VoiceConn,
    pub self_mute: bool,
    pub self_deaf: bool,
    pub speaking: HashSet<Id>,
    pub self_speaking: bool,
    pub ping_ms: Option<u32>,
    pub input_level_db: f32,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct AccountView {
    pub id: String,
    pub label: String,
    pub bot_name: String,
    pub bot_avatar_url: String,
    pub oauth_user: Option<String>,
    pub oauth_expired: bool,
    pub credentials_invalid: bool,
    pub active: bool,
}

/// Identity data obtained through OAuth2 ("Sign in with Discord") for the human user.
#[derive(Debug, Clone, Default)]
pub struct LinkedUser {
    pub user: User,
    pub guilds: Vec<PartialGuild>,
}

#[derive(Debug, Clone, Default)]
pub struct Session {
    pub conn: ConnStatus,
    pub latency_ms: Option<u32>,
    pub me: Option<User>,
    pub application_id: Option<Id>,
    /// Privileged intents were refused (close 4014) and we reconnected without them.
    pub intents_degraded: bool,
    pub ready: bool,
    pub accounts: Vec<AccountView>,
    pub linked: Option<LinkedUser>,
    pub needs_onboarding: bool,
    /// Offline demo data (no Discord connection); the UI shows a banner.
    pub demo: bool,
    pub audio_inputs: Vec<String>,
    pub audio_outputs: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct GuildState {
    /// The guild object with heavy vectors (channels/members/presences) moved out.
    pub guild: Guild,
    pub channel_ids: Vec<Id>,
    pub thread_ids: HashSet<Id>,
    pub members: HashMap<Id, Member>,
    pub voice_states: HashMap<Id, VoiceState>,
    pub available: bool,
    pub members_requested: bool,
}

impl GuildState {
    pub fn role(&self, id: Id) -> Option<&Role> {
        self.guild.roles.iter().find(|r| r.id == id)
    }
}

#[derive(Debug, Clone)]
pub struct StoreLimits {
    /// Messages kept for channels that are not currently open.
    pub inactive_channel_messages: usize,
    /// Hard cap for the open channel while scrolling back through history.
    pub active_channel_messages: usize,
    /// Number of channels whose message windows stay in memory.
    pub max_loaded_channels: usize,
}

impl Default for StoreLimits {
    fn default() -> Self {
        StoreLimits { inactive_channel_messages: 100, active_channel_messages: 3000, max_loaded_channels: 24 }
    }
}

#[derive(Debug, Default)]
pub struct Store {
    pub session: Session,
    pub voice: VoiceStatus,
    pub guilds: HashMap<Id, GuildState>,
    pub guild_order: Vec<Id>,
    pub channels: HashMap<Id, Channel>,
    pub private_channels: Vec<Id>,
    pub users: HashMap<Id, User>,
    pub presences: HashMap<Id, Presence>,
    pub messages: HashMap<Id, ChannelMessages>,
    pub typing: HashMap<Id, Vec<(Id, Instant)>>,
    pub read_states: HashMap<Id, Id>,
    pub mention_counts: HashMap<Id, u32>,
    /// Read states changed by the store itself (e.g. first sight of a channel) that the
    /// backend should persist.
    pub dirty_read_states: Vec<(Id, Id)>,
    pub active_channel: Option<Id>,
    pub limits: StoreLimits,
    loaded_lru: VecDeque<Id>,
    /// Incremented on any change; lets the UI skip work when nothing changed.
    pub generation: u64,
}

impl Store {
    pub fn new(limits: StoreLimits) -> Self {
        Store { limits, ..Default::default() }
    }

    pub fn shared(limits: StoreLimits) -> SharedStore {
        Arc::new(parking_lot::Mutex::new(Store::new(limits)))
    }

    pub fn me(&self) -> Option<Id> {
        self.session.me.as_ref().map(|u| u.id)
    }

    /// Forget everything tied to a gateway session (account switch / logout).
    pub fn reset_session_data(&mut self) {
        let accounts = std::mem::take(&mut self.session.accounts);
        let limits = self.limits.clone();
        let inputs = std::mem::take(&mut self.session.audio_inputs);
        let outputs = std::mem::take(&mut self.session.audio_outputs);
        *self = Store::new(limits);
        self.session.accounts = accounts;
        self.session.audio_inputs = inputs;
        self.session.audio_outputs = outputs;
        self.generation += 1;
    }

    pub fn apply(&mut self, d: &Dispatch) {
        self.generation = self.generation.wrapping_add(1);
        match d {
            Dispatch::Ready(r) => {
                self.session.me = Some(r.user.clone());
                self.session.application_id = Some(r.application.id);
                self.session.ready = true;
                self.users.insert(r.user.id, r.user.clone());
                for g in &r.guilds {
                    if !self.guilds.contains_key(&g.id) {
                        self.guild_order.push(g.id);
                        self.guilds.insert(
                            g.id,
                            GuildState { guild: Guild { id: g.id, unavailable: true, ..Default::default() }, ..Default::default() },
                        );
                    }
                }
            }
            Dispatch::Resumed => {}
            Dispatch::GuildCreate(g) | Dispatch::GuildUpdate(g) => self.upsert_guild(g, matches!(d, Dispatch::GuildCreate(_))),
            Dispatch::GuildDelete(g) => {
                if g.unavailable {
                    if let Some(gs) = self.guilds.get_mut(&g.id) {
                        gs.available = false;
                    }
                } else {
                    self.remove_guild(g.id);
                }
            }
            Dispatch::ChannelCreate(c) | Dispatch::ChannelUpdate(c) | Dispatch::ThreadCreate(c) | Dispatch::ThreadUpdate(c) => {
                self.upsert_channel((**c).clone())
            }
            Dispatch::ChannelDelete(c) | Dispatch::ThreadDelete(c) => self.remove_channel(c.id),
            Dispatch::ChannelPinsUpdate(p) => {
                if let Some(c) = self.channels.get_mut(&p.channel_id) {
                    c.last_pin_timestamp.clone_from(&p.last_pin_timestamp);
                }
            }
            Dispatch::ThreadListSync(s) => {
                if let Some(gs) = self.guilds.get_mut(&s.guild_id) {
                    let ids: Vec<Id> = gs
                        .thread_ids
                        .iter()
                        .copied()
                        .filter(|t| {
                            let parent = self.channels.get(t).and_then(|c| c.parent_id);
                            match &s.channel_ids {
                                Some(scope) => parent.is_some_and(|p| scope.contains(&p)),
                                None => true,
                            }
                        })
                        .collect();
                    for t in ids {
                        gs.thread_ids.remove(&t);
                        self.channels.remove(&t);
                    }
                }
                for t in &s.threads {
                    let mut t = t.clone();
                    t.guild_id = Some(s.guild_id);
                    self.upsert_channel(t);
                }
            }
            Dispatch::GuildRoleCreate(e) | Dispatch::GuildRoleUpdate(e) => {
                if let Some(gs) = self.guilds.get_mut(&e.guild_id) {
                    match gs.guild.roles.iter_mut().find(|r| r.id == e.role.id) {
                        Some(r) => *r = e.role.clone(),
                        None => gs.guild.roles.push(e.role.clone()),
                    }
                }
            }
            Dispatch::GuildRoleDelete(e) => {
                if let Some(gs) = self.guilds.get_mut(&e.guild_id) {
                    gs.guild.roles.retain(|r| r.id != e.role_id);
                }
            }
            Dispatch::GuildMemberAdd(m) | Dispatch::GuildMemberUpdate(m) => {
                if let Some(gid) = m.guild_id {
                    self.upsert_member(gid, (**m).clone());
                }
            }
            Dispatch::GuildMemberRemove(r) => {
                if let Some(gs) = self.guilds.get_mut(&r.guild_id) {
                    gs.members.remove(&r.user.id);
                }
            }
            Dispatch::GuildMembersChunk(c) => {
                for m in &c.members {
                    self.upsert_member(c.guild_id, m.clone());
                }
                for p in &c.presences {
                    self.upsert_presence(p.clone());
                }
            }
            Dispatch::GuildEmojisUpdate(e) => {
                if let Some(gs) = self.guilds.get_mut(&e.guild_id) {
                    gs.guild.emojis.clone_from(&e.emojis);
                }
            }
            Dispatch::GuildStickersUpdate(e) => {
                if let Some(gs) = self.guilds.get_mut(&e.guild_id) {
                    gs.guild.stickers.clone_from(&e.stickers);
                }
            }
            Dispatch::MessageCreate(m) => self.on_message_create(m),
            Dispatch::MessageUpdate(u) => {
                if let Some(cm) = self.messages.get_mut(&u.channel_id) {
                    cm.update(u);
                }
            }
            Dispatch::MessageDelete(e) => {
                if let Some(cm) = self.messages.get_mut(&e.channel_id) {
                    cm.remove(&[e.id]);
                }
            }
            Dispatch::MessageDeleteBulk(e) => {
                if let Some(cm) = self.messages.get_mut(&e.channel_id) {
                    cm.remove(&e.ids);
                }
            }
            Dispatch::MessageReactionAdd(r) => {
                let me = self.me();
                if let Some(cm) = self.messages.get_mut(&r.channel_id) {
                    cm.reaction_add(r, me);
                }
            }
            Dispatch::MessageReactionRemove(r) => {
                let me = self.me();
                if let Some(cm) = self.messages.get_mut(&r.channel_id) {
                    cm.reaction_remove(r, me);
                }
            }
            Dispatch::MessageReactionRemoveAll(r) | Dispatch::MessageReactionRemoveEmoji(r) => {
                if let Some(cm) = self.messages.get_mut(&r.channel_id) {
                    cm.reaction_clear(r.message_id, r.emoji.as_ref());
                }
            }
            Dispatch::PresenceUpdate(p) => self.upsert_presence((**p).clone()),
            Dispatch::TypingStart(t) => {
                if Some(t.user_id) == self.me() {
                    return;
                }
                if let (Some(gid), Some(m)) = (t.guild_id, &t.member) {
                    self.upsert_member(gid, m.clone());
                }
                let list = self.typing.entry(t.channel_id).or_default();
                list.retain(|(u, _)| *u != t.user_id);
                list.push((t.user_id, Instant::now()));
            }
            Dispatch::UserUpdate(u) => {
                self.users.insert(u.id, (**u).clone());
                if self.me() == Some(u.id) {
                    self.session.me = Some((**u).clone());
                }
            }
            Dispatch::VoiceStateUpdate(v) => self.upsert_voice_state((**v).clone()),
            Dispatch::VoiceServerUpdate(_) | Dispatch::Unknown(_) => {}
        }
    }

    fn upsert_guild(&mut self, g: &Guild, create: bool) {
        let mut g = g.clone();
        let channels = std::mem::take(&mut g.channels);
        let threads = std::mem::take(&mut g.threads);
        let members = std::mem::take(&mut g.members);
        let presences = std::mem::take(&mut g.presences);
        let voice_states = std::mem::take(&mut g.voice_states);
        let id = g.id;
        if !self.guilds.contains_key(&id) {
            self.guild_order.push(id);
        }
        let gs = self.guilds.entry(id).or_default();
        if !create {
            // GUILD_UPDATE carries no channels/members; keep ours.
            g.member_count = g.member_count.or(gs.guild.member_count);
            g.joined_at = g.joined_at.take().or(gs.guild.joined_at.take());
        }
        gs.guild = g;
        gs.available = true;
        if create {
            for cid in std::mem::take(&mut gs.channel_ids) {
                self.channels.remove(&cid);
            }
            let gs = self.guilds.get_mut(&id).expect("just inserted");
            gs.voice_states.clear();
            for mut c in channels {
                c.guild_id = Some(id);
                gs.channel_ids.push(c.id);
                self.channels.insert(c.id, c);
            }
            for mut t in threads {
                t.guild_id = Some(id);
                gs.thread_ids.insert(t.id);
                self.channels.insert(t.id, t);
            }
            for m in members {
                self.upsert_member(id, m);
            }
            for mut p in presences {
                p.guild_id = Some(id);
                self.upsert_presence(p);
            }
            for mut v in voice_states {
                v.guild_id = Some(id);
                self.upsert_voice_state(v);
            }
            self.init_read_states(id);
        }
    }

    /// Bots have no server-side read state. The first time we see a channel, treat its
    /// existing history as read so only messages after first connect show as unread.
    fn init_read_states(&mut self, guild: Id) {
        let Some(gs) = self.guilds.get(&guild) else { return };
        for cid in gs.channel_ids.iter().chain(gs.thread_ids.iter()) {
            if let Some(last) = self.channels.get(cid).and_then(|c| c.last_message_id)
                && !self.read_states.contains_key(cid)
            {
                self.read_states.insert(*cid, last);
                self.dirty_read_states.push((*cid, last));
            }
        }
    }

    fn remove_guild(&mut self, id: Id) {
        if let Some(gs) = self.guilds.remove(&id) {
            for c in gs.channel_ids.iter().chain(gs.thread_ids.iter()) {
                self.channels.remove(c);
                self.messages.remove(c);
            }
        }
        self.guild_order.retain(|g| *g != id);
    }

    pub fn upsert_channel(&mut self, c: Channel) {
        let id = c.id;
        if c.kind.is_private() {
            for u in &c.recipients {
                self.users.entry(u.id).or_insert_with(|| u.clone());
            }
            if !self.private_channels.contains(&id) {
                self.private_channels.push(id);
            }
            if let Some(last) = c.last_message_id
                && let std::collections::hash_map::Entry::Vacant(e) = self.read_states.entry(id)
            {
                e.insert(last);
                self.dirty_read_states.push((id, last));
            }
        } else if let Some(gid) = c.guild_id
            && let Some(gs) = self.guilds.get_mut(&gid)
        {
            if c.kind.is_thread() {
                let archived = c.thread_metadata.as_ref().is_some_and(|m| m.archived);
                if archived {
                    gs.thread_ids.remove(&id);
                } else {
                    gs.thread_ids.insert(id);
                }
            } else if !gs.channel_ids.contains(&id) {
                gs.channel_ids.push(id);
            }
        }
        match self.channels.get_mut(&id) {
            // Keep last_message_id monotonic; CHANNEL_UPDATE may carry a stale one.
            Some(existing) => {
                let last = existing.last_message_id.max(c.last_message_id);
                *existing = c;
                existing.last_message_id = last;
            }
            None => {
                self.channels.insert(id, c);
            }
        }
    }

    fn remove_channel(&mut self, id: Id) {
        if let Some(c) = self.channels.remove(&id)
            && let Some(gs) = c.guild_id.and_then(|g| self.guilds.get_mut(&g))
        {
            gs.channel_ids.retain(|x| *x != id);
            gs.thread_ids.remove(&id);
        }
        self.private_channels.retain(|x| *x != id);
        self.messages.remove(&id);
    }

    fn upsert_member(&mut self, guild: Id, mut m: Member) {
        let Some(user) = m.user.take() else { return };
        let uid = user.id;
        self.users.insert(uid, user);
        m.guild_id = None;
        if let Some(gs) = self.guilds.get_mut(&guild) {
            gs.members.insert(uid, m);
        }
    }

    fn upsert_presence(&mut self, p: Presence) {
        if p.status == Status::Offline {
            self.presences.remove(&p.user.id);
        } else {
            self.presences.insert(p.user.id, p);
        }
    }

    fn upsert_voice_state(&mut self, v: VoiceState) {
        let Some(gid) = v.guild_id else { return };
        if let Some(m) = v.member.clone() {
            self.upsert_member(gid, m);
        }
        if let Some(gs) = self.guilds.get_mut(&gid) {
            if v.channel_id.is_some() {
                let mut v = v;
                v.member = None;
                gs.voice_states.insert(v.user_id, v);
            } else {
                gs.voice_states.remove(&v.user_id);
            }
        }
    }

    fn on_message_create(&mut self, m: &Message) {
        let me = self.me();
        self.users.entry(m.author.id).or_insert_with(|| m.author.clone());
        if let (Some(gid), Some(member)) = (m.guild_id, &m.member)
            && let Some(gs) = self.guilds.get_mut(&gid)
        {
            gs.members.entry(m.author.id).or_insert_with(|| {
                let mut mm = member.clone();
                mm.user = None;
                mm
            });
        }
        match self.channels.get_mut(&m.channel_id) {
            Some(c) => c.last_message_id = c.last_message_id.max(Some(m.id)),
            None if m.guild_id.is_none() => {
                // First DM in this channel since startup: synthesize the private channel.
                let recipients = if Some(m.author.id) != me { vec![m.author.clone()] } else { vec![] };
                self.upsert_channel(Channel {
                    id: m.channel_id,
                    kind: ChannelKind::Dm,
                    recipients,
                    last_message_id: Some(m.id),
                    ..Default::default()
                });
            }
            None => {}
        }
        if let Some(list) = self.typing.get_mut(&m.channel_id) {
            list.retain(|(u, _)| *u != m.author.id);
        }
        let own = Some(m.author.id) == me;
        if own {
            self.mark_read(m.channel_id, m.id);
        } else if self.is_mention(m) {
            *self.mention_counts.entry(m.channel_id).or_default() += 1;
        }
        let cap = self.cap_for(m.channel_id);
        if let Some(cm) = self.messages.get_mut(&m.channel_id) {
            cm.push_live(m.clone(), cap);
        }
        if m.guild_id.is_none() {
            // Most recent DM first.
            self.private_channels.retain(|c| *c != m.channel_id);
            self.private_channels.insert(0, m.channel_id);
        }
    }

    /// Mentions us directly, via one of our roles, @everyone/@here, or replies to us.
    pub fn is_mention(&self, m: &Message) -> bool {
        let Some(me) = self.me() else { return false };
        if m.author.id == me {
            return false;
        }
        if m.guild_id.is_none() {
            return true;
        }
        if m.mentions_user(me) {
            return true;
        }
        if m.referenced_message.as_ref().is_some_and(|r| r.author.id == me) {
            return true;
        }
        let roles =
            m.guild_id.and_then(|g| self.guilds.get(&g)).and_then(|gs| gs.members.get(&me)).map(|mm| mm.roles.as_slice()).unwrap_or(&[]);
        m.mention_roles.iter().any(|r| roles.contains(r))
    }

    fn cap_for(&self, channel: Id) -> usize {
        if self.active_channel == Some(channel) {
            self.limits.active_channel_messages
        } else {
            self.limits.inactive_channel_messages.max(1)
        }
    }

    /// Results of a history request. `before` is the cursor used (None = latest page).
    pub fn messages_loaded(&mut self, channel: Id, page: Vec<Message>, before: Option<Id>, reached_start: bool) {
        self.generation = self.generation.wrapping_add(1);
        for m in &page {
            self.users.entry(m.author.id).or_insert_with(|| m.author.clone());
        }
        if let Some(newest) = page.iter().map(|m| m.id).max()
            && let Some(c) = self.channels.get_mut(&channel)
        {
            c.last_message_id = c.last_message_id.max(Some(newest));
        }
        let cap = self.cap_for(channel);
        self.touch_loaded(channel);
        self.messages.entry(channel).or_default().merge_page(page, before, reached_start, cap);
    }

    /// A page fetched `around` a message (search result / pin jump). Replaces the window;
    /// unless it reaches the channel's newest message, the newer end is marked as a gap.
    pub fn messages_loaded_around(&mut self, channel: Id, mut page: Vec<Message>) {
        self.generation = self.generation.wrapping_add(1);
        page.sort_by_key(|m| m.id);
        let newest_known = self.channels.get(&channel).and_then(|c| c.last_message_id);
        let reaches_present = match (page.last(), newest_known) {
            (Some(last), Some(n)) => last.id >= n,
            _ => false,
        };
        for m in &page {
            self.users.entry(m.author.id).or_insert_with(|| m.author.clone());
        }
        self.touch_loaded(channel);
        let cm = self.messages.entry(channel).or_default();
        cm.messages = page.into();
        cm.reached_start = false;
        cm.has_newer_gap = !reaches_present;
        cm.loading_older = false;
        cm.loaded_once = true;
        cm.version = cm.version.wrapping_add(1);
    }

    pub fn set_loading_older(&mut self, channel: Id, loading: bool) {
        self.messages.entry(channel).or_default().loading_older = loading;
    }

    fn touch_loaded(&mut self, channel: Id) {
        self.loaded_lru.retain(|c| *c != channel);
        self.loaded_lru.push_back(channel);
        while self.loaded_lru.len() > self.limits.max_loaded_channels {
            let Some(old) = self.loaded_lru.pop_front() else { break };
            if Some(old) == self.active_channel {
                self.loaded_lru.push_back(old);
                continue;
            }
            self.messages.remove(&old);
        }
    }

    /// Called when the UI opens a channel. Shrinks the previously open one.
    pub fn set_active_channel(&mut self, channel: Option<Id>) {
        if self.active_channel == channel {
            return;
        }
        if let Some(prev) = self.active_channel {
            let keep = self.limits.inactive_channel_messages;
            if let Some(cm) = self.messages.get_mut(&prev) {
                if cm.has_newer_gap {
                    // Can't keep a window that doesn't reach the present for an inactive channel.
                    self.messages.remove(&prev);
                } else {
                    cm.trim_to_newest(keep);
                }
            }
        }
        self.active_channel = channel;
        if let Some(c) = channel {
            self.touch_loaded(c);
        }
        self.generation = self.generation.wrapping_add(1);
    }

    pub fn mark_read(&mut self, channel: Id, message: Id) {
        let cur = self.read_states.get(&channel).copied();
        if cur.is_none_or(|c| message > c) {
            self.read_states.insert(channel, message);
            self.dirty_read_states.push((channel, message));
        }
        self.mention_counts.remove(&channel);
        self.generation = self.generation.wrapping_add(1);
    }

    pub fn load_read_states(&mut self, states: HashMap<Id, Id>) {
        for (c, m) in states {
            let e = self.read_states.entry(c).or_insert(m);
            *e = (*e).max(m);
        }
    }

    pub fn take_dirty_read_states(&mut self) -> Vec<(Id, Id)> {
        std::mem::take(&mut self.dirty_read_states)
    }

    pub fn is_unread(&self, channel: Id) -> bool {
        let Some(last) = self.channels.get(&channel).and_then(|c| c.last_message_id) else { return false };
        match self.read_states.get(&channel) {
            Some(read) => last > *read,
            None => false,
        }
    }

    pub fn mentions(&self, channel: Id) -> u32 {
        self.mention_counts.get(&channel).copied().unwrap_or(0)
    }

    /// (has unread in a visible channel, total mentions)
    pub fn guild_unread(&self, guild: Id) -> (bool, u32) {
        let Some(gs) = self.guilds.get(&guild) else { return (false, 0) };
        let mut unread = false;
        let mut mentions = 0;
        for cid in gs.channel_ids.iter().chain(gs.thread_ids.iter()) {
            let Some(c) = self.channels.get(cid) else { continue };
            if !c.kind.is_text_like() || c.kind.is_voice() {
                continue;
            }
            if !self.permissions_in(*cid).can_view() {
                continue;
            }
            unread |= self.is_unread(*cid);
            mentions += self.mentions(*cid);
        }
        (unread, mentions)
    }

    pub fn typing_users(&mut self, channel: Id) -> Vec<Id> {
        let now = Instant::now();
        match self.typing.get_mut(&channel) {
            Some(list) => {
                list.retain(|(_, at)| now.duration_since(*at) < TYPING_TTL);
                list.iter().map(|(u, _)| *u).collect()
            }
            None => vec![],
        }
    }

    /// Our (bot user's) effective permissions in a channel. DMs allow everything a DM can do.
    pub fn permissions_in(&self, channel: Id) -> Permissions {
        let Some(c) = self.channels.get(&channel) else { return Permissions::NONE };
        let Some(gid) = c.guild_id else {
            return Permissions::VIEW_CHANNEL
                | Permissions::SEND_MESSAGES
                | Permissions::READ_MESSAGE_HISTORY
                | Permissions::ADD_REACTIONS
                | Permissions::ATTACH_FILES
                | Permissions::EMBED_LINKS
                | Permissions::USE_EXTERNAL_EMOJIS
                | Permissions::PIN_MESSAGES;
        };
        let (Some(gs), Some(me)) = (self.guilds.get(&gid), self.me()) else { return Permissions::NONE };
        let Some(member) = gs.members.get(&me) else { return Permissions::NONE };
        // Threads inherit permissions from their parent channel.
        let overwrites = if c.kind.is_thread() {
            c.parent_id.and_then(|p| self.channels.get(&p)).map(|p| p.permission_overwrites.as_slice()).unwrap_or(&[])
        } else {
            &c.permission_overwrites
        };
        let p = permissions::channel_permissions(&gs.guild, me, member, overwrites);
        if c.kind.is_thread() && p.contains(Permissions::SEND_MESSAGES_IN_THREADS) {
            return p | Permissions::SEND_MESSAGES;
        }
        if c.kind.is_thread() {
            return p.remove(Permissions::SEND_MESSAGES);
        }
        p
    }

    pub fn member(&self, guild: Id, user: Id) -> Option<&Member> {
        self.guilds.get(&guild)?.members.get(&user)
    }

    /// Nickname > global name > username.
    pub fn display_name(&self, guild: Option<Id>, user: Id) -> String {
        if let Some(n) = guild.and_then(|g| self.member(g, user)).and_then(|m| m.nick.clone()).filter(|n| !n.is_empty()) {
            return n;
        }
        self.users.get(&user).map(|u| u.display_name().to_string()).unwrap_or_else(|| format!("<@{user}>"))
    }

    /// Color of the highest positioned colored role.
    pub fn member_color(&self, guild: Option<Id>, user: Id) -> Option<u32> {
        let gs = self.guilds.get(&guild?)?;
        let m = gs.members.get(&user)?;
        m.roles
            .iter()
            .filter_map(|r| gs.role(*r))
            .filter(|r| r.display_color().is_some())
            .max_by_key(|r| (r.position, r.id))
            .and_then(|r| r.display_color())
    }

    pub fn status_of(&self, user: Id) -> Status {
        match self.presences.get(&user) {
            Some(p) => p.status,
            // Discord sends no presence for ourselves; reflect the connection instead.
            None if Some(user) == self.me() && self.session.conn == ConnStatus::Connected => Status::Online,
            None => Status::Offline,
        }
    }

    pub fn voice_members(&self, guild: Id, channel: Id) -> Vec<&VoiceState> {
        let Some(gs) = self.guilds.get(&guild) else { return vec![] };
        let mut v: Vec<&VoiceState> = gs.voice_states.values().filter(|v| v.channel_id == Some(channel)).collect();
        v.sort_by_key(|v| v.user_id);
        v
    }

    pub fn channel_messages(&self, channel: Id) -> Option<&ChannelMessages> {
        self.messages.get(&channel)
    }

    pub fn channel_messages_mut(&mut self, channel: Id) -> &mut ChannelMessages {
        self.messages.entry(channel).or_default()
    }

    /// Add a locally echoed message that will be replaced by the gateway echo (matched by nonce).
    pub fn add_pending(&mut self, channel: Id, nonce: String, content: String) {
        let Some(me) = self.session.me.clone() else { return };
        let guild_id = self.channels.get(&channel).and_then(|c| c.guild_id);
        let msg = Message {
            id: Id::from_timestamp_ms(now_ms()),
            channel_id: channel,
            guild_id,
            author: me,
            content,
            timestamp: String::new(),
            ..Default::default()
        };
        self.messages.entry(channel).or_default().add_pending(nonce, msg);
        self.generation = self.generation.wrapping_add(1);
    }

    pub fn total_loaded_messages(&self) -> usize {
        self.messages.values().map(|m| m.messages.len()).sum()
    }

    pub fn guild_of(&self, channel: Id) -> Option<Id> {
        self.channels.get(&channel).and_then(|c| c.guild_id)
    }

    /// "#general (Server)" style location label for notifications/search.
    pub fn location_label(&self, channel: Id) -> String {
        let Some(c) = self.channels.get(&channel) else { return String::from("Unknown channel") };
        match c.guild_id.and_then(|g| self.guilds.get(&g)) {
            Some(gs) => format!("#{} ({})", c.display_name(), gs.guild.name),
            None => format!("@{}", self.dm_name(channel)),
        }
    }

    pub fn dm_name(&self, channel: Id) -> String {
        let Some(c) = self.channels.get(&channel) else { return String::new() };
        if let Some(n) = c.name.as_deref().filter(|n| !n.is_empty()) {
            return n.to_string();
        }
        let me = self.me();
        let names: Vec<String> = c
            .recipients
            .iter()
            .filter(|u| Some(u.id) != me)
            .map(|u| self.users.get(&u.id).unwrap_or(u).display_name().to_string())
            .collect();
        if names.is_empty() { format!("DM {}", c.id) } else { names.join(", ") }
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

#[cfg(test)]
mod tests;
