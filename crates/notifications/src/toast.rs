//! Windows backend: WinRT toasts for an unpackaged app identified by an AppUserModelID.

use std::path::{Path, PathBuf};

use crossbeam_channel::Sender;
use dn_core::Id;
use tauri_winrt_notification::{Duration, Toast};
use winreg::RegKey;
use winreg::enums::HKEY_CURRENT_USER;

use crate::Notifier;
use crate::content::Notification;
use crate::notifier::Backend;

/// Registers display name and icon for [`Notifier::APP_ID`]; without it Windows shows
/// toasts from an unknown app (or drops them). Best effort.
pub(crate) fn register_app(display_name: &str, icon: Option<&Path>) {
    if let Err(e) = write_registration(display_name, icon) {
        tracing::warn!(error = %e, "failed to register notification app identity");
    }
}

fn write_registration(display_name: &str, icon: Option<&Path>) -> std::io::Result<()> {
    let path = format!(r"Software\Classes\AppUserModelId\{}", Notifier::APP_ID);
    let (key, _) = RegKey::predef(HKEY_CURRENT_USER).create_subkey(path)?;
    key.set_value("DisplayName", &display_name)?;
    match icon {
        Some(icon) => {
            let absolute = std::path::absolute(icon).unwrap_or_else(|_| icon.to_path_buf());
            key.set_value("IconUri", &absolute.to_string_lossy().as_ref())
        }
        None => match key.delete_value("IconUri") {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        },
    }
}

pub(crate) struct SystemBackend;

impl SystemBackend {
    pub(crate) fn new(_display_name: &str, _icon_path: Option<PathBuf>) -> Self {
        Self
    }
}

impl Backend for SystemBackend {
    /// The activation callback runs on a WinRT thread while the toast is alive, so a click
    /// on the banner (or on it in the Action Center while the process still runs) is
    /// reported without any thread waiting per toast.
    fn deliver(&mut self, n: &Notification, activated: &Sender<Id>) -> Result<(), String> {
        let channel = n.channel_id;
        let activated = activated.clone();
        Toast::new(Notifier::APP_ID)
            .title(&n.title)
            .text1(&n.body)
            .duration(Duration::Short)
            .on_activated(move |_action| {
                let _ = activated.try_send(channel);
                Ok(())
            })
            .show()
            .map_err(|e| e.to_string())
    }
}
