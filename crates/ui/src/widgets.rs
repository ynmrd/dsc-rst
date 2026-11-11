//! Small reusable widgets.

use egui::{Color32, CornerRadius, FontId, Rect, Response, Sense, Stroke, Ui, Vec2, vec2};

use dn_core::Status;

use crate::textures::Textures;
use crate::theme::Palette;

/// Circular avatar with an optional status dot and speaking ring.
pub fn avatar(ui: &mut Ui, textures: &mut Textures, p: &Palette, url: &str, size: f32, status: Option<Status>, speaking: bool) -> Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(size), Sense::click());
    if ui.is_rect_visible(rect) {
        paint_avatar(ui, textures, p, url, rect, status, speaking);
    }
    resp
}

pub fn paint_avatar(ui: &Ui, textures: &mut Textures, p: &Palette, url: &str, rect: Rect, status: Option<Status>, speaking: bool) {
    let size = rect.width();
    let radius = CornerRadius::same((size / 2.0) as u8);
    match textures.get(url, (size * 2.0).max(32.0) as u32) {
        Some(tex) => {
            egui::Image::new((tex.id(), rect.size())).corner_radius(radius).paint_at(ui, rect);
        }
        None => {
            ui.painter().circle_filled(rect.center(), size / 2.0, p.hover);
        }
    }
    if speaking {
        ui.painter().circle_stroke(rect.center(), size / 2.0 + 1.0, Stroke::new(2.0, p.green));
    }
    if let Some(s) = status {
        let r = (size * 0.18).max(4.0);
        let c = rect.right_bottom() - vec2(r, r) + vec2(1.0, 1.0);
        ui.painter().circle_filled(c, r + 2.0, p.sidebar);
        ui.painter().circle_filled(c, r, p.status_color(s));
    }
}

/// Guild icon: image or initials, rounder when not selected (like the official rail).
pub fn guild_icon(
    ui: &mut Ui,
    textures: &mut Textures,
    p: &Palette,
    url: Option<&str>,
    initials: &str,
    selected: bool,
    dimmed: bool,
) -> Response {
    let size = 48.0;
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(size), Sense::click());
    let t = ui.ctx().animate_bool(resp.id, selected || resp.hovered());
    let radius = CornerRadius::same((24.0 - 8.0 * t) as u8);
    let tint = if dimmed { Color32::from_white_alpha(110) } else { Color32::WHITE };
    match url.and_then(|u| textures.get(u, 96)) {
        Some(tex) => {
            egui::Image::new((tex.id(), rect.size())).corner_radius(radius).tint(tint).paint_at(ui, rect);
        }
        None => {
            let fill = if selected || resp.hovered() { p.accent } else { p.sidebar };
            ui.painter().rect_filled(rect, radius, fill);
            let fs = if initials.chars().count() > 2 { 13.0 } else { 16.0 };
            ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, initials, FontId::proportional(fs), p.text_strong);
        }
    }
    resp
}

/// Red pill with a count (mentions).
pub fn count_badge(ui: &Ui, p: &Palette, anchor: egui::Pos2, count: u32) {
    let text = if count > 99 { "99+".to_string() } else { count.to_string() };
    let galley = ui.painter().layout_no_wrap(text, crate::theme::bold(11.0), Color32::WHITE);
    let w = (galley.size().x + 8.0).max(16.0);
    let rect = Rect::from_center_size(anchor, vec2(w, 16.0));
    ui.painter().rect(rect, CornerRadius::same(8), p.red, Stroke::new(3.0, p.rail), egui::StrokeKind::Outside);
    ui.painter().galley(rect.center() - galley.size() / 2.0, galley, Color32::WHITE);
}

/// Flat icon button that highlights on hover; `active` draws it in the accent/red colour.
pub fn icon_button(ui: &mut Ui, p: &Palette, icon: &str, tooltip: &str, active_color: Option<Color32>) -> Response {
    let size = vec2(32.0, 32.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    if resp.hovered() {
        ui.painter().rect_filled(rect, CornerRadius::same(4), p.hover);
    }
    let color = active_color.unwrap_or(if resp.hovered() { p.text_strong } else { p.muted });
    ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, icon, FontId::proportional(17.0), color);
    resp.on_hover_text(tooltip)
}

/// Primary (filled) button.
pub fn primary_button(ui: &mut Ui, p: &Palette, text: &str) -> Response {
    ui.add(
        egui::Button::new(egui::RichText::new(text).color(p.accent_text).strong())
            .fill(p.accent)
            .corner_radius(CornerRadius::same(4))
            .min_size(vec2(96.0, 34.0)),
    )
}

pub fn danger_button(ui: &mut Ui, p: &Palette, text: &str) -> Response {
    ui.add(
        egui::Button::new(egui::RichText::new(text).color(Color32::WHITE).strong())
            .fill(p.red)
            .corner_radius(CornerRadius::same(4))
            .min_size(vec2(96.0, 34.0)),
    )
}

/// Selectable full-width row used by the channel list, DMs and settings navigation.
pub fn nav_row(ui: &mut Ui, p: &Palette, selected: bool, height: f32, add: impl FnOnce(&mut Ui, bool)) -> Response {
    let width = ui.available_width();
    let (rect, resp) = ui.allocate_exact_size(vec2(width, height), Sense::click());
    if selected {
        ui.painter().rect_filled(rect, CornerRadius::same(4), p.selected);
    } else if resp.hovered() {
        ui.painter().rect_filled(rect, CornerRadius::same(4), p.hover);
    }
    let mut child = ui
        .new_child(egui::UiBuilder::new().max_rect(rect.shrink2(vec2(8.0, 0.0))).layout(egui::Layout::left_to_right(egui::Align::Center)));
    add(&mut child, resp.hovered());
    resp
}

pub fn section_label(ui: &mut Ui, p: &Palette, text: &str) {
    ui.label(egui::RichText::new(text.to_uppercase()).size(11.5).strong().color(p.muted));
}

pub fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = bytes as f64;
    let mut i = 0;
    while v >= 1024.0 && i < UNITS.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 { format!("{bytes} B") } else { format!("{v:.2} {}", UNITS[i]) }
}

#[cfg(test)]
mod tests {
    #[test]
    fn sizes() {
        assert_eq!(super::human_size(512), "512 B");
        assert_eq!(super::human_size(1536), "1.50 KB");
        assert_eq!(super::human_size(5 * 1024 * 1024), "5.00 MB");
    }
}
