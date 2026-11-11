//! Chat area: header, virtualized message list, typing indicator, composer, emoji picker
//! and the search results panel.

use std::time::{Duration, Instant};

use egui::text::{CCursor, CCursorRange};
use egui::{Color32, CornerRadius, Key, KeyboardShortcut, Modifiers, Rect, RichText, Sense, Stroke, Ui, vec2};

use dn_cache::Store;
use dn_cache::bus::{Command, SearchRequest, SearchSource};
use dn_core::{ChannelKind, Emoji, Id, Message, Permissions};
use dn_messaging::compose::{self, CompletionKind};

use crate::message::{self, RowCtx};
use crate::state::EmojiTarget;
use crate::theme::Palette;
use crate::virtual_list::{self, offsets, visible_range};
use crate::{DiscordApp, sidebar, widgets};

pub fn show(app: &mut DiscordApp, ui: &mut Ui, store: &mut Store) {
    let p = *app.palette();
    let Some(channel) = app.st.selected_channel.filter(|c| store.channels.contains_key(c)) else {
        empty_state(app, ui, store, &p);
        return;
    };
    header(app, ui, store, channel, &p);
    let kind = store.channels.get(&channel).map(|c| c.kind).unwrap_or_default();
    if kind.is_forum() {
        forum_view(app, ui, store, channel, &p);
        return;
    }
    let composer_h = 72.0
        + if app.st.composer(channel).reply_to.is_some() { 30.0 } else { 0.0 }
        + if app.st.composer(channel).attachments.is_empty() { 0.0 } else { 34.0 };
    let list_h = (ui.available_height() - composer_h - 22.0).max(80.0);
    ui.allocate_ui(vec2(ui.available_width(), list_h), |ui| message_list(app, ui, store, channel, &p));
    typing_line(ui, store, channel, &p);
    composer(app, ui, store, channel, &p);
    emoji_picker(app, ui.ctx(), store, &p);
}

fn empty_state(app: &mut DiscordApp, ui: &mut Ui, store: &Store, p: &Palette) {
    ui.centered_and_justified(|ui| {
        let text = if !store.session.ready {
            format!("{}\n\nWaiting for Discord…", sidebar::conn_label(store))
        } else if app.st.selected_guild.is_none() {
            "No direct message selected.\nUsers can DM the bot, or open a DM from a member's profile.".to_string()
        } else {
            "Select a channel".to_string()
        };
        ui.label(RichText::new(text).color(p.muted).size(16.0));
    });
}

fn header(app: &mut DiscordApp, ui: &mut Ui, store: &mut Store, channel: Id, p: &Palette) {
    let Some(c) = store.channels.get(&channel).cloned() else { return };
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 48.0), Sense::hover());
    ui.painter().hline(rect.x_range(), rect.bottom(), Stroke::new(1.0, p.rail));
    let mut hui = ui
        .new_child(egui::UiBuilder::new().max_rect(rect.shrink2(vec2(16.0, 8.0))).layout(egui::Layout::left_to_right(egui::Align::Center)));
    let name = if c.kind.is_private() { store.dm_name(channel) } else { c.display_name() };
    hui.label(RichText::new(sidebar::channel_icon(c.kind)).color(p.faint).size(18.0));
    hui.label(RichText::new(name).strong().color(p.text_strong).size(15.5));
    if let Some(t) = c.topic.as_deref().filter(|t| !t.is_empty()) {
        hui.separator();
        hui.add(egui::Label::new(RichText::new(t.replace('\n', " ")).color(p.muted).size(13.0)).truncate()).on_hover_text(t);
    }
    hui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        if c.guild_id.is_some() {
            let out = egui::TextEdit::singleline(&mut app.st.search.query)
                .id(egui::Id::unique("search-box"))
                .hint_text("Search")
                .desired_width(180.0)
                .show(ui);
            if out.response.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) && !app.st.search.query.trim().is_empty() {
                start_search(app, store, 0, None);
            }
            let show_members = app.link.settings.read().appearance.show_member_list;
            if widgets::icon_button(ui, p, "👥", "Member list", show_members.then_some(p.text_strong)).clicked() {
                let mut s = app.link.settings.write();
                s.appearance.show_member_list = !show_members;
                drop(s);
                app.st.search.open = false;
                app.link.send(Command::SettingsChanged);
            }
        }
        let pins_open = app.st.pins.as_ref().is_some_and(|(c, _, _)| *c == channel);
        if widgets::icon_button(ui, p, "📌", "Pinned messages", pins_open.then_some(p.text_strong)).clicked() {
            if pins_open {
                app.st.pins = None;
            } else {
                app.st.pins = Some((channel, vec![], false));
                app.link.send(Command::LoadPins { channel_id: channel });
            }
        }
        if store.voice.conn.channel().is_none()
            && c.kind.is_voice()
            && let Some(g) = c.guild_id
            && widgets::icon_button(ui, p, "🔊", "Join voice", None).clicked()
        {
            app.link.send(Command::JoinVoice { guild_id: g, channel_id: channel });
        }
    });
    if app.st.pins.as_ref().is_some_and(|(c, _, _)| *c == channel) {
        pins_popup(app, ui, store, channel, rect, p);
    }
}

pub fn start_search(app: &mut DiscordApp, store: &Store, offset: u32, channel_only: Option<Id>) {
    let req = SearchRequest {
        query: app.st.search.query.trim().to_string(),
        guild_id: app.st.selected_guild,
        channel_id: channel_only,
        author_id: None,
        offset,
    };
    let _ = store;
    app.st.search.open = true;
    app.st.search.loading = true;
    app.st.search.last = Some(req.clone());
    if offset == 0 {
        app.st.search.results.clear();
    }
    app.link.send(Command::Search(req));
}

