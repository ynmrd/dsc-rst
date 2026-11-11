//! Full-screen settings overlay.

use egui::{CornerRadius, RichText, Ui, vec2};

use dn_cache::Store;
use dn_cache::bus::Command;
use dn_settings::{InputMode, NotifyLevel, Settings, Theme};

use crate::state::SettingsTab;
use crate::theme::Palette;
use crate::{DiscordApp, widgets};

pub enum Outcome {
    Stay,
    Switch(SettingsTab),
    Close,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Support {
    Supported,
    AsBot,
    Partial,
    Unverified,
    Blocked,
}

impl Support {
    pub fn label(self) -> &'static str {
        match self {
            Support::Supported => "Supported",
            Support::AsBot => "Supported (as your bot)",
            Support::Partial => "Partial",
            Support::Unverified => "Implemented, unverified",
            Support::Blocked => "Blocked by Discord",
        }
    }
}

/// Feature compatibility matrix shown in the About tab (kept in sync with docs/COMPATIBILITY.md).
pub const MATRIX: &[(&str, Support, &str)] = &[
    ("Sign in with your Discord account (OAuth2 + PKCE)", Support::Supported, "Profile, avatar and server list via identify/guilds scopes"),
    ("Chat as your personal account", Support::Blocked, "No OAuth2 scope permits it; user tokens/self-bots violate the ToS"),
    ("Bot account sign-in, session restore, switching, logout", Support::Supported, "Token kept in the OS credential manager"),
    ("Servers, channels, categories, permissions", Support::AsBot, "Servers where your bot was added"),
    ("Send / receive / edit / delete messages", Support::AsBot, "Real-time via gateway v10"),
    ("Replies, mentions, reactions, custom emoji", Support::AsBot, "Custom emoji from servers the bot is in"),
    ("Attachments upload/download, image previews", Support::AsBot, "Previews are server-side resized"),
    ("Embeds, stickers (PNG/APNG/GIF), polls (view)", Support::Partial, "Lottie stickers and voting are not available to bots"),
    ("Message history, unread markers", Support::AsBot, "Read state is local (bots have none server-side)"),
    ("Search", Support::AsBot, "Discord guild search (needs Message Content intent) + local cache"),
    ("Pins, threads, typing indicators", Support::AsBot, "Forum post creation not implemented"),
    ("Direct messages", Support::Partial, "Only DMs with the bot; your personal DMs are not accessible"),
    ("Group DMs, friends list, friend requests", Support::Blocked, "Not exposed to bots/OAuth2 apps"),
    ("Presence / status", Support::AsBot, "Sets the bot's status; others' presence needs the Presence intent"),
    ("Voice channels: join/leave, two-way audio, mute/deafen", Support::Unverified, "Songbird driver with DAVE E2EE; not yet tested against official clients"),
    ("Per-user volume, VAD, push-to-talk, noise suppression", Support::Unverified, "Local audio pipeline is tested; Discord interop untested"),
    ("Video, webcam, screen sharing (Go Live)", Support::Blocked, "No documented API for bots or third-party clients"),
    ("GIF picker", Support::Blocked, "Discord's picker uses private endpoints; Tenor API shut down 2026"),
    ("Rich Presence / game activity for your account", Support::Blocked, "RPC scopes are partner-only"),
    ("Desktop notifications, tray, start on boot", Support::Supported, "Windows toasts, tray icon, Run key"),
];

