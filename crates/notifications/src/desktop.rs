//! Linux/BSD (freedesktop D-Bus) and macOS backend on top of notify-rust.

use std::path::Path;
use std::path::PathBuf;

use crossbeam_channel::Sender;
use dn_core::Id;

use crate::content::Notification;
use crate::notifier::Backend;

/// Nothing to register outside Windows.
pub(crate) fn register_app(_display_name: &str, _icon: Option<&Path>) {}

pub(crate) struct SystemBackend {
    app_name: String,
    icon: Option<String>,
    /// Whether the notification server renders a markup subset in bodies (queried once).
    #[cfg(all(unix, not(target_os = "macos")))]
    body_markup: Option<bool>,
}

impl SystemBackend {
    pub(crate) fn new(display_name: &str, icon_path: Option<PathBuf>) -> Self {
        Self {
            app_name: display_name.to_owned(),
            icon: icon_path.map(|p| p.to_string_lossy().into_owned()),
            #[cfg(all(unix, not(target_os = "macos")))]
            body_markup: None,
        }
    }
}

impl Backend for SystemBackend {
    fn deliver(&mut self, n: &Notification, activated: &Sender<Id>) -> Result<(), String> {
        let mut toast = notify_rust::Notification::new();
        toast.appname(&self.app_name).summary(&n.title);
        if let Some(icon) = &self.icon {
            toast.icon(icon);
        }
        self.send(toast, n, activated)
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
impl SystemBackend {
    fn send(&mut self, mut toast: notify_rust::Notification, n: &Notification, activated: &Sender<Id>) -> Result<(), String> {
        let body = if self.supports_body_markup() { escape_markup(&n.body) } else { n.body.clone() };
        // "default" is the action servers invoke when the notification body is clicked.
        toast.body(&body).action("default", "Open");
        let handle = toast.show().map_err(|e| e.to_string())?;
        click::watch(handle, n.channel_id, activated.clone());
        Ok(())
    }

    fn supports_body_markup(&mut self) -> bool {
        if let Some(known) = self.body_markup {
            return known;
        }
        match notify_rust::get_capabilities() {
            Ok(caps) => {
                let supported = caps.iter().any(|c| c == "body-markup");
                self.body_markup = Some(supported);
                supported
            }
            Err(_) => false,
        }
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
mod click {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use crossbeam_channel::Sender;
    use dn_core::Id;

    /// Each watcher parks a thread until its notification closes; cap them so servers that
    /// never report closure cannot accumulate threads.
    const MAX_WATCHERS: usize = 16;
    static WATCHERS: AtomicUsize = AtomicUsize::new(0);

    pub(super) fn watch(handle: notify_rust::NotificationHandle, channel: Id, activated: Sender<Id>) {
        if WATCHERS.fetch_add(1, Ordering::SeqCst) >= MAX_WATCHERS {
            WATCHERS.fetch_sub(1, Ordering::SeqCst);
            return;
        }
        let spawned = std::thread::Builder::new().name("dn-notify-click".into()).spawn(move || {
            handle.wait_for_action(|action| {
                if action == "default" {
                    let _ = activated.try_send(channel);
                }
            });
            WATCHERS.fetch_sub(1, Ordering::SeqCst);
        });
        if spawned.is_err() {
            WATCHERS.fetch_sub(1, Ordering::SeqCst);
        }
    }
}

#[cfg(not(all(unix, not(target_os = "macos"))))]
impl SystemBackend {
    /// No click reporting on this platform.
    fn send(&mut self, mut toast: notify_rust::Notification, n: &Notification, _activated: &Sender<Id>) -> Result<(), String> {
        toast.body(&n.body).show().map(|_| ()).map_err(|e| e.to_string())
    }
}

/// Servers advertising `body-markup` parse `<b>`, `<i>`, `<a>`, ...; user text must not be
/// mistaken for tags.
#[cfg(all(unix, not(target_os = "macos")))]
fn escape_markup(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            _ => out.push(c),
        }
    }
    out
}

#[cfg(all(test, unix, not(target_os = "macos")))]
mod tests {
    use super::*;

    #[test]
    fn markup_characters_are_escaped() {
        assert_eq!(escape_markup("a < b && <b>c</b>"), "a &lt; b &amp;&amp; &lt;b&gt;c&lt;/b&gt;");
        assert_eq!(escape_markup("plain é 😀"), "plain é 😀");
    }
}
