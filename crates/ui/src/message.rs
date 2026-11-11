//! Rendering of a single message row (cozy layout) and its interactions.

use chrono::{Datelike, Local, TimeZone};
use egui::{Color32, CornerRadius, Rect, RichText, Sense, Stroke, Ui, vec2};

use dn_cache::Store;
use dn_cache::bus::Command;
use dn_core::{Attachment, Embed, Emoji, Id, Message, Permissions, message_kind};

use crate::richtext::{self, RenderCtx, RichAction};
use crate::state::{EmojiTarget, Modal};
use crate::theme::{Palette, rgb};
use crate::{DiscordApp, widgets};

pub const GUTTER: f32 = 72.0;
pub const AVATAR: f32 = 40.0;

/// Per-row context computed by the list.
pub struct RowCtx {
    pub header: bool,
    pub pending: Option<Option<String>>,
    pub pending_nonce: Option<String>,
    pub mention: bool,
    pub perms: Permissions,
    pub me: Option<Id>,
    pub guild_id: Option<Id>,
}

pub fn format_time(ms: u64, use_24h: bool, full: bool) -> String {
    let Some(dt) = Local.timestamp_millis_opt(ms as i64).single() else { return String::new() };
    let time = if use_24h { dt.format("%H:%M").to_string() } else { dt.format("%-I:%M %p").to_string() };
    if !full {
        return time;
    }
    let today = Local::now().date_naive();
    let d = dt.date_naive();
    if d == today {
        format!("Today at {time}")
    } else if today.pred_opt() == Some(d) {
        format!("Yesterday at {time}")
    } else {
        format!("{:02}/{:02}/{} {time}", d.month(), d.day(), d.year())
    }
}

fn system_text(store: &Store, m: &Message) -> Option<(&'static str, String)> {
    let who = m.author_name().to_string();
    let s = match m.kind {
        message_kind::RECIPIENT_ADD => ("→", format!("{who} added someone to the group.")),
        message_kind::RECIPIENT_REMOVE => ("←", format!("{who} left the group.")),
        message_kind::CALL => ("📞", format!("{who} started a call.")),
        message_kind::CHANNEL_NAME_CHANGE => ("✏", format!("{who} changed the channel name: {}", m.content)),
        message_kind::CHANNEL_PINNED_MESSAGE => ("📌", format!("{who} pinned a message to this channel.")),
        message_kind::USER_JOIN => ("→", format!("{who} joined the server.")),
        message_kind::GUILD_BOOST | 9..=11 => ("✦", format!("{who} boosted the server!")),
        message_kind::THREAD_CREATED => ("🧵", format!("{who} started a thread: {}", m.content)),
        _ => return None,
    };
    let _ = store;
    Some(s)
}

