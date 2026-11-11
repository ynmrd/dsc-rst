//! User settings model, atomic persistence and application directory layout.

mod model;
mod paths;
#[cfg(test)]
mod tests;

use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Deserialize;
use serde_json::Value;

pub use model::{
    Advanced, Appearance, Behavior, InputMode, Keybinds, NotificationSettings, NotifyLevel, SETTINGS_VERSION, Settings, Theme,
    VoiceSettings, WindowState,
};
pub use paths::Paths;

const LOG_LEVELS: [&str; 6] = ["error", "warn", "info", "debug", "trace", "off"];

impl Settings {
    /// Loads settings from `path`. A missing file yields defaults. Text that is not
    /// valid JSON is moved aside as `<name>.bad-<unix-secs>` and defaults are returned;
    /// a file that parses but has individual bad values keeps every valid value (a copy
    /// is saved as `.bad-<unix-secs>`). The result is always sanitized.
    pub fn load(path: &Path) -> Self {
        let mut settings = match fs::read(path) {
            Ok(bytes) => Self::from_bytes(path, &bytes),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Self::default(),
            Err(e) => {
                tracing::warn!("cannot read settings {}: {e}; using defaults", path.display());
                Self::default()
            }
        };
        settings.version = SETTINGS_VERSION;
        settings.sanitize();
        settings
    }

    fn from_bytes(path: &Path, bytes: &[u8]) -> Self {
        // Editors on Windows like to prepend a UTF-8 BOM.
        let parsed: Result<Value, String> = std::str::from_utf8(bytes)
            .map_err(|e| e.to_string())
            .and_then(|t| serde_json::from_str(t.trim_start_matches('\u{feff}')).map_err(|e| e.to_string()));
        let value = match parsed {
            Ok(v) => v,
            Err(reason) => {
                tracing::warn!("settings {} are not valid JSON ({reason}); using defaults", path.display());
                backup(path, true);
                return Self::default();
            }
        };
        match Self::deserialize(&value) {
            Ok(s) => s,
            Err(first) => match &value {
                Value::Object(incoming) => {
                    let (salvaged, dropped) = salvage(incoming);
                    tracing::warn!("settings {} have invalid values ({first}); reset {}", path.display(), dropped.join(", "));
                    backup(path, false);
                    salvaged
                }
                _ => {
                    tracing::warn!("settings {} are not a JSON object; using defaults", path.display());
                    backup(path, true);
                    Self::default()
                }
            },
        }
    }

    /// Writes pretty JSON atomically: a temp file in the same directory is synced and
    /// renamed over the target, so a crash never leaves a half-written file.
    pub fn save(&self, path: &Path) -> io::Result<()> {
        let json = serde_json::to_vec_pretty(self).map_err(io::Error::other)?;
        write_atomic(path, &json)
    }

    /// Clamps numeric fields to their valid ranges and repairs unusable values.
    pub fn sanitize(&mut self) {
        let a = &mut self.appearance;
        fix_f32(&mut a.ui_scale, 0.75, 2.0, 1.0);
        fix_f32(&mut a.font_size, 10.0, 28.0, 15.0);

        let n = &mut self.notifications;
        n.per_channel.retain(|k, _| is_id(k));
        n.per_guild.retain(|k, _| is_id(k));
        n.muted_channels.retain(|k| is_id(k));
        n.muted_guilds.retain(|k| is_id(k));

        let v = &mut self.voice;
        fix_f32(&mut v.input_volume, 0.0, 2.0, 1.0);
        fix_f32(&mut v.output_volume, 0.0, 2.0, 1.0);
        fix_f32(&mut v.vad_threshold_db, -100.0, 0.0, -50.0);
        v.ptt_release_delay_ms = v.ptt_release_delay_ms.min(2000);
        v.bitrate_kbps = v.bitrate_kbps.clamp(8, 384);
        if v.ptt_key.trim().is_empty() {
            v.ptt_key = VoiceSettings::default().ptt_key;
        }
        v.user_volumes.retain(|k, _| is_id(k));
        v.user_volumes.values_mut().for_each(|g| fix_f32(g, 0.0, 2.0, 1.0));

        let k = &mut self.keybinds;
        let defaults = Keybinds::default();
        for (value, default) in [
            (&mut k.toggle_mute, defaults.toggle_mute),
            (&mut k.toggle_deafen, defaults.toggle_deafen),
            (&mut k.quick_switcher, defaults.quick_switcher),
            (&mut k.search, defaults.search),
        ] {
            if value.trim().is_empty() {
                *value = default;
            }
        }

        let adv = &mut self.advanced;
        adv.cache_max_messages = adv.cache_max_messages.clamp(1_000, 5_000_000);
        adv.memory_messages_per_channel = adv.memory_messages_per_channel.clamp(50, 5_000);
        adv.image_cache_mb = adv.image_cache_mb.clamp(8, 4_096);
        adv.disk_cache_mb = adv.disk_cache_mb.clamp(16, 32_768);
        let level = adv.log_level.trim().to_ascii_lowercase();
        adv.log_level = if LOG_LEVELS.contains(&level.as_str()) { level } else { Advanced::default().log_level };

        let w = &mut self.window;
        fix_f32(&mut w.width, 400.0, 16_384.0, 1280.0);
        fix_f32(&mut w.height, 300.0, 16_384.0, 760.0);
    }

