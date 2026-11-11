//! Channel sidebar: guild header, channel tree (with voice participants and threads) or DM
//! list, voice connection panel and the account panel.

use egui::{Color32, CornerRadius, RichText, Ui, vec2};

use dn_cache::bus::Command;
use dn_cache::{Store, VoiceConn};
use dn_core::{Channel, ChannelKind, Id};

use crate::DiscordApp;
use crate::state::{Modal, SettingsTab};
use crate::theme::Palette;
use crate::widgets;

pub fn channel_icon(k: ChannelKind) -> &'static str {
    match k {
        ChannelKind::GuildVoice => "🔊",
        ChannelKind::GuildStageVoice => "🎙",
        ChannelKind::GuildAnnouncement => "📢",
        ChannelKind::GuildForum | ChannelKind::GuildMedia => "💬",
        k if k.is_thread() => "↳",
        ChannelKind::Dm | ChannelKind::GroupDm => "@",
        _ => "#",
    }
}

pub fn show(app: &mut DiscordApp, ui: &mut Ui, store: &mut Store) {
    let p = *app.palette();
    let header_h = 48.0;
    // Header
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), header_h), egui::Sense::click());
    let title = match app.st.selected_guild.and_then(|g| store.guilds.get(&g)) {
        Some(gs) => gs.guild.name.clone(),
        None => "Direct Messages".to_string(),
    };
    ui.painter().text(rect.left_center() + vec2(16.0, 0.0), egui::Align2::LEFT_CENTER, &title, crate::theme::bold(15.0), p.text_strong);
    ui.painter().hline(rect.x_range(), rect.bottom(), egui::Stroke::new(1.0, p.rail));
    if let Some(gid) = app.st.selected_guild {
        resp.context_menu(|ui| {
            if let Some(app_id) = store.session.application_id {
                if ui.button("Invite the bot to another server").clicked() {
                    let url = invite_url(app_id, None);
                    ui.ctx().open_url(egui::OpenUrl::new_tab(url));
                    ui.close();
                }
            }
            if app.link.settings.read().behavior.developer_mode && ui.button("Copy server ID").clicked() {
                ui.ctx().copy_text(gid.to_string());
                ui.close();
            }
        });
    }

    let bottom_h = 56.0 + if store.voice.conn != VoiceConn::Idle { 64.0 } else { 0.0 };
    let list_h = (ui.available_height() - bottom_h).max(0.0);
    ui.allocate_ui(vec2(ui.available_width(), list_h), |ui| {
        egui::ScrollArea::vertical().id_salt("channels").auto_shrink([false, false]).show(ui, |ui| {
            ui.add_space(8.0);
            ui.spacing_mut().item_spacing.y = 2.0;
            egui::Frame::new().inner_margin(egui::Margin::symmetric(8, 0)).show(ui, |ui| match app.st.selected_guild {
                Some(g) => guild_channels(app, ui, store, g, &p),
                None => dm_list(app, ui, store, &p),
            });
        });
    });
    voice_panel(app, ui, store, &p);
    user_panel(app, ui, store, &p);
}

fn guild_channels(app: &mut DiscordApp, ui: &mut Ui, store: &mut Store, guild: Id, p: &Palette) {
    let tree: Vec<(Option<(Id, String)>, Vec<Channel>)> = store
        .channel_tree(guild)
        .into_iter()
        .map(|g| (g.category.map(|c| (c.id, c.display_name())), g.channels.into_iter().cloned().collect()))
        .collect();
    if tree.is_empty() {
        ui.label(RichText::new("No channels are visible to the bot here. Check its role permissions.").color(p.muted));
    }
    for (cat, channels) in tree {
        if let Some((cid, name)) = &cat {
            let collapsed = app.st.collapsed_categories.contains(cid);
            ui.add_space(12.0);
            let r = ui.add(
                egui::Label::new(RichText::new(format!("{} {}", if collapsed { "›" } else { "⌄" }, name.to_uppercase())).size(11.5).strong().color(p.muted))
                    .sense(egui::Sense::click())
                    .selectable(false),
            );
            if r.clicked() {
                if collapsed {
                    app.st.collapsed_categories.remove(cid);
                } else {
                    app.st.collapsed_categories.insert(*cid);
                }
            }
            if collapsed {
                // Collapsed categories still show the selected channel and unread ones.
                let visible: Vec<Channel> = channels.iter().filter(|c| app.st.selected_channel == Some(c.id) || store.is_unread(c.id)).cloned().collect();
                for c in &visible {
                    channel_row(app, ui, store, c, p, 0.0);
                }
                continue;
            }
        }
        for c in &channels {
            channel_row(app, ui, store, c, p, 0.0);
            if c.kind.is_voice() {
                voice_participants(app, ui, store, guild, c.id, p);
            }
            let show_threads = app.st.selected_channel.is_some_and(|s| {
                s == c.id || store.channels.get(&s).is_some_and(|sc| sc.parent_id == Some(c.id) && sc.kind.is_thread())
            });
            if show_threads || c.kind.is_forum() {
                let threads: Vec<Channel> = store.threads_of(c.id).into_iter().take(if c.kind.is_forum() { 8 } else { 6 }).cloned().collect();
                for t in &threads {
                    channel_row(app, ui, store, t, p, 16.0);
                }
            }
        }
    }
}