fn pins_popup(app: &mut DiscordApp, ui: &mut Ui, store: &mut Store, channel: Id, header: Rect, p: &Palette) {
    let mut close = false;
    let mut jump = None;
    egui::Area::new(egui::Id::unique("pins"))
        .fixed_pos(egui::pos2(header.right() - 440.0, header.bottom() + 4.0))
        .order(egui::Order::Foreground)
        .show(ui.ctx(), |ui| {
            egui::Frame::new()
                .fill(p.floating)
                .stroke(Stroke::new(1.0, p.divider))
                .corner_radius(CornerRadius::same(8))
                .inner_margin(12)
                .show(ui, |ui| {
                    ui.set_width(420.0);
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("Pinned Messages").strong().color(p.text_strong));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.small_button("✕").clicked() {
                                close = true;
                            }
                        });
                    });
                    let Some((_, msgs, loaded)) = app.st.pins.clone() else { return };
                    if !loaded {
                        ui.spinner();
                        return;
                    }
                    if msgs.is_empty() {
                        ui.label(RichText::new("This channel doesn't have any pinned messages yet.").color(p.muted));
                    }
                    egui::ScrollArea::vertical().max_height(480.0).show(ui, |ui| {
                        for m in &msgs {
                            egui::Frame::new().fill(p.chat).corner_radius(CornerRadius::same(6)).inner_margin(8).show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                ui.horizontal(|ui| {
                                    ui.label(RichText::new(m.author_name()).strong());
                                    ui.label(RichText::new(message::format_time(m.timestamp_ms(), false, true)).size(11.0).color(p.faint));
                                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                        if ui.small_button("Jump").clicked() {
                                            jump = Some(m.id);
                                        }
                                        if store.permissions_in(channel).can_pin() && ui.small_button("Unpin").clicked() {
                                            app.link.send(Command::Pin { channel_id: channel, message_id: m.id, pin: false });
                                        }
                                    });
                                });
                                let snippet = message::plain_snippet(store, m.guild_id, &m.content, 300);
                                ui.label(RichText::new(snippet).color(p.text));
                                if !m.attachments.is_empty() {
                                    ui.label(RichText::new(format!("📎 {} attachment(s)", m.attachments.len())).color(p.muted).size(12.0));
                                }
                            });
                            ui.add_space(6.0);
                        }
                    });
                });
        });
    if close {
        app.st.pins = None;
    }
    if let Some(id) = jump {
        app.st.pins = None;
        jump_to(app, store, channel, id);
    }
}

pub fn jump_to(app: &mut DiscordApp, store: &mut Store, channel: Id, message: Id) {
    if app.st.selected_channel != Some(channel) {
        app.open_channel(store, channel);
    }
    let loaded = store.channel_messages(channel).is_some_and(|cm| cm.get(message).is_some());
    if !loaded {
        app.link.send(Command::JumpToMessage { channel_id: channel, message_id: message });
    }
    app.st.list(channel).scroll_to_message = Some(message);
}

#[derive(Clone)]
enum Row {
    Intro,
    Loading,
    Day(String, Id),
    NewMarker,
    Msg(usize),
    Pending(usize),
}

fn row_key(r: &Row) -> u64 {
    match r {
        Row::Intro => 1,
        Row::Loading => 2,
        Row::NewMarker => 3,
        Row::Day(_, id) => id.0 ^ 0x5555_0000_0000_0000,
        Row::Msg(_) | Row::Pending(_) => 0,
    }
}

fn mix(a: u64, b: u64) -> u64 {
    (a ^ b.wrapping_mul(0x9E37_79B9_7F4A_7C15)).rotate_left(23)
}

fn message_signature(m: &Message, header: bool, editing: bool, mention: bool) -> u64 {
    let mut s = mix(m.content.len() as u64, crate::fxhash(&m.content));
    s = mix(s, m.edited_timestamp.as_ref().map(|t| crate::fxhash(t)).unwrap_or(0));
    s = mix(s, m.reactions.len() as u64);
    s = mix(s, m.reactions.iter().map(|r| r.count as u64 * 31 + r.me as u64).sum());
    s = mix(s, m.attachments.len() as u64 * 7 + m.embeds.len() as u64 * 13 + m.sticker_items.len() as u64);
    s = mix(s, header as u64 | (editing as u64) << 1 | (mention as u64) << 2 | (m.pinned as u64) << 3);
    s
}

fn estimate(m: &Message, header: bool) -> f32 {
    let lines = (m.content.len() as f32 / 90.0).ceil().max(1.0);
    (if header { 44.0 } else { 6.0 })
        + lines * 20.0
        + m.attachments.len() as f32 * 300.0
        + m.embeds.len() as f32 * 140.0
        + if m.reactions.is_empty() { 0.0 } else { 30.0 }
}

