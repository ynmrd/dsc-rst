//! First-run / add-account screen. Explains the authorized connection model and collects
//! the user's own bot token (stored only in the OS credential manager).

use egui::{CornerRadius, RichText, Stroke, Ui, vec2};

use dn_cache::Store;
use dn_cache::bus::Command;

use crate::{DiscordApp, widgets};

pub const PORTAL_URL: &str = "https://discord.com/developers/applications";

pub fn show(app: &mut DiscordApp, ui: &mut Ui, store: &mut Store) {
    let p = *app.palette();
    egui::CentralPanel::default().frame(egui::Frame::new().fill(p.rail)).show(ui, |ui| {
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            ui.vertical_centered(|ui| {
                ui.add_space(32.0);
                egui::Frame::new()
                    .fill(p.chat)
                    .corner_radius(CornerRadius::same(10))
                    .inner_margin(32)
                    .stroke(Stroke::new(1.0, p.divider))
                    .show(ui, |ui| {
                        ui.set_width(560.0);
                        ui.vertical(|ui| {
                            ui.spacing_mut().item_spacing.y = 10.0;
                            ui.label(RichText::new("Welcome to Discord Native").size(26.0).strong().color(p.text_strong));
                            ui.label(RichText::new("A native desktop client that talks to Discord only through its official, documented APIs.").color(p.muted));
                            egui::Frame::new().fill(p.sidebar).corner_radius(CornerRadius::same(6)).inner_margin(12).show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                ui.label(RichText::new("How it connects").strong().color(p.text_strong));
                                ui.label(
                                    "Discord does not allow third-party apps to log in as your personal account to chat \
                                     (\"self-bots\" break Discord's Terms of Service), so this app never asks for your password or user token. \
                                     Instead you connect your own Discord application's bot: you chat as that bot in servers you add it to, \
                                     and in DMs people send to it. Optionally, \"Sign in with Discord\" (OAuth2) shows your real profile and server list.",
                                );
                            });
                            step(ui, &p, "1", "Create an application", "Open the Developer Portal and click New Application.");
                            if ui.link(RichText::new("Open the Discord Developer Portal ↗").color(p.link)).clicked() {
                                let _ = open::that_detached(PORTAL_URL);
                            }
                            step(
                                ui,
                                &p,
                                "2",
                                "Get the bot token and enable intents",
                                "In the Bot tab: Reset Token and copy it. Under Privileged Gateway Intents enable Message Content, \
                                 Server Members and Presence (needed to read messages, list members and show status).",
                            );
                            step(ui, &p, "3", "Paste the token", "It's stored only in the Windows Credential Manager (or your OS keychain).");
                            let ob = &mut app.st.onboarding;
                            ui.add(
                                egui::TextEdit::singleline(&mut ob.token)
                                    .password(true)
                                    .hint_text("Bot token")
                                    .desired_width(f32::INFINITY)
                                    .margin(vec2(10.0, 8.0)),
                            );
                            if let Some(e) = &ob.error {
                                ui.label(RichText::new(e).color(p.red));
                            }
                            ui.horizontal(|ui| {
                                let busy = app.st.onboarding.busy;
                                let enabled = !busy && !app.st.onboarding.token.trim().is_empty();
                                let b = ui.add_enabled(enabled, egui::Button::new(RichText::new("Connect").strong().color(p.accent_text)).fill(p.accent).min_size(vec2(120.0, 36.0)));
                                if b.clicked() || (enabled && ui.input(|i| i.key_pressed(egui::Key::Enter))) {
                                    app.st.onboarding.busy = true;
                                    app.st.onboarding.error = None;
                                    app.link.send(Command::AddBotAccount { token: app.st.onboarding.token.trim().to_string() });
                                }
                                if busy {
                                    ui.spinner();
                                    ui.label(RichText::new("Validating with Discord…").color(p.muted));
                                }
                                if !store.session.accounts.is_empty() && ui.button("Cancel").clicked() {
                                    app.st.show_add_account = false;
                                    app.st.onboarding = Default::default();
                                }
                            });
                            step(
                                ui,
                                &p,
                                "4",
                                "Add the bot to your servers",
                                "After connecting, use the + button in the server list to invite the bot (requires Manage Server in that server).",
                            );
                            ui.add_space(6.0);
                            ui.label(
                                RichText::new("Not available through official APIs: your personal DMs, friends list, group DMs, and video/screen sharing. See Settings → About for the full compatibility matrix.")
                                    .size(12.0)
                                    .color(p.faint),
                            );
                        });
                    });
                if !store.session.accounts.is_empty() && store.session.needs_onboarding {
                    ui.add_space(16.0);
                    ui.label(RichText::new("Saved accounts").color(p.muted));
                    for a in store.session.accounts.clone() {
                        ui.horizontal(|ui| {
                            ui.label(&a.label);
                            if a.credentials_invalid {
                                ui.label(RichText::new("token invalid").color(p.red));
                            }
                            if ui.button("Use").clicked() {
                                app.link.send(Command::SwitchAccount(a.id.clone()));
                            }
                        });
                    }
                }
                let _ = widgets::human_size(0);
            });
        });
    });
}

fn step(ui: &mut Ui, p: &crate::theme::Palette, n: &str, title: &str, body: &str) {
    ui.horizontal_top(|ui| {
        let (r, _) = ui.allocate_exact_size(vec2(24.0, 24.0), egui::Sense::hover());
        ui.painter().circle_filled(r.center(), 12.0, p.accent);
        ui.painter().text(r.center(), egui::Align2::CENTER_CENTER, n, crate::theme::bold(13.0), p.accent_text);
        ui.vertical(|ui| {
            ui.label(RichText::new(title).strong().color(p.text_strong));
            ui.label(RichText::new(body).color(p.muted).size(13.0));
        });
    });
}