pub fn show(app: &mut DiscordApp, ctx: &egui::Context, store: &mut Store, tab: SettingsTab) -> Outcome {
    let p = *app.palette();
    let mut outcome = Outcome::Stay;
    let before = app.link.settings.read().clone();
    let mut s = before.clone();
    egui::Area::new(egui::Id::new("settings-overlay")).fixed_pos(egui::Pos2::ZERO).order(egui::Order::Foreground).show(ctx, |ui| {
        let screen = ctx.content_rect();
        ui.painter().rect_filled(screen, CornerRadius::ZERO, p.chat);
        let nav_w = (screen.width() * 0.3).clamp(200.0, 320.0);
        let nav = egui::Rect::from_min_size(screen.min, vec2(nav_w, screen.height()));
        ui.painter().rect_filled(nav, CornerRadius::ZERO, p.sidebar);
        let mut nav_ui = ui.new_child(egui::UiBuilder::new().max_rect(nav.shrink2(vec2(16.0, 40.0))));
        nav_ui.with_layout(egui::Layout::top_down(egui::Align::Max), |ui| {
            ui.set_max_width(200.0);
            widgets::section_label(ui, &p, "User settings");
            for (t, label) in [
                (SettingsTab::Account, "My Account"),
                (SettingsTab::Voice, "Voice & Audio"),
                (SettingsTab::Notifications, "Notifications"),
                (SettingsTab::Appearance, "Appearance"),
                (SettingsTab::Keybinds, "Keybinds"),
                (SettingsTab::Advanced, "Advanced"),
                (SettingsTab::Diagnostics, "Diagnostics"),
                (SettingsTab::About, "About & Compatibility"),
            ] {
                let r = widgets::nav_row(ui, &p, tab == t, 32.0, |ui, _| {
                    ui.label(RichText::new(label).color(if tab == t { p.text_strong } else { p.muted }));
                });
                if r.clicked() {
                    outcome = Outcome::Switch(t);
                }
            }
        });
        let content = egui::Rect::from_min_max(egui::pos2(nav.right() + 40.0, screen.top() + 40.0), egui::pos2((nav.right() + 780.0).min(screen.right() - 80.0), screen.bottom()));
        let mut cui = ui.new_child(egui::UiBuilder::new().max_rect(content));
        egui::ScrollArea::vertical().id_salt(("settings", tab as u8)).auto_shrink([false, false]).show(&mut cui, |ui| {
            ui.spacing_mut().item_spacing.y = 8.0;
            match tab {
                SettingsTab::Account => account(app, ui, store, &p),
                SettingsTab::Voice => voice(app, ui, store, &mut s, &p),
                SettingsTab::Notifications => notifications(ui, &mut s, &p),
                SettingsTab::Appearance => appearance(ui, &mut s, &p),
                SettingsTab::Keybinds => keybinds(app, ui, &mut s, &p),
                SettingsTab::Advanced => advanced(app, ui, &mut s, &p),
                SettingsTab::Diagnostics => diagnostics(app, ui, store, &p),
                SettingsTab::About => about(ui, &p),
            }
            ui.add_space(40.0);
        });
        let close_rect = egui::Rect::from_center_size(egui::pos2(screen.right() - 48.0, screen.top() + 52.0), vec2(36.0, 36.0));
        let close = ui.interact(close_rect, egui::Id::new("settings-close"), egui::Sense::click());
        ui.painter().circle_stroke(close_rect.center(), 18.0, egui::Stroke::new(2.0, if close.hovered() { p.text_strong } else { p.muted }));
        ui.painter().text(close_rect.center(), egui::Align2::CENTER_CENTER, "✕", egui::FontId::proportional(16.0), p.text);
        ui.painter().text(close_rect.center_bottom() + vec2(0.0, 10.0), egui::Align2::CENTER_TOP, "ESC", egui::FontId::proportional(11.0), p.muted);
        if close.clicked() {
            outcome = Outcome::Close;
        }
    });
    if s != before {
        s.sanitize();
        *app.link.settings.write() = s;
        app.link.send(Command::SettingsChanged);
    }
    outcome
}

fn title(ui: &mut Ui, p: &Palette, t: &str) {
    ui.label(RichText::new(t).size(20.0).strong().color(p.text_strong));
    ui.add_space(4.0);
}

fn card(ui: &mut Ui, p: &Palette, add: impl FnOnce(&mut Ui)) {
    egui::Frame::new().fill(p.sidebar).corner_radius(CornerRadius::same(8)).inner_margin(16).show(ui, |ui| {
        ui.set_width(ui.available_width());
        add(ui)
    });
}