    /// Per-user playback gain; 1.0 when no override is stored.
    pub fn user_volume(&self, user: u64) -> f32 {
        self.voice.user_volumes.get(&user.to_string()).copied().unwrap_or(1.0)
    }

    /// Notification level with precedence channel override, then guild override, then
    /// the default (`dm_level` for DMs, `default_guild_level` for guild channels).
    /// Muting is separate; see [`Settings::is_muted`].
    pub fn notify_level_for(&self, guild: Option<u64>, channel: u64) -> NotifyLevel {
        let n = &self.notifications;
        if let Some(level) = n.per_channel.get(&channel.to_string()) {
            return *level;
        }
        match guild {
            None => n.dm_level,
            Some(g) => n.per_guild.get(&g.to_string()).copied().unwrap_or(n.default_guild_level),
        }
    }

    /// True when the channel, or the guild it belongs to, is muted.
    pub fn is_muted(&self, guild: Option<u64>, channel: u64) -> bool {
        let n = &self.notifications;
        n.muted_channels.contains(&channel.to_string()) || guild.is_some_and(|g| n.muted_guilds.contains(&g.to_string()))
    }
}

/// Writes `data` to `path` atomically (temp file in the same directory, fsync, rename).
pub fn write_atomic(path: &Path, data: &[u8]) -> io::Result<()> {
    let dir = path.parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or(Path::new("."));
    fs::create_dir_all(dir)?;
    let mut tmp_name = path.file_name().ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "path has no file name"))?.to_os_string();
    tmp_name.push(".tmp");
    let tmp = dir.join(tmp_name);

    let result = (|| {
        let mut file = File::create(&tmp)?;
        file.write_all(data)?;
        file.flush()?;
        file.sync_all()?;
        drop(file);
        rename_replacing(&tmp, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
        return result;
    }
    #[cfg(unix)]
    if let Ok(d) = File::open(dir) {
        let _ = d.sync_all();
    }
    Ok(())
}

/// On Windows a briefly open target (virus scanner, indexer) makes the rename fail with
/// `PermissionDenied`; a few short retries ride that out.
fn rename_replacing(from: &Path, to: &Path) -> io::Result<()> {
    let mut retries = 0;
    loop {
        match fs::rename(from, to) {
            Err(e) if cfg!(windows) && e.kind() == io::ErrorKind::PermissionDenied && retries < 5 => {
                retries += 1;
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            result => return result,
        }
    }
}

fn is_id(key: &str) -> bool {
    key.parse::<u64>().is_ok()
}

fn fix_f32(v: &mut f32, min: f32, max: f32, default: f32) {
    *v = if v.is_finite() { v.clamp(min, max) } else { default };
}

fn backup(path: &Path, rename: bool) {
    let secs = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let mut name = path.file_name().map(|n| n.to_os_string()).unwrap_or_default();
    name.push(format!(".bad-{secs}"));
    let dest: PathBuf = path.with_file_name(name);
    let result =
        if rename { fs::rename(path, &dest).or_else(|_| fs::copy(path, &dest).map(|_| ())) } else { fs::copy(path, &dest).map(|_| ()) };
    match result {
        Ok(()) => tracing::warn!("saved the unreadable settings as {}", dest.display()),
        Err(e) => tracing::warn!("could not back up settings to {}: {e}", dest.display()),
    }
}

/// Builds settings from a JSON object that fails strict deserialization by taking each
/// value only if the whole document stays valid with it, and returns the dotted paths
/// of the values that had to be dropped.
fn salvage(incoming: &serde_json::Map<String, Value>) -> (Settings, Vec<String>) {
    let mut root = serde_json::to_value(Settings::default()).unwrap_or(Value::Null);
    let mut dropped = Vec::new();
    merge_valid(&mut root, incoming, &mut Vec::new(), &mut dropped);
    (Settings::deserialize(&root).unwrap_or_default(), dropped)
}

fn slot<'a>(root: &'a mut Value, path: &[String]) -> Option<&'a mut Value> {
    path.iter().try_fold(root, |v, key| v.get_mut(key))
}

fn merge_valid(root: &mut Value, incoming: &serde_json::Map<String, Value>, path: &mut Vec<String>, dropped: &mut Vec<String>) {
    for (key, value) in incoming {
        path.push(key.clone());
        let Some(current) = slot(root, path) else {
            // Unknown to the model: ignored, exactly as strict parsing would.
            path.pop();
            continue;
        };
        // Structs serialize as non-empty objects; empty objects are maps and replaced whole.
        let is_section = current.as_object().is_some_and(|o| !o.is_empty());
        match (is_section, value) {
            (true, Value::Object(inner)) => merge_valid(root, inner, path, dropped),
            _ => {
                let previous = std::mem::replace(current, value.clone());
                if Settings::deserialize(&*root).is_err() {
                    if let Some(current) = slot(root, path) {
                        *current = previous;
                    }
                    dropped.push(path.join("."));
                }
            }
        }
        path.pop();
    }
}
