//! Discord REST/gateway object models (API v10). Only fields this client uses are
//! modeled; every struct is lenient (`#[serde(default)]`) so additive API changes
//! never break deserialization.

use serde::{Deserialize, Serialize};

use crate::cdn;
use crate::id::Id;
use crate::permissions::Permissions;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct User {
    pub id: Id,
    pub username: String,
    pub global_name: Option<String>,
    pub discriminator: String,
    pub avatar: Option<String>,
    pub bot: bool,
    pub system: bool,
    pub banner: Option<String>,
    pub accent_color: Option<u32>,
    pub public_flags: u64,
    /// Only present on mention objects in messages.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub member: Option<Box<Member>>,
}

impl User {
    pub fn display_name(&self) -> &str {
        self.global_name.as_deref().filter(|s| !s.is_empty()).unwrap_or(&self.username)
    }

    /// `username` or legacy `username#1234`.
    pub fn tag(&self) -> String {
        if self.discriminator.is_empty() || self.discriminator == "0" {
            self.username.clone()
        } else {
            format!("{}#{}", self.username, self.discriminator)
        }
    }

    pub fn avatar_url(&self, size: u32) -> String {
        match &self.avatar {
            Some(hash) => cdn::user_avatar(self.id, hash, size),
            None => cdn::default_avatar(self.id, &self.discriminator),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RoleColors {
    pub primary_color: u32,
    pub secondary_color: Option<u32>,
    pub tertiary_color: Option<u32>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Role {
    pub id: Id,
    pub name: String,
    pub color: u32,
    pub colors: Option<RoleColors>,
    pub hoist: bool,
    pub icon: Option<String>,
    pub unicode_emoji: Option<String>,
    pub position: i32,
    pub permissions: Permissions,
    pub managed: bool,
    pub mentionable: bool,
}

impl Role {
    pub fn display_color(&self) -> Option<u32> {
        let c = self.colors.as_ref().map(|c| c.primary_color).filter(|c| *c != 0).unwrap_or(self.color);
        (c != 0).then_some(c)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Member {
    pub user: Option<User>,
    pub nick: Option<String>,
    pub avatar: Option<String>,
    pub roles: Vec<Id>,
    pub joined_at: Option<String>,
    pub premium_since: Option<String>,
    pub deaf: bool,
    pub mute: bool,
    pub pending: bool,
    pub communication_disabled_until: Option<String>,
    /// Present on GUILD_MEMBER_* dispatches.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub guild_id: Option<Id>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "u8", into = "u8")]
pub enum OverwriteKind {
    #[default]
    Role,
    Member,
}

impl From<u8> for OverwriteKind {
    fn from(v: u8) -> Self {
        if v == 1 { OverwriteKind::Member } else { OverwriteKind::Role }
    }
}
impl From<OverwriteKind> for u8 {
    fn from(v: OverwriteKind) -> u8 {
        match v {
            OverwriteKind::Role => 0,
            OverwriteKind::Member => 1,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Overwrite {
    pub id: Id,
    #[serde(rename = "type")]
    pub kind: OverwriteKind,
    pub allow: Permissions,
    pub deny: Permissions,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(from = "u8", into = "u8")]
pub enum ChannelKind {
    #[default]
    GuildText,
    Dm,
    GuildVoice,
    GroupDm,
    GuildCategory,
    GuildAnnouncement,
    AnnouncementThread,
    PublicThread,
    PrivateThread,
    GuildStageVoice,
    GuildDirectory,
    GuildForum,
    GuildMedia,
    Unknown(u8),
}

impl From<u8> for ChannelKind {
    fn from(v: u8) -> Self {
        use ChannelKind::*;
        match v {
            0 => GuildText,
            1 => Dm,
            2 => GuildVoice,
            3 => GroupDm,
            4 => GuildCategory,
            5 => GuildAnnouncement,
            10 => AnnouncementThread,
            11 => PublicThread,
            12 => PrivateThread,
            13 => GuildStageVoice,
            14 => GuildDirectory,
            15 => GuildForum,
            16 => GuildMedia,
            o => Unknown(o),
        }
    }
}

impl From<ChannelKind> for u8 {
    fn from(v: ChannelKind) -> u8 {
        use ChannelKind::*;
        match v {
            GuildText => 0,
            Dm => 1,
            GuildVoice => 2,
            GroupDm => 3,
            GuildCategory => 4,
            GuildAnnouncement => 5,
            AnnouncementThread => 10,
            PublicThread => 11,
            PrivateThread => 12,
            GuildStageVoice => 13,
            GuildDirectory => 14,
            GuildForum => 15,
            GuildMedia => 16,
            Unknown(o) => o,
        }
    }
}

impl ChannelKind {
    pub fn is_text_like(self) -> bool {
        use ChannelKind::*;
        matches!(
            self,
            GuildText | Dm | GroupDm | GuildAnnouncement | AnnouncementThread | PublicThread | PrivateThread | GuildVoice | GuildStageVoice
        )
    }
    pub fn is_voice(self) -> bool {
        matches!(self, ChannelKind::GuildVoice | ChannelKind::GuildStageVoice)
    }
    pub fn is_thread(self) -> bool {
        matches!(self, ChannelKind::AnnouncementThread | ChannelKind::PublicThread | ChannelKind::PrivateThread)
    }
    pub fn is_private(self) -> bool {
        matches!(self, ChannelKind::Dm | ChannelKind::GroupDm)
    }
    pub fn is_forum(self) -> bool {
        matches!(self, ChannelKind::GuildForum | ChannelKind::GuildMedia)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ThreadMetadata {
    pub archived: bool,
    pub auto_archive_duration: u32,
    pub archive_timestamp: Option<String>,
    pub locked: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Channel {
    pub id: Id,
    #[serde(rename = "type")]
    pub kind: ChannelKind,
    pub guild_id: Option<Id>,
    pub position: i32,
    pub permission_overwrites: Vec<Overwrite>,
    pub name: Option<String>,
    pub topic: Option<String>,
    pub nsfw: bool,
    pub last_message_id: Option<Id>,
    pub bitrate: Option<u32>,
    pub user_limit: Option<u32>,
    pub rate_limit_per_user: Option<u32>,
    pub recipients: Vec<User>,
    pub icon: Option<String>,
    pub owner_id: Option<Id>,
    pub parent_id: Option<Id>,
    pub last_pin_timestamp: Option<String>,
    pub rtc_region: Option<String>,
    pub message_count: Option<u32>,
    pub member_count: Option<u32>,
    pub thread_metadata: Option<ThreadMetadata>,
    pub flags: u64,
    /// Tags offered by a forum/media channel.
    pub available_tags: Vec<ForumTag>,
    /// Tags applied to a forum/media post (thread).
    pub applied_tags: Vec<Id>,
}

/// Tag of a forum or media channel.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ForumTag {
    pub id: Id,
    pub name: String,
    /// Only members with MANAGE_THREADS may apply it.
    pub moderated: bool,
    pub emoji_id: Option<Id>,
    pub emoji_name: Option<String>,
}

impl Channel {
    /// Channel flag: new forum/media posts must carry at least one tag.
    pub const FLAG_REQUIRE_TAG: u64 = 1 << 4;
    /// Maximum tags per forum post.
    pub const MAX_APPLIED_TAGS: usize = 5;

    pub fn requires_tag(&self) -> bool {
        self.flags & Self::FLAG_REQUIRE_TAG != 0
    }

    /// Human readable name; DMs fall back to recipient names.
    pub fn display_name(&self) -> String {
        if let Some(n) = self.name.as_deref().filter(|n| !n.is_empty()) {
            return n.to_string();
        }
        if !self.recipients.is_empty() {
            return self.recipients.iter().map(|u| u.display_name()).collect::<Vec<_>>().join(", ");
        }
        format!("channel-{}", self.id)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(default)]
pub struct Emoji {
    pub id: Option<Id>,
    pub name: Option<String>,
    pub animated: bool,
    pub roles: Vec<Id>,
    pub require_colons: bool,
    pub managed: bool,
    pub available: Option<bool>,
}

impl Emoji {
    pub fn unicode(s: &str) -> Self {
        Emoji { name: Some(s.to_string()), ..Default::default() }
    }

    pub fn custom(id: Id, name: &str, animated: bool) -> Self {
        Emoji { id: Some(id), name: Some(name.to_string()), animated, ..Default::default() }
    }

    pub fn url(&self, size: u32) -> Option<String> {
        self.id.map(|id| cdn::emoji(id, self.animated, size))
    }

    /// Format used in the reaction REST routes: `name:id` for custom, raw for unicode.
    pub fn reaction_key(&self) -> String {
        match (self.id, self.name.as_deref()) {
            (Some(id), Some(name)) => format!("{name}:{id}"),
            (Some(id), None) => format!("_:{id}"),
            (None, Some(name)) => name.to_string(),
            (None, None) => String::new(),
        }
    }

    /// Format used inside message content.
    pub fn message_format(&self) -> String {
        match self.id {
            Some(id) => format!("<{}:{}:{}>", if self.animated { "a" } else { "" }, self.name.as_deref().unwrap_or("_"), id),
            None => self.name.clone().unwrap_or_default(),
        }
    }

    /// Identity used to match reactions (custom emoji by id, unicode by name).
    pub fn same_as(&self, other: &Emoji) -> bool {
        match (self.id, other.id) {
            (Some(a), Some(b)) => a == b,
            (None, None) => self.name == other.name,
            _ => false,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Sticker {
    pub id: Id,
    pub name: String,
    pub description: Option<String>,
    pub tags: String,
    pub format_type: u8,
    pub available: Option<bool>,
    pub guild_id: Option<Id>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct StickerItem {
    pub id: Id,
    pub name: String,
    pub format_type: u8,
}

impl StickerItem {
    pub const PNG: u8 = 1;
    pub const APNG: u8 = 2;
    pub const LOTTIE: u8 = 3;
    pub const GIF: u8 = 4;

    /// Raster URL, or None for Lottie stickers which this client does not render.
    pub fn url(&self, size: u32) -> Option<String> {
        (self.format_type != Self::LOTTIE).then(|| cdn::sticker(self.id, self.format_type, size))
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct UnavailableGuild {
    pub id: Id,
    pub unavailable: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Guild {
    pub id: Id,
    pub name: String,
    pub icon: Option<String>,
    pub banner: Option<String>,
    pub description: Option<String>,
    pub owner_id: Option<Id>,
    pub roles: Vec<Role>,
    pub emojis: Vec<Emoji>,
    pub stickers: Vec<Sticker>,
    pub features: Vec<String>,
    pub system_channel_id: Option<Id>,
    pub rules_channel_id: Option<Id>,
    pub premium_tier: u8,
    pub approximate_member_count: Option<u64>,
    // GUILD_CREATE only:
    pub joined_at: Option<String>,
    pub large: bool,
    pub unavailable: bool,
    pub member_count: Option<u64>,
    pub voice_states: Vec<VoiceState>,
    pub members: Vec<Member>,
    pub channels: Vec<Channel>,
    pub threads: Vec<Channel>,
    pub presences: Vec<Presence>,
}

impl Guild {
    pub fn icon_url(&self, size: u32) -> Option<String> {
        self.icon.as_ref().map(|h| cdn::guild_icon(self.id, h, size))
    }

    /// Up to three initials, like the official client's icon fallback.
    pub fn initials(&self) -> String {
        self.name.split_whitespace().filter_map(|w| w.chars().next()).take(3).collect()
    }
}

/// Partial guild returned by `GET /users/@me/guilds` (OAuth2 `guilds` scope).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PartialGuild {
    pub id: Id,
    pub name: String,
    pub icon: Option<String>,
    pub owner: bool,
    pub permissions: Permissions,
    pub features: Vec<String>,
    pub approximate_member_count: Option<u64>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Attachment {
    pub id: Id,
    pub filename: String,
    pub title: Option<String>,
    pub description: Option<String>,
    pub content_type: Option<String>,
    pub size: u64,
    pub url: String,
    pub proxy_url: String,
    pub height: Option<u32>,
    pub width: Option<u32>,
    pub ephemeral: bool,
    pub duration_secs: Option<f64>,
    pub flags: u64,
}

impl Attachment {
    pub fn is_image(&self) -> bool {
        self.content_type.as_deref().is_some_and(|c| c.starts_with("image/")) || (self.width.is_some() && !self.is_video())
    }
    pub fn is_video(&self) -> bool {
        self.content_type.as_deref().is_some_and(|c| c.starts_with("video/"))
    }
    pub fn is_spoiler(&self) -> bool {
        self.filename.starts_with("SPOILER_") || self.flags & (1 << 3) != 0
    }
    /// Media-proxy URL resized server-side so we never download full-size originals for previews.
    pub fn preview_url(&self, max_w: u32, max_h: u32) -> String {
        let base = if self.proxy_url.is_empty() { &self.url } else { &self.proxy_url };
        cdn::with_size_params(base, self.width, self.height, max_w, max_h)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct EmbedFooter {
    pub text: String,
    pub icon_url: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct EmbedMedia {
    pub url: String,
    pub proxy_url: Option<String>,
    pub height: Option<u32>,
    pub width: Option<u32>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct EmbedProvider {
    pub name: Option<String>,
    pub url: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct EmbedAuthor {
    pub name: String,
    pub url: Option<String>,
    pub icon_url: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct EmbedField {
    pub name: String,
    pub value: String,
    pub inline: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Embed {
    pub title: Option<String>,
    #[serde(rename = "type")]
    pub kind: Option<String>,
    pub description: Option<String>,
    pub url: Option<String>,
    pub timestamp: Option<String>,
    pub color: Option<u32>,
    pub footer: Option<EmbedFooter>,
    pub image: Option<EmbedMedia>,
    pub thumbnail: Option<EmbedMedia>,
    pub video: Option<EmbedMedia>,
    pub provider: Option<EmbedProvider>,
    pub author: Option<EmbedAuthor>,
    pub fields: Vec<EmbedField>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Reaction {
    pub count: u32,
    pub me: bool,
    pub emoji: Emoji,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MessageReference {
    #[serde(rename = "type")]
    pub kind: u8,
    pub message_id: Option<Id>,
    pub channel_id: Option<Id>,
    pub guild_id: Option<Id>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fail_if_not_exists: Option<bool>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PollMedia {
    pub text: Option<String>,
    pub emoji: Option<Emoji>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PollAnswer {
    pub answer_id: u32,
    pub poll_media: PollMedia,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PollAnswerCount {
    pub id: u32,
    pub count: u32,
    pub me_voted: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PollResults {
    pub is_finalized: bool,
    pub answer_counts: Vec<PollAnswerCount>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Poll {
    pub question: PollMedia,
    pub answers: Vec<PollAnswer>,
    pub expiry: Option<String>,
    pub allow_multiselect: bool,
    pub results: Option<PollResults>,
}

/// Message types the UI renders specially.
pub mod message_kind {
    pub const DEFAULT: u8 = 0;
    pub const RECIPIENT_ADD: u8 = 1;
    pub const RECIPIENT_REMOVE: u8 = 2;
    pub const CALL: u8 = 3;
    pub const CHANNEL_NAME_CHANGE: u8 = 4;
    pub const CHANNEL_PINNED_MESSAGE: u8 = 6;
    pub const USER_JOIN: u8 = 7;
    pub const GUILD_BOOST: u8 = 8;
    pub const THREAD_CREATED: u8 = 18;
    pub const REPLY: u8 = 19;
    pub const CHAT_INPUT_COMMAND: u8 = 20;
    pub const THREAD_STARTER_MESSAGE: u8 = 21;
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Message {
    pub id: Id,
    pub channel_id: Id,
    pub guild_id: Option<Id>,
    pub author: User,
    pub member: Option<Member>,
    pub content: String,
    pub timestamp: String,
    pub edited_timestamp: Option<String>,
    pub tts: bool,
    pub mention_everyone: bool,
    pub mentions: Vec<User>,
    pub mention_roles: Vec<Id>,
    pub attachments: Vec<Attachment>,
    pub embeds: Vec<Embed>,
    pub reactions: Vec<Reaction>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nonce: Option<serde_json::Value>,
    pub pinned: bool,
    pub webhook_id: Option<Id>,
    #[serde(rename = "type")]
    pub kind: u8,
    pub message_reference: Option<MessageReference>,
    pub referenced_message: Option<Box<Message>>,
    pub flags: u64,
    pub sticker_items: Vec<StickerItem>,
    pub thread: Option<Channel>,
    pub poll: Option<Poll>,
}

impl Message {
    pub fn timestamp_ms(&self) -> u64 {
        chrono::DateTime::parse_from_rfc3339(&self.timestamp)
            .map(|d| d.timestamp_millis() as u64)
            .unwrap_or_else(|_| self.id.timestamp_ms())
    }

    /// Name shown for the author: guild nickname, then global name, then username.
    pub fn author_name(&self) -> &str {
        self.member.as_ref().and_then(|m| m.nick.as_deref()).filter(|n| !n.is_empty()).unwrap_or_else(|| self.author.display_name())
    }

    pub fn mentions_user(&self, user: Id) -> bool {
        self.mention_everyone || self.mentions.iter().any(|u| u.id == user)
    }

    /// Nonce as a string; Discord echoes whatever type the sender used.
    pub fn nonce_string(&self) -> Option<String> {
        match self.nonce.as_ref()? {
            serde_json::Value::String(s) => Some(s.clone()),
            serde_json::Value::Number(n) => Some(n.to_string()),
            _ => None,
        }
    }

    pub fn is_reply(&self) -> bool {
        self.kind == message_kind::REPLY || self.message_reference.as_ref().is_some_and(|r| r.kind == 0 && r.message_id.is_some())
    }
}

/// MESSAGE_UPDATE payload: may be partial; absent fields mean "unchanged".
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct MessageUpdate {
    pub id: Id,
    pub channel_id: Id,
    pub guild_id: Option<Id>,
    pub content: Option<String>,
    pub edited_timestamp: Option<String>,
    pub mention_everyone: Option<bool>,
    pub mentions: Option<Vec<User>>,
    pub mention_roles: Option<Vec<Id>>,
    pub attachments: Option<Vec<Attachment>>,
    pub embeds: Option<Vec<Embed>>,
    pub pinned: Option<bool>,
    pub flags: Option<u64>,
    pub poll: Option<Poll>,
}

impl MessageUpdate {
    pub fn apply_to(&self, m: &mut Message) {
        if let Some(c) = &self.content {
            m.content.clone_from(c);
        }
        if self.edited_timestamp.is_some() {
            m.edited_timestamp.clone_from(&self.edited_timestamp);
        }
        if let Some(v) = self.mention_everyone {
            m.mention_everyone = v;
        }
        if let Some(v) = &self.mentions {
            m.mentions.clone_from(v);
        }
        if let Some(v) = &self.mention_roles {
            m.mention_roles.clone_from(v);
        }
        if let Some(v) = &self.attachments {
            m.attachments.clone_from(v);
        }
        if let Some(v) = &self.embeds {
            m.embeds.clone_from(v);
        }
        if let Some(v) = self.pinned {
            m.pinned = v;
        }
        if let Some(v) = self.flags {
            m.flags = v;
        }
        if self.poll.is_some() {
            m.poll.clone_from(&self.poll);
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct VoiceState {
    pub guild_id: Option<Id>,
    pub channel_id: Option<Id>,
    pub user_id: Id,
    pub member: Option<Member>,
    pub session_id: String,
    pub deaf: bool,
    pub mute: bool,
    pub self_deaf: bool,
    pub self_mute: bool,
    pub self_stream: bool,
    pub self_video: bool,
    pub suppress: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct VoiceServerUpdate {
    pub token: String,
    pub guild_id: Id,
    pub endpoint: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Online,
    Idle,
    Dnd,
    Invisible,
    #[default]
    #[serde(other)]
    Offline,
}

impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Online => "online",
            Status::Idle => "idle",
            Status::Dnd => "dnd",
            Status::Invisible => "invisible",
            Status::Offline => "offline",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Status::Online => "Online",
            Status::Idle => "Idle",
            Status::Dnd => "Do Not Disturb",
            Status::Invisible => "Invisible",
            Status::Offline => "Offline",
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Activity {
    pub name: String,
    #[serde(rename = "type")]
    pub kind: u8,
    pub state: Option<String>,
    pub details: Option<String>,
    pub url: Option<String>,
}

impl Activity {
    pub const PLAYING: u8 = 0;
    pub const STREAMING: u8 = 1;
    pub const LISTENING: u8 = 2;
    pub const WATCHING: u8 = 3;
    pub const CUSTOM: u8 = 4;
    pub const COMPETING: u8 = 5;

    pub fn describe(&self) -> String {
        match self.kind {
            Self::PLAYING => format!("Playing {}", self.name),
            Self::STREAMING => format!("Streaming {}", self.name),
            Self::LISTENING => format!("Listening to {}", self.name),
            Self::WATCHING => format!("Watching {}", self.name),
            Self::CUSTOM => self.state.clone().unwrap_or_default(),
            Self::COMPETING => format!("Competing in {}", self.name),
            _ => self.name.clone(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PresenceUser {
    pub id: Id,
    #[serde(flatten)]
    pub rest: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Presence {
    pub user: PresenceUser,
    pub guild_id: Option<Id>,
    pub status: Status,
    pub activities: Vec<Activity>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PartialApplication {
    pub id: Id,
    pub flags: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Ready {
    pub v: u8,
    pub user: User,
    pub guilds: Vec<UnavailableGuild>,
    pub session_id: String,
    pub resume_gateway_url: String,
    pub application: PartialApplication,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct GuildMembersChunk {
    pub guild_id: Id,
    pub members: Vec<Member>,
    pub chunk_index: u32,
    pub chunk_count: u32,
    pub not_found: Vec<Id>,
    pub presences: Vec<Presence>,
    pub nonce: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct GuildMemberRemove {
    pub guild_id: Id,
    pub user: User,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct GuildRoleEvent {
    pub guild_id: Id,
    pub role: Role,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct GuildRoleDelete {
    pub guild_id: Id,
    pub role_id: Id,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct GuildEmojisUpdate {
    pub guild_id: Id,
    pub emojis: Vec<Emoji>,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct GuildStickersUpdate {
    pub guild_id: Id,
    pub stickers: Vec<Sticker>,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct ThreadListSync {
    pub guild_id: Id,
    pub channel_ids: Option<Vec<Id>>,
    pub threads: Vec<Channel>,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct ChannelPinsUpdate {
    pub guild_id: Option<Id>,
    pub channel_id: Id,
    pub last_pin_timestamp: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct MessageDelete {
    pub id: Id,
    pub channel_id: Id,
    pub guild_id: Option<Id>,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct MessageDeleteBulk {
    pub ids: Vec<Id>,
    pub channel_id: Id,
    pub guild_id: Option<Id>,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct ReactionEvent {
    pub user_id: Id,
    pub channel_id: Id,
    pub message_id: Id,
    pub guild_id: Option<Id>,
    pub member: Option<Member>,
    pub emoji: Emoji,
    pub burst: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct ReactionRemoveAll {
    pub channel_id: Id,
    pub message_id: Id,
    pub guild_id: Option<Id>,
    /// Present only for MESSAGE_REACTION_REMOVE_EMOJI.
    pub emoji: Option<Emoji>,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct TypingStart {
    pub channel_id: Id,
    pub guild_id: Option<Id>,
    pub user_id: Id,
    pub timestamp: u64,
    pub member: Option<Member>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_message_with_unknown_fields_and_nulls() {
        let json = r#"{
            "id":"1","channel_id":"2","author":{"id":"3","username":"alice","global_name":null,"avatar":null,"discriminator":"0"},
            "content":"hi <@3>","timestamp":"2024-01-01T00:00:00.000000+00:00","edited_timestamp":null,
            "mentions":[{"id":"3","username":"alice","member":{"roles":[],"nick":"Al"}}],
            "attachments":[{"id":"9","filename":"a.png","size":10,"url":"https://cdn.discordapp.com/a.png","proxy_url":"https://media.discordapp.net/a.png","width":800,"height":600,"content_type":"image/png"}],
            "embeds":[],"type":0,"flags":0,"some_new_field":{"x":1},"referenced_message":null,
            "components":[]
        }"#;
        let m: Message = serde_json::from_str(json).unwrap();
        assert_eq!(m.author.display_name(), "alice");
        assert!(m.attachments[0].is_image());
        assert!(m.mentions_user(Id(3)));
        assert_eq!(m.mentions[0].member.as_ref().unwrap().nick.as_deref(), Some("Al"));
        assert!(m.referenced_message.is_none());
    }

    #[test]
    fn channel_kind_roundtrip() {
        for v in [0u8, 1, 2, 3, 4, 5, 10, 11, 12, 13, 14, 15, 16, 99] {
            let k = ChannelKind::from(v);
            assert_eq!(u8::from(k), v);
        }
        let c: Channel = serde_json::from_str(r#"{"id":"1","type":15}"#).unwrap();
        assert!(c.kind.is_forum());
        assert!(!c.requires_tag());
    }

    #[test]
    fn forum_tags_parse() {
        let json = r#"{"id":"1","type":15,"flags":16,"available_tags":[
            {"id":"10","name":"bug","moderated":false,"emoji_id":null,"emoji_name":"🐛"},
            {"id":"11","name":"staff","moderated":true,"emoji_id":"99","emoji_name":null}]}"#;
        let c: Channel = serde_json::from_str(json).unwrap();
        assert!(c.requires_tag());
        assert_eq!(c.available_tags.len(), 2);
        assert_eq!(c.available_tags[0].emoji_name.as_deref(), Some("🐛"));
        assert!(c.available_tags[1].moderated);
        assert_eq!(c.available_tags[1].emoji_id, Some(Id(99)));
        let post: Channel = serde_json::from_str(r#"{"id":"2","type":11,"parent_id":"1","applied_tags":["10"]}"#).unwrap();
        assert_eq!(post.applied_tags, vec![Id(10)]);
    }

    #[test]
    fn message_update_partial_apply() {
        let mut m = Message { content: "old".into(), pinned: false, ..Default::default() };
        let u: MessageUpdate = serde_json::from_str(r#"{"id":"1","channel_id":"2","pinned":true}"#).unwrap();
        u.apply_to(&mut m);
        assert_eq!(m.content, "old");
        assert!(m.pinned);
        let u: MessageUpdate =
            serde_json::from_str(r#"{"id":"1","channel_id":"2","content":"new","edited_timestamp":"2024-01-01T00:00:00Z"}"#).unwrap();
        u.apply_to(&mut m);
        assert_eq!(m.content, "new");
        assert!(m.edited_timestamp.is_some());
    }

    #[test]
    fn emoji_formats() {
        let e = Emoji::custom(Id(5), "blob", true);
        assert_eq!(e.reaction_key(), "blob:5");
        assert_eq!(e.message_format(), "<a:blob:5>");
        assert_eq!(Emoji::unicode("👍").reaction_key(), "👍");
    }

    #[test]
    fn status_unknown_is_offline() {
        let s: Status = serde_json::from_str("\"weird\"").unwrap();
        assert_eq!(s, Status::Offline);
    }
}