fn account(app: &mut DiscordApp, ui: &mut Ui, store: &mut Store, p: &Palette) {
    title(ui, p, "My Account");
    if let Some(me) = store.session.me.clone() {
        card(ui, p, |ui| {
            ui.horizontal(|ui| {
                widgets::avatar(ui, &mut app.textures, p, &me.avatar_url(128), 64.0, None, false);
                ui.vertical(|ui| {
                    ui.label(RichText::new(me.display_name()).size(18.0).strong().color(p.text_strong));
                    ui.label(RichText::new(format!("{} · bot user", me.tag())).color(p.muted));
                    ui.label(RichText::new(crate::sidebar::conn_label(store)).color(p.faint).size(12.0));
                });
            });
            ui.horizontal(|ui| {
                if ui.button("Set status…").clicked() {
                    app.st.modal = None;
                    app.st.modal = Some(crate::state::Modal::Status { custom: String::new() });
                }
                if let Some(app_id) = store.session.application_id {
                    if ui.button("Invite bot to a server ↗").clicked() {
                        let _ = open::that_detached(crate::sidebar::invite_url(app_id, None));
                    }
                }
                if ui.button("Reconnect").clicked() {
                    app.link.send(Command::Reconnect);
                }
            });
            if store.session.intents_degraded {
                ui.label(
                    RichText::new("Privileged intents were rejected by Discord, so message content, member lists and presences may be missing. Enable them in the Developer Portal (Bot → Privileged Gateway Intents), then Reconnect.")
                        .color(p.yellow),
                );
            }
        });
    }
    ui.add_space(8.0);
    widgets::section_label(ui, p, "Accounts");
    let accounts = store.session.accounts.clone();
    for a in &accounts {
        card(ui, p, |ui| {
            ui.horizontal(|ui| {
                widgets::avatar(ui, &mut app.textures, p, &a.bot_avatar_url, 36.0, None, false);
                ui.vertical(|ui| {
                    ui.label(RichText::new(&a.label).strong().color(p.text_strong));
                    let mut detail = a.bot_name.clone();
                    if let Some(u) = &a.oauth_user {
                        detail.push_str(&format!(" · signed in as {u}"));
                    }
                    ui.label(RichText::new(detail).size(12.0).color(p.muted));
                    if a.credentials_invalid {
                        ui.label(RichText::new("Bot token rejected by Discord — add the account again with a new token.").size(12.0).color(p.red));
                    }
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button(RichText::new("Remove").color(p.red)).on_hover_text("Signs out and deletes stored credentials").clicked() {
                        app.link.send(Command::RemoveAccount(a.id.clone()));
                    }
                    if !a.active && ui.button("Switch").clicked() {
                        app.link.send(Command::SwitchAccount(a.id.clone()));
                    }
                    if a.active {
                        ui.label(RichText::new("Active").color(p.green));
                    }
                });
            });
        });
    }
    if ui.button("➕ Add another bot account").clicked() {
        app.st.show_add_account = true;
        app.st.modal = None;
    }
    ui.add_space(12.0);
    widgets::section_label(ui, p, "Sign in with Discord (optional)");
    card(ui, p, |ui| {
        match &store.session.linked {
            Some(l) => {
                ui.horizontal(|ui| {
                    widgets::avatar(ui, &mut app.textures, p, &l.user.avatar_url(96), 40.0, None, false);
                    ui.vertical(|ui| {
                        ui.label(RichText::new(l.user.display_name()).strong().color(p.text_strong));
                        ui.label(RichText::new(format!("@{} · {} servers", l.user.username, l.guilds.len())).color(p.muted).size(12.0));
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("Sign out").clicked() {
                            app.link.send(Command::UnlinkOAuth);
                        }
                    });
                });
            }
            None => {
                let expired = accounts.iter().any(|a| a.active && a.oauth_expired);
                if expired {
                    ui.label(RichText::new("Your Discord sign-in expired. Sign in again.").color(p.yellow));
                }
                ui.label(
                    RichText::new(
                        "Links your real Discord identity (profile and server list) using OAuth2 with PKCE in your browser. \
                         In the Developer Portal → OAuth2, add the redirect below and enable \"Public Client\" (or enter the client secret).",
                    )
                    .color(p.muted)
                    .size(13.0),
                );
                let ob = &mut app.st.onboarding;
                if ob.port.is_empty() {
                    ob.port = "53134".into();
                }
                ui.horizontal(|ui| {
                    ui.label("Redirect URI:");
                    let uri = format!("http://127.0.0.1:{}/callback", ob.port.trim());
                    ui.label(RichText::new(&uri).monospace());
                    if ui.small_button("Copy").clicked() {
                        ui.ctx().copy_text(uri);
                    }
                });
                ui.add(egui::TextEdit::singleline(&mut ob.client_id).hint_text("Application (client) ID").desired_width(320.0));
                ui.add(egui::TextEdit::singleline(&mut ob.client_secret).password(true).hint_text("Client secret (optional for public clients)").desired_width(320.0));
                ui.horizontal(|ui| {
                    ui.label("Port");
                    ui.add(egui::TextEdit::singleline(&mut ob.port).desired_width(70.0));
                });
                ui.horizontal(|ui| {
                    let busy = app.st.onboarding.oauth_busy;
                    let id_ok = app.st.onboarding.client_id.trim().chars().all(|c| c.is_ascii_digit()) && !app.st.onboarding.client_id.trim().is_empty();
                    if ui.add_enabled(!busy && id_ok, egui::Button::new("Sign in with Discord")).clicked() {
                        let ob = &mut app.st.onboarding;
                        ob.oauth_busy = true;
                        let secret = ob.client_secret.trim().to_string();
                        app.link.send(Command::BeginOAuth {
                            client_id: ob.client_id.trim().to_string(),
                            client_secret: (!secret.is_empty()).then_some(secret),
                            port: ob.port.trim().parse().unwrap_or(53134),
                        });
                    }
                    if busy {
                        ui.spinner();
                        ui.label(RichText::new("Waiting for the browser…").color(p.muted));
                        if ui.button("Cancel").clicked() {
                            app.link.send(Command::CancelOAuth);
                            app.st.onboarding.oauth_busy = false;
                        }
                    }
                });
            }
        }
    });
}

