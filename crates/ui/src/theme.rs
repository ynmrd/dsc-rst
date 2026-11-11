//! Colour palettes and egui style setup. Colours are our own dark/light/midnight palettes
//! in the familiar layout tones of desktop chat clients; no proprietary assets are used.

use egui::{Color32, CornerRadius, FontFamily, FontId, Stroke, TextStyle, Visuals};

use dn_settings::Theme;

#[derive(Debug, Clone, Copy)]
pub struct Palette {
    pub rail: Color32,
    pub sidebar: Color32,
    pub chat: Color32,
    pub floating: Color32,
    pub input: Color32,
    pub hover: Color32,
    pub selected: Color32,
    pub text: Color32,
    pub text_strong: Color32,
    pub muted: Color32,
    pub faint: Color32,
    pub accent: Color32,
    pub accent_text: Color32,
    pub green: Color32,
    pub yellow: Color32,
    pub red: Color32,
    pub link: Color32,
    pub mention_bg: Color32,
    pub mention_bar: Color32,
    pub code_bg: Color32,
    pub divider: Color32,
    pub embed_bg: Color32,
    pub dark: bool,
}

impl Palette {
    pub fn for_theme(theme: Theme) -> Palette {
        match theme {
            Theme::Dark => Palette {
                rail: hex(0x1e1f22),
                sidebar: hex(0x2b2d31),
                chat: hex(0x313338),
                floating: hex(0x111214),
                input: hex(0x383a40),
                hover: Color32::from_rgba_unmultiplied(78, 80, 88, 76),
                selected: Color32::from_rgba_unmultiplied(78, 80, 88, 153),
                text: hex(0xdbdee1),
                text_strong: hex(0xf2f3f5),
                muted: hex(0x949ba4),
                faint: hex(0x6d6f78),
                accent: hex(0x5b6ef5),
                accent_text: Color32::WHITE,
                green: hex(0x23a55a),
                yellow: hex(0xf0b232),
                red: hex(0xf23f43),
                link: hex(0x00a8fc),
                mention_bg: Color32::from_rgba_unmultiplied(240, 178, 50, 26),
                mention_bar: hex(0xf0b232),
                code_bg: hex(0x2b2d31),
                divider: hex(0x3f4147),
                embed_bg: hex(0x2b2d31),
                dark: true,
            },
            Theme::Midnight => Palette {
                rail: hex(0x000000),
                sidebar: hex(0x0b0b0d),
                chat: hex(0x000000),
                floating: hex(0x111214),
                input: hex(0x1a1b1e),
                hover: Color32::from_rgba_unmultiplied(78, 80, 88, 60),
                selected: Color32::from_rgba_unmultiplied(78, 80, 88, 120),
                text: hex(0xdbdee1),
                text_strong: hex(0xffffff),
                muted: hex(0x8e9297),
                faint: hex(0x5c5e66),
                accent: hex(0x5b6ef5),
                accent_text: Color32::WHITE,
                green: hex(0x23a55a),
                yellow: hex(0xf0b232),
                red: hex(0xf23f43),
                link: hex(0x00a8fc),
                mention_bg: Color32::from_rgba_unmultiplied(240, 178, 50, 22),
                mention_bar: hex(0xf0b232),
                code_bg: hex(0x16171a),
                divider: hex(0x222327),
                embed_bg: hex(0x111214),
                dark: true,
            },
            Theme::Light => Palette {
                rail: hex(0xe3e5e8),
                sidebar: hex(0xf2f3f5),
                chat: hex(0xffffff),
                floating: hex(0xffffff),
                input: hex(0xebedef),
                hover: Color32::from_rgba_unmultiplied(116, 127, 141, 40),
                selected: Color32::from_rgba_unmultiplied(116, 127, 141, 76),
                text: hex(0x313338),
                text_strong: hex(0x060607),
                muted: hex(0x5c5e66),
                faint: hex(0x80848e),
                accent: hex(0x4752c4),
                accent_text: Color32::WHITE,
                green: hex(0x1a8a4a),
                yellow: hex(0xc27c0e),
                red: hex(0xda373c),
                link: hex(0x006ce7),
                mention_bg: Color32::from_rgba_unmultiplied(240, 178, 50, 40),
                mention_bar: hex(0xc27c0e),
                code_bg: hex(0xf2f3f5),
                divider: hex(0xe1e2e4),
                embed_bg: hex(0xf2f3f5),
                dark: false,
            },
        }
    }

