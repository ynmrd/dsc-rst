//! Modal dialogs.

use egui::{Color32, CornerRadius, Key, RichText, Sense, Stroke, vec2};

use dn_cache::Store;
use dn_cache::bus::Command;
use dn_core::{Id, Status};

use crate::state::Modal;
use crate::{DiscordApp, settings_view, sidebar, widgets};

pub fn show(app: &mut DiscordApp, ctx: &egui::Context, store: &mut Store) {
    let Some(modal) = app.st.modal.clone() else { return };
    let p = *app.palette();
    let mut close = false;
    let mut next: Option<Modal> = None;
    match modal {
        Modal::Settings(tab) => {
            match settings_view::show(app, ctx, store, tab) {
                settings_view::Outcome::Stay => {}
                settings_view::Outcome::Switch(t) => next = Some(Modal::Settings(t)),
                settings_view::Outcome::Close => close = true,
            }
        }
        Modal::ConfirmDelete { channel_id, message_id, preview } => {
            let r = dialog(ctx, &p, "confirm-delete", |ui| {
                ui.label(RichText::new("Delete Message").size(20.0).strong().color(p.text_strong));
                ui.label("Are you sure you want to delete this message?");
                egui::Frame::new().fill(p.sidebar).corner_radius(CornerRadius::same(4)).inner_margin(8).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.label(preview.chars().take(400).collect::<String>());
                });
                ui.label(RichText::new("Tip: hold Shift when clicking Delete to skip this confirmation.").size(11.5).color(p.muted));
                let mut out = None;
                ui.horizontal(|ui| {
                    if widgets::danger_button(ui, &p, "Delete").clicked() || ui.input(|i| i.key_pressed(Key::Enter)) {
                        out = Some(true);
                    }
                    if ui.button("Cancel").clicked() {
                        out = Some(false);
                    }
                });
                out
            });
            match r {
                (Some(Some(true)), _) => {
                    app.link.send(Command::DeleteMessage { channel_id, message_id });
                    close = true;
                }
                (Some(Some(false)), _) | (_, true) => close = true,
                _ => {}
            }
        }
        Modal::LinkWarning { url } => {
            let r = dialog(ctx, &p, "link-warning", |ui| {
                ui.label(RichText::new("Leaving Discord Native").size(20.0).strong().color(p.text_strong));
                ui.label("This link is going to take you to the following website:");
                egui::Frame::new().fill(p.sidebar).corner_radius(CornerRadius::same(4)).inner_margin(8).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.label(RichText::new(&url).monospace().color(p.link));
                });
                ui.label("Are you sure you want to go there?");
                let mut out = None;
                ui.horizontal(|ui| {
                    if widgets::primary_button(ui, &p, "Visit Site").clicked() {
                        out = Some(true);
                    }
                    if ui.button("Go Back").clicked() {
                        out = Some(false);
                    }
                });
                out
            });
            match r {
                (Some(Some(true)), _) => {
                    let _ = open::that_detached(&url);
                    close = true;
                }
                (Some(Some(false)), _) | (_, true) => close = true,
                _ => {}
            }
        }
        Modal::Profile { user_id, guild_id } => close = profile(app, ctx, store, user_id, guild_id),
        Modal::CreateThread { channel_id, message_id, mut name } => {
            let r = dialog(ctx, &p, "create-thread", |ui| {
                ui.label(RichText::new("Create Thread").size(20.0).strong().color(p.text_strong));
                let edit = ui.add(egui::TextEdit::singleline(&mut name).desired_width(f32::INFINITY));
                edit.request_focus();
                let mut out = None;
                ui.horizontal(|ui| {
                    if widgets::primary_button(ui, &p, "Create").clicked() || ui.input(|i| i.key_pressed(Key::Enter)) {
                        out = Some(true);
                    }
                    if ui.button("Cancel").clicked() {
                        out = Some(false);
                    }
                });
                out
            });
            match r {
                (Some(Some(true)), _) if !name.trim().is_empty() => {
                    app.link.send(Command::CreateThread { channel_id, message_id, name: name.trim().to_string() });
                    close = true;
                }
                (Some(Some(false)), _) | (_, true) => close = true,
                _ => next = Some(Modal::CreateThread { channel_id, message_id, name }),
            }
        }
        Modal::QuickSwitcher { mut query, mut selected } => {
            let (picked, closed) = quick_switcher(ctx, store, &p, &mut query, &mut selected);
            if let Some(c) = picked {
                app.open_channel(store, c);
                close = true;
            } else if closed {
                close = true;
            } else {
                next = Some(Modal::QuickSwitcher { query, selected });
            }
        }
        Modal::ImageViewer { url, original, width, height } => {
            let screen = ctx.content_rect();
            let resp = egui::Modal::new(egui::Id::unique("image-viewer")).frame(egui::Frame::NONE).show(ctx, |ui| {
                let max = screen.size() * 0.85;
                let (w, h) = (width.max(1) as f32, height.max(1) as f32);
                let s = (max.x / w).min(max.y / h).min(1.0);
                let size = if width == 0 { vec2(600.0, 400.0) } else { vec2(w * s, h * s) };
                let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
                match app.textures.get(&url, size.x.max(size.y) as u32) {
                    Some(tex) => egui::Image::new((tex.id(), size)).paint_at(ui, rect),
                    None => {
                        ui.painter().rect_filled(rect, CornerRadius::same(4), p.sidebar);
                        ui.put(rect, egui::Spinner::new());
                    }
                }
                ui.horizontal(|ui| {
                    if ui.link(RichText::new("Open original").color(Color32::WHITE)).clicked() {
                        let _ = open::that_detached(&original);
                    }
                    if ui.link(RichText::new("Save…").color(Color32::WHITE)).clicked() {
                        let name = original.split('?').next().and_then(|u| u.rsplit('/').next()).unwrap_or("image").to_string();
                        crate::message::save_as(app, &original, &name);
                    }
                });
            });
            close = resp.should_close();
        }
        Modal::AddServer => {
            let app_id = store.session.application_id;
            let linked: Vec<(Id, String)> = store
                .session
                .linked
                .as_ref()
                .map(|l| l.guilds.iter().filter(|g| !store.guilds.contains_key(&g.id)).filter(|g| g.owner || g.permissions.contains(dn_core::Permissions::MANAGE_GUILD)).map(|g| (g.id, g.name.clone())).collect())
                .unwrap_or_default();
            let r = dialog(ctx, &p, "add-server", |ui| {
                ui.label(RichText::new("Add the bot to a server").size(20.0).strong().color(p.text_strong));
                ui.label(RichText::new("Discord only lets third-party apps act in servers where your bot has been added by someone with Manage Server permission.").color(p.muted));
                let Some(app_id) = app_id else {
                    ui.label("Not connected yet.");
                    return None;
                };
                if widgets::primary_button(ui, &p, "Open invite page").clicked() {
                    let _ = open::that_detached(sidebar::invite_url(app_id, None));
                }
                if !linked.is_empty() {
                    ui.add_space(8.0);
                    widgets::section_label(ui, &p, "Your servers without the bot");
                    egui::ScrollArea::vertical().max_height(260.0).show(ui, |ui| {
                        for (gid, name) in &linked {
                            ui.horizontal(|ui| {
                                ui.label(name);
                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                    if ui.button("Add bot").clicked() {
                                        let _ = open::that_detached(sidebar::invite_url(app_id, Some(*gid)));
                                    }
                                });
                            });
                        }
                    });
                } else if store.session.linked.is_none() {
                    ui.label(RichText::new("Tip: sign in with Discord in Settings → My Account to list your own servers here.").size(12.0).color(p.faint));
                }
                let mut out = None;
                if ui.button("Done").clicked() {
                    out = Some(());
                }
                out
            });
            if matches!(r, (Some(Some(())), _) | (_, true)) {
                close = true;
            }
        }
        Modal::Status { mut custom } => {
            let r = dialog(ctx, &p, "status", |ui| {
                ui.label(RichText::new("Set bot status").size(18.0).strong().color(p.text_strong));
                let mut out = None;
                for s in [Status::Online, Status::Idle, Status::Dnd, Status::Invisible] {
                    let row = widgets::nav_row(ui, &p, false, 32.0, |ui, _| {
                        let (r, _) = ui.allocate_exact_size(vec2(12.0, 12.0), Sense::hover());
                        ui.painter().circle_filled(r.center(), 5.0, p.status_color(s));
                        ui.label(s.label());
                    });
                    if row.clicked() {
                        out = Some(s);
                    }
                }
                ui.add_space(6.0);
                ui.label(RichText::new("Custom status").size(12.0).color(p.muted));
                ui.add(egui::TextEdit::singleline(&mut custom).hint_text("What's happening?").desired_width(f32::INFINITY));
                out
            });
            match r {
                (Some(Some(s)), _) => {
                    let text = custom.trim().to_string();
                    app.link.send(Command::SetPresence { status: s, custom: (!text.is_empty()).then_some(text) });
                    close = true;
                }
                (_, true) => close = true,
                _ => next = Some(Modal::Status { custom }),
            }
        }
    }
    if close {
        app.st.modal = None;
    } else if let Some(n) = next {
        app.st.modal = Some(n);
    }
}