fn device_combo(ui: &mut Ui, label: &str, value: &mut Option<String>, devices: &[String]) {
    ui.label(label);
    let shown = value.clone().unwrap_or_else(|| "Default".into());
    egui::ComboBox::from_id_salt(label).selected_text(shown).width(320.0).show_ui(ui, |ui| {
        ui.selectable_value(value, None, "Default");
        for d in devices {
            ui.selectable_value(value, Some(d.clone()), d);
        }
    });
}

fn voice(app: &mut DiscordApp, ui: &mut Ui, store: &mut Store, s: &mut Settings, p: &Palette) {
    title(ui, p, "Voice & Audio");
    if store.session.audio_inputs.is_empty() && store.session.audio_outputs.is_empty() {
        ui.label(RichText::new("No audio devices detected.").color(p.yellow));
    }
    ui.horizontal(|ui| {
        ui.vertical(|ui| device_combo(ui, "Input device", &mut s.voice.input_device, &store.session.audio_inputs));
        ui.vertical(|ui| device_combo(ui, "Output device", &mut s.voice.output_device, &store.session.audio_outputs));
    });
    if ui.small_button("Refresh devices").clicked() {
        app.link.send(Command::RefreshAudioDevices);
    }
    ui.horizontal(|ui| {
        ui.label("Input volume");
        ui.add(egui::Slider::new(&mut s.voice.input_volume, 0.0..=2.0).custom_formatter(|v, _| format!("{:.0}%", v * 100.0)));
        ui.label("Output volume");
        ui.add(egui::Slider::new(&mut s.voice.output_volume, 0.0..=2.0).custom_formatter(|v, _| format!("{:.0}%", v * 100.0)));
    });
    ui.add_space(8.0);
    widgets::section_label(ui, p, "Mic test");
    ui.horizontal(|ui| {
        let label = if app.st.mic_test { "Stop testing" } else { "Let's check" };
        if widgets::primary_button(ui, p, label).clicked() {
            app.st.mic_test = !app.st.mic_test;
            app.link.send(Command::MicTest(app.st.mic_test));
        }
        level_meter(ui, p, store.voice.input_level_db, s.voice.vad_threshold_db, s.voice.auto_sensitivity);
    });
    if app.st.mic_test || store.voice.conn.channel().is_some() {
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(50));
    }
    ui.add_space(8.0);
    widgets::section_label(ui, p, "Input mode");
    ui.radio_value(&mut s.voice.input_mode, InputMode::VoiceActivity, "Voice Activity");
    ui.radio_value(&mut s.voice.input_mode, InputMode::PushToTalk, "Push to Talk");
    match s.voice.input_mode {
        InputMode::VoiceActivity => {
            ui.checkbox(&mut s.voice.auto_sensitivity, "Automatically determine input sensitivity");
            ui.add_enabled(!s.voice.auto_sensitivity, egui::Slider::new(&mut s.voice.vad_threshold_db, -100.0..=0.0).suffix(" dB").text("Sensitivity"));
        }
        InputMode::PushToTalk => {
            ui.horizontal(|ui| {
                ui.label("Shortcut");
                keybind_capture(ui, p, &mut s.voice.ptt_key, "ptt");
            });
            ui.add(egui::Slider::new(&mut s.voice.ptt_release_delay_ms, 0..=2000).suffix(" ms").text("Release delay"));
            #[cfg(windows)]
            ui.label(RichText::new("Push-to-talk works even when the window isn't focused.").size(12.0).color(p.muted));
            #[cfg(not(windows))]
            ui.label(RichText::new("On this platform push-to-talk works while the window is focused.").size(12.0).color(p.muted));
        }
    }
    ui.add_space(8.0);
    widgets::section_label(ui, p, "Voice processing");
    ui.checkbox(&mut s.voice.noise_suppression, "Noise suppression (RNNoise)");
    ui.checkbox(&mut s.voice.auto_gain, "Automatic gain control");
    ui.label(RichText::new("Echo cancellation is not implemented; use headphones or your OS/device echo cancellation.").size(12.0).color(p.faint));
    ui.add(egui::Slider::new(&mut s.voice.bitrate_kbps, 8..=128).suffix(" kbps").text("Send bitrate"));
}

