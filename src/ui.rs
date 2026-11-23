// SPDX-License-Identifier: GPL-3.0-or-later
//! Look and feel: the dark slate theme, cards, badges, pills and the
//! pipeline stepper.

use crate::logbook::{Step, StepStatus};
use eframe::egui::{self, Color32, CornerRadius, Margin, RichText, Stroke, Ui, Vec2};

pub const BG: Color32 = Color32::from_rgb(0x11, 0x13, 0x18);
pub const CARD: Color32 = Color32::from_rgb(0x18, 0x1C, 0x24);
pub const CONTROL: Color32 = Color32::from_rgb(0x1F, 0x25, 0x30);
pub const BORDER: Color32 = Color32::from_rgb(0x2B, 0x32, 0x40);
pub const TEXT: Color32 = Color32::from_rgb(0xE6, 0xED, 0xF3);
pub const MUTED: Color32 = Color32::from_rgb(0x8B, 0x94, 0x9E);
pub const ACCENT: Color32 = Color32::from_rgb(0x38, 0xBD, 0xF8);
pub const SUCCESS: Color32 = Color32::from_rgb(0x34, 0xD3, 0x99);
pub const WARNING: Color32 = Color32::from_rgb(0xFB, 0xBF, 0x24);
pub const ERROR: Color32 = Color32::from_rgb(0xF8, 0x71, 0x71);

pub fn apply_theme(ctx: &egui::Context) {
    let mut v = egui::Visuals::dark();
    v.panel_fill = BG;
    v.window_fill = CARD;
    v.window_stroke = Stroke::new(1.0_f32, BORDER);
    v.extreme_bg_color = Color32::from_rgb(0x0C, 0x0E, 0x12);
    v.faint_bg_color = CONTROL;
    v.override_text_color = Some(TEXT);
    v.hyperlink_color = ACCENT;
    v.selection.bg_fill = ACCENT.linear_multiply(0.35);
    v.selection.stroke = Stroke::new(1.0_f32, ACCENT);
    v.window_corner_radius = CornerRadius::same(12);
    for w in [
        &mut v.widgets.inactive,
        &mut v.widgets.hovered,
        &mut v.widgets.active,
        &mut v.widgets.open,
    ] {
        w.corner_radius = CornerRadius::same(8);
    }
    v.widgets.noninteractive.bg_fill = CARD;
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, BORDER);
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0_f32, TEXT);
    v.widgets.inactive.bg_fill = CONTROL;
    v.widgets.inactive.weak_bg_fill = CONTROL;
    v.widgets.inactive.bg_stroke = Stroke::new(1.0_f32, BORDER);
    v.widgets.hovered.bg_fill = Color32::from_rgb(0x27, 0x2F, 0x3D);
    v.widgets.hovered.weak_bg_fill = Color32::from_rgb(0x27, 0x2F, 0x3D);
    v.widgets.hovered.bg_stroke = Stroke::new(1.0_f32, ACCENT.linear_multiply(0.6));
    v.widgets.active.bg_fill = Color32::from_rgb(0x2D, 0x37, 0x48);
    v.widgets.active.bg_stroke = Stroke::new(1.0_f32, ACCENT);
    ctx.set_visuals(v);
    ctx.style_mut(|s| {
        s.spacing.item_spacing = Vec2::new(8.0, 8.0);
        s.spacing.button_padding = Vec2::new(12.0, 6.0);
        s.spacing.interact_size.y = 30.0;
        s.text_styles
            .insert(egui::TextStyle::Heading, egui::FontId::proportional(20.0));
        s.text_styles
            .insert(egui::TextStyle::Body, egui::FontId::proportional(14.5));
        s.text_styles
            .insert(egui::TextStyle::Button, egui::FontId::proportional(14.5));
        s.text_styles
            .insert(egui::TextStyle::Monospace, egui::FontId::monospace(13.0));
    });
}

/// A rounded card with generous padding.
pub fn card<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    egui::Frame::new()
        .fill(CARD)
        .stroke(Stroke::new(1.0_f32, BORDER))
        .corner_radius(CornerRadius::same(12))
        .inner_margin(Margin::same(15))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui)
        })
        .inner
}

/// A tinted banner (info / warning / error / success).
pub fn banner<R>(ui: &mut Ui, color: Color32, add: impl FnOnce(&mut Ui) -> R) -> R {
    egui::Frame::new()
        .fill(color.linear_multiply(0.10))
        .stroke(Stroke::new(1.0_f32, color.linear_multiply(0.6)))
        .corner_radius(CornerRadius::same(10))
        .inner_margin(Margin::symmetric(14, 10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui)
        })
        .inner
}

