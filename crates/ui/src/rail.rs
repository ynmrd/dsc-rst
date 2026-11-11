//! Left server rail: Home (DMs), guild icons with unread/mention indicators, add-server.

use egui::{CornerRadius, Rect, Ui, vec2};

use dn_cache::Store;

use crate::DiscordApp;
use crate::state::Modal;
use crate::widgets;

pub fn show(app: &mut DiscordApp, ui: &mut Ui, store: &mut Store) {
    let p = *app.palette();
    egui::ScrollArea::vertical().scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden).show(ui, |ui| {
        ui.vertical_centered(|ui| {
            ui.spacing_mut().item_spacing.y = 8.0;
            // Home / direct messages
            let home_sel = app.st.selected_guild.is_none();
            let dm_mentions: u32 = store.private_channels.iter().map(|c| store.mentions(*c)).sum();
            let r = widgets::guild_icon(ui, &mut app.textures, &p, None, "DM", home_sel, false).on_hover_text("Direct Messages");
            pill(ui, &p, r.rect, home_sel, false, r.hovered());
            if dm_mentions > 0 {
                widgets::count_badge(ui, &p, r.rect.right_bottom() - vec2(4.0, 4.0), dm_mentions);
            }
            if r.clicked() {
                app.select_guild(store, None);
            }
            let sep = ui.allocate_exact_size(vec2(32.0, 2.0), egui::Sense::hover()).0;
            ui.painter().rect_filled(sep, CornerRadius::same(1), p.divider);

            let order = store.guild_order.clone();
            for gid in order {
                let Some(gs) = store.guilds.get(&gid) else { continue };
                let name = if gs.guild.name.is_empty() { "Loading…".to_string() } else { gs.guild.name.clone() };
                let url = gs.guild.icon_url(96);
                let initials = gs.guild.initials();
                let available = gs.available;
                let selected = app.st.selected_guild == Some(gid);
                let (unread, mentions) = store.guild_unread(gid);
                let muted = app.link.settings.read().notifications.muted_guilds.contains(&gid.to_string());
                let r = widgets::guild_icon(ui, &mut app.textures, &p, url.as_deref(), &initials, selected, !available)
                    .on_hover_text(if available { name.clone() } else { format!("{name} (unavailable)") });
                pill(ui, &p, r.rect, selected, unread && !muted, r.hovered());
                if mentions > 0 {
                    widgets::count_badge(ui, &p, r.rect.right_bottom() - vec2(4.0, 4.0), mentions);
                }
                if r.clicked() && available {
                    app.select_guild(store, Some(gid));
                }
                r.context_menu(|ui| {
                    if ui.button("Mark as read").clicked() {
                        let ids: Vec<_> = store.guilds.get(&gid).map(|g| g.channel_ids.clone()).unwrap_or_default();
                        for c in ids {
                            if let Some(last) = store.channels.get(&c).and_then(|c| c.last_message_id) {
                                app.link.send(dn_cache::bus::Command::MarkRead { channel_id: c, message_id: last });
                            }
                        }
                        ui.close();
                    }
                    let label = if muted { "Unmute server" } else { "Mute server" };
                    if ui.button(label).clicked() {
                        {
                            let mut s = app.link.settings.write();
                            let set = &mut s.notifications.muted_guilds;
                            if muted {
                                set.remove(&gid.to_string());
                            } else {
                                set.insert(gid.to_string());
                            }
                        }
                        app.link.send(dn_cache::bus::Command::SettingsChanged);
                        ui.close();
                    }
                    if app.link.settings.read().behavior.developer_mode && ui.button("Copy server ID").clicked() {
                        ui.ctx().copy_text(gid.to_string());
                        ui.close();
                    }
                });
            }

            let add = ui.add_sized(
                vec2(48.0, 48.0),
                egui::Button::new(egui::RichText::new("+").size(24.0).color(p.green)).fill(p.sidebar).corner_radius(CornerRadius::same(24)),
            );
            if add.on_hover_text("Add the bot to a server").clicked() {
                app.st.modal = Some(Modal::AddServer);
            }
        });
    });
}

/// White indicator on the rail's left edge: dot for unread, tall bar for selected.
fn pill(ui: &Ui, p: &crate::theme::Palette, icon: Rect, selected: bool, unread: bool, hovered: bool) {
    let h = if selected {
        40.0
    } else if hovered {
        20.0
    } else if unread {
        8.0
    } else {
        return;
    };
    let x = ui.max_rect().left() - 12.0;
    let r = Rect::from_center_size(egui::pos2(x + 2.0, icon.center().y), vec2(8.0, h));
    ui.painter().with_clip_rect(ui.ctx().content_rect()).rect_filled(r, CornerRadius::same(4), p.text_strong);
}
