//! Renders the Discord Markdown AST (dn-messaging) with egui. Each paragraph becomes one
//! selectable galley; links, mentions and spoilers are hit-tested by character index, and
//! custom emoji are painted over reserved non-breaking spaces.

use std::collections::HashSet;
use std::ops::Range;

use egui::text::{CCursor, LayoutJob, TextFormat};
use egui::text_selection::LabelSelectionState;
use egui::{Color32, CornerRadius, FontFamily, FontId, Pos2, Rect, Sense, Stroke, Ui, vec2};

use dn_cache::Store;
use dn_core::Id;
use dn_messaging::markdown::{Block, Inline};

use crate::textures::Textures;
use crate::theme::{BOLD, BOLD_ITALIC, Palette};

#[derive(Debug, Clone, PartialEq)]
pub enum RichAction {
    OpenUrl { url: String, masked: bool },
    Profile(Id),
    Channel(Id),
}

pub struct RenderCtx<'a> {
    pub palette: &'a Palette,
    pub store: &'a Store,
    pub guild_id: Option<Id>,
    pub textures: &'a mut Textures,
    pub font_size: f32,
    pub use_24h: bool,
    pub now_unix: i64,
    pub revealed: &'a mut HashSet<(u64, usize)>,
    pub key: u64,
    pub actions: &'a mut Vec<RichAction>,
    pub jumbo: bool,
    pub color: Color32,
}

#[derive(Debug, Clone)]
enum SpanKind {
    Link { url: String, masked: bool },
    User(Id),
    Channel(Id),
    Spoiler(usize),
    Emoji { url: String },
}

#[derive(Debug, Clone)]
struct Span {
    chars: Range<usize>,
    kind: SpanKind,
}

#[derive(Clone, Copy, Default)]
struct Style {
    bold: bool,
    italic: bool,
    underline: bool,
    strike: bool,
    spoiler: Option<usize>,
    link: bool,
    size_scale: f32,
}

struct Builder<'b, 'a> {
    rc: &'b mut RenderCtx<'a>,
    job: LayoutJob,
    chars: usize,
    spans: Vec<Span>,
    spoiler_counter: usize,
}

impl<'b, 'a> Builder<'b, 'a> {
    fn new(rc: &'b mut RenderCtx<'a>, spoiler_base: usize) -> Self {
        Builder { rc, job: LayoutJob::default(), chars: 0, spans: vec![], spoiler_counter: spoiler_base }
    }

    fn font(&self, s: &Style) -> FontId {
        let size = self.rc.font_size * if s.size_scale > 0.0 { s.size_scale } else { 1.0 };
        let family = match s.bold {
            true => FontFamily::Name(if s.italic { BOLD_ITALIC } else { BOLD }.into()),
            false => FontFamily::Proportional,
        };
        FontId::new(size, family)
    }

    fn format(&self, s: &Style) -> TextFormat {
        let p = self.rc.palette;
        let mut f = TextFormat { font_id: self.font(s), color: self.rc.color, italics: s.italic, ..Default::default() };
        if s.link {
            f.color = p.link;
        }
        if s.underline {
            f.underline = Stroke::new(1.0, f.color);
        }
        if s.strike {
            f.strikethrough = Stroke::new(1.0, f.color);
        }
        if let Some(id) = s.spoiler {
            if self.rc.revealed.contains(&(self.rc.key, id)) {
                f.background = p.hover;
            } else {
                let hidden = if p.dark { Color32::from_gray(30) } else { Color32::from_gray(200) };
                f.background = hidden;
                f.color = hidden;
                f.underline = Stroke::NONE;
            }
        }
        f
    }

    fn push(&mut self, text: &str, fmt: TextFormat) -> Range<usize> {
        let start = self.chars;
        self.chars += text.chars().count();
        self.job.append(text, 0.0, fmt);
        start..self.chars
    }

