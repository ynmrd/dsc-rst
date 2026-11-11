//! Settings data model. Every struct is `#[serde(default)]`, so older or hand-edited
//! files with missing fields load with defaults and unknown fields are ignored.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

pub const SETTINGS_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub version: u32,
    pub appearance: Appearance,
    pub notifications: NotificationSettings,
    pub voice: VoiceSettings,
    pub behavior: Behavior,
    pub keybinds: Keybinds,
    pub advanced: Advanced,
    pub window: WindowState,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            version: SETTINGS_VERSION,
            appearance: Appearance::default(),
            notifications: NotificationSettings::default(),
            voice: VoiceSettings::default(),
            behavior: Behavior::default(),
            keybinds: Keybinds::default(),
            advanced: Advanced::default(),
            window: WindowState::default(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    #[default]
    Dark,
    Light,
    Midnight,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Appearance {
    pub theme: Theme,
    /// Clamped to `0.75..=2.0`.
    pub ui_scale: f32,
    pub font_size: f32,
    pub compact_mode: bool,
    pub show_avatars_in_compact: bool,
    pub reduce_motion: bool,
    pub use_24h_time: bool,
    pub show_embeds: bool,
    pub show_reactions: bool,
    pub show_images: bool,
    pub animate_emoji: bool,
    pub show_member_list: bool,
}

impl Default for Appearance {
    fn default() -> Self {
        Self {
            theme: Theme::Dark,
            ui_scale: 1.0,
            font_size: 15.0,
            compact_mode: false,
            show_avatars_in_compact: false,
            reduce_motion: false,
            use_24h_time: false,
            show_embeds: true,
            show_reactions: true,
            show_images: true,
            animate_emoji: false,
            show_member_list: true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NotifyLevel {
    All,
    #[default]
    Mentions,
    Nothing,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct NotificationSettings {
    pub enabled: bool,
    pub default_guild_level: NotifyLevel,
    pub dm_level: NotifyLevel,
    pub suppress_everyone: bool,
    pub flash_taskbar: bool,
    pub show_preview: bool,
    /// Channel id (decimal string) to level override.
    pub per_channel: BTreeMap<String, NotifyLevel>,
    pub per_guild: BTreeMap<String, NotifyLevel>,
    pub muted_channels: BTreeSet<String>,
    pub muted_guilds: BTreeSet<String>,
}

impl Default for NotificationSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            default_guild_level: NotifyLevel::Mentions,
            dm_level: NotifyLevel::All,
            suppress_everyone: false,
            flash_taskbar: true,
            show_preview: true,
            per_channel: BTreeMap::new(),
            per_guild: BTreeMap::new(),
            muted_channels: BTreeSet::new(),
            muted_guilds: BTreeSet::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InputMode {
    #[default]
    VoiceActivity,
    PushToTalk,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct VoiceSettings {
    /// `None` selects the system default device.
    pub input_device: Option<String>,
    pub output_device: Option<String>,
    /// Linear gain, `0.0..=2.0`.
    pub input_volume: f32,
    pub output_volume: f32,
    pub input_mode: InputMode,
    /// Voice-activity threshold, `-100.0..=0.0` dBFS.
    pub vad_threshold_db: f32,
    pub auto_sensitivity: bool,
    /// Key identifier (W3C `code` style, e.g. `Backquote`).
    pub ptt_key: String,
    pub ptt_release_delay_ms: u32,
    pub noise_suppression: bool,
    pub auto_gain: bool,
    pub bitrate_kbps: u32,
    /// User id (decimal string) to linear gain, `0.0..=2.0`.
    pub user_volumes: BTreeMap<String, f32>,
}

impl Default for VoiceSettings {
    fn default() -> Self {
        Self {
            input_device: None,
            output_device: None,
            input_volume: 1.0,
            output_volume: 1.0,
            input_mode: InputMode::VoiceActivity,
            vad_threshold_db: -50.0,
            auto_sensitivity: true,
            ptt_key: "Backquote".into(),
            ptt_release_delay_ms: 20,
            noise_suppression: true,
            auto_gain: true,
            bitrate_kbps: 64,
            user_volumes: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Behavior {
    pub start_on_boot: bool,
    pub start_minimized: bool,
    pub close_to_tray: bool,
    /// `true` renders with wgpu, `false` falls back to OpenGL (glow).
    pub hardware_acceleration: bool,
    pub developer_mode: bool,
    pub confirm_delete: bool,
    pub send_typing: bool,
}

impl Default for Behavior {
    fn default() -> Self {
        Self {
            start_on_boot: false,
            start_minimized: false,
            close_to_tray: true,
            hardware_acceleration: true,
            developer_mode: false,
            confirm_delete: true,
            send_typing: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Keybinds {
    pub toggle_mute: String,
    pub toggle_deafen: String,
    pub quick_switcher: String,
    pub search: String,
}

impl Default for Keybinds {
    fn default() -> Self {
        Self {
            toggle_mute: "Ctrl+Shift+M".into(),
            toggle_deafen: "Ctrl+Shift+D".into(),
            quick_switcher: "Ctrl+K".into(),
            search: "Ctrl+F".into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Advanced {
    pub intent_members: bool,
    pub intent_presences: bool,
    pub intent_message_content: bool,
    /// Cap on messages kept in the SQLite cache.
    pub cache_max_messages: u32,
    /// Messages kept in memory per open channel.
    pub memory_messages_per_channel: u32,
    pub image_cache_mb: u32,
    pub disk_cache_mb: u32,
    /// One of `error`, `warn`, `info`, `debug`, `trace`.
    pub log_level: String,
    pub gateway_compression: bool,
}

impl Default for Advanced {
    fn default() -> Self {
        Self {
            intent_members: true,
            intent_presences: true,
            intent_message_content: true,
            cache_max_messages: 200_000,
            memory_messages_per_channel: 600,
            image_cache_mb: 96,
            disk_cache_mb: 256,
            log_level: "info".into(),
            gateway_compression: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WindowState {
    pub width: f32,
    pub height: f32,
    pub maximized: bool,
}

impl Default for WindowState {
    fn default() -> Self {
        Self { width: 1280.0, height: 760.0, maximized: false }
    }
}
