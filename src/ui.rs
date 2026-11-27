// SPDX-License-Identifier: GPL-3.0-or-later
//! Look and feel: House Standard v1.3 light and dark palettes (navy hull,
//! emerald accent), the Inter font,
//! Lucide icons, cards, badges, buttons, segmented switches, the step
//! tracker and toasts.
#![allow(non_snake_case)]

use crate::logbook::{Step, StepStatus};
use eframe::egui::{
    self, Color32, CornerRadius, FontFamily, FontId, Margin, RichText, Stroke, Ui, Vec2,
};
use std::sync::atomic::{AtomicBool, Ordering};

// ------------------------------------------------------------------ palette

struct Pal {
    bg: Color32,
    side: Color32,
    card: Color32,
    control: Color32,
    hover: Color32,
    border: Color32,
    text: Color32,
    muted: Color32,
    accent: Color32,
    accent_btn: Color32,
    on_accent: Color32,
    accent_soft: Color32,
    success: Color32,
    warning: Color32,
    error: Color32,
}

const fn rgb(hex: u32) -> Color32 {
    Color32::from_rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

/// House Standard v1.3, dark: a navy hull with StowLock's emerald. Filled
/// buttons carry ink text. Every text colour meets 4.5:1 contrast on every
/// background it's used on (the tests below check).
const DARK: Pal = Pal {
    bg: rgb(0x0C1016),
    side: rgb(0x080B10),
    card: rgb(0x131922),
    control: rgb(0x1A212C),
    hover: rgb(0x212A37),
    border: rgb(0x263040),
    text: rgb(0xE8EDF4),
    muted: rgb(0x9AA6B8),
    accent: rgb(0x34D399),
    accent_btn: rgb(0x34D399),
    on_accent: rgb(0x0C1016),
    accent_soft: rgb(0x10302A),
    success: rgb(0x6EE7B7),
    warning: rgb(0xFBBF24),
    error: rgb(0xF87171),
};

/// House Standard v1.3, light.
const LIGHT: Pal = Pal {
    bg: rgb(0xF5F7FA),
    side: rgb(0xEDF1F5),
    card: rgb(0xFFFFFF),
    control: rgb(0xF1F4F8),
    hover: rgb(0xE9EEF4),
    border: rgb(0xDCE3EB),
    text: rgb(0x0F1620),
    muted: rgb(0x556274),
    accent: rgb(0x047857),
    accent_btn: rgb(0x047857),
    on_accent: rgb(0xFFFFFF),
    accent_soft: rgb(0xDCF3EA),
    success: rgb(0x047857),
    warning: rgb(0x975A06),
    error: rgb(0xB91C1C),
};

static IS_LIGHT: AtomicBool = AtomicBool::new(false);

fn pal() -> &'static Pal {
    if IS_LIGHT.load(Ordering::Relaxed) {
        &LIGHT
    } else {
        &DARK
    }
}

pub fn is_light() -> bool {
    IS_LIGHT.load(Ordering::Relaxed)
}

pub fn BG() -> Color32 {
    pal().bg
}
pub fn SIDE() -> Color32 {
    pal().side
}
pub fn CARD() -> Color32 {
    pal().card
}
pub fn CONTROL() -> Color32 {
    pal().control
}
pub fn BORDER() -> Color32 {
    pal().border
}
pub fn TEXT() -> Color32 {
    pal().text
}
pub fn MUTED() -> Color32 {
    pal().muted
}
pub fn ACCENT() -> Color32 {
    pal().accent
}
pub fn ACCENT_BTN() -> Color32 {
    pal().accent_btn
}
pub fn ON_ACCENT() -> Color32 {
    pal().on_accent
}
pub fn ACCENT_SOFT() -> Color32 {
    pal().accent_soft
}
pub fn SUCCESS() -> Color32 {
    pal().success
}
pub fn WARNING() -> Color32 {
    pal().warning
}
pub fn ERROR() -> Color32 {
    pal().error
}