fn message_list(app: &mut DiscordApp, ui: &mut Ui, store: &mut Store, channel: Id, p: &Palette) {
    let Some(cm) = store.channel_messages(channel) else {
        ui.centered_and_justified(|ui| ui.spinner());
        return;
    };
    if !cm.loaded_once && cm.messages.is_empty() {
        ui.centered_and_justified(|ui| ui.spinner());
        return;
    }
    let msgs: Vec<Message> = cm.messages.iter().cloned().collect();
    let pending: Vec<(String, Message, Option<String>)> =
        cm.pending.iter().map(|p| (p.nonce.clone(), p.message.clone(), p.failed.clone())).collect();
    let (reached_start, has_gap, loading_older) = (cm.reached_start, cm.has_newer_gap, cm.loading_older);
    let me = store.me();
    let guild_id = store.guild_of(channel);
    let perms = store.permissions_in(channel);
    let editing = app.st.editing.as_ref().map(|(id, _)| *id);

    // Rows
    let marker = app.st.list(channel).unread_marker;
    let mut rows: Vec<Row> = Vec::with_capacity(msgs.len() + 8);
    rows.push(if reached_start { Row::Intro } else { Row::Loading });
    let mut headers = Vec::with_capacity(msgs.len());
    let mut marker_placed = false;
    for (i, m) in msgs.iter().enumerate() {
        let prev = if i > 0 { Some(&msgs[i - 1]) } else { None };
        if let Some(day) = dn_messaging::grouping::day_divider(prev, m) {
            rows.push(Row::Day(day, m.id));
        }
        if !marker_placed && marker.is_some_and(|mk| m.id > mk) && Some(m.author.id) != me {
            rows.push(Row::NewMarker);
            marker_placed = true;
        }
        headers.push(dn_messaging::grouping::starts_group(prev, m) || (marker_placed && matches!(rows.last(), Some(Row::NewMarker))));
        rows.push(Row::Msg(i));
    }
    for i in 0..pending.len() {
        rows.push(Row::Pending(i));
    }

    let width = ui.available_width();
    let list = app.st.lists.entry(channel).or_default();
    list.heights.set_width(width);
    let mut sigs = Vec::with_capacity(rows.len());
    let heights: Vec<f32> = rows
        .iter()
        .map(|r| {
            let (key, sig, est) = match r {
                Row::Msg(i) => {
                    let m = &msgs[*i];
                    let mention = store.is_mention(m);
                    (m.id.0, message_signature(m, headers[*i], editing == Some(m.id), mention), estimate(m, headers[*i]))
                }
                Row::Pending(i) => (crate::fxhash(&pending[*i].0), pending[*i].2.is_some() as u64 + 1, 44.0),
                Row::Intro => (1, 0, 120.0),
                Row::Loading => (2, 0, 60.0),
                Row::Day(_, _) => (row_key(r), 0, 36.0),
                Row::NewMarker => (3, 0, 20.0),
            };
            sigs.push((key, sig));
            list.heights.get(key, sig).unwrap_or(est)
        })
        .collect();
    let offs = offsets(heights.iter().copied());
    let total = *offs.last().unwrap_or(&0.0);

    // Keep the viewport anchored when older messages were prepended.
    let oldest = msgs.first().map(|m| m.id);
    let mut forced_offset = None;
    if list.last_oldest != oldest {
        if let (Some((anchor_id, delta)), Some(_)) = (list.anchor, list.last_oldest)
            && let Some(row) = rows.iter().position(|r| matches!(r, Row::Msg(i) if msgs[*i].id == anchor_id))
        {
            forced_offset = Some(virtual_list::anchored_offset(&offs, row, delta));
        }
        list.last_oldest = oldest;
    }
    if let Some(target) = list.scroll_to_message
        && let Some(row) = rows.iter().position(|r| matches!(r, Row::Msg(i) if msgs[*i].id == target))
    {
        forced_offset = Some((offs[row] - 120.0).max(0.0));
        list.scroll_to_message = None;
        list.flash = Some((target, Instant::now()));
    }
    if let Some(y) = list.scroll_to.take() {
        // Callers pass f32::MAX for "bottom"; an unclamped value overflows to NaN in layout.
        forced_offset = Some(y.clamp(0.0, total));
    }
    let forced_offset = forced_offset.map(|y: f32| if y.is_finite() { y.clamp(0.0, total) } else { total });
    let flash = list.flash.filter(|(_, at)| at.elapsed() < Duration::from_millis(1500));

    let mut sa = egui::ScrollArea::vertical().id_salt(("chat", channel)).auto_shrink([false, false]).stick_to_bottom(true);
    if let Some(y) = forced_offset {
        sa = sa.vertical_scroll_offset(y);
    }
    let mut measured: Vec<(u64, u64, f32, f32)> = vec![];
    let mut first_visible: Option<(Id, f32)> = None;
    let mut near_top = false;
    let output = sa.show_viewport(ui, |ui, viewport| {
        ui.set_min_height(total);
        let origin = ui.min_rect().min;
        near_top = viewport.min.y < 800.0;
        for i in visible_range(&offs, viewport.min.y - 300.0, viewport.max.y + 300.0) {
            let top = origin.y + offs[i];
            let rect = Rect::from_min_size(egui::pos2(origin.x, top), vec2(width, f32::INFINITY));
            let mut child = ui.new_child(
                egui::UiBuilder::new().max_rect(rect).layout(egui::Layout::top_down(egui::Align::Min)).id_salt(("row", sigs[i].0)),
            );
            match &rows[i] {
                Row::Intro => intro(&mut child, store, channel, p),
                Row::Loading => {
                    child.add_space(16.0);
                    child.vertical_centered(|ui| {
                        if loading_older {
                            ui.spinner();
                        } else {
                            ui.label(RichText::new("Scroll up to load older messages").color(p.faint).size(12.0));
                        }
                    });
                    child.add_space(16.0);
                }
                Row::Day(text, _) => {
                    child.add_space(16.0);
                    let (r, _) = child.allocate_exact_size(vec2(width, 12.0), Sense::hover());
                    child.painter().hline(r.x_range().shrink(16.0), r.center().y, Stroke::new(1.0, p.divider));
                    let g = child.painter().layout_no_wrap(text.clone(), crate::theme::bold(11.5), p.muted);
                    let tr = Rect::from_center_size(r.center(), g.size() + vec2(12.0, 0.0));
                    child.painter().rect_filled(tr, CornerRadius::ZERO, p.chat);
                    child.painter().galley(tr.center() - g.size() / 2.0, g, p.muted);
                    child.add_space(8.0);
                }
                Row::NewMarker => {
                    let (r, _) = child.allocate_exact_size(vec2(width, 16.0), Sense::hover());
                    child.painter().hline(r.x_range().shrink(16.0), r.center().y, Stroke::new(1.0, p.red));
                    child.painter().text(
                        r.right_center() - vec2(20.0, 0.0),
                        egui::Align2::RIGHT_CENTER,
                        " NEW ",
                        crate::theme::bold(10.0),
                        Color32::WHITE,
                    );
                }
                Row::Msg(mi) => {
                    let m = &msgs[*mi];
                    let rc = RowCtx {
                        header: headers[*mi],
                        pending: None,
                        pending_nonce: None,
                        mention: store.is_mention(m),
                        perms,
                        me,
                        guild_id,
                    };
                    message::show(app, &mut child, store, m, &rc);
                    if first_visible.is_none() && top + 1.0 >= origin.y + viewport.min.y {
                        first_visible = Some((m.id, offs[i] - viewport.min.y));
                    }
                    if flash.is_some_and(|(id, _)| id == m.id) {
                        child.painter().rect_filled(child.min_rect(), CornerRadius::ZERO, p.accent.gamma_multiply(0.15));
                        child.ctx().request_repaint_after(Duration::from_millis(100));
                    }
                }
                Row::Pending(pi) => {
                    let (nonce, m, failed) = &pending[*pi];
                    let rc = RowCtx {
                        header: true,
                        pending: Some(failed.clone()),
                        pending_nonce: Some(nonce.clone()),
                        mention: false,
                        perms,
                        me,
                        guild_id,
                    };
                    message::show(app, &mut child, store, m, &rc);
                }
            }
            let h = child.min_rect().height().max(1.0);
            measured.push((sigs[i].0, sigs[i].1, h, heights[i]));
        }
    });
    let list = app.st.lists.entry(channel).or_default();
    let mut shifted = false;
    for (key, sig, h, est) in measured {
        list.heights.put(key, sig, h);
        shifted |= (h - est).abs() > 0.5;
    }
    if shifted {
        ui.ctx().request_repaint();
    }
    if list.heights.len() > 6000 {
        let keep: std::collections::HashSet<u64> = sigs.iter().map(|(k, _)| *k).collect();
        list.heights.retain(|k| keep.contains(&k));
    }
    list.anchor = first_visible;
    let max_offset = (output.content_size.y - output.inner_rect.height()).max(0.0);
    list.at_bottom = output.state.clamped_offset().y >= max_offset - 4.0;
    let at_bottom = list.at_bottom;

    // Load older history when near the top.
    if near_top
        && !reached_start
        && !loading_older
        && let Some(oldest) = oldest
    {
        store.set_loading_older(channel, true);
        app.link.send(Command::LoadOlder { channel_id: channel, before: oldest });
    }
    // Mark read when the newest message is visible and the window is focused.
    if at_bottom
        && !has_gap
        && app.st.window_focused
        && store.is_unread(channel)
        && let Some(last) = store.channels.get(&channel).and_then(|c| c.last_message_id)
    {
        let list = app.st.list(channel);
        if list.mark_sent != Some(last) {
            list.mark_sent = Some(last);
            app.link.send(Command::MarkRead { channel_id: channel, message_id: last });
        }
    }
    if has_gap || (!at_bottom && max_offset - output.state.clamped_offset().y > 1500.0) {
        let r = output.inner_rect;
        let bar = Rect::from_min_size(egui::pos2(r.left() + 16.0, r.bottom() - 36.0), vec2(r.width() - 32.0, 30.0));
        ui.painter().rect_filled(bar, CornerRadius::same(6), p.accent);
        ui.painter().text(
            bar.left_center() + vec2(12.0, 0.0),
            egui::Align2::LEFT_CENTER,
            "You're viewing older messages",
            egui::FontId::proportional(13.0),
            Color32::WHITE,
        );
        let jr = Rect::from_min_size(egui::pos2(bar.right() - 130.0, bar.top()), vec2(130.0, 30.0));
        ui.painter().text(jr.center(), egui::Align2::CENTER_CENTER, "Jump to Present ↓", crate::theme::bold(13.0), Color32::WHITE);
        if ui.interact(jr, egui::Id::unique(("jump", channel)), Sense::click()).clicked() {
            if has_gap {
                app.link.send(Command::JumpToPresent { channel_id: channel });
            }
            app.st.list(channel).scroll_to = Some(f32::MAX);
        }
    }
}