    fn inline(&mut self, items: &[Inline], style: Style) {
        for item in items {
            match item {
                Inline::Text(t) => {
                    let f = self.format(&style);
                    let r = self.push(t, f);
                    if let Some(id) = style.spoiler {
                        self.spans.push(Span { chars: r, kind: SpanKind::Spoiler(id) });
                    }
                }
                Inline::Bold(c) => self.inline(c, Style { bold: true, ..style }),
                Inline::Italic(c) => self.inline(c, Style { italic: true, ..style }),
                Inline::Underline(c) => self.inline(c, Style { underline: true, ..style }),
                Inline::Strike(c) => self.inline(c, Style { strike: true, ..style }),
                Inline::Spoiler(c) => {
                    let id = self.spoiler_counter;
                    self.spoiler_counter += 1;
                    self.inline(c, Style { spoiler: Some(id), ..style });
                }
                Inline::Code(code) => {
                    let mut f = self.format(&style);
                    f.font_id = FontId::new(self.rc.font_size * 0.87, FontFamily::Monospace);
                    if style.spoiler.is_none_or(|id| self.rc.revealed.contains(&(self.rc.key, id))) {
                        f.background = self.rc.palette.code_bg;
                    }
                    self.push(code, f);
                }
                Inline::Link { text, url } => {
                    let start = self.chars;
                    self.inline(text, Style { link: true, ..style });
                    self.spans.push(Span { chars: start..self.chars, kind: SpanKind::Link { url: url.clone(), masked: true } });
                }
                Inline::AutoLink(url) => {
                    let f = self.format(&Style { link: true, ..style });
                    let r = self.push(url, f);
                    self.spans.push(Span { chars: r, kind: SpanKind::Link { url: url.clone(), masked: false } });
                }
                Inline::UserMention(id) => {
                    let name = format!("@{}", self.rc.store.display_name(self.rc.guild_id, *id));
                    let r = self.mention(&name, style);
                    self.spans.push(Span { chars: r, kind: SpanKind::User(*id) });
                }
                Inline::RoleMention(id) => {
                    let role = self.rc.guild_id.and_then(|g| self.rc.store.guilds.get(&g)).and_then(|g| g.role(*id));
                    let name = role.map(|r| format!("@{}", r.name)).unwrap_or_else(|| "@unknown-role".into());
                    let color = role.and_then(|r| r.display_color());
                    let mut f = self.format(&style);
                    let c = color.map(crate::theme::rgb).unwrap_or(self.rc.palette.accent);
                    f.color = c;
                    f.background = c.gamma_multiply(0.15);
                    self.push(&name, f);
                }
                Inline::ChannelMention(id) => {
                    let name = match self.rc.store.channels.get(id) {
                        Some(c) => format!("#{}", c.display_name()),
                        None => "#unknown".to_string(),
                    };
                    let r = self.mention(&name, style);
                    self.spans.push(Span { chars: r, kind: SpanKind::Channel(*id) });
                }
                Inline::Everyone => {
                    self.mention("@everyone", style);
                }
                Inline::Here => {
                    self.mention("@here", style);
                }
                Inline::CustomEmoji { name, id, animated } => {
                    let size = self.emoji_size();
                    let url = dn_core::cdn::emoji(*id, *animated, (size * 2.0) as u32);
                    let mut f = self.format(&style);
                    // Reserve roughly `size` px: NBSP is ~0.25em wide in common fonts.
                    f.font_id.size = size;
                    f.extra_letter_spacing = size * 0.78;
                    f.background = Color32::TRANSPARENT;
                    let r = self.push("\u{00A0}", f);
                    self.spans.push(Span { chars: r, kind: SpanKind::Emoji { url } });
                    let _ = name;
                }
                Inline::Timestamp { unix, style: ts } => {
                    let text = dn_messaging::timestamp::format(*unix, *ts, self.rc.now_unix, self.rc.use_24h);
                    let mut f = self.format(&style);
                    f.background = self.rc.palette.code_bg;
                    self.push(&text, f);
                }
                Inline::SlashCommand { name, .. } => {
                    let label = format!("/{name}");
                    self.mention(&label, style);
                }
                Inline::LineBreak => {
                    let f = self.format(&style);
                    self.push("\n", f);
                }
            }
        }
    }