fn channel_row(app: &mut DiscordApp, ui: &mut Ui, store: &mut Store, c: &Channel, p: &Palette, indent: f32) {
    let selected = app.st.selected_channel == Some(c.id);
    let unread = store.is_unread(c.id) && !c.kind.is_voice();
    let mentions = store.mentions(c.id);
    let muted = app.link.settings.read().notifications.muted_channels.contains(&c.id.to_string());
    let in_voice = store.voice.conn.channel().map(|(_, ch)| ch) == Some(c.id);
    let name = c.display_name();
    let icon = channel_icon(c.kind);
    let resp = widgets::nav_row(ui, p, selected, 32.0, |ui, hovered| {
        ui.add_space(indent);
        let color = if selected || (unread && !muted) {
            p.text_strong
        } else if hovered {
            p.text
        } else if muted {
            p.faint
        } else {
            p.muted
        };
        ui.label(RichText::new(icon).color(p.faint).size(15.0));
        let mut text = RichText::new(&name).color(color);
        if unread && !muted {
            text = text.strong();
        }
        if in_voice {
            text = text.color(p.green);
        }
        ui.add(egui::Label::new(text).truncate().selectable(false));
        if mentions > 0 {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let (r, _) = ui.allocate_exact_size(vec2(20.0, 16.0), egui::Sense::hover());
                widgets::count_badge(ui, p, r.center(), mentions);
            });
        }
    });
    if unread && !muted && !selected {
        let r = egui::Rect::from_center_size(egui::pos2(ui.max_rect().left() - 6.0, resp.rect.center().y), vec2(8.0, 8.0));
        ui.painter().with_clip_rect(ui.ctx().content_rect()).rect_filled(r, CornerRadius::same(4), p.text_strong);
    }
    if resp.clicked() {
        if c.kind.is_voice() {
            if let Some(g) = c.guild_id {
                if !in_voice {
                    app.link.send(Command::JoinVoice { guild_id: g, channel_id: c.id });
                }
            }
        }
        app.open_channel(store, c.id);
    }
    let topic = c.topic.clone();
    let resp = match topic.filter(|t| !t.is_empty()) {
        Some(t) => resp.on_hover_text(t),
        None => resp,
    };
    resp.context_menu(|ui| {
        if c.kind.is_voice() {
            if in_voice {
                if ui.button("Disconnect").clicked() {
                    app.link.send(Command::LeaveVoice);
                    ui.close();
                }
            } else if let Some(g) = c.guild_id {
                if ui.button("Join voice").clicked() {
                    app.link.send(Command::JoinVoice { guild_id: g, channel_id: c.id });
                    ui.close();
                }
            }
        }
        if let Some(last) = c.last_message_id {
            if ui.button("Mark as read").clicked() {
                app.link.send(Command::MarkRead { channel_id: c.id, message_id: last });
                ui.close();
            }
        }
        let label = if muted { "Unmute channel" } else { "Mute channel" };
        if ui.button(label).clicked() {
            {
                let mut s = app.link.settings.write();
                let set = &mut s.notifications.muted_channels;
                if muted {
                    set.remove(&c.id.to_string());
                } else {
                    set.insert(c.id.to_string());
                }
            }
            app.link.send(Command::SettingsChanged);
            ui.close();
        }
        if app.link.settings.read().behavior.developer_mode && ui.button("Copy channel ID").clicked() {
            ui.ctx().copy_text(c.id.to_string());
            ui.close();
        }
    });
}