fn intro(ui: &mut Ui, store: &Store, channel: Id, p: &Palette) {
    let Some(c) = store.channels.get(&channel) else { return };
    ui.add_space(24.0);
    egui::Frame::new().inner_margin(egui::Margin::symmetric(16, 0)).show(ui, |ui| {
        let (title, sub) = if c.kind.is_private() {
            let n = store.dm_name(channel);
            (n.clone(), format!("This is the beginning of your direct message history with {n}."))
        } else {
            let n = c.display_name();
            (format!("Welcome to #{n}!"), format!("This is the start of the #{n} channel."))
        };
        ui.label(RichText::new(title).size(28.0).strong().color(p.text_strong));
        ui.label(RichText::new(sub).color(p.muted));
    });
    ui.add_space(16.0);
}

fn typing_line(ui: &mut Ui, store: &mut Store, channel: Id, p: &Palette) {
    let users = store.typing_users(channel);
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 22.0), Sense::hover());
    if users.is_empty() {
        return;
    }
    let guild = store.guild_of(channel);
    let names: Vec<String> = users.iter().map(|u| store.display_name(guild, *u)).collect();
    let text = match names.len() {
        1 => format!("{} is typing…", names[0]),
        2 => format!("{} and {} are typing…", names[0], names[1]),
        3 => format!("{}, {} and {} are typing…", names[0], names[1], names[2]),
        _ => "Several people are typing…".to_string(),
    };
    ui.painter().text(rect.left_center() + vec2(18.0, 0.0), egui::Align2::LEFT_CENTER, text, egui::FontId::proportional(12.5), p.text);
    // Typing entries expire on their own; make sure we repaint to clear them.
    ui.ctx().request_repaint_after(Duration::from_secs(1));
}