    fn mention(&mut self, text: &str, style: Style) -> Range<usize> {
        let mut f = self.format(&style);
        if style.spoiler.is_none_or(|id| self.rc.revealed.contains(&(self.rc.key, id))) {
            f.color = if self.rc.palette.dark { Color32::from_rgb(201, 205, 251) } else { self.rc.palette.accent };
            f.background = self.rc.palette.accent.gamma_multiply(0.3);
        }
        self.push(text, f)
    }

    fn emoji_size(&self) -> f32 {
        if self.rc.jumbo { 48.0 } else { self.rc.font_size * 1.375 }
    }
}

/// Render parsed blocks. Returns true if anything was clicked (so callers can skip
/// their own click handling).
pub fn render_blocks(ui: &mut Ui, blocks: &[Block], rc: &mut RenderCtx) {
    let mut spoiler_base = 0;
    for block in blocks {
        render_block(ui, block, rc, &mut spoiler_base, 0);
    }
}

fn render_block(ui: &mut Ui, block: &Block, rc: &mut RenderCtx, spoiler_base: &mut usize, depth: usize) {
    match block {
        Block::Paragraph(inl) => paragraph(ui, inl, rc, spoiler_base, Style::default()),
        Block::Heading { level, content } => {
            let scale = match level {
                1 => 1.5,
                2 => 1.25,
                _ => 1.1,
            };
            ui.add_space(4.0);
            paragraph(ui, content, rc, spoiler_base, Style { bold: true, size_scale: scale, ..Default::default() });
        }
        Block::Subtext(inl) => {
            let prev = rc.color;
            rc.color = rc.palette.muted;
            paragraph(ui, inl, rc, spoiler_base, Style { size_scale: 0.8, ..Default::default() });
            rc.color = prev;
        }
        Block::CodeBlock { lang, code } => {
            let p = rc.palette;
            egui::Frame::new()
                .fill(p.code_bg)
                .stroke(Stroke::new(1.0, p.divider))
                .corner_radius(CornerRadius::same(4))
                .inner_margin(8)
                .show(ui, |ui| {
                    ui.set_width(ui.available_width().min(900.0));
                    if let Some(l) = lang.as_deref().filter(|l| !l.is_empty()) {
                        ui.label(egui::RichText::new(l).small().color(p.faint));
                    }
                    let job = LayoutJob::simple(
                        code.trim_end_matches('\n').to_string(),
                        FontId::new(rc.font_size * 0.87, FontFamily::Monospace),
                        p.text,
                        ui.available_width(),
                    );
                    ui.add(egui::Label::new(job).selectable(true));
                });
        }
        Block::Quote(inner) => {
            let p = rc.palette;
            let resp = ui.horizontal(|ui| {
                ui.add_space(12.0);
                ui.vertical(|ui| {
                    for b in inner {
                        render_block(ui, b, rc, spoiler_base, depth + 1);
                    }
                })
            });
            let r = resp.response.rect;
            ui.painter().rect_filled(Rect::from_min_size(r.min, vec2(4.0, r.height())), CornerRadius::same(2), p.faint);
        }
        Block::List { ordered, start, items } => {
            for (i, item) in items.iter().enumerate() {
                ui.horizontal_top(|ui| {
                    let marker = if *ordered {
                        format!("{}.", *start as usize + i)
                    } else if depth.is_multiple_of(2) {
                        "•".to_string()
                    } else {
                        "◦".to_string()
                    };
                    ui.add_space(8.0 + depth as f32 * 4.0);
                    ui.label(egui::RichText::new(marker).color(rc.palette.muted));
                    ui.vertical(|ui| {
                        for b in item {
                            render_block(ui, b, rc, spoiler_base, depth + 1);
                        }
                    });
                });
            }
        }
    }
}

