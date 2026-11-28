// SPDX-License-Identifier: GPL-3.0-or-later
//! The App's health ui code (part of the StowLock window).
use super::*;

impl App {
    /// Health → Your tools: what the family's checks use, and one Install.
    pub(super) fn tools_card(&mut self, ui: &mut egui::Ui) {
        if self.tools.is_none() && !self.tools_busy {
            self.tools = Some(Vec::new()); // surveying
            self.survey_tools();
        }
        let tools = self.tools.clone().unwrap_or_default();
        ui::card(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(ui::semibold("Your tools", 15.0));
                if ui.small_button("Check again").clicked() {
                    self.survey_tools();
                }
            });
            ui.label(ui::muted("What checks use on this computer. Missing ones mean some checks can't run here (GitHub still runs them).").size(12.5));
            if tools.is_empty() {
                ui.label(ui::muted("Looking…"));
                return;
            }
            egui::Grid::new("tools")
                .num_columns(3)
                .spacing([14.0, 4.0])
                .show(ui, |ui| {
                    for t in &tools {
                        ui.label(t.name);
                        match (&t.version, t.other_fix) {
                            (Some(v), _) => ui.label(
                                RichText::new(format!("{} {v}", ui::icon::CHECK)).color(SUCCESS()),
                            ),
                            (None, Some(fix)) => {
                                ui.label(RichText::new(format!("needs: {fix}")).color(WARNING()))
                            }
                            (None, None) => ui.label(RichText::new("missing").color(WARNING())),
                        };
                        ui.label(ui::muted(t.why).size(12.5));
                        ui.end_row();
                    }
                });
            let missing = crate::tools::missing_packages(&tools);
            ui.horizontal_wrapped(|ui| {
                self.install_button(ui, &missing, false);
                if let Some(fix) = tools
                    .iter()
                    .find_map(|t| t.version.is_none().then_some(t.other_fix).flatten())
                {
                    ui.label(ui::mono(fix));
                    if ui.small_button("Copy").clicked() {
                        ui.ctx().copy_text(fix.to_string());
                    }
                }
            });
        });
        ui.add_space(8.0);
    }

    pub(super) fn health_screen(&mut self, ui: &mut egui::Ui) {
        let mut run = false;
        ui::title(ui, "Health", &self.health.summary());
        ui.add_space(8.0);
        // The self-test first: its button and its results in one place.
        let testing = self.busy.as_deref() == Some("Running self-test");
        ui::card_active(ui, testing, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label(ui::semibold("Self-test", 15.0));
                if testing {
                    ui.spinner();
                    ui.label(ui::muted("Running… about a minute."));
                }
            });
            ui.add(egui::Label::new(ui::muted("Tries every part of StowLock in a sandbox. It never touches your projects and never writes to GitHub.").size(12.5)).wrap());
            ui.add_space(4.0);
            ui.horizontal_wrapped(|ui| {
                if ui
                    .add_enabled(
                        self.busy.is_none(),
                        egui::Button::new(RichText::new("Run self-test").strong()),
                    )
                    .clicked()
                {
                    run = true;
                }
                ui::switch(
                    ui,
                    &mut self.selftest_online,
                    "Include read-only GitHub checks",
                );
            });
            if let Some((r, path)) = &self.last_selftest {
                ui.add_space(6.0);
                let (_, failed, _) = r.counts();
                ui.horizontal_wrapped(|ui| {
                    if failed == 0 {
                        ui::badge(ui, "Self-test passed", SUCCESS());
                    } else {
                        ui::badge(ui, "Self-test found problems", ERROR());
                    }
                    ui.add(egui::Label::new(RichText::new(r.summary_line()).strong()).wrap());
                });
                for c in r
                    .checks
                    .iter()
                    .filter(|c| c.outcome != selftest::Outcome::Pass)
                {
                    let (word, col) = if c.outcome == selftest::Outcome::Fail {
                        ("Failed", ERROR())
                    } else {
                        ("Skipped", MUTED())
                    };
                    ui.add(
                        egui::Label::new(
                            RichText::new(format!("{word}: {} {} — {}", c.id, c.name, c.detail))
                                .color(col)
                                .size(12.5),
                        )
                        .wrap(),
                    );
                }
                ui.horizontal_wrapped(|ui| {
                    if ui.button("Copy summary").clicked() {
                        let text = report::redact(&r.to_markdown(), &self.redact_ctx());
                        ui.ctx().copy_text(text);
                    }
                    if failed > 0 && ui.button("Copy AI fix prompt").clicked() {
                        ui.ctx()
                            .copy_text(report::redact(&r.ai_prompts(), &self.redact_ctx()));
                    }
                    if let Some(p) = path {
                        if ui.button("Open report").clicked() {
                            let _ = proc::xdg_open(&p.display().to_string());
                        }
                    }
                });
            }
        });
        ui.add_space(8.0);
        self.tools_card(ui);
        ui.label(ui::semibold("From your own use", 15.0));
        ui.add(egui::Label::new(ui::muted("How each part did the last time you used it. \"Needs attention\" means the last try didn't finish; it clears when that part next works.").size(12.5)).wrap());
        ui.add_space(4.0);
        let cols = if ui.available_width() > 900.0 {
            4
        } else if ui.available_width() > 560.0 {
            3
        } else {
            2
        };
        let cw = (ui.available_width() - 10.0 * (cols as f32 - 1.0)) / cols as f32;
        egui::Grid::new("health")
            .num_columns(cols)
            .spacing([10.0, 10.0])
            .min_col_width(cw)
            .max_col_width(cw)
            .show(ui, |ui| {
                for (i, a) in health::AREAS.iter().enumerate() {
                    let s = self.health.get(a);
                    let col = match s.status {
                        Health::Working => SUCCESS(),
                        Health::NeedsAttention => WARNING(),
                        Health::Broken => ERROR(),
                        Health::NotUsed => BORDER(),
                    };
                    ui::card(ui, |ui| {
                        ui.vertical(|ui| {
                            ui.horizontal(|ui| {
                                let (r, _) = ui.allocate_exact_size(
                                    egui::vec2(10.0, 10.0),
                                    egui::Sense::hover(),
                                );
                                ui.painter().circle_filled(r.center(), 4.0, col);
                                ui.add(egui::Label::new(ui::semibold(*a, 14.0)).truncate());
                            });
                            let note = if s.status == Health::NotUsed {
                                "Not used yet".to_string()
                            } else if s.hint.is_empty() {
                                format!("{} · {}", s.status.label(), s.checked_at)
                            } else {
                                format!("{} · {}", s.status.label(), s.hint)
                            };
                            ui.add(egui::Label::new(ui::muted(&note).size(12.5)).truncate())
                                .on_hover_text(format!("{note}\n{} · {}", s.source, s.checked_at));
                        })
                    });
                    if (i + 1) % cols == 0 {
                        ui.end_row();
                    }
                }
            });
        if run {
            self.run_selftest();
        }
    }
}