struct Candidate {
    label: String,
    detail: String,
    display: String,
    wire: Option<String>,
}

fn completions(store: &Store, channel: Id, kind: &CompletionKind, query: &str) -> Vec<Candidate> {
    let q = query.to_lowercase();
    let guild = store.guild_of(channel);
    let mut out = vec![];
    match kind {
        CompletionKind::User | CompletionKind::Role => {
            let mut users: Vec<Id> = match guild.and_then(|g| store.guilds.get(&g)) {
                Some(gs) => gs.members.keys().copied().collect(),
                None => store.channels.get(&channel).map(|c| c.recipients.iter().map(|u| u.id).collect()).unwrap_or_default(),
            };
            users.sort();
            for u in users {
                let name = store.display_name(guild, u);
                let uname = store.users.get(&u).map(|x| x.username.clone()).unwrap_or_default();
                if name.to_lowercase().contains(&q) || uname.to_lowercase().contains(&q) {
                    out.push(Candidate { label: name.clone(), detail: uname, display: format!("@{name}"), wire: Some(format!("<@{u}>")) });
                }
                if out.len() >= 8 {
                    break;
                }
            }
            if let Some(gs) = guild.and_then(|g| store.guilds.get(&g)) {
                for r in gs.guild.roles.iter().filter(|r| r.mentionable && r.name.to_lowercase().contains(&q)).take(3) {
                    out.push(Candidate {
                        label: format!("@{}", r.name),
                        detail: "role".into(),
                        display: format!("@{}", r.name),
                        wire: Some(format!("<@&{}>", r.id)),
                    });
                }
            }
        }
        CompletionKind::Channel => {
            if let Some(g) = guild {
                for grp in store.channel_tree(g) {
                    for c in grp.channels {
                        let n = c.display_name();
                        if n.to_lowercase().contains(&q) && out.len() < 8 {
                            out.push(Candidate {
                                label: format!("#{n}"),
                                detail: String::new(),
                                display: format!("#{n}"),
                                wire: Some(format!("<#{}>", c.id)),
                            });
                        }
                    }
                }
            }
        }
        CompletionKind::Emoji => {
            for gid in guild.into_iter().chain(store.guild_order.iter().copied().filter(|g| Some(*g) != guild)) {
                if let Some(gs) = store.guilds.get(&gid) {
                    for e in gs.guild.emojis.iter().filter(|e| e.available != Some(false)) {
                        let n = e.name.clone().unwrap_or_default();
                        if n.to_lowercase().contains(&q) && out.len() < 5 {
                            out.push(Candidate {
                                label: format!(":{n}:"),
                                detail: gs.guild.name.clone(),
                                display: format!(":{n}:"),
                                wire: Some(e.message_format()),
                            });
                        }
                    }
                }
            }
            for (emoji, code) in dn_messaging::emoji::search(query, 10 - out.len().min(10)) {
                out.push(Candidate { label: format!("{emoji}  :{code}:"), detail: String::new(), display: emoji.to_string(), wire: None });
            }
        }
    }
    out
}