/// Name a widget for screen readers (and the click-through tests) when the
/// label beside it isn't tied to it.
pub fn named(resp: egui::Response, name: &str) -> egui::Response {
    resp.ctx
        .accesskit_node_builder(resp.id, |n| n.set_label(name));
    resp
}

// ------------------------------------------------------------------ icons

/// Lucide icon glyphs (from the bundled, trimmed icon font). Not every
/// glyph in the font is used on screen yet.
#[allow(dead_code)]
pub mod icon {
    pub const UPLOAD: &str = "\u{e19e}";
    pub const LAYERS: &str = "\u{e529}";
    pub const HISTORY: &str = "\u{e1f5}";
    pub const ACTIVITY: &str = "\u{e038}";
    pub const BUG: &str = "\u{e20c}";
    pub const SLIDERS: &str = "\u{e29a}";
    pub const CHECK: &str = "\u{e06c}";
    pub const PACKAGE: &str = "\u{e129}";
    pub const FOLDER: &str = "\u{e0d7}";
    pub const X: &str = "\u{e1b2}";
    pub const CIRCLE_HELP: &str = "\u{e082}";
    pub const KEYBOARD: &str = "\u{e284}";
    pub const CIRCLE_CHECK: &str = "\u{e226}";
    pub const CIRCLE_ALERT: &str = "\u{e077}";
    pub const TRIANGLE_ALERT: &str = "\u{e193}";
    pub const REFRESH: &str = "\u{e145}";
    pub const EXTERNAL: &str = "\u{e0b9}";
    pub const COPY: &str = "\u{e09e}";
    pub const DOWNLOAD: &str = "\u{e0b2}";
    pub const SAVE: &str = "\u{e14d}";
    pub const TRASH: &str = "\u{e18e}";
    pub const ROTATE_CCW: &str = "\u{e148}";
    pub const FOLDER_OPEN: &str = "\u{e247}";
    pub const PLAY: &str = "\u{e13c}";
    pub const TERMINAL: &str = "\u{e181}";
    pub const INFO: &str = "\u{e0f9}";
    pub const ARROW_UP: &str = "\u{e07b}";
    pub const SHIELD: &str = "\u{e1ff}";
    pub const TAG: &str = "\u{e17f}";
    pub const BRANCH: &str = "\u{e0e2}";
    pub const ARCHIVE: &str = "\u{e041}";
    pub const USER: &str = "\u{e19f}";
    pub const CHEVRON: &str = "\u{e06f}";
    pub const PLUS: &str = "\u{e13d}";
    pub const UNDO: &str = "\u{e2a1}";
    pub const SUN: &str = "\u{e178}";
    pub const MOON: &str = "\u{e11e}";
    pub const MONITOR: &str = "\u{e11d}";
    pub const FILE_TEXT: &str = "\u{e0cc}";
    pub const CIRCLE: &str = "\u{e076}";
    pub const CIRCLE_X: &str = "\u{e084}";
    pub const PEN: &str = "\u{e172}";
    pub const EYE_OFF: &str = "\u{e0bb}";
    pub const LOG_OUT: &str = "\u{e10e}";
}

// ------------------------------------------------------------------ fonts