fn paragraph(ui: &mut Ui, inl: &[Inline], rc: &mut RenderCtx, spoiler_base: &mut usize, style: Style) {
    if inl.is_empty() {
        return;
    }
    let mut b = Builder::new(rc, *spoiler_base);
    b.job.wrap.max_width = ui.available_width();
    b.inline(inl, style);
    *spoiler_base = b.spoiler_counter;
    let Builder { job, spans, .. } = b;
    show_job(ui, job, spans, rc);
}

/// Inline-only text (embed titles, field values, reply previews).
pub fn render_inline(ui: &mut Ui, inl: &[Inline], rc: &mut RenderCtx, bold: bool, max_rows: Option<usize>) {
    let mut b = Builder::new(rc, 0);
    b.job.wrap.max_width = ui.available_width();
    if let Some(rows) = max_rows {
        b.job.wrap.max_rows = rows;
        b.job.wrap.break_anywhere = false;
    }
    b.inline(inl, Style { bold, ..Default::default() });
    let Builder { job, spans, .. } = b;
    show_job(ui, job, spans, rc);
}

fn show_job(ui: &mut Ui, job: LayoutJob, spans: Vec<Span>, rc: &mut RenderCtx) {
    let (pos, galley, resp) = egui::Label::new(job).sense(Sense::click()).layout_in_ui(ui);
    if !ui.is_rect_visible(resp.rect) {
        return;
    }
    LabelSelectionState::label_text_selection(ui, &resp, pos, galley.clone(), rc.color, Stroke::NONE);

    let char_rect = |idx: usize| -> Rect {
        let a = galley.pos_from_cursor(CCursor::new(idx));
        let b = galley.pos_from_cursor(CCursor::new(idx + 1));
        let right = if (b.min.y - a.min.y).abs() < 1.0 { b.min.x } else { a.min.x + a.height() };
        Rect::from_min_max(a.min, Pos2::new(right, a.max.y)).translate(pos.to_vec2())
    };

    for span in &spans {
        if let SpanKind::Emoji { url } = &span.kind {
            let r = char_rect(span.chars.start);
            let size = r.height().min(r.width().max(16.0)).min(64.0);
            let rect = Rect::from_center_size(r.center(), vec2(size, size));
            match rc.textures.get(url, (size * 2.0) as u32) {
                Some(tex) => {
                    egui::Image::new((tex.id(), rect.size())).paint_at(ui, rect);
                }
                None => {
                    ui.painter().rect_filled(rect.shrink(2.0), CornerRadius::same(4), rc.palette.hover);
                }
            }
        }
    }

    let hovered_char = resp.hover_pos().map(|p| galley.cursor_from_pos(p - pos).index.0);
    if let Some(ci) = hovered_char {
        if spans.iter().any(|s| s.chars.contains(&ci) && !matches!(s.kind, SpanKind::Emoji { .. })) {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        if let Some(Span { kind: SpanKind::Link { url, .. }, .. }) = spans.iter().find(|s| s.chars.contains(&ci)) {
            let url = url.clone();
            resp.clone().on_hover_text_at_pointer(url);
        }
    }
    if resp.clicked()
        && let Some(ci) = resp.interact_pointer_pos().map(|p| galley.cursor_from_pos(p - pos).index.0)
    {
        // Spoilers first: clicking hidden text reveals it instead of following links.
        if let Some(Span { kind: SpanKind::Spoiler(id), .. }) =
            spans.iter().find(|s| s.chars.contains(&ci) && matches!(s.kind, SpanKind::Spoiler(_)))
            && rc.revealed.insert((rc.key, *id))
        {
            return;
        }
        for s in spans.iter().filter(|s| s.chars.contains(&ci)) {
            match &s.kind {
                SpanKind::Link { url, masked } => rc.actions.push(RichAction::OpenUrl { url: url.clone(), masked: *masked }),
                SpanKind::User(id) => rc.actions.push(RichAction::Profile(*id)),
                SpanKind::Channel(id) => rc.actions.push(RichAction::Channel(*id)),
                _ => {}
            }
        }
    }
}
