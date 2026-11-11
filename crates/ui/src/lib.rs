//! Native egui/wgpu user interface for Discord Native.
//!
//! The UI never blocks: it reads `dn_cache::Store` (shared with the backend), sends
//! `Command`s, and uploads textures decoded on background threads.

mod chat;
pub mod fonts;
mod members;
mod message;
mod modals;
mod onboarding;
mod rail;
pub mod richtext;
mod settings_view;
mod sidebar;
pub mod state;
pub mod stress;
pub mod textures;
pub mod theme;
pub mod virtual_list;
pub mod widgets;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use egui::{Color32, CornerRadius, Key, KeyboardShortcut, Modifiers, Ui};

use dn_cache::Store;
use dn_cache::bus::{Command, UiEvent, UiLink};
use dn_core::Id;
use dn_messaging::markdown::Block;

use crate::state::{Modal, SettingsTab, UiState};
use crate::textures::Textures;
use crate::theme::Palette;

pub const APP_TITLE: &str = "Discord Native";

pub struct UiOptions {
    pub media: Option<dn_media::MediaLoader>,
    pub texture_budget_bytes: usize,
    pub use_system_fonts: bool,
    /// Called when the user chooses "close to tray" so the shell can show a tray icon.
    pub has_tray: bool,
    /// The shell polls the push-to-talk key globally (Windows); skip the in-window check.
    pub global_ptt: bool,
}

pub struct DiscordApp {
    pub link: UiLink,
    pub st: UiState,
    textures: Textures,
    palette: Palette,
    applied: Option<(dn_settings::Theme, u32, u32, bool)>,
    frame_stats: dn_diagnostics::FrameStats,
    metrics: dn_diagnostics::MetricsSampler,
    last_sample: (Instant, dn_diagnostics::ProcessSample),

    md_cache: HashMap<(Id, u64), Arc<Vec<Block>>>,
    has_tray: bool,
    global_ptt: bool,
    quit_requested: bool,
    focus_sent: Option<bool>,
    pending_open: Option<Id>,
}

impl DiscordApp {
    pub fn new(ctx: &egui::Context, link: UiLink, opts: UiOptions) -> Self {
        ctx.set_fonts(fonts::definitions(opts.use_system_fonts));
        let theme = link.settings.read().appearance.theme;
        let st = UiState { window_focused: true, ..Default::default() };
        let repaint_ctx = ctx.clone();
        link.repaint.install(move |d| if d.is_zero() { repaint_ctx.request_repaint() } else { repaint_ctx.request_repaint_after(d) });
        let mut app = DiscordApp {
            link,
            st,
            textures: Textures::new(opts.media, opts.texture_budget_bytes),
            palette: Palette::for_theme(theme),
            applied: None,
            frame_stats: dn_diagnostics::FrameStats::new(240),
            metrics: dn_diagnostics::MetricsSampler::new(),
            last_sample: (Instant::now(), Default::default()),

            md_cache: HashMap::new(),
            has_tray: opts.has_tray,
            global_ptt: opts.global_ptt,
            quit_requested: false,
            focus_sent: None,
            pending_open: None,
        };
        app.apply_settings(ctx);
        app
    }

    /// Ask the app to really quit (tray "Quit"), bypassing close-to-tray.
    pub fn request_quit(&mut self) {
        self.quit_requested = true;
    }

    fn apply_settings(&mut self, ctx: &egui::Context) {
        let s = self.link.settings.read();
        let a = &s.appearance;
        let key = (a.theme, (a.font_size * 10.0) as u32, (a.ui_scale * 100.0) as u32, a.reduce_motion);
        if self.applied == Some(key) {
            return;
        }
        self.palette = Palette::for_theme(a.theme);
        theme::apply(ctx, &self.palette, a.font_size, a.reduce_motion);
        ctx.set_zoom_factor(a.ui_scale.clamp(0.75, 2.0));
        self.textures.set_budget(s.advanced.image_cache_mb as usize * 1024 * 1024);
        self.applied = Some(key);
    }