/// Inter for text (with egui's own fonts as fallback) and the Lucide icon
/// font as a fallback in every family, so an icon can sit inside any text.
pub fn install_fonts(ctx: &egui::Context) {
    let mut f = egui::FontDefinitions::default();
    f.font_data.insert(
        "inter".into(),
        std::sync::Arc::new(egui::FontData::from_static(include_bytes!(
            "../assets/fonts/Inter-Regular.ttf"
        ))),
    );
    f.font_data.insert(
        "inter-semibold".into(),
        std::sync::Arc::new(egui::FontData::from_static(include_bytes!(
            "../assets/fonts/Inter-SemiBold.ttf"
        ))),
    );
    f.font_data.insert(
        "lucide".into(),
        std::sync::Arc::new(egui::FontData::from_static(include_bytes!(
            "../assets/fonts/lucide.ttf"
        ))),
    );
    let prop = f.families.entry(FontFamily::Proportional).or_default();
    prop.insert(0, "inter".into());
    prop.push("lucide".into());
    f.families
        .entry(FontFamily::Monospace)
        .or_default()
        .push("lucide".into());
    let mut semi = vec!["inter-semibold".to_string()];
    semi.extend(
        f.families
            .get(&FontFamily::Proportional)
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter(|n| n != "inter"),
    );
    f.families.insert(FontFamily::Name("semibold".into()), semi);
    ctx.set_fonts(f);
}

/// Text in Inter SemiBold.
pub fn semibold(text: impl Into<String>, size: f32) -> RichText {
    RichText::new(text.into())
        .family(FontFamily::Name("semibold".into()))
        .size(size)
}

/// A screen's title.
pub fn title(ui: &mut Ui, text: &str, subtitle: &str) {
    ui.label(semibold(text, 24.0).color(TEXT()));
    if !subtitle.is_empty() {
        ui.add_space(-4.0);
        ui.label(RichText::new(subtitle).size(14.0).color(MUTED()));
    }
}

// ------------------------------------------------------------------ theme

/// Apply the light or dark palette. Cheap; call when the theme changes.
pub fn apply_theme(ctx: &egui::Context, light: bool) {
    IS_LIGHT.store(light, Ordering::Relaxed);
    let p = pal();
    let mut v = if light {
        egui::Visuals::light()
    } else {
        egui::Visuals::dark()
    };
    v.panel_fill = p.bg;
    v.window_fill = p.card;
    v.window_stroke = Stroke::new(1.0_f32, p.border);
    v.window_shadow = egui::epaint::Shadow {
        offset: [0, 8],
        blur: 24,
        spread: 0,
        color: Color32::from_black_alpha(if light { 30 } else { 90 }),
    };
    v.extreme_bg_color = p.control;
    v.faint_bg_color = p.control;
    v.code_bg_color = p.control;
    v.override_text_color = Some(p.text);
    v.hyperlink_color = p.accent;
    v.selection.bg_fill = p.accent_btn.linear_multiply(0.35);
    v.selection.stroke = Stroke::new(1.0_f32, p.accent);
    v.window_corner_radius = CornerRadius::same(16);
    v.window_stroke = Stroke::new(1.0_f32, p.accent.gamma_multiply(0.18));
    v.menu_corner_radius = CornerRadius::same(8);
    for w in [
        &mut v.widgets.noninteractive,
        &mut v.widgets.inactive,
        &mut v.widgets.hovered,
        &mut v.widgets.active,
        &mut v.widgets.open,
    ] {
        w.corner_radius = CornerRadius::same(8);
    }
    v.widgets.noninteractive.bg_fill = p.card;
    v.widgets.noninteractive.weak_bg_fill = p.card;
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, p.border);
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0_f32, p.text);
    v.widgets.inactive.bg_fill = p.control;
    v.widgets.inactive.weak_bg_fill = p.control;
    v.widgets.inactive.bg_stroke = Stroke::new(1.0_f32, p.border);
    v.widgets.inactive.fg_stroke = Stroke::new(1.0_f32, p.text);
    v.widgets.hovered.bg_fill = p.hover;
    v.widgets.hovered.weak_bg_fill = p.hover;
    v.widgets.hovered.bg_stroke = Stroke::new(1.0_f32, p.muted);
    v.widgets.hovered.fg_stroke = Stroke::new(1.0_f32, p.text);
    v.widgets.active.bg_fill = p.hover;
    v.widgets.active.weak_bg_fill = p.hover;
    v.widgets.active.bg_stroke = Stroke::new(1.0_f32, p.accent);
    v.widgets.active.fg_stroke = Stroke::new(1.0_f32, p.text);
    v.widgets.open.bg_fill = p.hover;
    v.widgets.open.weak_bg_fill = p.hover;
    ctx.set_visuals(v);
    ctx.all_styles_mut(|s| {
        s.spacing.item_spacing = Vec2::new(8.0, 8.0);
        s.spacing.button_padding = Vec2::new(14.0, 7.0);
        s.spacing.interact_size.y = 34.0;
        s.spacing.window_margin = Margin::same(20);
        s.text_styles.insert(
            egui::TextStyle::Heading,
            FontId::new(20.0, FontFamily::Name("semibold".into())),
        );
        s.text_styles
            .insert(egui::TextStyle::Body, FontId::proportional(14.5));
        s.text_styles
            .insert(egui::TextStyle::Button, FontId::proportional(14.5));
        s.text_styles
            .insert(egui::TextStyle::Small, FontId::proportional(12.5));
        s.text_styles
            .insert(egui::TextStyle::Monospace, FontId::monospace(13.0));
    });
}