/// Standard centered dialog. Returns (content result, should_close).
fn dialog<R>(ctx: &egui::Context, p: &crate::theme::Palette, id: &str, add: impl FnOnce(&mut egui::Ui) -> R) -> (Option<R>, bool) {
    let resp = egui::Modal::new(egui::Id::unique(id))
        .frame(egui::Frame::new().fill(p.chat).corner_radius(CornerRadius::same(8)).inner_margin(20).stroke(Stroke::new(1.0, p.divider)))
        .show(ctx, |ui| {
            ui.set_width(440.0);
            ui.spacing_mut().item_spacing.y = 10.0;
            add(ui)
        });
    let close = resp.should_close();
    (Some(resp.inner), close)
}

fn profile(app: &mut DiscordApp, ctx: &egui::Context, store: &mut Store, user_id: Id, guild_id: Option<Id>) -> bool {
    let p = *app.palette();
    let fetched = app.st.profiles.get(&user_id).cloned();
    let user = fetched.as_ref().map(|(u, _)| u.clone()).or_else(|| store.users.get(&user_id).cloned());
    let member = guild_id.and_then(|g| store.member(g, user_id).cloned()).or_else(|| fetched.and_then(|(_, m)| m));
    let mut open_dm = false;
    let resp = egui::Modal::new(egui::Id::unique("profile"))
        .frame(egui::Frame::new().fill(p.floating).corner_radius(CornerRadius::same(8)))
        .show(ctx, |ui| {
            ui.set_width(340.0);
            let Some(user) = &user else {
                egui::Frame::new().inner_margin(20).show(ui, |ui| {
                    ui.spinner();
                });
                return;
            };
            let banner = user.accent_color.map(crate::theme::rgb).unwrap_or(p.accent);
            let (br, _) = ui.allocate_exact_size(vec2(340.0, 60.0), Sense::hover());
            ui.painter().rect_filled(br, CornerRadius { nw: 8, ne: 8, sw: 0, se: 0 }, banner);
            let av = egui::Rect::from_min_size(br.left_bottom() + vec2(16.0, -36.0), vec2(80.0, 80.0));
            ui.painter().circle_filled(av.center(), 44.0, p.floating);
            widgets::paint_avatar(ui, &mut app.textures, &p, &user.avatar_url(160), av, Some(store.status_of(user_id)), false);
            ui.add_space(48.0);
            egui::Frame::new().inner_margin(egui::Margin::symmetric(16, 8)).show(ui, |ui| {
                ui.set_width(308.0);
                let nick = member.as_ref().and_then(|m| m.nick.clone());
                ui.horizontal(|ui| {
                    ui.label(RichText::new(nick.as_deref().unwrap_or(user.display_name())).size(20.0).strong().color(p.text_strong));
                    if user.bot {
                        ui.label(RichText::new(" BOT ").size(10.0).color(Color32::WHITE).background_color(p.accent));
                    }
                });
                ui.label(RichText::new(user.tag()).color(p.muted));
                if let Some(pr) = store.presences.get(&user_id) {
                    for a in &pr.activities {
                        let d = a.describe();
                        if !d.is_empty() {
                            ui.label(RichText::new(d).color(p.text).size(13.0));
                        }
                    }
                }
                ui.separator();
                if let Some(joined) = member.as_ref().and_then(|m| m.joined_at.clone()).and_then(|j| chrono::DateTime::parse_from_rfc3339(&j).ok()) {
                    widgets::section_label(ui, &p, "Member since");
                    ui.label(joined.format("%b %d, %Y").to_string());
                }
                widgets::section_label(ui, &p, "Discord member since");
                ui.label(user_id.datetime().format("%b %d, %Y").to_string());
                if let (Some(m), Some(gs)) = (&member, guild_id.and_then(|g| store.guilds.get(&g))) {
                    if !m.roles.is_empty() {
                        widgets::section_label(ui, &p, "Roles");
                        ui.horizontal_wrapped(|ui| {
                            for r in m.roles.iter().filter_map(|r| gs.role(*r)) {
                                let c = r.display_color().map(crate::theme::rgb).unwrap_or(p.muted);
                                egui::Frame::new().fill(p.sidebar).corner_radius(CornerRadius::same(4)).inner_margin(egui::Margin::symmetric(6, 2)).show(ui, |ui| {
                                    ui.horizontal(|ui| {
                                        let (dot, _) = ui.allocate_exact_size(vec2(10.0, 10.0), Sense::hover());
                                        ui.painter().circle_filled(dot.center(), 5.0, c);
                                        ui.label(RichText::new(&r.name).size(12.0));
                                    });
                                });
                            }
                        });
                    }
                }
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if Some(user_id) != store.me() && !user.bot && widgets::primary_button(ui, &p, "Message").clicked() {
                        open_dm = true;
                    }
                    if app.link.settings.read().behavior.developer_mode && ui.button("Copy ID").clicked() {
                        ui.ctx().copy_text(user_id.to_string());
                    }
                });
                if store.voice.conn.channel().is_some() && Some(user_id) != store.me() {
                    ui.label(RichText::new("User volume").size(12.0).color(p.muted));
                    let mut vol = app.link.settings.read().voice.user_volumes.get(&user_id.to_string()).copied().unwrap_or(1.0);
                    if ui.add(egui::Slider::new(&mut vol, 0.0..=2.0).custom_formatter(|v, _| format!("{:.0}%", v * 100.0))).changed() {
                        app.link.settings.write().voice.user_volumes.insert(user_id.to_string(), vol);
                        app.link.send(Command::SetUserVolume { user_id, volume: vol });
                    }
                }
            });
        });
    if open_dm {
        app.link.send(Command::OpenDm { user_id });
        return true;
    }
    resp.should_close()
}