fn level_meter(ui: &mut Ui, p: &Palette, level_db: f32, threshold_db: f32, auto: bool) {
    let (rect, _) = ui.allocate_exact_size(vec2(320.0, 10.0), egui::Sense::hover());
    ui.painter().rect_filled(rect, CornerRadius::same(5), p.input);
    let frac = ((level_db + 100.0) / 100.0).clamp(0.0, 1.0);
    let above = level_db > threshold_db;
    ui.painter().rect_filled(egui::Rect::from_min_size(rect.min, vec2(rect.width() * frac, rect.height())), CornerRadius::same(5), if above || auto { p.green } else { p.yellow });
    if !auto {
        let x = rect.left() + rect.width() * ((threshold_db + 100.0) / 100.0).clamp(0.0, 1.0);
        ui.painter().vline(x, rect.y_range().expand(3.0), egui::Stroke::new(2.0, p.text_strong));
    }
}

fn keybind_capture(ui: &mut Ui, p: &Palette, value: &mut String, id: &str) {
    let capture_id = egui::Id::new(("capture", id));
    let capturing = ui.ctx().data(|d| d.get_temp::<bool>(capture_id)).unwrap_or(false);
    let label = if capturing { "Press a key…".to_string() } else { value.clone() };
    if ui.add(egui::Button::new(RichText::new(label).monospace()).min_size(vec2(140.0, 28.0)).fill(if capturing { p.red.gamma_multiply(0.4) } else { p.input })).clicked() {
        ui.ctx().data_mut(|d| d.insert_temp(capture_id, !capturing));
    }
    if capturing {
        let pressed = ui.input(|i| {
            i.events.iter().find_map(|e| match e {
                egui::Event::Key { key, pressed: true, .. } => Some(*key),
                _ => None,
            })
        });
        if let Some(k) = pressed {
            if k != egui::Key::Escape {
                *value = k.name().to_string();
            }
            ui.ctx().data_mut(|d| d.insert_temp(capture_id, false));
        }
    }
}

fn notifications(ui: &mut Ui, s: &mut Settings, p: &Palette) {
    title(ui, p, "Notifications");
    ui.checkbox(&mut s.notifications.enabled, "Enable desktop notifications");
    ui.checkbox(&mut s.notifications.show_preview, "Show message preview");
    ui.checkbox(&mut s.notifications.flash_taskbar, "Flash taskbar on mentions");
    ui.checkbox(&mut s.notifications.suppress_everyone, "Suppress @everyone and @here");
    level_picker(ui, "Default server notifications", &mut s.notifications.default_guild_level);
    level_picker(ui, "Direct messages", &mut s.notifications.dm_level);
    ui.label(RichText::new("Right-click a server or channel to mute it.").size(12.0).color(p.faint));
}

fn level_picker(ui: &mut Ui, label: &str, v: &mut NotifyLevel) {
    ui.horizontal(|ui| {
        ui.label(label);
        ui.radio_value(v, NotifyLevel::All, "All messages");
        ui.radio_value(v, NotifyLevel::Mentions, "Only mentions");
        ui.radio_value(v, NotifyLevel::Nothing, "Nothing");
    });
}

