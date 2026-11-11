//! UI-only state (selection, drafts, open dialogs). Server state lives in `dn_cache::Store`.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::Instant;

use dn_cache::bus::{SearchRequest, SearchSource};
use dn_core::{Emoji, Id, Member, Message, User};

use crate::virtual_list::HeightCache;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SettingsTab {
    #[default]
    Account,
    Voice,
    Notifications,
    Appearance,
    Keybinds,
    Advanced,
    Diagnostics,
    About,
}

#[derive(Debug, Clone)]
pub enum Modal {
    Settings(SettingsTab),
    ConfirmDelete { channel_id: Id, message_id: Id, preview: String },
    LinkWarning { url: String },
    Profile { user_id: Id, guild_id: Option<Id> },
    CreateThread { channel_id: Id, message_id: Id, name: String },
    QuickSwitcher { query: String, selected: usize },
    ImageViewer { url: String, original: String, width: u32, height: u32 },
    AddServer,
    Status { custom: String },
}

#[derive(Debug, Clone, PartialEq)]
pub enum EmojiTarget {
    Composer,
    Reaction { channel_id: Id, message_id: Id },
}

#[derive(Debug, Default)]
pub struct SearchState {
    pub open: bool,
    pub query: String,
    pub last: Option<SearchRequest>,
    pub loading: bool,
    pub total: u64,
    pub results: Vec<Message>,
    pub source: Option<SearchSource>,
    pub note: Option<String>,
}

#[derive(Debug)]
pub struct Toast {
    pub text: String,
    pub error: bool,
    pub until: Instant,
}

#[derive(Debug, Default)]
pub struct Composer {
    pub reply_to: Option<Id>,
    pub mention_reply: bool,
    pub attachments: Vec<PathBuf>,
    /// Display token -> wire format, e.g. "@alice" -> "<@123>".
    pub mentions: Vec<(String, String)>,
    pub completion_selected: usize,
}

/// "New post" form of a forum/media channel. Attachments live in that channel's `Composer`.
#[derive(Debug, Default)]
pub struct ForumDraft {
    pub open: bool,
    pub title: String,
    pub body: String,
    pub tags: Vec<Id>,
    /// Waiting for `UiEvent::ForumPostResult`.
    pub posting: bool,
    pub error: Option<String>,
}

#[derive(Debug, Default)]
pub struct OnboardingState {
    pub token: String,
    pub busy: bool,
    pub error: Option<String>,
    pub client_id: String,
    pub client_secret: String,
    pub port: String,
    pub oauth_busy: bool,
}

/// Per-channel list state for the virtualized message view.
#[derive(Debug, Default)]
pub struct ListState {
    pub heights: HeightCache,
    /// Anchor used to keep position stable when older messages are prepended.
    pub anchor: Option<(Id, f32)>,
    pub last_oldest: Option<Id>,
    pub at_bottom: bool,
    /// The read marker captured when the channel was opened ("NEW" divider).
    pub unread_marker: Option<Id>,
    pub scroll_to: Option<f32>,
    pub scroll_to_message: Option<Id>,
    pub flash: Option<(Id, Instant)>,
    pub mark_sent: Option<Id>,
}

#[derive(Default)]
pub struct UiState {
    pub selected_guild: Option<Id>,
    pub selected_channel: Option<Id>,
    pub last_channel_per_guild: HashMap<Option<Id>, Id>,
    pub drafts: HashMap<Id, String>,
    pub composer: HashMap<Id, Composer>,
    pub editing: Option<(Id, String)>,
    pub modal: Option<Modal>,
    pub search: SearchState,
    pub pins: Option<(Id, Vec<Message>, bool)>,
    pub toasts: Vec<Toast>,
    pub collapsed_categories: HashSet<Id>,
    pub revealed_spoilers: HashSet<(u64, usize)>,
    pub emoji_picker: Option<EmojiTarget>,
    pub emoji_query: String,
    /// Channel whose sticker picker is open.
    pub sticker_picker: Option<Id>,
    pub forum_drafts: HashMap<Id, ForumDraft>,
    pub lists: HashMap<Id, ListState>,
    pub typing_sent: Option<(Id, Instant)>,
    pub ptt_down: bool,
    pub profiles: HashMap<Id, (User, Option<Member>)>,
    pub onboarding: OnboardingState,
    pub hovered_message: Option<Id>,
    pub window_focused: bool,
    pub show_add_account: bool,
    pub voice_volume_popup: Option<Id>,
    pub file_dialog_rx: Option<crossbeam_channel::Receiver<(Id, Vec<PathBuf>)>>,
    pub save_dialog_rx: Option<crossbeam_channel::Receiver<(String, Option<PathBuf>)>>,
    pub recent_emoji: Vec<Emoji>,
    pub mic_test: bool,
    pub stress: Option<crate::stress::StressTest>,
}

impl UiState {
    pub fn toast(&mut self, text: impl Into<String>, error: bool) {
        self.toasts.push(Toast { text: text.into(), error, until: Instant::now() + std::time::Duration::from_secs(5) });
        if self.toasts.len() > 5 {
            self.toasts.remove(0);
        }
    }

    pub fn composer(&mut self, channel: Id) -> &mut Composer {
        self.composer.entry(channel).or_default()
    }

    pub fn list(&mut self, channel: Id) -> &mut ListState {
        self.lists.entry(channel).or_default()
    }

    pub fn push_recent_emoji(&mut self, e: Emoji) {
        self.recent_emoji.retain(|x| !x.same_as(&e));
        self.recent_emoji.insert(0, e);
        self.recent_emoji.truncate(24);
    }
}
