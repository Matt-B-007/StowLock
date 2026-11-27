// SPDX-License-Identifier: GPL-3.0-or-later
//! The App's sidebar code (part of the StowLock window).
use super::*;

impl App {
    // ------------------------------------------------------------ sidebar

    pub(super) fn sidebar(&mut self, root: &mut egui::Ui) {
        let ctx = &root.ctx().clone();
        let compact = ctx.content_rect().width() < 820.0;
        let logo = self
            .logo
            .get_or_insert_with(|| {
                ctx.load_texture(
                    "stowlock-logo",
                    egui::ColorImage::from_rgba_unmultiplied([64, 64], &crate::icon::rgba(64)),
                    egui::TextureOptions::LINEAR,
                )
            })
            .clone();
        egui::Panel::left("nav")
            .resizable(false)
            .exact_size(if compact { 68.0 } else { 236.0 })
            .frame(
                egui::Frame::new()
                    .fill(ui::SIDE())
                    .inner_margin(egui::Margin::symmetric(12, 18))
                    .stroke(egui::Stroke::new(1.0_f32, BORDER())),
            )
            .show(root, |ui| {
                ui.horizontal(|ui| {
                    ui.add_space(6.0);
                    ui.add(egui::Image::new(&logo).fit_to_exact_size(egui::vec2(30.0, 30.0)));
                    if !compact {
                        ui.vertical(|ui| {
                            ui.spacing_mut().item_spacing.y = 0.0;
                            ui.label(ui::semibold("StowLock", 16.0));
                            ui.label(ui::muted(format!("v{APP_VERSION}")).size(12.0));
                        });
                    }
                });
                ui.add_space(18.0);
                let today = self.history.today().len();
                let (broken, attn) = self.health.counts();
                let saved = self.attempts.len();
                let update = matches!(self.update, UpdateState::Available(_));
                let items: [(Tab, &str, &str, Option<String>); 6] = [
                    (Tab::Stage, ui::icon::UPLOAD, "Stage", None),
                    (
                        Tab::Projects,
                        ui::icon::LAYERS,
                        "Projects",
                        (saved > 0).then(|| format!("{saved} saved")),
                    ),
                    (
                        Tab::History,
                        ui::icon::HISTORY,
                        "History",
                        (today > 0).then(|| format!("{today} today")),
                    ),
                    (
                        Tab::Health,
                        ui::icon::ACTIVITY,
                        "Health",
                        (broken + attn > 0).then(|| format!("{}", broken + attn)),
                    ),
                    (Tab::Report, ui::icon::BUG, "Bug report", None),
                    (
                        Tab::Settings,
                        ui::icon::SLIDERS,
                        "Settings",
                        update.then(|| "Update".to_string()),
                    ),
                ];
                ui.spacing_mut().item_spacing.y = 4.0;
                for (tab, icon, label, badge) in items {
                    if ui::nav_item(ui, icon, label, self.tab == tab, badge.as_deref(), compact)
                        .clicked()
                    {
                        if tab == Tab::Report {
                            self.open_bug_report();
                        } else if tab == Tab::Projects {
                            self.refresh_attempts();
                        }
                        self.tab = tab;
                    }
                }
                ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
                    let signed_in = self.env.login.clone();
                    let dot = if signed_in.is_some() {
                        ui::SUCCESS()
                    } else {
                        WARNING()
                    };
                    if compact {
                        let label = signed_in.clone().unwrap_or_else(|| "Not signed in".into());
                        if ui::nav_item(ui, ui::icon::USER, &label, false, None, true).clicked() {
                            self.tab = Tab::Settings;
                        }
                        self.help_menu(ui, true);
                        return;
                    }
                    let resp = egui::Frame::new()
                        .fill(ui::CARD())
                        .stroke(egui::Stroke::new(1.0_f32, BORDER()))
                        .corner_radius(egui::CornerRadius::same(12))
                        .inner_margin(egui::Margin::same(if compact { 8 } else { 12 }))
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            if compact {
                                ui.vertical_centered(|ui| {
                                    ui.label(RichText::new(ui::icon::USER).size(18.0).color(dot));
                                });
                            } else {
                                ui.horizontal(|ui| {
                                    let (r, _) = ui.allocate_exact_size(
                                        egui::vec2(10.0, 10.0),
                                        egui::Sense::hover(),
                                    );
                                    ui.painter().circle_filled(r.center(), 4.0, dot);
                                    ui.add(
                                        egui::Label::new(ui::semibold(
                                            signed_in
                                                .clone()
                                                .unwrap_or_else(|| "Not signed in".into()),
                                            13.0,
                                        ))
                                        .truncate(),
                                    );
                                });
                                ui.label(
                                    ui::muted(if self.config.beta_updates() {
                                        "Beta updates"
                                    } else {
                                        "Stable updates"
                                    })
                                    .size(12.0),
                                );
                            }
                        })
                        .response
                        .interact(egui::Sense::click())
                        .on_hover_cursor(egui::CursorIcon::PointingHand)
                        .on_hover_text("Account and updates");
                    if resp.clicked() {
                        self.tab = Tab::Settings;
                    }
                    ui.add_space(4.0);
                    self.help_menu(ui, false);
                });
            });
    }

    // ------------------------------------------------------------ history

    pub(super) fn history_screen(&mut self, ui: &mut egui::Ui) {
        ui::title(ui, "History", "Every push and archive, newest first.");
        ui.add_space(8.0);
        let mut pushes = self.history.pushes.clone();
        pushes.reverse();
        if pushes.is_empty() {
            ui::card(ui, |ui| {
                ui.vertical_centered(|ui| {
                    ui.add_space(16.0);
                    ui.label(RichText::new(ui::icon::HISTORY).size(30.0).color(MUTED()));
                    ui.label(ui::semibold("Nothing pushed yet", 16.0));
                    ui.label(ui::muted(
                        "Pushes and archives show up here, so you can see what went where.",
                    ));
                    ui.add_space(16.0);
                });
            });
            return;
        }
        let mut last_date = String::new();
        ui::card(ui, |ui| {
            for r in pushes.iter().take(200) {
                if r.date() != last_date {
                    last_date = r.date().to_string();
                    if ui.cursor().top() > ui.min_rect().top() + 30.0 {
                        ui.add_space(6.0);
                    }
                    ui::section_title(ui, &last_date);
                }
                self.today_row(ui, r);
            }
        });
    }

    // ------------------------------------------------------------ settings parts

    pub(super) fn appearance_and_updates(&mut self, ui: &mut egui::Ui) {
        ui::card(ui, |ui| {
            ui::section_title(ui, "Updates");
            ui.horizontal_wrapped(|ui| {
                ui.vertical(|ui| {
                    ui.set_max_width((ui.available_width() - 250.0).max(220.0));
                    ui.label(ui::semibold("Update channel", 14.5));
                    ui.add(egui::Label::new(ui::muted(if self.config.beta_updates() {
                        "Beta: every newer version, betas included. Never offers an older one."
                    } else {
                        "Stable: full releases only. Betas are skipped."
                    }).size(13.0)).wrap());
                });
                let sel = usize::from(self.config.beta_updates());
                if let Some(i) = ui::segmented(ui, &["Stable", "Beta"], sel, 220.0) {
                    self.config.update_channel = if i == 1 {
                        "beta".into()
                    } else {
                        "stable".into()
                    };
                    if matches!(self.update, UpdateState::Available(_)) {
                        self.update = UpdateState::None;
                    }
                }
            });
            ui.add_space(6.0);
            ui.horizontal_wrapped(|ui| {
                ui.checkbox(
                    &mut self.config.check_for_updates,
                    "Check for updates when StowLock opens",
                );
                if ui::button_enabled(
                    ui,
                    self.pacman_pkg.is_none(),
                    ui::icon::REFRESH,
                    "Check now",
                )
                .clicked()
                {
                    self.check_updates(true);
                }
            });
            ui.horizontal_wrapped(|ui| {
                let line = match &self.rust {
                    Some(r) => r.line(),
                    None => "Rust: not checked yet".into(),
                };
                ui.label(ui::muted(line).size(13.0));
                if ui
                    .add_enabled(!self.rust_busy, egui::Button::new("Check Rust").small())
                    .clicked()
                {
                    self.rust_later = false;
                    self.check_rust();
                }
                if matches!(
                    self.rust,
                    Some(crate::rustcheck::RustState::UpdateAvailable { .. })
                        | Some(crate::rustcheck::RustState::NoDefault)
                ) && ui
                    .add_enabled(!self.rust_busy, egui::Button::new("Update Rust").small())
                    .clicked()
                {
                    self.update_rust();
                }
            });
            ui.checkbox(
                &mut self.config.rust_update_check,
                "Check for a newer Rust once a day (rustup)",
            );
            ui.checkbox(
                &mut self.config.desktop_notifications,
                "Desktop notifications when a push or GitHub's build finishes (while StowLock isn't in front)",
            );
            match &self.update {
                UpdateState::Available(rel) => {
                    let label = rel.label();
                    let tag = rel.tag.clone();
                    ui::banner(ui, ACCENT(), |ui| {
                        ui.horizontal_wrapped(|ui| {
                            ui.label(format!("StowLock {label} is available."));
                            if self.pacman_pkg.is_none()
                                && ui::primary(ui, &format!("Update to {label}"), true).clicked()
                            {
                                self.apply_update(tag.clone());
                            }
                        });
                    });
                }
                _ => {
                    if let Some(n) = &self.update_note {
                        ui.label(ui::muted(n).size(13.0));
                    }
                }
            }
            if let Some(pkg) = &self.pacman_pkg {
                ui::inset(ui, |ui| {
                    ui.add(egui::Label::new(format!("Installed from the AUR ({pkg}). Updates come through your AUR helper, for example paru or yay.")).wrap());
                });
            } else if let Some(prev) = updater::previous_version(&self.paths) {
                ui.horizontal_wrapped(|ui| {
                    ui.label(ui::muted(format!(
                        "Version {prev} is kept from before the last update."
                    )));
                    if ui::button(ui, ui::icon::UNDO, &format!("Go back to {prev}")).clicked() {
                        self.start_rollback();
                    }
                });
            }
        });
        ui.add_space(8.0);
        ui::card(ui, |ui| {
            ui::section_title(ui, "Appearance");
            let sel = match self.config.theme.as_str() {
                "light" => 1,
                "dark" => 2,
                _ => 0,
            };
            if let Some(i) = ui::segmented(ui, &["Follow system", "Light", "Dark"], sel, 340.0) {
                self.config.theme = ["", "light", "dark"][i].to_string();
            }
        });
    }
}