// ------------------------------------------------------------------ pieces

/// A rounded card with generous padding.
pub fn card<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    card_active(ui, false, add)
}

/// The accent at a given strength (0–1), for edges and glows.
pub fn accent_alpha(strength: f32) -> Color32 {
    ACCENT().gamma_multiply(strength)
}

/// A card. Its edge is the accent at low strength (like the logo's edge);
/// when it's the active card, or holds the focused control, small accent
/// brackets mark its corners (the family's sci-fi detail).
pub fn card_active<R>(ui: &mut Ui, active: bool, add: impl FnOnce(&mut Ui) -> R) -> R {
    let out = egui::Frame::new()
        .fill(CARD())
        .stroke(Stroke::new(1.0_f32, accent_alpha(0.18)))
        .corner_radius(CornerRadius::same(16))
        .inner_margin(Margin::same(20))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui)
        });
    let rect = out.response.rect;
    let ctx = ui.ctx().clone();
    let focused_inside = ctx
        .memory(|m| m.focused())
        .and_then(|id| ctx.read_response(id))
        .is_some_and(|r| rect.contains_rect(r.rect));
    if active || focused_inside {
        corner_brackets(ui.painter(), rect);
    }
    out.inner
}

/// Four small L-shaped brackets just inside a card's rounded corners.
fn corner_brackets(p: &egui::Painter, r: egui::Rect) {
    let s = Stroke::new(2.0_f32, ACCENT());
    let (inset, len) = (5.0, 14.0);
    let r = r.shrink(inset);
    for (corner, dx, dy) in [
        (r.left_top(), 1.0, 1.0),
        (r.right_top(), -1.0, 1.0),
        (r.left_bottom(), 1.0, -1.0),
        (r.right_bottom(), -1.0, -1.0),
    ] {
        p.line_segment([corner, corner + Vec2::new(dx * len, 0.0)], s);
        p.line_segment([corner, corner + Vec2::new(0.0, dy * len)], s);
    }
}

/// A quieter inset panel inside a card.
pub fn inset<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    egui::Frame::new()
        .fill(BG())
        .stroke(Stroke::new(1.0_f32, BORDER()))
        .corner_radius(CornerRadius::same(12))
        .inner_margin(Margin::symmetric(14, 12))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui)
        })
        .inner
}

/// A tinted banner (info / warning / error / success).
pub fn banner<R>(ui: &mut Ui, color: Color32, add: impl FnOnce(&mut Ui) -> R) -> R {
    egui::Frame::new()
        .fill(color.linear_multiply(if is_light() { 0.08 } else { 0.12 }))
        .stroke(Stroke::new(1.0_f32, color.linear_multiply(0.55)))
        .corner_radius(CornerRadius::same(12))
        .inner_margin(Margin::symmetric(16, 12))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui)
        })
        .inner
}