fn appearance(ui: &mut Ui, s: &mut Settings, p: &Palette) {
    title(ui, p, "Appearance");
    widgets::section_label(ui, p, "Theme");
    ui.horizontal(|ui| {
        ui.radio_value(&mut s.appearance.theme, Theme::Dark, "Dark");
        ui.radio_value(&mut s.appearance.theme, Theme::Light, "Light");
        ui.radio_value(&mut s.appearance.theme, Theme::Midnight, "Midnight");
    });
    ui.add(egui::Slider::new(&mut s.appearance.font_size, 12.0..=20.0).suffix(" px").text("Chat font size"));
    ui.add(egui::Slider::new(&mut s.appearance.ui_scale, 0.75..=2.0).text("Zoom level"));
    ui.checkbox(&mut s.appearance.use_24h_time, "24-hour time");
    ui.checkbox(&mut s.appearance.show_embeds, "Show embeds and link previews");
    ui.checkbox(&mut s.appearance.show_images, "Show images and attachments inline");
    ui.checkbox(&mut s.appearance.show_reactions, "Show reactions");
    ui.checkbox(&mut s.appearance.reduce_motion, "Reduce motion");
}

fn keybinds(app: &mut DiscordApp, ui: &mut Ui, s: &mut Settings, p: &Palette) {
    title(ui, p, "Keybinds");
    let rows = [
        ("Toggle mute", s.keybinds.toggle_mute.clone()),
        ("Toggle deafen", s.keybinds.toggle_deafen.clone()),
        ("Quick switcher", s.keybinds.quick_switcher.clone()),
        ("Search", s.keybinds.search.clone()),
        ("Previous / next channel", "Alt+Up / Alt+Down".into()),
        ("Edit last message", "Up (empty composer)".into()),
        ("Cancel reply / edit, mark read", "Escape".into()),
        ("New line in message", "Shift+Enter".into()),
    ];
    for (label, key) in rows {
        ui.horizontal(|ui| {
            ui.label(RichText::new(label).color(p.text));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(RichText::new(key).monospace().color(p.muted));
            });
        });
        ui.separator();
    }
    ui.horizontal(|ui| {
        ui.label("Push to talk");
        keybind_capture(ui, p, &mut s.voice.ptt_key, "ptt-keybinds");
    });
    let _ = app;
}

fn advanced(app: &mut DiscordApp, ui: &mut Ui, s: &mut Settings, p: &Palette) {
    title(ui, p, "Advanced");
    ui.checkbox(&mut s.behavior.developer_mode, "Developer mode (copy IDs from context menus)");
    ui.checkbox(&mut s.behavior.confirm_delete, "Confirm before deleting messages");
    ui.checkbox(&mut s.behavior.send_typing, "Send typing indicator");
    ui.add_space(8.0);
    widgets::section_label(ui, p, "Windows");
    ui.checkbox(&mut s.behavior.start_on_boot, "Open Discord Native when you sign in to Windows");
    ui.checkbox(&mut s.behavior.start_minimized, "Start minimized to tray");
    ui.checkbox(&mut s.behavior.close_to_tray, "Close button minimizes to tray");
    ui.checkbox(&mut s.behavior.hardware_acceleration, "Hardware acceleration (wgpu; off = OpenGL) — restart required");
    ui.add_space(8.0);
    widgets::section_label(ui, p, "Gateway intents (reconnect to apply)");
    ui.checkbox(&mut s.advanced.intent_message_content, "Message Content (privileged)");
    ui.checkbox(&mut s.advanced.intent_members, "Server Members (privileged)");
    ui.checkbox(&mut s.advanced.intent_presences, "Presence (privileged)");
    ui.checkbox(&mut s.advanced.gateway_compression, "zlib-stream transport compression");
    if ui.button("Reconnect now").clicked() {
        app.link.send(Command::Reconnect);
    }
    ui.add_space(8.0);
    widgets::section_label(ui, p, "Caches");
    ui.add(egui::Slider::new(&mut s.advanced.image_cache_mb, 16..=1024).suffix(" MB").text("GPU image cache"));
    ui.add(egui::Slider::new(&mut s.advanced.disk_cache_mb, 32..=4096).suffix(" MB").text("Disk image cache"));
    ui.add(egui::Slider::new(&mut s.advanced.cache_max_messages, 10_000..=2_000_000).text("Messages kept in local database"));
    ui.add(egui::Slider::new(&mut s.advanced.memory_messages_per_channel, 50..=3000).text("Messages kept in memory per inactive channel"));
    ui.horizontal(|ui| {
        ui.label("Log level");
        egui::ComboBox::from_id_salt("loglevel").selected_text(s.advanced.log_level.clone()).show_ui(ui, |ui| {
            for l in ["error", "warn", "info", "debug", "trace"] {
                ui.selectable_value(&mut s.advanced.log_level, l.to_string(), l);
            }
        });
    });
    let paths = dn_settings::Paths::detect();
    ui.horizontal(|ui| {
        if ui.button("Open logs folder").clicked() {
            let _ = open::that_detached(&paths.logs_dir);
        }
        if ui.button("Open data folder").clicked() {
            let _ = open::that_detached(&paths.data_dir);
        }
    });
}