/// Draws the message. Returns nothing; interactions are sent as commands / modal changes.
pub fn show(app: &mut DiscordApp, ui: &mut Ui, store: &Store, m: &Message, rc: &RowCtx) {
    let p = *app.palette();
    let (use_24h, show_embeds, show_images, show_reactions, dev) = {
        let s = app.link.settings.read();
        (
            s.appearance.use_24h_time,
            s.appearance.show_embeds,
            s.appearance.show_images,
            s.appearance.show_reactions,
            s.behavior.developer_mode,
        )
    };
    let width = ui.available_width();
    let top = ui.cursor().min;
    let font_size = ui.style().text_styles[&egui::TextStyle::Body].size;

    if let Some((icon, text)) = system_text(store, m) {
        ui.horizontal(|ui| {
            ui.add_space(GUTTER - 30.0);
            ui.label(RichText::new(icon).color(p.green));
            ui.label(RichText::new(text).color(p.muted).italics());
            ui.label(RichText::new(format_time(m.timestamp_ms(), use_24h, true)).color(p.faint).size(11.0));
        });
        ui.add_space(2.0);
        return;
    }

    if rc.header {
        ui.add_space(14.0);
    }
    let mut actions: Vec<RichAction> = vec![];
    let row = ui.horizontal_top(|ui| {
        ui.add_space(16.0);
        // Gutter: avatar on headers, hover timestamp on continuation lines.
        let (gutter, _) = ui.allocate_exact_size(vec2(GUTTER - 16.0 - 8.0, if rc.header { AVATAR } else { 18.0 }), Sense::hover());
        ui.vertical(|ui| {
            ui.set_max_width(width - GUTTER - 24.0);
            if m.is_reply() {
                reply_preview(app, ui, store, m, &p);
            }
            if rc.header {
                let r = Rect::from_min_size(gutter.min, vec2(AVATAR, AVATAR));
                let url = m.author.avatar_url(80);
                widgets::paint_avatar(ui, &mut app.textures, &p, &url, r, None, false);
                if ui.interact(r, ui.scope_id().with(("av", m.id)), Sense::click()).clicked() {
                    actions.push(RichAction::Profile(m.author.id));
                }
                ui.horizontal(|ui| {
                    let color = p.role_color(store.member_color(rc.guild_id, m.author.id));
                    let name = match rc.guild_id {
                        Some(g) => {
                            store.member(g, m.author.id).and_then(|mm| mm.nick.clone()).unwrap_or_else(|| m.author_name().to_string())
                        }
                        None => m.author_name().to_string(),
                    };
                    let nr = ui.add(egui::Label::new(RichText::new(name).color(color).strong()).sense(Sense::click()).selectable(false));
                    if nr.clicked() {
                        actions.push(RichAction::Profile(m.author.id));
                    }
                    if m.author.bot || m.webhook_id.is_some() {
                        let tag = if m.webhook_id.is_some() { "APP" } else { "BOT" };
                        ui.label(RichText::new(format!(" {tag} ")).size(10.0).color(Color32::WHITE).background_color(p.accent));
                    }
                    ui.label(RichText::new(format_time(m.timestamp_ms(), use_24h, true)).color(p.faint).size(12.0));
                });
            }
            content(app, ui, store, m, rc, &p, font_size, use_24h, &mut actions);
            if show_images || show_embeds {
                attachments(app, ui, m, &p, show_images);
            }
            if show_embeds {
                for (i, e) in m.embeds.iter().enumerate() {
                    embed(app, ui, store, m, i, e, &p, font_size, use_24h, &mut actions);
                }
            }
            stickers(app, ui, m, &p);
            if let Some(poll) = &m.poll {
                poll_view(ui, poll, &p);
            }
            if let Some(t) = &m.thread {
                let label = format!("🧵 {} — {} messages", t.display_name(), t.message_count.unwrap_or(0));
                if ui.link(RichText::new(label).color(p.link)).clicked() {
                    actions.push(RichAction::Channel(t.id));
                }
            }
            if show_reactions && (!m.reactions.is_empty()) {
                reactions(app, ui, m, rc, &p);
            }
            if let Some(Some(err)) = &rc.pending {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(format!("Failed to send: {err}")).color(p.red).size(12.0));
                    if let Some(nonce) = &rc.pending_nonce {
                        if ui.small_button("Retry").clicked() {
                            app.link.send(Command::RetrySend { channel_id: m.channel_id, nonce: nonce.clone() });
                        }
                        if ui.small_button("Discard").clicked() {
                            app.link.send(Command::DiscardPending { channel_id: m.channel_id, nonce: nonce.clone() });
                        }
                    }
                });
            }
        });
        gutter
    });
    let gutter = row.inner;
    let row_rect = Rect::from_min_max(top, egui::pos2(top.x + width, ui.cursor().min.y));
    let hovered = ui.rect_contains_pointer(row_rect) && rc.pending.is_none();

    // Background highlights (painted under content via a separate layer would be ideal; a
    // translucent overlay keeps it simple and cheap).
    if rc.mention {
        ui.painter().rect_filled(row_rect, CornerRadius::ZERO, p.mention_bg);
        ui.painter().rect_filled(Rect::from_min_size(row_rect.min, vec2(2.0, row_rect.height())), CornerRadius::ZERO, p.mention_bar);
    } else if hovered {
        ui.painter().rect_filled(row_rect, CornerRadius::ZERO, p.hover.gamma_multiply(0.4));
    }
    if !rc.header && hovered {
        ui.painter().text(
            egui::pos2(gutter.min.x, row_rect.min.y + 3.0),
            egui::Align2::LEFT_TOP,
            format_time(m.timestamp_ms(), use_24h, false),
            egui::FontId::proportional(10.5),
            p.faint,
        );
    }
    if hovered {
        app.st.hovered_message = Some(m.id);
        toolbar(app, ui, m, rc, row_rect, &p);
    }
    let resp = ui.interact(row_rect, ui.scope_id().with(("row", m.id)), Sense::click());
    resp.context_menu(|ui| context_menu(app, ui, store, m, rc, dev));

    for a in actions {
        handle_action(app, ui, store, a, rc.guild_id);
    }
}