/// The small uppercase label at the top of a card.
pub fn section_title(ui: &mut Ui, text: &str) {
    ui.label(semibold(text.to_uppercase(), 12.0).color(MUTED()));
}

/// A small pill such as NEW REPO / UPDATE / BETA.
pub fn badge(ui: &mut Ui, text: &str, color: Color32) {
    egui::Frame::new()
        .fill(color.linear_multiply(if is_light() { 0.12 } else { 0.18 }))
        .corner_radius(CornerRadius::same(255))
        .inner_margin(Margin::symmetric(10, 3))
        .show(ui, |ui| {
            ui.label(semibold(text, 12.0).color(color));
        });
}

pub fn mono(text: impl Into<String>) -> RichText {
    RichText::new(text.into()).monospace()
}

pub fn muted(text: impl Into<String>) -> RichText {
    RichText::new(text.into()).color(MUTED())
}

/// Icon + label, for buttons.
pub fn with_icon(icon: &str, text: &str) -> String {
    if icon.is_empty() {
        text.to_string()
    } else {
        format!("{icon}  {text}")
    }
}

/// The main action button (filled emerald).
pub fn primary(ui: &mut Ui, text: &str, enabled: bool) -> egui::Response {
    let btn =
        egui::Button::new(semibold(text, 14.5).color(if enabled { ON_ACCENT() } else { MUTED() }))
            .fill(if enabled { ACCENT_BTN() } else { CONTROL() })
            .stroke(Stroke::NONE)
            .corner_radius(CornerRadius::same(8))
            .min_size(Vec2::new(150.0, 40.0));
    ui.add_enabled(enabled, btn)
}

/// The window's single main action: a primary button with a soft accent
/// glow (only ever one at a time).
pub fn primary_glow(ui: &mut Ui, text: &str, enabled: bool) -> egui::Response {
    let behind = ui.painter().add(egui::Shape::Noop);
    let r = primary(ui, text, enabled);
    if enabled {
        let glow = egui::epaint::Shadow {
            offset: [0, 0],
            blur: 12,
            spread: 0,
            color: accent_alpha(0.35),
        };
        ui.painter()
            .set(behind, glow.as_shape(r.rect, CornerRadius::same(8)));
    }
    r
}

/// A normal outlined button with an optional icon.
pub fn button(ui: &mut Ui, icon: &str, text: &str) -> egui::Response {
    ui.add(
        egui::Button::new(semibold(with_icon(icon, text), 14.0).color(TEXT()))
            .min_size(Vec2::new(0.0, 36.0)),
    )
}

/// Same as [`button`] but can be disabled.
pub fn button_enabled(ui: &mut Ui, enabled: bool, icon: &str, text: &str) -> egui::Response {
    ui.add_enabled(
        enabled,
        egui::Button::new(semibold(with_icon(icon, text), 14.0)).min_size(Vec2::new(0.0, 36.0)),
    )
}

/// A segmented switch. Returns the index picked this frame, if any.
pub fn segmented(ui: &mut Ui, options: &[&str], selected: usize, width: f32) -> Option<usize> {
    let mut picked = None;
    egui::Frame::new()
        .fill(BG())
        .stroke(Stroke::new(1.0_f32, BORDER()))
        .corner_radius(CornerRadius::same(8))
        .inner_margin(Margin::same(3))
        .show(ui, |ui| {
            ui.set_width(width - 8.0);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                let each = (ui.available_width() - 2.0 * (options.len() as f32 - 1.0))
                    / options.len() as f32;
                for (i, o) in options.iter().enumerate() {
                    let on = i == selected;
                    let b = egui::Button::new(semibold(*o, 13.0).color(if on {
                        TEXT()
                    } else {
                        MUTED()
                    }))
                    .fill(if on { CARD() } else { Color32::TRANSPARENT })
                    .stroke(if on {
                        Stroke::new(1.0_f32, BORDER())
                    } else {
                        Stroke::NONE
                    })
                    .corner_radius(CornerRadius::same(8))
                    .min_size(Vec2::new(each, 30.0));
                    if ui.add(b).clicked() && !on {
                        picked = Some(i);
                    }
                }
            });
        });
    picked
}