fn diagnostics(app: &mut DiscordApp, ui: &mut Ui, store: &mut Store, p: &Palette) {
    title(ui, p, "Diagnostics");
    let sample = app.last_sample.1;
    let fs = app.frame_stats.summary();
    let (tex_n, tex_b) = app.textures.stats();
    let grid = |ui: &mut Ui, rows: Vec<(&str, String)>| {
        egui::Grid::new(ui.next_auto_id()).num_columns(2).spacing(vec2(24.0, 6.0)).show(ui, |ui| {
            for (k, v) in rows {
                ui.label(RichText::new(k).color(p.muted));
                ui.label(RichText::new(v).monospace());
                ui.end_row();
            }
        });
    };
    card(ui, p, |ui| {
        widgets::section_label(ui, p, "Process");
        grid(ui, vec![
            ("Working set (RSS)", widgets::human_size(sample.rss_bytes)),
            ("Private memory", widgets::human_size(sample.private_bytes)),
            ("CPU (all cores)", format!("{:.1}%", sample.cpu_percent)),
            ("Threads", sample.threads.to_string()),
        ]);
    });
    card(ui, p, |ui| {
        widgets::section_label(ui, p, "Rendering (frames actually drawn; idle = no frames)");
        grid(ui, vec![
            ("Average frame interval", format!("{:.1} ms", fs.avg_ms)),
            ("p95 / p99", format!("{:.1} / {:.1} ms", fs.p95_ms, fs.p99_ms)),
            ("Effective FPS while active", format!("{:.0}", fs.fps)),
            ("GPU textures", format!("{tex_n} ({})", widgets::human_size(tex_b as u64))),
        ]);
    });
    card(ui, p, |ui| {
        widgets::section_label(ui, p, "Client state");
        grid(ui, vec![
            ("Gateway", crate::sidebar::conn_label(store)),
            ("Servers / channels", format!("{} / {}", store.guilds.len(), store.channels.len())),
            ("Cached users", store.users.len().to_string()),
            ("Messages in memory", store.total_loaded_messages().to_string()),
            ("Loaded channel windows", store.messages.len().to_string()),
        ]);
    });
    ui.add_space(8.0);
    widgets::section_label(ui, p, "UI stress test");
    ui.label(RichText::new("Renders synthetic messages offline (no Discord traffic) and auto-scrolls to measure frame times of the virtualized list.").color(p.muted).size(12.5));
    ui.horizontal(|ui| {
        for n in [10_000usize, 50_000, 100_000] {
            if ui.button(format!("Run with {n} messages")).clicked() {
                app.st.stress = Some(crate::stress::StressTest::new(n));
                app.st.modal = None;
            }
        }
    });
    ui.ctx().request_repaint_after(std::time::Duration::from_secs(2));
}

fn about(ui: &mut Ui, p: &Palette) {
    title(ui, p, "About & Compatibility");
    ui.label(format!("Discord Native {} — native Rust client (egui + wgpu). Not affiliated with or endorsed by Discord Inc.", env!("CARGO_PKG_VERSION")));
    ui.label(RichText::new("A complete replacement for the official client is not achievable with Discord's authorized interfaces: personal-account chat, personal DMs, friends and video/screen sharing are not available to third-party apps.").color(p.yellow));
    ui.add_space(8.0);
    egui::Grid::new("matrix").num_columns(3).striped(true).spacing(vec2(16.0, 6.0)).show(ui, |ui| {
        ui.label(RichText::new("Feature").strong());
        ui.label(RichText::new("Status").strong());
        ui.label(RichText::new("Notes").strong());
        ui.end_row();
        for (feature, support, note) in MATRIX {
            ui.label(*feature);
            let c = match support {
                Support::Supported | Support::AsBot => p.green,
                Support::Partial | Support::Unverified => p.yellow,
                Support::Blocked => p.red,
            };
            ui.label(RichText::new(support.label()).color(c));
            ui.label(RichText::new(*note).color(p.muted).size(12.5));
            ui.end_row();
        }
    });
}
