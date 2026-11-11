//! Right-hand member list (grouped by hoisted role and presence).

use egui::{RichText, Ui, vec2};

use dn_cache::Store;

use crate::state::Modal;
use crate::{DiscordApp, widgets};

pub fn show(app: &mut DiscordApp, ui: &mut Ui, store: &mut Store) {
    let p = *app.palette();
    let Some(guild) = app.st.selected_guild else { return };
    let groups = store.member_list(guild, app.st.selected_channel);
    let total = store.guilds.get(&guild).and_then(|g| g.guild.member_count.or(g.guild.approximate_member_count)).unwrap_or(0);
    let shown: usize = groups.iter().map(|g| g.members.len()).sum();
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        if groups.is_empty() {
            ui.label(
                RichText::new("Member list unavailable. Enable the Server Members intent for this bot in the Developer Portal.")
                    .color(p.muted)
                    .size(12.5),
            );
        }
        for g in groups {
            ui.add_space(12.0);
            widgets::section_label(ui, &p, &format!("{} — {}", g.title, g.members.len()));
            ui.add_space(2.0);
            for m in g.members {
                let url = store.users.get(&m.user_id).map(|u| u.avatar_url(64)).unwrap_or_default();
                let offline = matches!(m.status, dn_core::Status::Offline | dn_core::Status::Invisible);
                let resp = widgets::nav_row(ui, &p, false, 42.0, |ui, hovered| {
                    widgets::avatar(ui, &mut app.textures, &p, &url, 32.0, Some(m.status), store.voice.speaking.contains(&m.user_id));
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = 0.0;
                        ui.add_space(4.0);
                        let mut color = p.role_color(m.color);
                        if offline && !hovered {
                            color = color.gamma_multiply(0.5);
                        }
                        ui.horizontal(|ui| {
                            ui.add(egui::Label::new(RichText::new(&m.name).color(color)).truncate().selectable(false));
                            if m.bot {
                                ui.label(RichText::new(" BOT ").size(9.5).color(egui::Color32::WHITE).background_color(p.accent));
                            }
                        });
                        if let Some(a) = &m.activity {
                            ui.add(egui::Label::new(RichText::new(a).size(11.5).color(p.muted)).truncate().selectable(false));
                        }
                    });
                });
                if resp.clicked() {
                    app.st.modal = Some(Modal::Profile { user_id: m.user_id, guild_id: Some(guild) });
                    app.link.send(dn_cache::bus::Command::FetchProfile { user_id: m.user_id, guild_id: Some(guild) });
                }
            }
        }
        if (shown as u64) < total && total > 0 {
            ui.add_space(8.0);
            ui.label(RichText::new(format!("Showing {shown} of {total} members")).size(11.5).color(p.faint));
        }
        ui.add_space(vec2(0.0, 8.0).y);
    });
}