    fn drain_events(&mut self, ctx: &egui::Context) {
        while let Ok(ev) = self.link.events.try_recv() {
            match ev {
                UiEvent::Toast { text, error } => self.st.toast(text, error),
                UiEvent::SearchResults { request, total, messages, source, note } => {
                    if self.st.search.last.as_ref() == Some(&*request) {
                        self.st.search.loading = false;
                        self.st.search.total = total;
                        if request.offset == 0 {
                            self.st.search.results = messages;
                        } else {
                            self.st.search.results.extend(messages);
                        }
                        self.st.search.source = Some(source);
                        self.st.search.note = note;
                    }
                }
                UiEvent::Pins { channel_id, messages } => self.st.pins = Some((channel_id, messages, true)),
                UiEvent::Profile { user, member, .. } => {
                    self.st.profiles.insert(user.id, (*user, member.map(|m| *m)));
                }
                UiEvent::OpenChannel(c) => {
                    self.pending_open = Some(c);
                    // Also used for toast clicks: bring the window back from the tray.
                    ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                }
                UiEvent::OAuthFinished(r) => {
                    self.st.onboarding.oauth_busy = false;
                    match r {
                        Ok(name) => self.st.toast(format!("Signed in as {name}"), false),
                        Err(e) => self.st.toast(format!("Sign-in failed: {e}"), true),
                    }
                }
                UiEvent::AccountAdded(r) => {
                    self.st.onboarding.busy = false;
                    match r {
                        Ok(name) => {
                            self.st.onboarding.token.clear();
                            self.st.onboarding.error = None;
                            self.st.show_add_account = false;
                            self.st.toast(format!("Connected as {name}"), false);
                        }
                        Err(e) => self.st.onboarding.error = Some(e),
                    }
                }
                UiEvent::RequestAttention => {
                    if !self.st.window_focused && self.link.settings.read().notifications.flash_taskbar {
                        ctx.send_viewport_cmd(egui::ViewportCommand::RequestUserAttention(egui::UserAttentionType::Informational));
                    }
                }
            }
        }
    }

    fn markdown(&mut self, id: Id, content: &str) -> Arc<Vec<Block>> {
        let key = (id, fxhash(content));
        if let Some(b) = self.md_cache.get(&key) {
            return b.clone();
        }
        if self.md_cache.len() > 4000 {
            self.md_cache.clear();
        }
        let blocks = Arc::new(dn_messaging::markdown::parse(content));
        self.md_cache.insert(key, blocks.clone());
        blocks
    }

    fn handle_shortcuts(&mut self, ctx: &egui::Context, store: &mut Store) {
        let (mute, deaf, switcher, search, escape, alt_up, alt_down) = ctx.input_mut(|i| {
            (
                i.consume_shortcut(&KeyboardShortcut::new(Modifiers::COMMAND | Modifiers::SHIFT, Key::M)),
                i.consume_shortcut(&KeyboardShortcut::new(Modifiers::COMMAND | Modifiers::SHIFT, Key::D)),
                i.consume_shortcut(&KeyboardShortcut::new(Modifiers::COMMAND, Key::K)),
                i.consume_shortcut(&KeyboardShortcut::new(Modifiers::COMMAND, Key::F)),
                i.key_pressed(Key::Escape),
                i.consume_shortcut(&KeyboardShortcut::new(Modifiers::ALT, Key::ArrowUp)),
                i.consume_shortcut(&KeyboardShortcut::new(Modifiers::ALT, Key::ArrowDown)),
            )
        });
        if mute {
            let m = !store.voice.self_mute;
            self.link.send(Command::SetMute(m));
        }
        if deaf {
            let d = !store.voice.self_deaf;
            self.link.send(Command::SetDeaf(d));
        }
        if switcher {
            self.st.modal = Some(Modal::QuickSwitcher { query: String::new(), selected: 0 });
        }
        if search && self.st.selected_guild.is_some() {
            self.st.search.open = true;
            ctx.memory_mut(|m| m.request_focus(egui::Id::unique("search-box")));
        }
        if escape {
            if self.st.modal.is_some() {
                self.st.modal = None;
            } else if self.st.emoji_picker.is_some() {
                self.st.emoji_picker = None;
            } else if self.st.editing.is_some() {
                self.st.editing = None;
            } else if let Some(c) = self.st.selected_channel
                && self.st.composer(c).reply_to.take().is_none()
                && let Some(last) = store.channels.get(&c).and_then(|ch| ch.last_message_id)
            {
                self.link.send(Command::MarkRead { channel_id: c, message_id: last });
            }
        }
        if alt_up || alt_down {
            self.step_channel(store, if alt_up { -1 } else { 1 });
        }
        // In-window push-to-talk (a global key watcher in the shell covers unfocused windows on Windows).
        let ptt_mode = matches!(self.link.settings.read().voice.input_mode, dn_settings::InputMode::PushToTalk);
        if ptt_mode && !self.global_ptt && store.voice.conn.channel().is_some() {
            let key = self.link.settings.read().voice.ptt_key.clone();
            if let Some(k) = Key::from_name(&key) {
                let down = ctx.input(|i| i.key_down(k)) && !ctx.egui_wants_keyboard_input();
                if down != self.st.ptt_down {
                    self.st.ptt_down = down;
                    self.link.send(Command::PushToTalk(down));
                }
            }
        }
    }