fn quick_switcher(ctx: &egui::Context, store: &Store, p: &crate::theme::Palette, query: &mut String, selected: &mut usize) -> (Option<Id>, bool) {
    let q = query.to_lowercase();
    let mut cands: Vec<(i32, Id, String, String)> = vec![];
    for gid in &store.guild_order {
        let Some(gs) = store.guilds.get(gid) else { continue };
        for grp in store.channel_tree(*gid) {
            for c in grp.channels {
                if c.kind.is_voice() && !q.is_empty() && !c.display_name().to_lowercase().contains(&q) {
                    continue;
                }
                let name = c.display_name();
                let score = score(&name.to_lowercase(), &q);
                if score > 0 {
                    cands.push((score + store.is_unread(c.id) as i32, c.id, format!("{} {}", sidebar::channel_icon(c.kind), name), gs.guild.name.clone()));
                }
            }
        }
    }
    for c in store.dm_list() {
        let name = store.dm_name(c.id);
        let s = score(&name.to_lowercase(), &q);
        if s > 0 {
            cands.push((s + 1, c.id, format!("@ {name}"), "Direct message".into()));
        }
    }
    cands.sort_by(|a, b| b.0.cmp(&a.0).then(a.2.cmp(&b.2)));
    cands.truncate(12);
    let mut picked = None;
    let resp = egui::Modal::new(egui::Id::unique("quick-switcher"))
        .frame(egui::Frame::new().fill(p.chat).corner_radius(CornerRadius::same(8)).inner_margin(16))
        .show(ctx, |ui| {
            ui.set_width(520.0);
            ui.label(RichText::new("Where would you like to go?").strong().color(p.text_strong));
            let e = ui.add(egui::TextEdit::singleline(query).hint_text("Search channels and DMs").desired_width(f32::INFINITY));
            e.request_focus();
            if e.changed() {
                *selected = 0;
            }
            if !cands.is_empty() {
                ui.input_mut(|i| {
                    if i.consume_key(egui::Modifiers::NONE, Key::ArrowDown) {
                        *selected = (*selected + 1) % cands.len();
                    }
                    if i.consume_key(egui::Modifiers::NONE, Key::ArrowUp) {
                        *selected = (*selected + cands.len() - 1) % cands.len();
                    }
                });
                if ui.input(|i| i.key_pressed(Key::Enter)) {
                    picked = Some(cands[(*selected).min(cands.len() - 1)].1);
                }
            }
            for (i, (_, id, label, detail)) in cands.iter().enumerate() {
                let row = widgets::nav_row(ui, p, i == *selected, 34.0, |ui, _| {
                    ui.label(RichText::new(label).color(p.text_strong));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(RichText::new(detail).size(12.0).color(p.faint));
                    });
                });
                if row.clicked() {
                    picked = Some(*id);
                }
            }
            ui.label(RichText::new("Tip: Ctrl+K opens this anywhere. Use ↑ ↓ and Enter.").size(11.0).color(p.faint));
        });
    (picked, resp.should_close())
}

/// Simple fuzzy score: prefix > word-start > substring > subsequence; 0 = no match.
pub fn score(hay: &str, needle: &str) -> i32 {
    if needle.is_empty() {
        return 1;
    }
    if hay.starts_with(needle) {
        return 100;
    }
    if hay.split(['-', '_', ' ']).any(|w| w.starts_with(needle)) {
        return 60;
    }
    if hay.contains(needle) {
        return 40;
    }
    let mut it = hay.chars();
    if needle.chars().all(|c| it.any(|h| h == c)) { 10 } else { 0 }
}

#[cfg(test)]
mod tests {
    #[test]
    fn fuzzy_scores() {
        use super::score;
        assert!(score("general", "gen") > score("off-topic-general", "gen"));
        assert!(score("off-topic", "top") > score("stop", "top"));
        assert_eq!(score("abc", "x"), 0);
        assert!(score("voice-chat", "vct") > 0);
    }
}
