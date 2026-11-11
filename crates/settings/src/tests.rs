use std::ffi::OsString;
use std::fs;

use super::*;

fn dir() -> tempfile::TempDir {
    tempfile::tempdir().unwrap()
}

fn names(dir: &Path) -> Vec<String> {
    let mut v: Vec<_> = fs::read_dir(dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    v.sort();
    v
}

#[test]
fn defaults_match_spec() {
    let s = Settings::default();
    assert_eq!(s.version, SETTINGS_VERSION);
    assert_eq!(s.appearance.theme, Theme::Dark);
    assert_eq!((s.appearance.ui_scale, s.appearance.font_size), (1.0, 15.0));
    assert!(!s.appearance.animate_emoji);
    assert_eq!(s.notifications.default_guild_level, NotifyLevel::Mentions);
    assert_eq!(s.notifications.dm_level, NotifyLevel::All);
    assert_eq!(s.voice.input_volume, 1.0);
    assert_eq!(s.voice.vad_threshold_db, -50.0);
    assert!(s.voice.auto_sensitivity && s.voice.noise_suppression && s.voice.auto_gain);
    assert_eq!((s.voice.ptt_key.as_str(), s.voice.ptt_release_delay_ms, s.voice.bitrate_kbps), ("Backquote", 20, 64));
    assert_eq!(s.voice.input_mode, InputMode::VoiceActivity);
    assert!(s.behavior.close_to_tray && s.behavior.hardware_acceleration && s.behavior.confirm_delete && s.behavior.send_typing);
    assert!(!s.behavior.start_on_boot && !s.behavior.start_minimized && !s.behavior.developer_mode);
    assert_eq!(s.keybinds.toggle_mute, "Ctrl+Shift+M");
    assert_eq!(s.keybinds.toggle_deafen, "Ctrl+Shift+D");
    assert_eq!(s.keybinds.quick_switcher, "Ctrl+K");
    assert_eq!(s.keybinds.search, "Ctrl+F");
    assert!(s.advanced.intent_members && s.advanced.intent_presences && s.advanced.intent_message_content);
    assert_eq!((s.advanced.cache_max_messages, s.advanced.memory_messages_per_channel), (200_000, 600));
    assert_eq!((s.advanced.image_cache_mb, s.advanced.disk_cache_mb), (96, 256));
    assert_eq!(s.advanced.log_level, "info");
    assert_eq!((s.window.width, s.window.height, s.window.maximized), (1280.0, 760.0, false));

    let mut sanitized = s.clone();
    sanitized.sanitize();
    assert_eq!(sanitized, s, "defaults are already within every clamp");
}

#[test]
fn defaults_roundtrip_through_file() {
    let d = dir();
    let path = d.path().join("settings.json");
    let defaults = Settings::default();
    defaults.save(&path).unwrap();
    assert_eq!(Settings::load(&path), defaults);

    let json = fs::read_to_string(&path).unwrap();
    assert!(json.contains("\"theme\": \"dark\""), "pretty, lowercase enum: {json}");
    assert!(json.contains("\"input_mode\": \"voice_activity\""));

    let mut custom = Settings::default();
    custom.appearance.theme = Theme::Midnight;
    custom.appearance.ui_scale = 1.25;
    custom.notifications.per_channel.insert("42".into(), NotifyLevel::Nothing);
    custom.notifications.muted_guilds.insert("7".into());
    custom.voice.input_device = Some("Microphone (USB)".into());
    custom.voice.user_volumes.insert("99".into(), 0.5);
    custom.voice.input_mode = InputMode::PushToTalk;
    custom.window = WindowState { width: 1500.5, height: 900.0, maximized: true };
    custom.save(&path).unwrap();
    assert_eq!(Settings::load(&path), custom);
}

#[test]
fn missing_file_gives_defaults_without_creating_anything() {
    let d = dir();
    assert_eq!(Settings::load(&d.path().join("settings.json")), Settings::default());
    assert!(names(d.path()).is_empty());
}

#[test]
fn unknown_fields_ignored_and_missing_defaulted() {
    let d = dir();
    let path = d.path().join("settings.json");
    fs::write(
        &path,
        r#"{
            "version": 0,
            "appearance": { "theme": "light", "retired_option": true },
            "voice": { "bitrate_kbps": 96 },
            "future_section": { "a": [1, 2, 3] }
        }"#,
    )
    .unwrap();
    let s = Settings::load(&path);
    assert_eq!(s.version, SETTINGS_VERSION);
    assert_eq!(s.appearance.theme, Theme::Light);
    assert_eq!(s.appearance.ui_scale, 1.0);
    assert_eq!(s.voice.bitrate_kbps, 96);
    assert_eq!(s.voice.ptt_key, "Backquote");
    assert_eq!(s.keybinds, Keybinds::default());
    assert_eq!(s.advanced, Advanced::default());
    assert_eq!(names(d.path()), vec!["settings.json"], "a clean load leaves no backup");
}