/// A sidebar entry: icon, label, optional count badge.
pub fn nav_item(
    ui: &mut Ui,
    icon: &str,
    label: &str,
    on: bool,
    badge: Option<&str>,
    compact: bool,
) -> egui::Response {
    let fill = if on {
        ACCENT_SOFT()
    } else {
        Color32::TRANSPARENT
    };
    let color = if on { ACCENT() } else { MUTED() };
    let w = ui.available_width();
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, 40.0), egui::Sense::click());
    let hovered = resp.hovered();
    let painter = ui.painter_at(rect);
    painter.rect_filled(
        rect,
        CornerRadius::same(8),
        if hovered && !on { pal().hover } else { fill },
    );
    let c = if hovered { TEXT() } else { color };
    let c = if on { ACCENT() } else { c };
    if compact {
        painter.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            icon,
            FontId::proportional(18.0),
            c,
        );
    } else {
        painter.text(
            egui::pos2(rect.left() + 14.0, rect.center().y),
            egui::Align2::LEFT_CENTER,
            icon,
            FontId::proportional(17.0),
            c,
        );
        painter.text(
            egui::pos2(rect.left() + 44.0, rect.center().y),
            egui::Align2::LEFT_CENTER,
            label,
            FontId::new(14.5, FontFamily::Name("semibold".into())),
            c,
        );
        if let Some(b) = badge {
            let galley =
                painter.layout_no_wrap(b.to_string(), FontId::proportional(11.5), ACCENT());
            let bw = galley.size().x + 14.0;
            let br = egui::Rect::from_center_size(
                egui::pos2(rect.right() - 10.0 - bw / 2.0, rect.center().y),
                Vec2::new(bw, 20.0),
            );
            painter.rect_filled(br, CornerRadius::same(255), ACCENT_SOFT());
            painter.galley(br.center() - galley.size() / 2.0, galley, ACCENT());
        }
    }
    // A real, named button for screen readers (and the click-through tests).
    resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::Button, true, on, label));
    let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
    if compact {
        resp.on_hover_text(label)
    } else {
        resp
    }
}

fn duration(ms: u128) -> String {
    if ms >= 1000 {
        format!("{:.1} s", ms as f64 / 1000.0)
    } else {
        format!("{ms} ms")
    }
}