    fn step_channel(&mut self, store: &mut Store, dir: i32) {
        let ids: Vec<Id> = match self.st.selected_guild {
            Some(g) => store
                .channel_tree(g)
                .into_iter()
                .flat_map(|grp| grp.channels.into_iter())
                .filter(|c| !c.kind.is_voice() && c.kind.is_text_like())
                .map(|c| c.id)
                .collect(),
            None => store.dm_list().iter().map(|c| c.id).collect(),
        };
        if ids.is_empty() {
            return;
        }
        let cur = self.st.selected_channel.and_then(|c| ids.iter().position(|x| *x == c));
        let next = match cur {
            Some(i) => (i as i32 + dir).rem_euclid(ids.len() as i32) as usize,
            None => 0,
        };
        self.open_channel(store, ids[next]);
    }

    /// Select a channel (and its guild), load its history and mark it active.
    pub fn open_channel(&mut self, store: &mut Store, channel: Id) {
        let guild = store.guild_of(channel);
        self.st.selected_guild = guild;
        if self.st.selected_channel != Some(channel) {
            self.st.editing = None;
            self.st.emoji_picker = None;
        }
        self.st.selected_channel = Some(channel);
        self.st.last_channel_per_guild.insert(guild, channel);
        store.set_active_channel(Some(channel));
        let marker = store.read_states.get(&channel).copied();
        let unread = store.is_unread(channel);
        let list = self.st.list(channel);
        list.unread_marker = if unread { marker } else { None };
        list.at_bottom = true;
        list.anchor = None;
        // The backend serves cached history immediately and refreshes the latest page.
        self.link.send(Command::OpenChannel { channel_id: channel });
        if let Some(g) = guild
            && store.guilds.get(&g).is_some_and(|gs| !gs.members_requested)
        {
            self.link.send(Command::RequestMembers { guild_id: g });
        }
    }

    pub fn select_guild(&mut self, store: &mut Store, guild: Option<Id>) {
        self.st.selected_guild = guild;
        self.st.search.open = false;
        let remembered = self.st.last_channel_per_guild.get(&guild).copied();
        let target = remembered.filter(|c| store.channels.contains_key(c)).or_else(|| match guild {
            Some(g) => store
                .channel_tree(g)
                .into_iter()
                .flat_map(|grp| grp.channels.into_iter())
                .find(|c| c.kind.is_text_like() && !c.kind.is_voice())
                .map(|c| c.id),
            None => store.dm_list().first().map(|c| c.id),
        });
        match target {
            Some(c) => self.open_channel(store, c),
            None => {
                self.st.selected_channel = None;
                store.set_active_channel(None);
            }
        }
    }

    fn toasts(&mut self, ctx: &egui::Context) {
        let now = Instant::now();
        self.st.toasts.retain(|t| t.until > now);
        if self.st.toasts.is_empty() {
            return;
        }
        let p = self.palette;
        egui::Area::new(egui::Id::unique("toasts"))
            .anchor(egui::Align2::RIGHT_BOTTOM, egui::vec2(-16.0, -80.0))
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                for t in &self.st.toasts {
                    egui::Frame::new()
                        .fill(if t.error { p.red } else { p.floating })
                        .corner_radius(CornerRadius::same(6))
                        .inner_margin(10)
                        .show(ui, |ui| {
                            ui.set_max_width(360.0);
                            ui.label(egui::RichText::new(&t.text).color(if t.error { Color32::WHITE } else { p.text }));
                        });
                    ui.add_space(6.0);
                }
            });
        let next = self.st.toasts.iter().map(|t| t.until).min().unwrap_or(now);
        ctx.request_repaint_after(next.saturating_duration_since(now) + Duration::from_millis(50));
    }

    fn handle_dropped_files(&mut self, ctx: &egui::Context) {
        let files: Vec<std::path::PathBuf> =
            ctx.input(|i| i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).filter(|p| !p.as_os_str().is_empty()).collect());
        if files.is_empty() {
            return;
        }
        match self.st.selected_channel {
            Some(c) => {
                let comp = self.st.composer(c);
                comp.attachments.extend(files);
                comp.attachments.truncate(10);
            }
            None => self.st.toast("Open a channel before dropping files", true),
        }
    }

    fn poll_dialogs(&mut self) {
        if let Some(rx) = &self.st.file_dialog_rx
            && let Ok((channel, files)) = rx.try_recv()
        {
            let comp = self.st.composer(channel);
            comp.attachments.extend(files);
            comp.attachments.truncate(10);
            self.st.file_dialog_rx = None;
        }
        if let Some(rx) = &self.st.save_dialog_rx
            && let Ok((url, path)) = rx.try_recv()
        {
            if let Some(path) = path {
                self.link.send(Command::DownloadTo { url, path });
            }
            self.st.save_dialog_rx = None;
        }
    }

    fn sample_process(&mut self) {
        let now = Instant::now();
        if now.duration_since(self.last_sample.0) > Duration::from_secs(2) {
            self.last_sample = (now, self.metrics.sample());
        }
    }

    pub fn palette(&self) -> &Palette {
        &self.palette
    }

    pub(crate) fn pending_open_channel(&mut self, c: Id) {
        self.pending_open = Some(c);
    }
}