fn voice_participants(app: &mut DiscordApp, ui: &mut Ui, store: &mut Store, guild: Id, channel: Id, p: &Palette) {
    let members: Vec<(Id, bool, bool, bool)> = store
        .voice_members(guild, channel)
        .iter()
        .map(|v| (v.user_id, v.self_mute || v.mute, v.self_deaf || v.deaf, v.self_stream))
        .collect();
    let me = store.me();
    for (uid, muted, deaf, streaming) in members {
        let name = store.display_name(Some(guild), uid);
        let url = store.users.get(&uid).map(|u| u.avatar_url(48)).unwrap_or_default();
        let speaking = store.voice.speaking.contains(&uid) || (Some(uid) == me && store.voice.self_speaking);
        let resp = widgets::nav_row(ui, p, false, 28.0, |ui, _| {
            ui.add_space(24.0);
            widgets::avatar(ui, &mut app.textures, p, &url, 22.0, None, speaking);
            ui.add(egui::Label::new(RichText::new(&name).color(if speaking { p.text_strong } else { p.muted }).size(13.5)).truncate().selectable(false));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if deaf {
                    ui.label(RichText::new("🔇").color(p.red).size(12.0)).on_hover_text("Deafened");
                } else if muted {
                    ui.label(RichText::new("🎤").color(p.red).size(12.0)).on_hover_text("Muted");
                }
                if streaming {
                    ui.label(RichText::new("LIVE").color(Color32::WHITE).background_color(p.red).size(10.0));
                }
            });
        });
        if resp.clicked() {
            app.st.modal = Some(Modal::Profile { user_id: uid, guild_id: Some(guild) });
        }
        if Some(uid) != me {
            resp.context_menu(|ui| {
                ui.label("User volume");
                let mut vol = app.link.settings.read().voice.user_volumes.get(&uid.to_string()).copied().unwrap_or(1.0);
                if ui.add(egui::Slider::new(&mut vol, 0.0..=2.0).custom_formatter(|v, _| format!("{:.0}%", v * 100.0))).changed() {
                    app.link.settings.write().voice.user_volumes.insert(uid.to_string(), vol);
                    app.link.send(Command::SetUserVolume { user_id: uid, volume: vol });
                }
            });
        }
    }
}

fn dm_list(app: &mut DiscordApp, ui: &mut Ui, store: &mut Store, p: &Palette) {
    widgets::section_label(ui, p, "Direct messages");
    ui.add_space(4.0);
    let dms: Vec<(Id, String, String, bool, u32)> = store
        .dm_list()
        .iter()
        .map(|c| {
            let other = c.recipients.iter().find(|u| Some(u.id) != store.me());
            let url = other.map(|u| u.avatar_url(64)).unwrap_or_default();
            (c.id, store.dm_name(c.id), url, store.is_unread(c.id), store.mentions(c.id))
        })
        .collect();
    if dms.is_empty() {
        ui.label(
            RichText::new(
                "No conversations yet. Bots can only see DMs that users send to the bot, or ones you open from a member's profile.",
            )
            .color(p.muted)
            .size(13.0),
        );
    }
    for (cid, name, url, unread, mentions) in dms {
        let selected = app.st.selected_channel == Some(cid);
        let resp = widgets::nav_row(ui, p, selected, 42.0, |ui, _| {
            widgets::avatar(ui, &mut app.textures, p, &url, 32.0, None, false);
            let mut t = RichText::new(&name).color(if selected || unread { p.text_strong } else { p.muted });
            if unread {
                t = t.strong();
            }
            ui.add(egui::Label::new(t).truncate().selectable(false));
            if mentions > 0 {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let (r, _) = ui.allocate_exact_size(vec2(20.0, 16.0), egui::Sense::hover());
                    widgets::count_badge(ui, p, r.center(), mentions);
                });
            }
        });
        if resp.clicked() {
            app.open_channel(store, cid);
        }
    }
}

fn voice_panel(app: &mut DiscordApp, ui: &mut Ui, store: &mut Store, p: &Palette) {
    let (label, color, channel) = match &store.voice.conn {
        VoiceConn::Idle => return,
        VoiceConn::Connecting { channel_id, .. } => ("Connecting…", p.yellow, Some(*channel_id)),
        VoiceConn::Connected { channel_id, .. } => ("Voice Connected", p.green, Some(*channel_id)),
        VoiceConn::Reconnecting { channel_id, .. } => ("Reconnecting…", p.yellow, Some(*channel_id)),
        VoiceConn::Failed(_) => ("Voice Failed", p.red, None),
    };
    let detail = match (&store.voice.conn, channel) {
        (VoiceConn::Failed(e), _) => e.clone(),
        (_, Some(c)) => store.location_label(c).trim_start_matches('#').to_string(),
        _ => String::new(),
    };
    egui::Frame::new().fill(p.rail.gamma_multiply(0.6)).inner_margin(egui::Margin::symmetric(10, 8)).show(ui, |ui| {
        ui.set_min_height(48.0);
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.label(RichText::new(label).color(color).strong().size(13.5));
                ui.add(egui::Label::new(RichText::new(detail).color(p.muted).size(12.0)).truncate());
                if let Some(ms) = store.voice.ping_ms {
                    ui.label(RichText::new(format!("{ms} ms")).color(p.faint).size(11.0));
                }
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if widgets::icon_button(ui, p, "📞", "Disconnect", Some(p.red)).clicked() {
                    app.link.send(Command::LeaveVoice);
                }
                if let Some(c) = channel {
                    if widgets::icon_button(ui, p, "💬", "Open voice channel chat", None).clicked() {
                        app.open_channel(store, c);
                    }
                }
            });
        });
    });
}