pub fn handle_action(app: &mut DiscordApp, _ui: &Ui, store: &Store, a: RichAction, guild: Option<Id>) {
    match a {
        RichAction::OpenUrl { url, masked } => {
            if masked {
                app.st.modal = Some(Modal::LinkWarning { url });
            } else {
                let _ = open::that_detached(&url);
            }
        }
        RichAction::Profile(user) => {
            app.st.modal = Some(Modal::Profile { user_id: user, guild_id: guild });
            app.link.send(Command::FetchProfile { user_id: user, guild_id: guild });
        }
        RichAction::Channel(c) => {
            if store.channels.contains_key(&c) {
                app.pending_open_channel(c);
            } else {
                app.st.toast("That channel isn't visible to this bot.", true);
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn content(
    app: &mut DiscordApp,
    ui: &mut Ui,
    store: &Store,
    m: &Message,
    rc: &RowCtx,
    p: &Palette,
    font_size: f32,
    use_24h: bool,
    actions: &mut Vec<RichAction>,
) {
    if let Some((id, text)) = app.st.editing.clone().filter(|(id, _)| *id == m.id) {
        let mut text = text;
        let out = egui::TextEdit::multiline(&mut text).desired_rows(1).desired_width(f32::INFINITY).id_salt(("edit", id)).show(ui);
        out.response.request_focus();
        let (enter, shift) = ui.input(|i| (i.key_pressed(egui::Key::Enter), i.modifiers.shift));
        ui.label(RichText::new("escape to cancel • enter to save").size(11.0).color(p.muted));
        if enter && !shift {
            let trimmed = text.trim_end().to_string();
            if trimmed.is_empty() {
                app.st.modal = Some(Modal::ConfirmDelete { channel_id: m.channel_id, message_id: m.id, preview: m.content.clone() });
            } else if trimmed != m.content {
                app.link.send(Command::EditMessage { channel_id: m.channel_id, message_id: m.id, content: trimmed });
            }
            app.st.editing = None;
        } else {
            app.st.editing = Some((id, text));
        }
        return;
    }
    if m.content.is_empty() {
        return;
    }
    let blocks = app.markdown(m.id, &m.content);
    let jumbo = dn_messaging::emoji::is_emoji_only(&m.content, 30);
    let color = if rc.pending.is_some() { p.muted } else { p.text };
    let mut rctx = RenderCtx {
        palette: p,
        store,
        guild_id: rc.guild_id,
        textures: &mut app.textures,
        font_size: if jumbo { font_size * 2.6 } else { font_size },
        use_24h,
        now_unix: chrono::Utc::now().timestamp(),
        revealed: &mut app.st.revealed_spoilers,
        key: m.id.0,
        actions,
        jumbo,
        color,
    };
    richtext::render_blocks(ui, &blocks, &mut rctx);
    if m.edited_timestamp.is_some() {
        ui.label(RichText::new("(edited)").size(10.5).color(p.faint));
    }
}

/// Single-line plain text with mentions resolved to names (reply previews, pins, search).
pub fn plain_snippet(store: &Store, guild: Option<Id>, content: &str, max_chars: usize) -> String {
    use dn_messaging::markdown::{Inline, parse, to_plain_text};
    let resolve = |inl: &Inline| -> Option<String> {
        match inl {
            Inline::UserMention(id) => Some(format!("@{}", store.display_name(guild, *id))),
            Inline::ChannelMention(id) => {
                Some(format!("#{}", store.channels.get(id).map(|c| c.display_name()).unwrap_or_else(|| "unknown".into())))
            }
            Inline::RoleMention(id) => Some(format!(
                "@{}",
                guild.and_then(|g| store.guilds.get(&g)).and_then(|g| g.role(*id)).map(|r| r.name.clone()).unwrap_or_else(|| "role".into())
            )),
            _ => None,
        }
    };
    let text = to_plain_text(&parse(content), &resolve);
    let line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if line.chars().count() > max_chars { format!("{}…", line.chars().take(max_chars).collect::<String>()) } else { line }
}

fn reply_preview(app: &mut DiscordApp, ui: &mut Ui, store: &Store, m: &Message, p: &Palette) {
    let (name, snippet, target) = match &m.referenced_message {
        Some(r) => {
            let snippet = plain_snippet(store, r.guild_id.or(m.guild_id), &r.content, 120);
            let snippet = if snippet.is_empty() && !r.attachments.is_empty() { "Click to see attachment".to_string() } else { snippet };
            (format!("@{}", r.author_name()), snippet, Some(r.id))
        }
        None => (String::new(), "Original message was deleted".to_string(), None),
    };
    let resp = ui
        .horizontal(|ui| {
            ui.label(RichText::new("╭").color(p.faint));
            if !name.is_empty() {
                ui.label(RichText::new(name).color(p.muted).strong().size(12.5));
            }
            ui.add(egui::Label::new(RichText::new(snippet).color(p.muted).size(12.5)).truncate().selectable(false));
        })
        .response;
    if let Some(t) = target {
        let r = ui.interact(resp.rect, ui.scope_id().with(("reply", m.id)), Sense::click());
        if r.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
            if store.channel_messages(m.channel_id).and_then(|cm| cm.get(t)).is_none() {
                app.st.toast("Referenced message is not loaded; scroll up to find it.", false);
            }
            app.st.list(m.channel_id).scroll_to_message = Some(t);
        }
    }
}

fn attachments(app: &mut DiscordApp, ui: &mut Ui, m: &Message, p: &Palette, show_images: bool) {
    for (i, a) in m.attachments.iter().enumerate() {
        if a.is_image() && show_images {
            image_attachment(app, ui, m, i, a, p);
        } else {
            file_card(app, ui, a, p);
        }
    }
}

fn fit(w: Option<u32>, h: Option<u32>, max_w: f32, max_h: f32) -> egui::Vec2 {
    let (w, h) = (w.unwrap_or(400).max(1) as f32, h.unwrap_or(300).max(1) as f32);
    let s = (max_w / w).min(max_h / h).min(1.0);
    vec2((w * s).max(16.0), (h * s).max(16.0))
}

fn image_attachment(app: &mut DiscordApp, ui: &mut Ui, m: &Message, idx: usize, a: &Attachment, p: &Palette) {
    let size = fit(a.width, a.height, ui.available_width().min(400.0), 300.0);
    let url = a.preview_url(size.x as u32 * 2, size.y as u32 * 2);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    let key = (m.id.0, 10_000 + idx);
    let spoiler = a.is_spoiler() && !app.st.revealed_spoilers.contains(&key);
    if ui.is_rect_visible(rect) {
        match app.textures.get(&url, (size.x.max(size.y) * 2.0) as u32) {
            Some(tex) => {
                let tint = if spoiler { Color32::from_gray(20) } else { Color32::WHITE };
                egui::Image::new((tex.id(), size)).corner_radius(CornerRadius::same(6)).tint(tint).paint_at(ui, rect);
            }
            None => {
                ui.painter().rect_filled(rect, CornerRadius::same(6), p.embed_bg);
                let label = if app.textures.has_failed(&url) { "⚠ image unavailable" } else { "Loading…" };
                ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, label, egui::FontId::proportional(12.0), p.faint);
            }
        }
        if spoiler {
            ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, "SPOILER", crate::theme::bold(14.0), Color32::WHITE);
        }
    }
    let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text(&a.filename);
    if resp.clicked() {
        if spoiler {
            app.st.revealed_spoilers.insert(key);
        } else {
            app.st.modal = Some(Modal::ImageViewer {
                url: a.preview_url(1600, 1200),
                original: a.url.clone(),
                width: a.width.unwrap_or(0),
                height: a.height.unwrap_or(0),
            });
        }
    }
    resp.context_menu(|ui| {
        if ui.button("Save image…").clicked() {
            save_as(app, &a.url, &a.filename);
            ui.close();
        }
        if ui.button("Open in browser").clicked() {
            let _ = open::that_detached(&a.url);
            ui.close();
        }
        if ui.button("Copy link").clicked() {
            ui.ctx().copy_text(a.url.clone());
            ui.close();
        }
    });
}

fn file_card(app: &mut DiscordApp, ui: &mut Ui, a: &Attachment, p: &Palette) {
    egui::Frame::new().fill(p.embed_bg).stroke(Stroke::new(1.0, p.divider)).corner_radius(CornerRadius::same(6)).inner_margin(10).show(
        ui,
        |ui| {
            ui.set_max_width(420.0);
            ui.horizontal(|ui| {
                let icon = if a.is_video() {
                    "🎞"
                } else if a.content_type.as_deref().is_some_and(|c| c.starts_with("audio/")) {
                    "🎵"
                } else {
                    "📄"
                };
                ui.label(RichText::new(icon).size(24.0));
                ui.vertical(|ui| {
                    if ui.link(RichText::new(&a.filename).color(p.link)).clicked() {
                        let _ = open::that_detached(&a.url);
                    }
                    ui.label(RichText::new(widgets::human_size(a.size)).color(p.faint).size(11.5));
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if widgets::icon_button(ui, p, "⬇", "Download", None).clicked() {
                        save_as(app, &a.url, &a.filename);
                    }
                });
            });
        },
    );
}

pub fn save_as(app: &mut DiscordApp, url: &str, filename: &str) {
    let (tx, rx) = crossbeam_channel::bounded(1);
    let url = url.to_string();
    let name = filename.to_string();
    // Native dialogs block; run them off the UI thread.
    std::thread::spawn(move || {
        let path = rfd::FileDialog::new().set_file_name(&name).save_file();
        let _ = tx.send((url, path));
    });
    app.st.save_dialog_rx = Some(rx);
}

#[allow(clippy::too_many_arguments)]
fn embed(
    app: &mut DiscordApp,
    ui: &mut Ui,
    store: &Store,
    m: &Message,
    idx: usize,
    e: &Embed,
    p: &Palette,
    font_size: f32,
    use_24h: bool,
    actions: &mut Vec<RichAction>,
) {
    let kind = e.kind.as_deref().unwrap_or("rich");
    // Media-only embeds (image/gifv) render as a plain image like attachments.
    if matches!(kind, "image" | "gifv")
        && let Some(t) = e.thumbnail.as_ref().or(e.image.as_ref())
    {
        let size = fit(t.width, t.height, ui.available_width().min(400.0), 300.0);
        let src = t.proxy_url.clone().unwrap_or_else(|| t.url.clone());
        let url = dn_core::cdn::with_size_params(&src, t.width, t.height, size.x as u32 * 2, size.y as u32 * 2);
        let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
        match app.textures.get(&url, (size.x.max(size.y) * 2.0) as u32) {
            Some(tex) => egui::Image::new((tex.id(), size)).corner_radius(CornerRadius::same(6)).paint_at(ui, rect),
            None => {
                ui.painter().rect_filled(rect, CornerRadius::same(6), p.embed_bg);
            }
        }
        if kind == "gifv" {
            ui.painter().text(rect.left_top() + vec2(6.0, 6.0), egui::Align2::LEFT_TOP, "GIF", crate::theme::bold(11.0), Color32::WHITE);
        }
        if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked()
            && let Some(u) = &e.url
        {
            let _ = open::that_detached(u);
        }
        return;
    }
    let bar = e.color.map(rgb).unwrap_or(p.divider);
    let resp = egui::Frame::new()
        .fill(p.embed_bg)
        .corner_radius(CornerRadius::same(4))
        .inner_margin(egui::Margin { left: 14, right: 12, top: 10, bottom: 12 })
        .show(ui, |ui| {
            ui.set_max_width(516.0);
            let mut rctx = RenderCtx {
                palette: p,
                store,
                guild_id: m.guild_id,
                textures: &mut app.textures,
                font_size: font_size * 0.93,
                use_24h,
                now_unix: chrono::Utc::now().timestamp(),
                revealed: &mut app.st.revealed_spoilers,
                key: m.id.0 ^ ((idx as u64 + 1) << 56),
                actions,
                jumbo: false,
                color: p.text,
            };
            ui.horizontal_top(|ui| {
                ui.vertical(|ui| {
                    ui.set_max_width(if e.thumbnail.is_some() && kind == "rich" { 400.0 } else { 500.0 });
                    if let Some(pr) = e.provider.as_ref().and_then(|pr| pr.name.clone()) {
                        ui.label(RichText::new(pr).size(11.5).color(p.muted));
                    }
                    if let Some(a) = &e.author {
                        ui.horizontal(|ui| {
                            if let Some(icon) = &a.icon_url
                                && let Some(tex) = rctx.textures.get(icon, 48)
                            {
                                ui.add(egui::Image::new((tex.id(), vec2(20.0, 20.0))).corner_radius(CornerRadius::same(10)));
                            }
                            let r = ui.add(egui::Label::new(RichText::new(&a.name).strong().size(13.0)).sense(Sense::click()));
                            if r.clicked()
                                && let Some(u) = &a.url
                            {
                                rctx.actions.push(RichAction::OpenUrl { url: u.clone(), masked: false });
                            }
                        });
                    }
                    if let Some(t) = &e.title {
                        let inl = dn_messaging::markdown::parse_inline(t);
                        if let Some(u) = &e.url {
                            let r = ui.add(egui::Label::new(RichText::new(t).strong().color(p.link)).sense(Sense::click()));
                            if r.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                                rctx.actions.push(RichAction::OpenUrl { url: u.clone(), masked: false });
                            }
                        } else {
                            richtext::render_inline(ui, &inl, &mut rctx, true, None);
                        }
                    }
                    if let Some(d) = &e.description {
                        let blocks = dn_messaging::markdown::parse(d);
                        richtext::render_blocks(ui, &blocks, &mut rctx);
                    }
                    if !e.fields.is_empty() {
                        let mut i = 0;
                        while i < e.fields.len() {
                            let mut run = 1;
                            if e.fields[i].inline {
                                while i + run < e.fields.len() && e.fields[i + run].inline && run < 3 {
                                    run += 1;
                                }
                            }
                            ui.horizontal_top(|ui| {
                                let w = (ui.available_width() / run as f32) - 8.0;
                                for f in &e.fields[i..i + run] {
                                    ui.vertical(|ui| {
                                        ui.set_width(w);
                                        ui.label(RichText::new(&f.name).strong().size(13.0));
                                        let blocks = dn_messaging::markdown::parse(&f.value);
                                        richtext::render_blocks(ui, &blocks, &mut rctx);
                                    });
                                }
                            });
                            i += run;
                        }
                    }
                    if let Some(img) = &e.image {
                        let size = fit(img.width, img.height, ui.available_width().min(500.0), 300.0);
                        let src = img.proxy_url.clone().unwrap_or_else(|| img.url.clone());
                        let url = dn_core::cdn::with_size_params(&src, img.width, img.height, size.x as u32 * 2, size.y as u32 * 2);
                        let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
                        match rctx.textures.get(&url, (size.x.max(size.y) * 2.0) as u32) {
                            Some(tex) => egui::Image::new((tex.id(), size)).corner_radius(CornerRadius::same(4)).paint_at(ui, rect),
                            None => {
                                ui.painter().rect_filled(rect, CornerRadius::same(4), p.hover);
                            }
                        }
                    }
                    if e.footer.is_some() || e.timestamp.is_some() {
                        let mut parts = vec![];
                        if let Some(f) = &e.footer {
                            parts.push(f.text.clone());
                        }
                        if let Some(ts) = e.timestamp.as_deref().and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok()) {
                            parts.push(format_time(ts.timestamp_millis() as u64, use_24h, true));
                        }
                        ui.label(RichText::new(parts.join(" • ")).size(11.5).color(p.muted));
                    }
                });
                if kind != "image"
                    && let Some(t) = &e.thumbnail
                {
                    let size = fit(t.width, t.height, 80.0, 80.0);
                    let src = t.proxy_url.clone().unwrap_or_else(|| t.url.clone());
                    let url = dn_core::cdn::with_size_params(&src, t.width, t.height, 160, 160);
                    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
                    if let Some(tex) = rctx.textures.get(&url, 160) {
                        egui::Image::new((tex.id(), size)).corner_radius(CornerRadius::same(4)).paint_at(ui, rect);
                    }
                }
            });
        });
    let r = resp.response.rect;
    ui.painter().rect_filled(Rect::from_min_size(r.min, vec2(4.0, r.height())), CornerRadius { nw: 4, sw: 4, ne: 0, se: 0 }, bar);
    ui.add_space(4.0);
}

