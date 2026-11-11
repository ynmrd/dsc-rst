use std::fmt;

use dn_core::{Channel, Id, Message, User};
use serde::{Deserialize, Serialize};

/// `GET /applications/@me`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Application {
    pub id: Id,
    pub name: String,
    pub icon: Option<String>,
    pub description: String,
    pub bot_public: bool,
    pub bot_require_code_grant: bool,
    pub flags: u64,
    pub owner: Option<User>,
    pub approximate_guild_count: Option<u64>,
}

/// `GET /gateway/bot`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GatewayBot {
    pub url: String,
    pub shards: u32,
    pub session_start_limit: SessionStartLimit,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SessionStartLimit {
    pub total: u32,
    pub remaining: u32,
    /// Milliseconds until `remaining` resets.
    pub reset_after: u64,
    pub max_concurrency: u32,
}

/// Message history page; at most one of `before`/`after`/`around` is sent
/// (precedence: around, before, after).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MessagePage {
    pub before: Option<Id>,
    pub after: Option<Id>,
    pub around: Option<Id>,
    /// 1..=100, clamped.
    pub limit: u8,
}

impl Default for MessagePage {
    fn default() -> Self {
        MessagePage { before: None, after: None, around: None, limit: 50 }
    }
}

impl MessagePage {
    pub fn latest(limit: u8) -> Self {
        MessagePage { limit, ..Default::default() }
    }
    pub fn before(id: Id, limit: u8) -> Self {
        MessagePage { before: Some(id), limit, ..Default::default() }
    }
    pub fn after(id: Id, limit: u8) -> Self {
        MessagePage { after: Some(id), limit, ..Default::default() }
    }
    pub fn around(id: Id, limit: u8) -> Self {
        MessagePage { around: Some(id), limit, ..Default::default() }
    }
}

/// Body of `POST /channels/{id}/messages`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CreateMessage {
    pub content: String,
    /// Up to 25 characters; enables `enforce_nonce` deduplication.
    pub nonce: Option<String>,
    pub reply_to: Option<Id>,
    pub mention_replied_user: bool,
    pub sticker_ids: Vec<Id>,
    pub files: Vec<FileUpload>,
    pub tts: bool,
}

/// Body of `POST /channels/{id}/threads` for forum and media channels.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CreateForumPost {
    /// Post title, 1-100 characters.
    pub name: String,
    /// Opening message; `content`, `sticker_ids`, `files` and `mention_replied_user` are used.
    pub message: CreateMessage,
    /// Up to five tag ids from the channel's `available_tags`.
    pub applied_tags: Vec<Id>,
}

/// Response to [`CreateForumPost`]: the new thread with its opening message nested.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct ForumPost {
    #[serde(flatten)]
    pub channel: Channel,
    #[serde(default)]
    pub message: Option<Message>,
}

#[derive(Clone, Default, PartialEq)]
pub struct FileUpload {
    pub filename: String,
    pub bytes: Vec<u8>,
    pub content_type: Option<String>,
    pub spoiler: bool,
}

impl FileUpload {
    /// Name sent to Discord: path components stripped, `SPOILER_` prefix when needed.
    pub fn upload_name(&self) -> String {
        let base = self.filename.rsplit(['/', '\\']).next().unwrap_or_default();
        let base: String = base.chars().filter(|c| !c.is_control() && *c != '"').collect();
        let base = if base.is_empty() { "file".to_string() } else { base };
        if self.spoiler && !base.starts_with("SPOILER_") { format!("SPOILER_{base}") } else { base }
    }
}

impl fmt::Debug for FileUpload {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FileUpload")
            .field("filename", &self.filename)
            .field("len", &self.bytes.len())
            .field("content_type", &self.content_type)
            .field("spoiler", &self.spoiler)
            .finish()
    }
}

/// Guild message search parameters (`GET /guilds/{id}/messages/search`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SearchQuery {
    pub content: Option<String>,
    pub channel_ids: Vec<Id>,
    pub author_ids: Vec<Id>,
    /// `link`, `embed`, `poll`, `file`, `video`, `image`, `sound`, `sticker`, `snapshot` (prefix `-` to negate).
    pub has: Vec<String>,
    /// 1..=25; 0 means the default (25).
    pub limit: u8,
    /// Max 9975.
    pub offset: u32,
    /// `timestamp` or `relevance`.
    pub sort_by: Option<String>,
    /// `asc` or `desc`.
    pub sort_order: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum SearchOutcome {
    Results {
        total: u64,
        messages: Vec<Message>,
    },
    /// The guild is still being indexed (HTTP 202); retry after the given seconds.
    Indexing {
        retry_after: f64,
    },
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PinnedMessage {
    pub pinned_at: String,
    pub message: Message,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PinsPage {
    pub items: Vec<PinnedMessage>,
    pub has_more: bool,
}

#[derive(Deserialize, Default)]
#[serde(default)]
pub(crate) struct SearchResponse {
    pub total_results: u64,
    pub messages: Vec<Vec<Message>>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
pub(crate) struct IndexingResponse {
    pub retry_after: Option<f64>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
pub(crate) struct ThreadsResponse {
    pub threads: Vec<dn_core::Channel>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
pub(crate) struct GatewayResponse {
    pub url: String,
}

#[derive(Deserialize, Default)]
#[serde(default)]
pub(crate) struct RateLimitBody {
    pub retry_after: Option<f64>,
    pub global: bool,
}