fn user_panel(app: &mut DiscordApp, ui: &mut Ui, store: &mut Store, p: &Palette) {
    let Some(me) = store.session.me.clone() else {
        egui::Frame::new().fill(p.rail.gamma_multiply(0.6)).inner_margin(10).show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.label(RichText::new(conn_label(store)).color(p.muted));
        });
        return;
    };
    egui::Frame::new().fill(p.rail.gamma_multiply(0.6)).inner_margin(egui::Margin::symmetric(8, 8)).show(ui, |ui| {
        ui.set_min_width(ui.available_width());
        ui.horizontal(|ui| {
            let status = app.link.settings.read().behavior.developer_mode;
            let _ = status;
            let my_status = store.status_of(me.id);
            let r = widgets::avatar(ui, &mut app.textures, p, &me.avatar_url(64), 32.0, Some(my_status_or_online(my_status)), store.voice.self_speaking);
            if r.on_hover_text("Set status").clicked() {
                app.st.modal = Some(Modal::Status { custom: String::new() });
            }
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                ui.add(egui::Label::new(RichText::new(me.display_name()).color(p.text_strong).strong().size(13.5)).truncate());
                ui.label(RichText::new(conn_label(store)).color(p.muted).size(11.5));
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                if widgets::icon_button(ui, p, "⚙", "User settings", None).clicked() {
                    app.open_settings(SettingsTab::Account);
                }
                let deaf = store.voice.self_deaf;
                if widgets::icon_button(ui, p, "🎧", if deaf { "Undeafen" } else { "Deafen" }, deaf.then_some(p.red)).clicked() {
                    app.link.send(Command::SetDeaf(!deaf));
                }
                let mute = store.voice.self_mute;
                if widgets::icon_button(ui, p, "🎤", if mute { "Unmute" } else { "Mute" }, mute.then_some(p.red)).clicked() {
                    app.link.send(Command::SetMute(!mute));
                }
            });
        });
    });
}

fn my_status_or_online(s: dn_core::Status) -> dn_core::Status {
    // Bots receive no presence update for themselves; show online while connected.
    if s == dn_core::Status::Offline { dn_core::Status::Online } else { s }
}

pub fn conn_label(store: &Store) -> String {
    match &store.session.conn {
        dn_cache::ConnStatus::Offline => "Offline".into(),
        dn_cache::ConnStatus::Connecting => "Connecting…".into(),
        dn_cache::ConnStatus::Connected => match store.session.latency_ms {
            Some(ms) => format!("Online · {ms} ms"),
            None => "Online".into(),
        },
        dn_cache::ConnStatus::Reconnecting { attempt, delay_secs } => format!("Reconnecting in {delay_secs}s (#{attempt})"),
        dn_cache::ConnStatus::Fatal(e) => format!("Error: {e}"),
    }
}

/// Bot invite URL with the permissions this client needs.
pub fn invite_url(app_id: Id, guild: Option<Id>) -> String {
    use dn_core::Permissions as P;
    let perms = P::VIEW_CHANNEL
        | P::SEND_MESSAGES
        | P::SEND_MESSAGES_IN_THREADS
        | P::CREATE_PUBLIC_THREADS
        | P::EMBED_LINKS
        | P::ATTACH_FILES
        | P::READ_MESSAGE_HISTORY
        | P::ADD_REACTIONS
        | P::USE_EXTERNAL_EMOJIS
        | P::USE_EXTERNAL_STICKERS
        | P::PIN_MESSAGES
        | P::CONNECT
        | P::SPEAK
        | P::USE_VAD;
    let mut url = format!("https://discord.com/oauth2/authorize?client_id={app_id}&scope=bot&permissions={}", perms.0);
    if let Some(g) = guild {
        url.push_str(&format!("&guild_id={g}&disable_guild_select=true"));
    }
    url
}