fn stickers(app: &mut DiscordApp, ui: &mut Ui, m: &Message, p: &Palette) {
    for s in &m.sticker_items {
        match s.url(320) {
            Some(url) => {
                let (rect, resp) = ui.allocate_exact_size(vec2(160.0, 160.0), Sense::hover());
                match app.textures.get(&url, 320) {
                    Some(tex) => egui::Image::new((tex.id(), rect.size())).paint_at(ui, rect),
                    None => {
                        ui.painter().rect_filled(rect, CornerRadius::same(8), p.hover);
                    }
                }
                resp.on_hover_text(&s.name);
            }
            None => {
                ui.label(
                    RichText::new(format!("[Sticker: {}] (animated Lottie stickers aren't rendered)", s.name)).color(p.muted).italics(),
                );
            }
        }
    }
}

fn poll_view(ui: &mut Ui, poll: &dn_core::Poll, p: &Palette) {
    egui::Frame::new().fill(p.embed_bg).corner_radius(CornerRadius::same(8)).inner_margin(12).show(ui, |ui| {
        ui.set_max_width(440.0);
        ui.label(RichText::new(poll.question.text.clone().unwrap_or_default()).strong());
        let counts = poll.results.as_ref().map(|r| r.answer_counts.clone()).unwrap_or_default();
        let total: u32 = counts.iter().map(|c| c.count).sum::<u32>().max(1);
        for a in &poll.answers {
            let c = counts.iter().find(|c| c.id == a.answer_id).map(|c| c.count).unwrap_or(0);
            let frac = c as f32 / total as f32;
            let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 28.0), Sense::hover());
            ui.painter().rect_filled(rect, CornerRadius::same(4), p.hover);
            ui.painter().rect_filled(
                Rect::from_min_size(rect.min, vec2(rect.width() * frac, rect.height())),
                CornerRadius::same(4),
                p.accent.gamma_multiply(0.4),
            );
            let label = format!(
                "{} {}",
                a.poll_media.emoji.as_ref().and_then(|e| e.name.clone()).unwrap_or_default(),
                a.poll_media.text.clone().unwrap_or_default()
            );
            ui.painter().text(
                rect.left_center() + vec2(8.0, 0.0),
                egui::Align2::LEFT_CENTER,
                label.trim(),
                egui::FontId::proportional(13.0),
                p.text,
            );
            ui.painter().text(
                rect.right_center() - vec2(8.0, 0.0),
                egui::Align2::RIGHT_CENTER,
                format!("{c}"),
                egui::FontId::proportional(12.0),
                p.muted,
            );
        }
        let status = if poll.results.as_ref().is_some_and(|r| r.is_finalized) {
            "Final results"
        } else {
            "Bots can't vote; vote from an official client"
        };
        ui.label(RichText::new(status).size(11.0).color(p.faint));
    });
}