fn composer(app: &mut DiscordApp, ui: &mut Ui, store: &mut Store, channel: Id, p: &Palette) {
    let perms = store.permissions_in(channel);
    let ctx = ui.ctx().clone();
    egui::Frame::new().inner_margin(egui::Margin { left: 16, right: 16, top: 0, bottom: 16 }).show(ui, |ui| {
        if !perms.can_send() {
            egui::Frame::new().fill(p.input).corner_radius(CornerRadius::same(8)).inner_margin(12).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(RichText::new("The bot does not have permission to send messages in this channel.").color(p.muted));
            });
            return;
        }
        // Reply bar
        if let Some(reply) = app.st.composer(channel).reply_to {
            let name = store
                .channel_messages(channel)
                .and_then(|cm| cm.get(reply))
                .map(|m| m.author_name().to_string())
                .unwrap_or_else(|| "a message".into());
            egui::Frame::new()
                .fill(p.sidebar)
                .corner_radius(CornerRadius { nw: 8, ne: 8, sw: 0, se: 0 })
                .inner_margin(egui::Margin::symmetric(12, 6))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(format!("Replying to {name}")).color(p.muted).size(13.0));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.small_button("✕").clicked() {
                                app.st.composer(channel).reply_to = None;
                            }
                            let c = app.st.composer(channel);
                            let label = if c.mention_reply { "@ ON" } else { "@ OFF" };
                            if ui
                                .small_button(RichText::new(label).color(if c.mention_reply { p.link } else { p.muted }))
                                .on_hover_text("Ping the author")
                                .clicked()
                            {
                                c.mention_reply = !c.mention_reply;
                            }
                        });
                    });
                });
        }
        // Attachment chips
        let files = app.st.composer(channel).attachments.clone();
        if !files.is_empty() {
            ui.horizontal_wrapped(|ui| {
                for (i, f) in files.iter().enumerate() {
                    let name = f.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                    let size = std::fs::metadata(f).map(|m| m.len()).unwrap_or(0);
                    egui::Frame::new().fill(p.sidebar).corner_radius(CornerRadius::same(6)).inner_margin(6).show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(format!("📎 {name} ({})", widgets::human_size(size)));
                            if ui.small_button("✕").clicked() {
                                app.st.composer(channel).attachments.remove(i);
                            }
                        });
                    });
                }
            });
        }
        let input_id = egui::Id::unique(("composer", channel));
        let mut text = app.st.drafts.get(&channel).cloned().unwrap_or_default();

        // Completion state from last frame's cursor.
        let cursor_char = egui::TextEdit::load_state(&ctx, input_id).and_then(|s| s.cursor.char_range()).map(|r| r.primary.index.0);
        let cursor_byte = cursor_char.map(|c| text.char_indices().nth(c).map(|(b, _)| b).unwrap_or(text.len()));
        let token = cursor_byte.and_then(|b| compose::completion_at(&text, b));
        let cands = token.as_ref().map(|t| completions(store, channel, &t.kind, &t.query)).unwrap_or_default();
        let popup_open = !cands.is_empty() && ctx.memory(|m| m.has_focus(input_id));
        let mut accept = None;
        if popup_open {
            let sel = &mut app.st.composer(channel).completion_selected;
            *sel = (*sel).min(cands.len() - 1);
            ctx.input_mut(|i| {
                if i.consume_key(Modifiers::NONE, Key::ArrowDown) {
                    *sel = (*sel + 1) % cands.len();
                }
                if i.consume_key(Modifiers::NONE, Key::ArrowUp) {
                    *sel = (*sel + cands.len() - 1) % cands.len();
                }
                if i.consume_key(Modifiers::NONE, Key::Tab) || i.consume_key(Modifiers::NONE, Key::Enter) {
                    accept = Some(*sel);
                }
            });
        }

        let mut send = false;
        let resp = egui::Frame::new().fill(p.input).corner_radius(CornerRadius::same(8)).inner_margin(egui::Margin::symmetric(8, 6)).show(
            ui,
            |ui| {
                ui.horizontal(|ui| {
                    if widgets::icon_button(ui, p, "➕", "Upload a file", None).clicked() && perms.contains(Permissions::ATTACH_FILES) {
                        let (tx, rx) = crossbeam_channel::bounded(1);
                        std::thread::spawn(move || {
                            let files = rfd::FileDialog::new().pick_files().unwrap_or_default();
                            let _ = tx.send((channel, files));
                        });
                        app.st.file_dialog_rx = Some(rx);
                    }
                    let hint = match store.channels.get(&channel) {
                        Some(c) if c.kind.is_private() => format!("Message @{}", store.dm_name(channel)),
                        Some(c) => format!("Message #{}", c.display_name()),
                        None => String::new(),
                    };
                    let w = ui.available_width() - 40.0;
                    let out = egui::TextEdit::multiline(&mut text)
                        .id(input_id)
                        .hint_text(hint)
                        .desired_rows(1)
                        .desired_width(w)
                        .frame(egui::Frame::NONE)
                        .return_key(Some(KeyboardShortcut::new(Modifiers::SHIFT, Key::Enter)))
                        .show(ui);
                    if let (Some(i), Some(t)) = (accept, token.as_ref()) {
                        let c = &cands[i];
                        let b = cursor_byte.unwrap_or(text.len());
                        let (new_text, new_cursor) = compose::apply_completion(&text, t, b, &c.display);
                        text = new_text;
                        if let Some(w) = &c.wire {
                            app.st.composer(channel).mentions.push((c.display.clone(), w.clone()));
                        }
                        let ci = text[..new_cursor.min(text.len())].chars().count();
                        let mut state = out.state.clone();
                        state.cursor.set_char_range(Some(CCursorRange::one(CCursor::new(ci))));
                        state.store(&ctx, input_id);
                    } else if out.response.has_focus() {
                        let enter = ctx.input(|i| i.key_pressed(Key::Enter) && !i.modifiers.shift);
                        if enter && !popup_open {
                            send = true;
                        }
                        let up = ctx.input(|i| i.key_pressed(Key::ArrowUp));
                        if up
                            && text.is_empty()
                            && let Some(m) = store
                                .channel_messages(channel)
                                .and_then(|cm| cm.messages.iter().rev().find(|m| Some(m.author.id) == store.me()))
                        {
                            app.st.editing = Some((m.id, m.content.clone()));
                        }
                    }
                    if out.response.changed() && !text.trim().is_empty() && app.link.settings.read().behavior.send_typing {
                        let due = app.st.typing_sent.is_none_or(|(c, at)| c != channel || at.elapsed() > Duration::from_secs(8));
                        if due {
                            app.st.typing_sent = Some((channel, Instant::now()));
                            app.link.send(Command::Typing { channel_id: channel });
                        }
                    }
                    if widgets::icon_button(ui, p, "☺", "Emoji", None).clicked() {
                        app.st.emoji_picker = match app.st.emoji_picker {
                            Some(EmojiTarget::Composer) => None,
                            _ => Some(EmojiTarget::Composer),
                        };
                    }
                });
            },
        );
        if popup_open && accept.is_none() {
            let sel = app.st.composer(channel).completion_selected;
            let r = resp.response.rect;
            egui::Area::new(egui::Id::unique("completion"))
                .fixed_pos(egui::pos2(r.left(), r.top() - 8.0 - cands.len() as f32 * 30.0 - 16.0))
                .order(egui::Order::Foreground)
                .show(&ctx, |ui| {
                    egui::Frame::new().fill(p.floating).corner_radius(CornerRadius::same(8)).inner_margin(8).show(ui, |ui| {
                        ui.set_width(r.width() - 16.0);
                        for (i, c) in cands.iter().enumerate() {
                            let row = widgets::nav_row(ui, p, i == sel, 28.0, |ui, _| {
                                ui.label(RichText::new(&c.label).color(p.text_strong));
                                ui.label(RichText::new(&c.detail).color(p.faint).size(12.0));
                            });
                            if row.clicked() {
                                app.st.composer(channel).completion_selected = i;
                                // Applied next frame through the same path as Tab.
                                ctx.input_mut(|inp| {
                                    inp.events.push(egui::Event::Key {
                                        key: Key::Tab,
                                        physical_key: None,
                                        pressed: true,
                                        repeat: false,
                                        modifiers: Modifiers::NONE,
                                    })
                                });
                            }
                        }
                    });
                });
        }
        if send {
            send_message(app, store, channel, &mut text);
        }
        if text.is_empty() {
            app.st.drafts.remove(&channel);
        } else {
            app.st.drafts.insert(channel, text);
        }
    });
}