/// The step tracker: one tile per step, in a grid (3 across when there's
/// room), each with a status icon, the step's name and a short detail.
pub fn stepper(ui: &mut Ui, names: &[&str], steps: &[Step], waiting: &[&str]) {
    let cols = if ui.available_width() > 640.0 {
        3
    } else if ui.available_width() > 400.0 {
        2
    } else {
        1
    };
    egui::Grid::new(("stepper", names.len()))
        .num_columns(cols)
        .spacing([10.0, 10.0])
        .min_col_width((ui.available_width() - 10.0 * (cols as f32 - 1.0)) / cols as f32)
        .max_col_width((ui.available_width() - 10.0 * (cols as f32 - 1.0)) / cols as f32)
        .show(ui, |ui| {
            for (i, name) in names.iter().enumerate() {
                let st = steps.iter().find(|s| s.name == *name);
                inset(ui, |ui| {
                    ui.horizontal(|ui| {
                        let (r, _) =
                            ui.allocate_exact_size(Vec2::splat(26.0), egui::Sense::hover());
                        let p = ui.painter_at(r);
                        match st.map(|s| s.status) {
                            Some(StepStatus::Passed) => {
                                p.circle_filled(r.center(), 13.0, ACCENT_BTN());
                                p.text(
                                    r.center(),
                                    egui::Align2::CENTER_CENTER,
                                    icon::CHECK,
                                    FontId::proportional(14.0),
                                    ON_ACCENT(),
                                );
                            }
                            Some(StepStatus::Failed) => {
                                p.circle_filled(r.center(), 13.0, ERROR());
                                p.text(
                                    r.center(),
                                    egui::Align2::CENTER_CENTER,
                                    icon::X,
                                    FontId::proportional(14.0),
                                    CARD(),
                                );
                            }
                            Some(StepStatus::Skipped) => {
                                p.circle_stroke(r.center(), 11.0, Stroke::new(2.0_f32, BORDER()));
                                p.text(
                                    r.center(),
                                    egui::Align2::CENTER_CENTER,
                                    "–",
                                    FontId::proportional(13.0),
                                    MUTED(),
                                );
                            }
                            Some(StepStatus::Running) => {
                                ui.put(r, egui::Spinner::new().size(20.0).color(ACCENT()));
                            }
                            None => {
                                p.circle_stroke(r.center(), 11.0, Stroke::new(2.0_f32, BORDER()));
                            }
                        }
                        ui.vertical(|ui| {
                            ui.spacing_mut().item_spacing.y = 2.0;
                            ui.add(egui::Label::new(semibold(*name, 14.0)).truncate());
                            let detail = match st {
                                Some(s) if s.status == StepStatus::Running => {
                                    "Running…".to_string()
                                }
                                Some(s) => {
                                    let d = s.detail.lines().next().unwrap_or("").trim();
                                    let t = if s.millis > 0 {
                                        duration(s.millis)
                                    } else {
                                        String::new()
                                    };
                                    match (d.is_empty(), t.is_empty()) {
                                        (true, _) => t,
                                        (false, true) => d.to_string(),
                                        (false, false) => format!("{d} · {t}"),
                                    }
                                }
                                None => waiting.get(i).copied().unwrap_or("Waiting").to_string(),
                            };
                            ui.add(
                                egui::Label::new(RichText::new(detail).size(12.5).color(MUTED()))
                                    .truncate(),
                            );
                        });
                    });
                });
                if (i + 1) % cols == 0 {
                    ui.end_row();
                }
            }
        });
}

/// Draw a toast in the bottom-right corner. Returns false once it's gone.
pub fn toast(ctx: &egui::Context, text: &str, age_secs: f32, bottom_gap: f32) -> bool {
    const SHOW: f32 = 4.0;
    if age_secs > SHOW {
        return false;
    }
    let alpha = ((SHOW - age_secs) / 0.4).clamp(0.0, 1.0);
    egui::Area::new(egui::Id::unique("toast"))
        .order(egui::Order::Foreground)
        .anchor(egui::Align2::RIGHT_BOTTOM, Vec2::new(-24.0, -bottom_gap))
        .interactable(false)
        .show(ctx, |ui| {
            ui.set_opacity(alpha);
            egui::Frame::new()
                .fill(CARD())
                .stroke(Stroke::new(1.0_f32, BORDER()))
                .corner_radius(CornerRadius::same(12))
                .inner_margin(Margin::symmetric(16, 12))
                .shadow(egui::epaint::Shadow {
                    offset: [0, 8],
                    blur: 24,
                    spread: 0,
                    color: Color32::from_black_alpha(if is_light() { 30 } else { 110 }),
                })
                .show(ui, |ui| {
                    ui.set_max_width(380.0);
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(icon::CIRCLE_CHECK).size(18.0).color(ACCENT()));
                        ui.add(egui::Label::new(RichText::new(text).size(14.0)).wrap());
                    });
                });
        });
    ctx.request_repaint();
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lum(c: Color32) -> f64 {
        let f = |v: u8| {
            let c = v as f64 / 255.0;
            if c <= 0.03928 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * f(c.r()) + 0.7152 * f(c.g()) + 0.0722 * f(c.b())
    }

    fn contrast(a: Color32, b: Color32) -> f64 {
        let (x, y) = (lum(a), lum(b));
        (x.max(y) + 0.05) / (x.min(y) + 0.05)
    }

    /// House Standard v1.3: every text colour on every surface, both themes
    /// (text ≥ 12.3:1, muted ≥ 5.0:1, accent and status ≥ 4.5:1), plus the
    /// ink or white on filled accent buttons.
    #[test]
    fn every_text_colour_is_readable_in_both_themes() {
        for (name, p) in [("dark", &DARK), ("light", &LIGHT)] {
            for (fg, fg_name, min) in [
                (p.text, "text", 12.3),
                (p.muted, "muted", 5.0),
                (p.accent, "accent", 4.5),
                (p.success, "success", 4.5),
                (p.warning, "warning", 4.5),
                (p.error, "error", 4.5),
            ] {
                for (bg, bg_name) in [
                    (p.bg, "bg"),
                    (p.side, "sidebar"),
                    (p.card, "card"),
                    (p.control, "control"),
                    (p.hover, "hover"),
                ] {
                    let c = contrast(fg, bg);
                    assert!(c >= min, "{name}: {fg_name} on {bg_name} is {c:.2}:1");
                }
            }
            let button = if name == "dark" { 7.2 } else { 5.1 };
            let c = contrast(p.on_accent, p.accent_btn);
            assert!(c >= button, "{name}: filled button text is {c:.2}:1");
            assert!(
                contrast(p.accent, p.accent_soft) >= 4.5,
                "{name}: selected nav item"
            );
        }
    }
}