fn reactions(app: &mut DiscordApp, ui: &mut Ui, m: &Message, rc: &RowCtx, p: &Palette) {
    let can_react = rc.perms.contains(Permissions::ADD_REACTIONS);
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(4.0, 4.0);
        for r in &m.reactions {
            let fill = if r.me { p.accent.gamma_multiply(0.25) } else { p.embed_bg };
            let stroke = if r.me { Stroke::new(1.0, p.accent) } else { Stroke::new(1.0, Color32::TRANSPARENT) };
            let resp = egui::Frame::new()
                .fill(fill)
                .stroke(stroke)
                .corner_radius(CornerRadius::same(8))
                .inner_margin(egui::Margin::symmetric(6, 2))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        emoji_glyph(app, ui, &r.emoji, 16.0);
                        ui.label(RichText::new(r.count.to_string()).size(12.5).color(if r.me { p.text_strong } else { p.muted }));
                    });
                })
                .response;
            let resp = ui.interact(resp.rect, ui.scope_id().with(("react", m.id, r.emoji.reaction_key())), Sense::click());
            if resp.on_hover_text(format!(":{}:", r.emoji.name.clone().unwrap_or_default())).clicked() && (can_react || r.me) {
                app.link.send(Command::React { channel_id: m.channel_id, message_id: m.id, emoji: r.emoji.clone(), add: !r.me });
            }
        }
        if can_react && widgets::icon_button(ui, p, "☺", "Add reaction", None).clicked() {
            app.st.emoji_picker = Some(EmojiTarget::Reaction { channel_id: m.channel_id, message_id: m.id });
        }
    });
}

