//! Offline UI stress test: renders a large synthetic history with the same virtualization
//! strategy as the chat view and auto-scrolls through it while recording frame times.

use std::time::{Duration, Instant};

use egui::{RichText, Ui, vec2};

use dn_diagnostics::FrameStats;

use crate::theme::Palette;
use crate::virtual_list::{HeightCache, offsets, visible_range};

pub struct StressTest {
    messages: Vec<(String, String)>,
    heights: HeightCache,
    stats: FrameStats,
    /// CPU time spent building each frame (excludes tessellation and GPU work).
    build: FrameStats,
    started: Instant,
    last: Option<Instant>,
    offset: f32,
    done: Option<String>,
}

const WORDS: &[&str] = &[
    "the",
    "quick",
    "brown",
    "fox",
    "jumps",
    "over",
    "lazy",
    "dog",
    "**bold**",
    "_italic_",
    "`code`",
    "rust",
    "native",
    "egui",
    "discord",
    "message",
    "scrolling",
    "virtualized",
    "list",
    "https://example.com/page",
    "||spoiler||",
    "~~strike~~",
];

impl StressTest {
    pub fn new(n: usize) -> Self {
        // Deterministic xorshift so runs are comparable.
        let mut x: u64 = 0x2545_f491_4f6c_dd1d;
        let mut next = move || {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x
        };
        let messages = (0..n)
            .map(|i| {
                let words = 3 + (next() % 40) as usize;
                let text: Vec<&str> = (0..words).map(|_| WORDS[(next() % WORDS.len() as u64) as usize]).collect();
                (format!("user{}", i % 37), text.join(" "))
            })
            .collect();
        StressTest {
            messages,
            heights: HeightCache::default(),
            stats: FrameStats::new(100_000),
            build: FrameStats::new(100_000),
            started: Instant::now(),
            last: None,
            offset: 0.0,
            done: None,
        }
    }

    /// Returns false when the user closes the results.
    pub fn show(&mut self, ui: &mut Ui, p: &Palette) -> bool {
        let now = Instant::now();
        if let Some(prev) = self.last
            && self.done.is_none()
        {
            self.stats.record(now - prev);
        }
        self.last = Some(now);
        let mut keep = true;
        egui::CentralPanel::default().frame(egui::Frame::new().fill(p.chat)).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new(format!("Stress test: {} messages", self.messages.len())).strong().color(p.text_strong));
                if let Some(r) = &self.done {
                    ui.label(RichText::new(r).monospace().color(p.green));
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("Close").clicked() {
                        keep = false;
                    }
                });
            });
            let width = ui.available_width();
            self.heights.set_width(width);
            let heights: Vec<f32> = (0..self.messages.len()).map(|i| self.heights.get(i as u64, 0).unwrap_or(44.0)).collect();
            let offs = offsets(heights);
            let total = *offs.last().unwrap_or(&0.0);
            let mut measured = vec![];
            let out =
                egui::ScrollArea::vertical().auto_shrink([false, false]).vertical_scroll_offset(self.offset).show_viewport(ui, |ui, vp| {
                    ui.set_min_height(total);
                    let origin = ui.min_rect().min;
                    for i in visible_range(&offs, vp.min.y, vp.max.y) {
                        let rect = egui::Rect::from_min_size(origin + vec2(0.0, offs[i]), vec2(width, f32::INFINITY));
                        let mut child =
                            ui.new_child(egui::UiBuilder::new().max_rect(rect).layout(egui::Layout::top_down(egui::Align::Min)));
                        let (author, text) = &self.messages[i];
                        child.add_space(6.0);
                        child.label(RichText::new(author).strong().color(p.text_strong));
                        child.add(egui::Label::new(RichText::new(text).color(p.text)).wrap());
                        measured.push((i as u64, child.min_rect().height()));
                    }
                });
            for (k, h) in measured {
                self.heights.put(k, 0, h);
            }
            let max = (out.content_size.y - out.inner_rect.height()).max(0.0);
            if self.done.is_none() {
                // ~40 screens per second: far faster than a user scrolls.
                self.offset += out.inner_rect.height() * 0.66;
                if self.offset >= max || self.started.elapsed() > Duration::from_secs(60) {
                    let s = self.stats.summary();
                    let b = self.build.summary();
                    self.done = Some(format!(
                        "frame interval avg {:.2} ms · p95 {:.2} · p99 {:.2} · max {:.2} ({:.0} fps) | UI build avg {:.2} ms · p95 {:.2} · max {:.2} | {} frames over {:.1}s",
                        s.avg_ms,
                        s.p95_ms,
                        s.p99_ms,
                        s.max_ms,
                        s.fps,
                        b.avg_ms,
                        b.p95_ms,
                        b.max_ms,
                        self.stats.len(),
                        self.started.elapsed().as_secs_f32()
                    ));
                    tracing::info!(result = self.done.as_deref().unwrap_or(""), messages = self.messages.len(), "UI stress test finished");
                } else {
                    ui.ctx().request_repaint();
                }
            }
        });
        if self.done.is_none() {
            self.build.record(now.elapsed());
        }
        keep
    }

    pub fn result(&self) -> Option<&str> {
        self.done.as_deref()
    }
}