#[test]
fn corrupt_file_is_backed_up() {
    let d = dir();
    let path = d.path().join("settings.json");
    fs::write(&path, "{ not json at all").unwrap();
    assert_eq!(Settings::load(&path), Settings::default());
    let files = names(d.path());
    assert_eq!(files.len(), 1, "{files:?}");
    assert!(files[0].starts_with("settings.json.bad-"));
    assert_eq!(fs::read_to_string(d.path().join(&files[0])).unwrap(), "{ not json at all");

    // The next save works normally next to the backup.
    Settings::default().save(&path).unwrap();
    assert_eq!(names(d.path()).len(), 2);
}

#[test]
fn non_object_and_non_utf8_files_are_backed_up() {
    for content in [&b"[1, 2, 3]"[..], b"\"just a string\"", b"\xff\xfe\x00garbage", b""] {
        let d = dir();
        let path = d.path().join("settings.json");
        fs::write(&path, content).unwrap();
        assert_eq!(Settings::load(&path), Settings::default());
        assert!(names(d.path()).iter().any(|n| n.starts_with("settings.json.bad-")), "{content:?}");
    }
}

#[test]
fn utf8_bom_is_tolerated() {
    let d = dir();
    let path = d.path().join("settings.json");
    fs::write(&path, "\u{feff}{\"appearance\":{\"theme\":\"midnight\"}}").unwrap();
    assert_eq!(Settings::load(&path).appearance.theme, Theme::Midnight);
    assert_eq!(names(d.path()), vec!["settings.json"]);
}

#[test]
fn invalid_values_reset_only_themselves() {
    let d = dir();
    let path = d.path().join("settings.json");
    let original = r#"{
        "appearance": { "theme": "solarized", "ui_scale": 1.5, "compact_mode": "yes" },
        "notifications": { "dm_level": 5, "suppress_everyone": true, "per_channel": { "1": "nothing" } },
        "voice": { "input_mode": "push_to_talk", "bitrate_kbps": -4 },
        "window": "wide"
    }"#;
    fs::write(&path, original).unwrap();
    let s = Settings::load(&path);

    assert_eq!(s.appearance.theme, Theme::Dark);
    assert_eq!(s.appearance.ui_scale, 1.5);
    assert!(!s.appearance.compact_mode);
    assert_eq!(s.notifications.dm_level, NotifyLevel::All);
    assert!(s.notifications.suppress_everyone);
    assert_eq!(s.notifications.per_channel["1"], NotifyLevel::Nothing);
    assert_eq!(s.voice.input_mode, InputMode::PushToTalk);
    assert_eq!(s.voice.bitrate_kbps, 64);
    assert_eq!(s.window, WindowState::default());

    let files = names(d.path());
    assert_eq!(files.len(), 2, "{files:?}");
    let backup = files.iter().find(|n| n.starts_with("settings.json.bad-")).unwrap();
    assert_eq!(fs::read_to_string(d.path().join(backup)).unwrap(), original, "the original text is preserved");
}

