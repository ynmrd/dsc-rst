//! Start-on-boot registration (Windows Run key, XDG autostart on Linux).

#[cfg(windows)]
pub fn set_autostart(enabled: bool) -> Result<(), String> {
    use winreg::RegKey;
    use winreg::enums::{HKEY_CURRENT_USER, KEY_SET_VALUE};

    const RUN: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let key = hkcu.open_subkey_with_flags(RUN, KEY_SET_VALUE).map_err(|e| e.to_string())?;
    if enabled {
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        key.set_value("DiscordNative", &format!("\"{}\" --minimized", exe.display())).map_err(|e| e.to_string())
    } else {
        match key.delete_value("DiscordNative") {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.to_string()),
        }
    }
}

#[cfg(target_os = "linux")]
pub fn set_autostart(enabled: bool) -> Result<(), String> {
    let dir = std::env::var_os("XDG_CONFIG_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".config")))
        .ok_or("no home directory")?
        .join("autostart");
    autostart_desktop_file(&dir, enabled)
}

#[cfg(not(any(windows, target_os = "linux")))]
pub fn set_autostart(_enabled: bool) -> Result<(), String> {
    Err("start on boot is not supported on this platform".into())
}

#[cfg(any(target_os = "linux", test))]
fn autostart_desktop_file(dir: &std::path::Path, enabled: bool) -> Result<(), String> {
    let file = dir.join("discord-native.desktop");
    if !enabled {
        return match std::fs::remove_file(&file) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.to_string()),
            _ => Ok(()),
        };
    }
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let body = format!(
        "[Desktop Entry]\nType=Application\nName=Discord Native\nExec=\"{}\" --minimized\nX-GNOME-Autostart-enabled=true\n",
        exe.display()
    );
    std::fs::write(&file, body).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    #[test]
    fn desktop_file_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let auto = dir.path().join("autostart");
        super::autostart_desktop_file(&auto, true).unwrap();
        let body = std::fs::read_to_string(auto.join("discord-native.desktop")).unwrap();
        assert!(body.contains("--minimized"));
        super::autostart_desktop_file(&auto, false).unwrap();
        assert!(!auto.join("discord-native.desktop").exists());
        // Disabling twice is fine.
        super::autostart_desktop_file(&auto, false).unwrap();
    }
}