// Opened channel requested by the backend (e.g. after creating a DM) is applied at frame start.
impl DiscordApp {
    #[allow(clippy::needless_pass_by_ref_mut)]
    fn apply_pending_open(&mut self, store: &mut Store) {
        if let Some(c) = self.pending_open.take()
            && store.channels.contains_key(&c)
        {
            self.open_channel(store, c);
        }
    }
}

impl eframe::App for DiscordApp {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.drain_events(ctx);
        let close = ctx.input(|i| i.viewport().close_requested());
        if close && !self.quit_requested && self.has_tray && self.link.settings.read().behavior.close_to_tray {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
        }
        let focused = ctx.input(|i| i.viewport().focused.unwrap_or(true));
        self.st.window_focused = focused;
        if self.focus_sent != Some(focused) {
            self.focus_sent = Some(focused);
            self.link.send(Command::FocusChanged { window_focused: focused });
        }
    }

    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let frame_start = Instant::now();
        self.sample_process();
        self.apply_settings(&ctx);
        self.textures.begin_frame(&ctx);
        self.handle_dropped_files(&ctx);
        self.poll_dialogs();

        let store_arc = self.link.store.clone();
        let mut store = store_arc.lock();
        self.apply_pending_open(&mut store);

        if store.session.needs_onboarding || self.st.show_add_account {
            onboarding::show(self, ui, &mut store);
        } else if let Some(stress) = self.st.stress.as_mut() {
            if !stress.show(ui, &self.palette) {
                self.st.stress = None;
            }
        } else {
            self.handle_shortcuts(&ctx, &mut store);
            self.main_layout(ui, &mut store);
            modals::show(self, &ctx, &mut store);
        }
        drop(store);
        self.toasts(&ctx);
        // CPU time to build this frame (tessellation and GPU work happen after `ui` returns).
        self.frame_stats.record(frame_start.elapsed());
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.link.send(Command::Shutdown);
    }
}

impl DiscordApp {
    fn main_layout(&mut self, ui: &mut Ui, store: &mut Store) {
        let p = self.palette;
        if store.session.demo {
            egui::Panel::top("demo-banner").exact_size(26.0).resizable(false).frame(egui::Frame::new().fill(p.yellow)).show(ui, |ui| {
                ui.centered_and_justified(|ui| {
                    ui.label(
                        egui::RichText::new("Offline demo data — not connected to Discord. Start without --demo to connect your bot.")
                            .color(Color32::BLACK)
                            .strong(),
                    );
                });
            });
        }
        if self.st.selected_channel.is_none() && store.session.ready && !store.guild_order.is_empty() && self.st.selected_guild.is_none() {
            let first = store.guild_order.first().copied();
            self.select_guild(store, first);
        }
        egui::Panel::left("rail")
            .exact_size(72.0)
            .resizable(false)
            .frame(egui::Frame::new().fill(p.rail).inner_margin(egui::Margin::symmetric(12, 8)))
            .show(ui, |ui| rail::show(self, ui, store));
        egui::Panel::left("sidebar")
            .exact_size(240.0)
            .resizable(false)
            .frame(egui::Frame::new().fill(p.sidebar))
            .show(ui, |ui| sidebar::show(self, ui, store));
        let show_members = self.link.settings.read().appearance.show_member_list;
        if self.st.search.open && self.st.selected_guild.is_some() {
            egui::Panel::right("search")
                .default_size(400.0)
                .resizable(true)
                .frame(egui::Frame::new().fill(p.sidebar).inner_margin(8))
                .show(ui, |ui| chat::search_panel(self, ui, store));
        } else if show_members && self.st.selected_guild.is_some() && self.st.selected_channel.is_some() {
            egui::Panel::right("members")
                .exact_size(240.0)
                .resizable(false)
                .frame(egui::Frame::new().fill(p.sidebar).inner_margin(egui::Margin::symmetric(8, 8)))
                .show(ui, |ui| members::show(self, ui, store));
        }
        egui::CentralPanel::default().frame(egui::Frame::new().fill(p.chat)).show(ui, |ui| chat::show(self, ui, store));
    }

    pub(crate) fn open_settings(&mut self, tab: SettingsTab) {
        self.st.modal = Some(Modal::Settings(tab));
    }
}

/// Cheap non-cryptographic hash for cache keys.
pub(crate) fn fxhash(s: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}