fn send_message(app: &mut DiscordApp, store: &mut Store, channel: Id, text: &mut String) {
    let comp = app.st.composer(channel);
    let files = std::mem::take(&mut comp.attachments);
    if text.trim().is_empty() && files.is_empty() {
        return;
    }
    let mut content = text.trim_end().to_string();
    let mut maps = std::mem::take(&mut comp.mentions);
    maps.sort_by_key(|(d, _)| std::cmp::Reverse(d.len()));
    for (display, wire) in &maps {
        content = content.replace(display.as_str(), wire);
    }
    let content = dn_messaging::emoji::replace_shortcodes(&content);
    let reply = comp.reply_to.take();
    let mention_reply = comp.mention_reply;
    let mut chunks = compose::split_message(&content, 2000);
    if chunks.is_empty() {
        chunks.push(String::new());
    }
    for (i, chunk) in chunks.into_iter().enumerate() {
        app.link.send(Command::SendMessage {
            channel_id: channel,
            content: chunk,
            reply_to: if i == 0 { reply } else { None },
            mention_reply,
            files: if i == 0 { files.clone() } else { vec![] },
            sticker_ids: vec![],
        });
    }
    text.clear();
    app.st.list(channel).scroll_to = Some(f32::MAX);
    let _ = store;
}

fn forum_view(app: &mut DiscordApp, ui: &mut Ui, store: &mut Store, channel: Id, p: &Palette) {
    let threads: Vec<(Id, String, Option<u32>)> =
        store.threads_of(channel).iter().map(|t| (t.id, t.display_name(), t.message_count)).collect();
    egui::ScrollArea::vertical().show(ui, |ui| {
        egui::Frame::new().inner_margin(16).show(ui, |ui| {
            ui.label(RichText::new("Forum posts").size(20.0).strong().color(p.text_strong));
            ui.label(
                RichText::new("Active posts visible to the bot. Creating new forum posts isn't supported in this client yet.")
                    .color(p.muted),
            );
            ui.add_space(8.0);
            if threads.is_empty() {
                ui.label(RichText::new("No active posts.").color(p.faint));
            }
            for (id, name, count) in threads {
                let r = egui::Frame::new().fill(p.sidebar).corner_radius(CornerRadius::same(8)).inner_margin(12).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.label(RichText::new(&name).strong().color(p.text_strong));
                    ui.label(RichText::new(format!("{} messages", count.unwrap_or(0))).color(p.muted).size(12.0));
                });
                if ui
                    .interact(r.response.rect, egui::Id::unique(("post", id)), Sense::click())
                    .on_hover_cursor(egui::CursorIcon::PointingHand)
                    .clicked()
                {
                    app.open_channel(store, id);
                }
                ui.add_space(6.0);
            }
        });
    });
}

pub fn search_panel(app: &mut DiscordApp, ui: &mut Ui, store: &mut Store) {
    let p = *app.palette();
    ui.horizontal(|ui| {
        let title = if app.st.search.loading { "Searching…".to_string() } else { format!("{} results", app.st.search.total) };
        ui.label(RichText::new(title).strong().color(p.text_strong));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.small_button("✕").clicked() {
                app.st.search.open = false;
            }
        });
    });
    if let Some(src) = app.st.search.source {
        let label = match src {
            SearchSource::Discord => "Discord server search",
            SearchSource::Local => "Local cache search (messages this client has seen)",
        };
        ui.label(RichText::new(label).size(11.5).color(p.faint));
    }
    if let Some(n) = &app.st.search.note {
        ui.label(RichText::new(n).size(12.0).color(p.yellow));
    }
    if let Some(c) = app.st.selected_channel
        && ui.small_button("Search only this channel").clicked()
    {
        start_search(app, store, 0, Some(c));
    }
    ui.separator();
    let results = app.st.search.results.clone();
    let mut jump = None;
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        for m in &results {
            let r = egui::Frame::new().fill(p.chat).corner_radius(CornerRadius::same(6)).inner_margin(8).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.label(RichText::new(m.author_name()).strong().color(p.text_strong));
                    ui.label(RichText::new(message::format_time(m.timestamp_ms(), false, true)).size(11.0).color(p.faint));
                });
                ui.label(RichText::new(store.location_label(m.channel_id)).size(11.5).color(p.muted));
                let snippet = message::plain_snippet(store, m.guild_id, &m.content, 280);
                ui.label(snippet);
            });
            if ui
                .interact(r.response.rect, egui::Id::unique(("sr", m.id)), Sense::click())
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .clicked()
            {
                jump = Some((m.channel_id, m.id));
            }
            ui.add_space(4.0);
        }
        let shown = results.len() as u64;
        if !app.st.search.loading && shown < app.st.search.total && ui.button("Load more").clicked() {
            let ch = app.st.search.last.as_ref().and_then(|l| l.channel_id);
            start_search(app, store, shown as u32, ch);
        }
    });
    if let Some((c, m)) = jump
        && store.channels.contains_key(&c)
    {
        jump_to(app, store, c, m);
    }
}