pub fn emoji_glyph(app: &mut DiscordApp, ui: &mut Ui, e: &Emoji, size: f32) {
    match e.url((size * 2.0) as u32) {
        Some(url) => {
            let (rect, _) = ui.allocate_exact_size(vec2(size, size), Sense::hover());
            if let Some(tex) = app.textures.get(&url, (size * 2.0) as u32) {
                egui::Image::new((tex.id(), rect.size())).paint_at(ui, rect);
            }
        }
        None => {
            ui.label(RichText::new(e.name.clone().unwrap_or_default()).size(size));
        }
    }
}

fn toolbar(app: &mut DiscordApp, ui: &mut Ui, m: &Message, rc: &RowCtx, row: Rect, p: &Palette) {
    let own = rc.me == Some(m.author.id);
    let n = 3 + own as usize;
    let w = n as f32 * 32.0 + 8.0;
    let rect = Rect::from_min_size(egui::pos2(row.right() - w - 16.0, row.top() - 14.0), vec2(w, 34.0));
    ui.painter().rect(rect, CornerRadius::same(6), p.chat, Stroke::new(1.0, p.divider), egui::StrokeKind::Inside);
    let mut child =
        ui.new_child(egui::UiBuilder::new().max_rect(rect.shrink(1.0)).layout(egui::Layout::left_to_right(egui::Align::Center)));
    child.spacing_mut().item_spacing.x = 0.0;
    child.add_space(3.0);
    if rc.perms.contains(Permissions::ADD_REACTIONS) && widgets::icon_button(&mut child, p, "☺", "Add reaction", None).clicked() {
        app.st.emoji_picker = Some(EmojiTarget::Reaction { channel_id: m.channel_id, message_id: m.id });
    }
    if rc.perms.can_send() && widgets::icon_button(&mut child, p, "↩", "Reply", None).clicked() {
        let c = app.st.composer(m.channel_id);
        c.reply_to = Some(m.id);
        c.mention_reply = true;
        child.ctx().memory_mut(|mem| mem.request_focus(egui::Id::unique(("composer", m.channel_id))));
    }
    if own && widgets::icon_button(&mut child, p, "✏", "Edit", None).clicked() {
        app.st.editing = Some((m.id, m.content.clone()));
    }
    let more = widgets::icon_button(&mut child, p, "⋯", "More", None);
    if more.clicked() {
        app.st.hovered_message = Some(m.id);
    }
    let _ = more;
}

