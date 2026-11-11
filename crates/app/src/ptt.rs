//! Global push-to-talk on Windows: polls the configured key with GetAsyncKeyState so PTT
//! works while another application has focus. Only polls while in a voice channel with PTT on.

use std::time::Duration;

use dn_cache::bus::{Command, UiLink};
use dn_settings::InputMode;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;

/// Map egui key names (as stored in settings) to Windows virtual-key codes.
pub fn vk_for(name: &str) -> Option<i32> {
    let n = name.trim();
    if n.len() == 1 {
        let c = n.chars().next()?.to_ascii_uppercase();
        if c.is_ascii_alphanumeric() {
            return Some(c as i32);
        }
    }
    if let Some(f) = n.strip_prefix('F').and_then(|d| d.parse::<i32>().ok()) {
        if (1..=24).contains(&f) {
            return Some(0x70 + f - 1);
        }
    }
    Some(match n {
        "Backtick" | "Backquote" | "`" => 0xC0,
        "Space" => 0x20,
        "Tab" => 0x09,
        "Enter" => 0x0D,
        "Escape" => 0x1B,
        "Backspace" => 0x08,
        "Insert" => 0x2D,
        "Delete" => 0x2E,
        "Home" => 0x24,
        "End" => 0x23,
        "PageUp" => 0x21,
        "PageDown" => 0x22,
        "ArrowLeft" => 0x25,
        "ArrowUp" => 0x26,
        "ArrowRight" => 0x27,
        "ArrowDown" => 0x28,
        "Minus" | "-" => 0xBD,
        "Equals" | "=" => 0xBB,
        "OpenBracket" | "[" => 0xDB,
        "CloseBracket" | "]" => 0xDD,
        "Backslash" | "\\" => 0xDC,
        "Semicolon" | ";" => 0xBA,
        "Quote" | "'" => 0xDE,
        "Comma" | "," => 0xBC,
        "Period" | "." => 0xBE,
        "Slash" | "/" => 0xBF,
        _ => return None,
    })
}

pub fn spawn(link: UiLink) {
    let _ = std::thread::Builder::new().name("dn-ptt".into()).spawn(move || {
        let mut down = false;
        loop {
            let (active, key) = {
                let s = link.settings.read();
                let ptt = s.voice.input_mode == InputMode::PushToTalk;
                let in_voice = link.store.try_lock().map(|st| st.voice.conn.channel().is_some()).unwrap_or(down);
                (ptt && in_voice, s.voice.ptt_key.clone())
            };
            if !active {
                if down {
                    down = false;
                    link.send(Command::PushToTalk(false));
                }
                std::thread::sleep(Duration::from_millis(250));
                continue;
            }
            let pressed = vk_for(&key).is_some_and(|vk| unsafe { GetAsyncKeyState(vk) } as u16 & 0x8000 != 0);
            if pressed != down {
                down = pressed;
                link.send(Command::PushToTalk(pressed));
            }
            if link.commands.is_closed() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    });
}