pub fn emoji_picker(app: &mut DiscordApp, ctx: &egui::Context, store: &Store, p: &Palette) {
    let Some(target) = app.st.emoji_picker.clone() else { return };
    let mut chosen: Option<Emoji> = None;
    let mut close = false;
    let screen = ctx.content_rect();
    egui::Area::new(egui::Id::unique("emoji-picker"))
        .fixed_pos(egui::pos2(screen.right() - 420.0, screen.bottom() - 520.0))
        .order(egui::Order::Foreground)
        .show(ctx, |ui| {
            egui::Frame::new()
                .fill(p.floating)
                .stroke(Stroke::new(1.0, p.divider))
                .corner_radius(CornerRadius::same(8))
                .inner_margin(10)
                .show(ui, |ui| {
                    ui.set_width(380.0);
                    ui.horizontal(|ui| {
                        let r = ui.add(
                            egui::TextEdit::singleline(&mut app.st.emoji_query).hint_text("Find the perfect emoji").desired_width(320.0),
                        );
                        r.request_focus();
                        if ui.small_button("✕").clicked() {
                            close = true;
                        }
                    });
                    let q = app.st.emoji_query.trim().to_lowercase();
                    let guild = app.st.selected_guild;
                    let mut custom: Vec<Emoji> = vec![];
                    for gid in guild.into_iter().chain(store.guild_order.iter().copied().filter(|g| Some(*g) != guild)) {
                        if let Some(gs) = store.guilds.get(&gid) {
                            custom.extend(
                                gs.guild
                                    .emojis
                                    .iter()
                                    .filter(|e| e.available != Some(false))
                                    .filter(|e| q.is_empty() || e.name.as_deref().unwrap_or("").to_lowercase().contains(&q))
                                    .cloned(),
                            );
                        }
                    }
                    let mut unicode: Vec<(String, Vec<&'static str>)> = vec![];
                    if q.is_empty() {
                        for (g, list) in dn_messaging::emoji::categories() {
                            unicode.push((g.to_string(), list));
                        }
                    } else {
                        unicode.push(("Results".into(), dn_messaging::emoji::search(&q, 200).into_iter().map(|(e, _)| e).collect()));
                    }
                    // Flatten into fixed-height rows so the grid can be virtualized.
                    const PER_ROW: usize = 9;
                    enum GridRow {
                        Title(String),
                        Custom(Vec<Emoji>),
                        Unicode(Vec<&'static str>),
                    }
                    let mut rows = vec![];
                    if q.is_empty() && !app.st.recent_emoji.is_empty() {
                        rows.push(GridRow::Title("Frequently used".into()));
                        for c in app.st.recent_emoji.chunks(PER_ROW) {
                            rows.push(GridRow::Custom(c.to_vec()));
                        }
                    }
                    if !custom.is_empty() {
                        rows.push(GridRow::Title("Server emoji".into()));
                        for c in custom.chunks(PER_ROW) {
                            rows.push(GridRow::Custom(c.to_vec()));
                        }
                    }
                    for (title, list) in unicode {
                        rows.push(GridRow::Title(title));
                        for c in list.chunks(PER_ROW) {
                            rows.push(GridRow::Unicode(c.to_vec()));
                        }
                    }
                    egui::ScrollArea::vertical().max_height(420.0).auto_shrink([false, false]).show_rows(
                        ui,
                        38.0,
                        rows.len(),
                        |ui, range| {
                            for row in &rows[range] {
                                match row {
                                    GridRow::Title(t) => {
                                        ui.add_sized(
                                            vec2(360.0, 34.0),
                                            egui::Label::new(RichText::new(t.to_uppercase()).size(11.0).strong().color(p.muted)),
                                        );
                                    }
                                    GridRow::Custom(list) => {
                                        ui.horizontal(|ui| {
                                            for e in list {
                                                let (r, resp) = ui.allocate_exact_size(vec2(34.0, 34.0), Sense::click());
                                                if resp.hovered() {
                                                    ui.painter().rect_filled(r, CornerRadius::same(4), p.hover);
                                                }
                                                match e.url(64) {
                                                    Some(url) => {
                                                        if let Some(tex) = app.textures.get(&url, 64) {
                                                            egui::Image::new((tex.id(), vec2(28.0, 28.0)))
                                                                .paint_at(ui, Rect::from_center_size(r.center(), vec2(28.0, 28.0)));
                                                        }
                                                    }
                                                    None => {
                                                        ui.painter().text(
                                                            r.center(),
                                                            egui::Align2::CENTER_CENTER,
                                                            e.name.clone().unwrap_or_default(),
                                                            egui::FontId::proportional(24.0),
                                                            p.text,
                                                        );
                                                    }
                                                }
                                                if resp.on_hover_text(format!(":{}:", e.name.clone().unwrap_or_default())).clicked() {
                                                    chosen = Some(e.clone());
                                                }
                                            }
                                        });
                                    }
                                    GridRow::Unicode(list) => {
                                        ui.horizontal(|ui| {
                                            for e in list {
                                                let (r, resp) = ui.allocate_exact_size(vec2(34.0, 34.0), Sense::click());
                                                if resp.hovered() {
                                                    ui.painter().rect_filled(r, CornerRadius::same(4), p.hover);
                                                }
                                                ui.painter().text(
                                                    r.center(),
                                                    egui::Align2::CENTER_CENTER,
                                                    *e,
                                                    egui::FontId::proportional(24.0),
                                                    p.text,
                                                );
                                                if resp.clicked() {
                                                    chosen = Some(Emoji::unicode(e));
                                                }
                                            }
                                        });
                                    }
                                }
                            }
                        },
                    );
                });
        });
    if let Some(e) = chosen {
        app.st.push_recent_emoji(e.clone());
        match target {
            EmojiTarget::Composer => {
                if let Some(c) = app.st.selected_channel {
                    let token = match e.id {
                        Some(_) => {
                            let display = format!(":{}:", e.name.clone().unwrap_or_default());
                            app.st.composer(c).mentions.push((display.clone(), e.message_format()));
                            display
                        }
                        None => e.name.clone().unwrap_or_default(),
                    };
                    app.st.drafts.entry(c).or_default().push_str(&token);
                }
            }
            EmojiTarget::Reaction { channel_id, message_id } => {
                app.link.send(Command::React { channel_id, message_id, emoji: e, add: true });
                close = true;
            }
        }
    }
    if close {
        app.st.emoji_picker = None;
        app.st.emoji_query.clear();
    }
    let _ = ChannelKind::GuildText;
}