fn context_menu(app: &mut DiscordApp, ui: &mut Ui, store: &Store, m: &Message, rc: &RowCtx, dev: bool) {
    let own = rc.me == Some(m.author.id);
    if rc.perms.contains(Permissions::ADD_REACTIONS) && ui.button("☺  Add Reaction").clicked() {
        app.st.emoji_picker = Some(EmojiTarget::Reaction { channel_id: m.channel_id, message_id: m.id });
        ui.close();
    }
    if rc.perms.can_send() && ui.button("↩  Reply").clicked() {
        let c = app.st.composer(m.channel_id);
        c.reply_to = Some(m.id);
        c.mention_reply = true;
        ui.close();
    }
    if own && ui.button("✏  Edit Message").clicked() {
        app.st.editing = Some((m.id, m.content.clone()));
        ui.close();
    }
    if rc.perms.can_pin() {
        let label = if m.pinned { "📌  Unpin Message" } else { "📌  Pin Message" };
        if ui.button(label).clicked() {
            app.link.send(Command::Pin { channel_id: m.channel_id, message_id: m.id, pin: !m.pinned });
            ui.close();
        }
    }
    let channel_is_thread = store.channels.get(&m.channel_id).is_some_and(|c| c.kind.is_thread());
    if m.guild_id.is_some()
        && !channel_is_thread
        && m.thread.is_none()
        && rc.perms.contains(Permissions::CREATE_PUBLIC_THREADS)
        && ui.button("🧵  Create Thread").clicked()
    {
        let name: String = m.content.chars().take(40).collect();
        app.st.modal = Some(Modal::CreateThread {
            channel_id: m.channel_id,
            message_id: m.id,
            name: if name.is_empty() { "New Thread".into() } else { name },
        });
        ui.close();
    }
    if ui.button("⧉  Copy Text").clicked() {
        ui.ctx().copy_text(m.content.clone());
        ui.close();
    }
    if ui.button("🔗  Copy Message Link").clicked() {
        let g = m.guild_id.map(|g| g.to_string()).unwrap_or_else(|| "@me".into());
        ui.ctx().copy_text(format!("https://discord.com/channels/{g}/{}/{}", m.channel_id, m.id));
        ui.close();
    }
    if ui.button("Mark Unread").clicked() {
        // Mark everything before this message as read.
        app.link.send(Command::MarkRead { channel_id: m.channel_id, message_id: Id(m.id.0.saturating_sub(1)) });
        ui.close();
    }
    if dev && ui.button("Copy Message ID").clicked() {
        ui.ctx().copy_text(m.id.to_string());
        ui.close();
    }
    if own || rc.perms.contains(Permissions::MANAGE_MESSAGES) {
        ui.separator();
        if ui.button(RichText::new("🗑  Delete Message").color(app.palette().red)).clicked() {
            let confirm = app.link.settings.read().behavior.confirm_delete;
            let shift = ui.input(|i| i.modifiers.shift);
            if confirm && !shift {
                app.st.modal = Some(Modal::ConfirmDelete { channel_id: m.channel_id, message_id: m.id, preview: m.content.clone() });
            } else {
                app.link.send(Command::DeleteMessage { channel_id: m.channel_id, message_id: m.id });
            }
            ui.close();
        }
    }
}