#[test]
fn sanitize_clamps_ranges() {
    let mut s = Settings::default();
    s.appearance.ui_scale = 9.0;
    s.appearance.font_size = 1.0;
    s.voice.input_volume = -3.0;
    s.voice.output_volume = 50.0;
    s.voice.vad_threshold_db = -500.0;
    s.voice.ptt_release_delay_ms = 1_000_000;
    s.voice.bitrate_kbps = 100_000;
    s.voice.ptt_key = "  ".into();
    s.voice.user_volumes.insert("5".into(), 7.0);
    s.voice.user_volumes.insert("6".into(), f32::NAN);
    s.voice.user_volumes.insert("not-an-id".into(), 1.0);
    s.notifications.per_channel.insert("abc".into(), NotifyLevel::All);
    s.notifications.muted_channels.insert("x".into());
    s.notifications.muted_channels.insert("12".into());
    s.keybinds.search = String::new();
    s.advanced.cache_max_messages = 1;
    s.advanced.memory_messages_per_channel = 10_000_000;
    s.advanced.image_cache_mb = 0;
    s.advanced.disk_cache_mb = u32::MAX;
    s.advanced.log_level = "LOUD".into();
    s.window = WindowState { width: f32::INFINITY, height: 1.0, maximized: false };
    s.sanitize();

    assert_eq!(s.appearance.ui_scale, 2.0);
    assert_eq!(s.appearance.font_size, 10.0);
    assert_eq!((s.voice.input_volume, s.voice.output_volume), (0.0, 2.0));
    assert_eq!(s.voice.vad_threshold_db, -100.0);
    assert_eq!(s.voice.ptt_release_delay_ms, 2000);
    assert_eq!(s.voice.bitrate_kbps, 384);
    assert_eq!(s.voice.ptt_key, "Backquote");
    assert_eq!(s.voice.user_volumes.len(), 2);
    assert_eq!(s.voice.user_volumes["5"], 2.0);
    assert_eq!(s.voice.user_volumes["6"], 1.0);
    assert!(s.notifications.per_channel.is_empty());
    assert_eq!(s.notifications.muted_channels.len(), 1);
    assert_eq!(s.keybinds.search, "Ctrl+F");
    assert_eq!(s.advanced.cache_max_messages, 1_000);
    assert_eq!(s.advanced.memory_messages_per_channel, 5_000);
    assert_eq!(s.advanced.image_cache_mb, 8);
    assert_eq!(s.advanced.disk_cache_mb, 32_768);
    assert_eq!(s.advanced.log_level, "info");
    assert_eq!((s.window.width, s.window.height), (1280.0, 300.0));

    s.appearance.ui_scale = 0.1;
    s.appearance.font_size = f32::NAN;
    s.advanced.log_level = " DeBuG ".into();
    s.sanitize();
    assert_eq!((s.appearance.ui_scale, s.appearance.font_size), (0.75, 15.0));
    assert_eq!(s.advanced.log_level, "debug");
}