#[cfg(test)]
mod glyph_tests {
    use super::*;

    /// Copy buttons need the window toolkit's clipboard support, which is a
    /// separate switch since egui 0.37 (without it, Copy silently does nothing).
    #[test]
    fn the_clipboard_is_switched_on() {
        let toml = include_str!("../Cargo.toml");
        let line = toml
            .lines()
            .find(|l| l.starts_with("eframe"))
            .expect("eframe in Cargo.toml");
        for feature in ["\"clipboard\"", "\"links\""] {
            assert!(line.contains(feature), "eframe needs {feature}: {line}");
        }
    }

    /// Every symbol the app writes can be drawn by its fonts (a missing one
    /// shows as an empty box).
    #[test]
    fn every_symbol_in_the_app_has_a_glyph() {
        let mut chars = std::collections::BTreeSet::new();
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut dirs = vec![src];
        while let Some(d) = dirs.pop() {
            for e in std::fs::read_dir(d).unwrap().flatten() {
                let p = e.path();
                if p.is_dir() {
                    dirs.push(p);
                } else if p.extension().is_some_and(|x| x == "rs") {
                    // Only what the app draws: not comments, the tests at the
                    // end of each file, or the README tree (never on screen).
                    for line in std::fs::read_to_string(&p).unwrap().lines() {
                        if line.starts_with("#[cfg(test)]") {
                            break;
                        }
                        if line.trim_start().starts_with("//") {
                            continue;
                        }
                        chars.extend(line.chars().filter(|c| !c.is_ascii()));
                    }
                }
            }
        }
        let ctx = egui::Context::default();
        install_fonts(&ctx);
        ctx.run_ui(egui::RawInput::default(), |_| {})
            .textures_delta
            .clear();
        // Box lines for the README's file tree, and the "✕" a test runner
        // prints (only matched, never drawn).
        for c in "─│└├✕".chars() {
            chars.remove(&c);
        }
        let missing: Vec<char> = ctx.fonts_mut(|f| {
            chars
                .iter()
                .copied()
                .filter(|c| {
                    !f.has_glyph(&egui::FontId::proportional(14.0), *c)
                        || !f.has_glyph(&egui::FontId::monospace(14.0), *c)
                })
                .collect()
        });
        assert!(missing.is_empty(), "no glyph for: {missing:?}");
    }
}