pub fn section_title(ui: &mut Ui, text: &str) {
    ui.label(RichText::new(text).size(13.0).color(MUTED).strong());
}

/// A small filled badge such as NEW REPO / UPDATE / BETA.
pub fn badge(ui: &mut Ui, text: &str, color: Color32) {
    egui::Frame::new()
        .fill(color.linear_multiply(0.18))
        .stroke(Stroke::new(1.0_f32, color))
        .corner_radius(CornerRadius::same(6))
        .inner_margin(Margin::symmetric(8, 2))
        .show(ui, |ui| {
            ui.label(RichText::new(text).size(12.0).strong().color(color));
        });
}

pub fn mono(text: impl Into<String>) -> RichText {
    RichText::new(text.into()).monospace()
}

pub fn muted(text: impl Into<String>) -> RichText {
    RichText::new(text.into()).color(MUTED)
}

/// The big primary action button.
pub fn primary(ui: &mut Ui, text: &str, enabled: bool) -> egui::Response {
    let btn = egui::Button::new(RichText::new(text).strong().size(15.5).color(if enabled {
        BG
    } else {
        MUTED
    }))
    .fill(if enabled { ACCENT } else { CONTROL })
    .corner_radius(CornerRadius::same(10))
    .min_size(Vec2::new(170.0, 38.0));
    ui.add_enabled(enabled, btn)
}

pub fn status_color(s: StepStatus) -> Color32 {
    match s {
        StepStatus::Running => ACCENT,
        StepStatus::Passed => SUCCESS,
        StepStatus::Failed => ERROR,
        StepStatus::Skipped => MUTED,
    }
}

/// Connected pipeline stepper drawn with the painter.
pub fn stepper(ui: &mut Ui, names: &[&str], steps: &[Step]) {
    let width = ui.available_width();
    let height = 64.0;
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, height), egui::Sense::hover());
    let painter = ui.painter_at(rect);
    let n = names.len().max(1);
    let gap = width / n as f32;
    let y = rect.top() + 18.0;
    let status_of = |name: &str| steps.iter().find(|s| s.name == name);
    for (i, name) in names.iter().enumerate() {
        let x = rect.left() + gap * (i as f32 + 0.5);
        if i + 1 < n {
            let next_x = rect.left() + gap * (i as f32 + 1.5);
            let done = status_of(name)
                .map(|s| s.status == StepStatus::Passed || s.status == StepStatus::Skipped)
                .unwrap_or(false);
            painter.line_segment(
                [egui::pos2(x + 12.0, y), egui::pos2(next_x - 12.0, y)],
                Stroke::new(
                    2.0_f32,
                    if done {
                        SUCCESS.linear_multiply(0.7)
                    } else {
                        BORDER
                    },
                ),
            );
        }
        let st = status_of(name);
        let color = st.map(|s| status_color(s.status)).unwrap_or(BORDER);
        let filled = st.is_some();
        painter.circle(
            egui::pos2(x, y),
            10.0,
            if filled {
                color.linear_multiply(0.25)
            } else {
                CARD
            },
            Stroke::new(2.0_f32, color),
        );
        let glyph = match st.map(|s| s.status) {
            Some(StepStatus::Passed) => "✔",
            Some(StepStatus::Failed) => "✖",
            Some(StepStatus::Skipped) => "–",
            Some(StepStatus::Running) => "…",
            None => "",
        };
        if !glyph.is_empty() {
            painter.text(
                egui::pos2(x, y),
                egui::Align2::CENTER_CENTER,
                glyph,
                egui::FontId::proportional(11.0),
                color,
            );
        }
        painter.text(
            egui::pos2(x, y + 20.0),
            egui::Align2::CENTER_TOP,
            *name,
            egui::FontId::proportional(11.5),
            if filled { TEXT } else { MUTED },
        );
        if let Some(s) = st {
            if s.millis > 0 && s.status != StepStatus::Running {
                let t = if s.millis >= 1000 {
                    format!("{:.1}s", s.millis as f64 / 1000.0)
                } else {
                    format!("{}ms", s.millis)
                };
                painter.text(
                    egui::pos2(x, y + 36.0),
                    egui::Align2::CENTER_TOP,
                    t,
                    egui::FontId::monospace(10.5),
                    MUTED,
                );
            }
        }
    }
}