#[test]
fn load_sanitizes() {
    let d = dir();
    let path = d.path().join("settings.json");
    fs::write(&path, r#"{"appearance":{"ui_scale":99},"version":42}"#).unwrap();
    let s = Settings::load(&path);
    assert_eq!(s.appearance.ui_scale, 2.0);
    assert_eq!(s.version, SETTINGS_VERSION);
}

#[test]
fn atomic_save_leaves_no_temp_file() {
    let d = dir();
    let path = d.path().join("config/settings.json");
    let mut s = Settings::default();
    s.save(&path).unwrap();
    s.appearance.theme = Theme::Light;
    s.save(&path).unwrap();
    assert_eq!(names(&d.path().join("config")), vec!["settings.json"]);
    assert_eq!(Settings::load(&path).appearance.theme, Theme::Light);
}

#[test]
fn failed_save_cleans_up_and_keeps_previous_file() {
    let d = dir();
    let path = d.path().join("settings.json");
    Settings::default().save(&path).unwrap();
    let before = fs::read(&path).unwrap();

    // A directory squatting on the target makes the final rename fail.
    let blocked = d.path().join("blocked.json");
    fs::create_dir(&blocked).unwrap();
    assert!(Settings::default().save(&blocked).is_err());
    assert_eq!(names(d.path()), vec!["blocked.json", "settings.json"]);
    assert_eq!(fs::read(&path).unwrap(), before);

    // A file where a parent directory should be.
    assert!(Settings::default().save(&path.join("nested.json")).is_err());
}

#[test]
fn paths_under_layout() {
    let d = dir();
    let p = Paths::under(d.path());
    assert_eq!(p.config_dir, d.path().join("config"));
    assert_eq!(p.data_dir, d.path().join("data"));
    assert_eq!(p.cache_dir, d.path().join("cache"));
    assert_eq!(p.logs_dir, d.path().join("logs"));
    assert_eq!(p.settings_file(), d.path().join("config/settings.json"));
    assert_eq!(p.accounts_file(), d.path().join("config/accounts.json"));
    assert_eq!(p.database_file(), d.path().join("data/cache.sqlite3"));
    assert_eq!(p.image_cache_dir(), d.path().join("cache/images"));
    assert_eq!(p.crash_dir(), d.path().join("data/crashes"));

    assert!(!p.config_dir.exists());
    p.ensure().unwrap();
    p.ensure().unwrap();
    for dir in [&p.config_dir, &p.data_dir, &p.cache_dir, &p.logs_dir, &p.image_cache_dir(), &p.crash_dir()] {
        assert!(dir.is_dir(), "{}", dir.display());
    }
}

#[test]
fn dn_home_overrides_platform_dirs() {
    let d = dir();
    let home = OsString::from(d.path());
    assert_eq!(Paths::from_override(Some(home)), Paths::under(d.path()));

    let platform = Paths::from_override(None);
    assert_ne!(platform, Paths::under(d.path()));
    assert_eq!(Paths::from_override(Some(OsString::new())), platform, "empty DN_HOME counts as unset");
    assert_eq!(platform.logs_dir, platform.data_dir.join("logs"));
    assert!(platform.config_dir.to_string_lossy().to_lowercase().contains("discordnative"));
}

#[test]
fn notify_level_precedence() {
    let mut s = Settings::default();
    // Defaults: guild -> mentions, DM -> all.
    assert_eq!(s.notify_level_for(Some(10), 100), NotifyLevel::Mentions);
    assert_eq!(s.notify_level_for(None, 100), NotifyLevel::All);

    s.notifications.default_guild_level = NotifyLevel::Nothing;
    s.notifications.dm_level = NotifyLevel::Mentions;
    assert_eq!(s.notify_level_for(Some(10), 100), NotifyLevel::Nothing);
    assert_eq!(s.notify_level_for(None, 100), NotifyLevel::Mentions);

    s.notifications.per_guild.insert("10".into(), NotifyLevel::All);
    assert_eq!(s.notify_level_for(Some(10), 100), NotifyLevel::All);
    assert_eq!(s.notify_level_for(Some(11), 100), NotifyLevel::Nothing);
    assert_eq!(s.notify_level_for(None, 10), NotifyLevel::Mentions, "a guild override never applies to DMs");

    s.notifications.per_channel.insert("100".into(), NotifyLevel::Mentions);
    assert_eq!(s.notify_level_for(Some(10), 100), NotifyLevel::Mentions, "channel beats guild");
    assert_eq!(s.notify_level_for(Some(10), 101), NotifyLevel::All);
    s.notifications.per_channel.insert("200".into(), NotifyLevel::All);
    assert_eq!(s.notify_level_for(None, 200), NotifyLevel::All, "channel beats the DM default");
}

#[test]
fn mute_and_user_volume_lookups() {
    let mut s = Settings::default();
    assert!(!s.is_muted(Some(1), 2));
    s.notifications.muted_channels.insert("2".into());
    assert!(s.is_muted(Some(1), 2));
    assert!(s.is_muted(None, 2));
    assert!(!s.is_muted(Some(1), 3));
    s.notifications.muted_guilds.insert("1".into());
    assert!(s.is_muted(Some(1), 3));
    assert!(!s.is_muted(None, 3), "guild mutes do not apply to DMs");

    assert_eq!(s.user_volume(55), 1.0);
    s.voice.user_volumes.insert("55".into(), 0.25);
    assert_eq!(s.user_volume(55), 0.25);
}