    pub fn status_color(&self, s: dn_core::Status) -> Color32 {
        match s {
            dn_core::Status::Online => self.green,
            dn_core::Status::Idle => self.yellow,
            dn_core::Status::Dnd => self.red,
            dn_core::Status::Invisible | dn_core::Status::Offline => self.faint,
        }
    }

    pub fn role_color(&self, c: Option<u32>) -> Color32 {
        c.map(rgb).unwrap_or(self.text_strong)
    }
}

pub const fn hex(v: u32) -> Color32 {
    Color32::from_rgb((v >> 16) as u8, (v >> 8) as u8, v as u8)
}

pub fn rgb(v: u32) -> Color32 {
    hex(v & 0xff_ffff)
}

pub const BOLD: &str = "bold";
pub const ITALIC: &str = "italic";
pub const BOLD_ITALIC: &str = "bold-italic";

pub fn bold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(BOLD.into()))
}

pub fn regular(size: f32) -> FontId {
    FontId::new(size, FontFamily::Proportional)
}

pub fn mono(size: f32) -> FontId {
    FontId::new(size, FontFamily::Monospace)
}

/// Install visuals + text styles for the palette.
pub fn apply(ctx: &egui::Context, p: &Palette, font_size: f32, reduce_motion: bool) {
    let mut v = if p.dark { Visuals::dark() } else { Visuals::light() };
    v.panel_fill = p.chat;
    v.window_fill = p.floating;
    v.extreme_bg_color = p.input;
    v.faint_bg_color = p.sidebar;
    v.code_bg_color = p.code_bg;
    v.hyperlink_color = p.link;
    v.override_text_color = Some(p.text);
    v.window_stroke = Stroke::new(1.0, p.divider);
    v.window_corner_radius = CornerRadius::same(8);
    v.menu_corner_radius = CornerRadius::same(6);
    v.selection.bg_fill = p.accent.gamma_multiply(0.6);
    v.selection.stroke = Stroke::new(1.0, p.text_strong);
    let w = &mut v.widgets;
    w.noninteractive.bg_fill = p.sidebar;
    w.noninteractive.weak_bg_fill = p.sidebar;
    w.noninteractive.bg_stroke = Stroke::new(1.0, p.divider);
    w.noninteractive.fg_stroke = Stroke::new(1.0, p.text);
    w.inactive.bg_fill = p.input;
    w.inactive.weak_bg_fill = p.input;
    w.inactive.fg_stroke = Stroke::new(1.0, p.text);
    w.inactive.corner_radius = CornerRadius::same(4);
    w.hovered.bg_fill = p.hover;
    w.hovered.weak_bg_fill = p.hover;
    w.hovered.fg_stroke = Stroke::new(1.0, p.text_strong);
    w.hovered.corner_radius = CornerRadius::same(4);
    w.active.bg_fill = p.selected;
    w.active.weak_bg_fill = p.selected;
    w.active.fg_stroke = Stroke::new(1.0, p.text_strong);
    w.active.corner_radius = CornerRadius::same(4);
    w.open.weak_bg_fill = p.selected;
    ctx.set_visuals(v);

    let s = font_size.clamp(10.0, 24.0);
    ctx.all_styles_mut(|style| {
        style.text_styles = [
            (TextStyle::Small, FontId::new(s * 0.8, FontFamily::Proportional)),
            (TextStyle::Body, FontId::new(s, FontFamily::Proportional)),
            (TextStyle::Button, FontId::new(s * 0.93, FontFamily::Proportional)),
            (TextStyle::Heading, FontId::new(s * 1.33, FontFamily::Name(BOLD.into()))),
            (TextStyle::Monospace, FontId::new(s * 0.87, FontFamily::Monospace)),
        ]
        .into();
        style.spacing.item_spacing = egui::vec2(8.0, 4.0);
        style.spacing.button_padding = egui::vec2(10.0, 5.0);
        style.spacing.scroll.bar_width = 8.0;
        style.spacing.scroll.floating = true;
        style.animation_time = if reduce_motion { 0.0 } else { 0.12 };
        style.interaction.selectable_labels = true;
        style.url_in_tooltip = true;
    });
}
