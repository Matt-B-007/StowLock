// SPDX-License-Identifier: GPL-3.0-or-later
//! The App's report ui code (part of the StowLock window).
use super::*;

impl App {
    // ------------------------------------------------------------ bug report

    pub(super) fn report_screen(&mut self, ui: &mut egui::Ui) {
        let mut rebuild = false;
        let mut download = false;
        ui.horizontal_wrapped(|ui| {
            ui.vertical(|ui| {
                ui::title(
                    ui,
                    "Bug report",
                    "Everything needed to fix a problem, with your personal details hidden.",
                );
            });
        });
        ui.add_space(8.0);
        // Side by side from the default window size up; stacked below that,
        // with the form first.
        let wide = ui.available_width() >= 720.0;
        let left_w = if !wide {
            ui.available_width()
        } else if ui.available_width() >= 900.0 {
            340.0
        } else {
            300.0
        };
        let mut left = |this: &mut Self, ui: &mut egui::Ui| {
            ui::card(ui, |ui| {
                ui::section_title(ui, "What happened?");
                ui.label(ui::muted("Optional. A sentence or two helps a lot.").size(13.0));
                rebuild |= ui
                    .add(
                        egui::TextEdit::multiline(&mut this.bug.what)
                            .hint_text("What were you doing?")
                            .desired_rows(3)
                            .desired_width(f32::INFINITY),
                    )
                    .changed();
                rebuild |= ui
                    .add(
                        egui::TextEdit::multiline(&mut this.bug.expected)
                            .hint_text("What did you expect to happen?")
                            .desired_rows(2)
                            .desired_width(f32::INFINITY),
                    )
                    .changed();
            });
            ui.add_space(8.0);
            ui::card(ui, |ui| {
                ui::section_title(ui, "Include");
                for sec in REPORT_SECTIONS {
                    let mut on = !this.excluded(sec);
                    if ui.checkbox(&mut on, sec).changed() {
                        if on {
                            this.bug.excluded.retain(|x| x != sec);
                        } else {
                            this.bug.excluded.push(sec.to_string());
                        }
                        rebuild = true;
                    }
                }
                ui.label(
                    ui::muted("System details and StowLock's version are always included.")
                        .size(12.5),
                );
            });
            ui.add_space(8.0);
            ui::card(ui, |ui| {
                ui::section_title(ui, "Hidden before it leaves your computer");
                let hidden = count_hidden(&this.bug.preview);
                ui.horizontal(|ui| {
                    ui.label(ui::semibold(hidden.to_string(), 28.0).color(ACCENT()));
                    ui.label(ui::muted(if hidden == 1 {
                        "detail hidden"
                    } else {
                        "details hidden"
                    }));
                });
                for line in [
                    "Your username, home folder and computer name",
                    "GitHub account, organisations and repo names",
                    "Project names and file names, emails, IP and MAC addresses",
                    "Tokens, keys and anything that looks like a secret",
                ] {
                    ui.label(ui::muted(format!("•  {line}")).size(13.0));
                }
                ui.add_space(6.0);
                ui.label(ui::semibold("Also hide these words", 13.5));
                let mut remove: Option<usize> = None;
                ui.horizontal_wrapped(|ui| {
                    for (i, w) in this.config.report_hide_words.iter().enumerate() {
                        if ui
                            .add(
                                egui::Button::new(
                                    RichText::new(format!("{w}  {}", ui::icon::X)).size(12.5),
                                )
                                .corner_radius(egui::CornerRadius::same(255)),
                            )
                            .on_hover_text("Stop hiding this word")
                            .clicked()
                        {
                            remove = Some(i);
                        }
                    }
                });
                if let Some(i) = remove {
                    this.config.report_hide_words.remove(i);
                    rebuild = true;
                }
                let r = ui::named(
                    ui.add(
                        egui::TextEdit::singleline(&mut this.bug.hide_input)
                            .hint_text("Type a word and press Enter")
                            .desired_width(f32::INFINITY),
                    ),
                    "A word to hide",
                );
                if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    let w = this.bug.hide_input.trim().to_string();
                    if !w.is_empty() && !this.config.report_hide_words.contains(&w) {
                        this.config.report_hide_words.push(w);
                        rebuild = true;
                    }
                    this.bug.hide_input.clear();
                    r.request_focus();
                }
                ui.horizontal_wrapped(|ui| {
                    rebuild |= ui
                        .checkbox(&mut this.bug.include_public, "Show public repo names")
                        .changed();
                    rebuild |= ui
                        .checkbox(&mut this.bug.include_username, "Show my GitHub username")
                        .changed();
                });
                ui.add(egui::Label::new(ui::muted("Nothing is ever sent automatically. The report only leaves your computer when you copy, save or download it. Your typed note is hidden the same way.").size(12.5)).wrap());
            });
        };
        let mut right = |this: &mut Self, ui: &mut egui::Ui| {
            ui::card(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.add(
                        egui::Label::new(
                            ui::mono(report::bug_report_file_name(&this.bug.created))
                                .color(MUTED()),
                        )
                        .truncate(),
                    );
                });
                ui.label(ui::muted("Preview: exactly what gets shared.").size(12.5));
                egui::Frame::new()
                    .fill(ui::BG())
                    .corner_radius(egui::CornerRadius::same(8))
                    .inner_margin(egui::Margin::same(12))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        egui::ScrollArea::both()
                            .max_height(440.0)
                            .id_salt("bugprev")
                            .show(ui, |ui| {
                                let mut p = this.bug.preview.clone();
                                let w = ui.available_width();
                                ui::named(
                                    ui.add(
                                        egui::TextEdit::multiline(&mut p)
                                            .code_editor()
                                            .frame(egui::Frame::NONE)
                                            .desired_width(w),
                                    ),
                                    "Bug report preview",
                                );
                            });
                    });
                ui.add_space(4.0);
                ui.horizontal_wrapped(|ui| {
                    // Save and Copy are the main actions; the public GitHub
                    // issue comes last, with a warning before it opens.
                    if ui::primary(
                        ui,
                        &ui::with_icon(ui::icon::DOWNLOAD, "Save to file…"),
                        !this.bug.saving,
                    )
                    .on_hover_text("Save the report as a .txt file")
                    .clicked()
                    {
                        download = true;
                    }
                    if ui::primary(ui, &ui::with_icon(ui::icon::COPY, "Copy report"), true)
                        .on_hover_text("Copy the report, private things already hidden")
                        .clicked()
                    {
                        ui.ctx().copy_text(this.bug.preview.clone());
                        this.toast("Copied. Paste it into a GitHub issue or an AI chat.");
                    }
                    if ui::primary(
                        ui,
                        &ui::with_icon(ui::icon::FOLDER, "Open log folder"),
                        true,
                    )
                    .on_hover_text("StowLock's log and saved reports")
                    .clicked()
                    {
                        let _ = std::fs::create_dir_all(this.paths.reports_dir());
                        let _ = proc::xdg_open(&this.paths.state_dir.display().to_string());
                    }
                    if ui::button(ui, ui::icon::SAVE, "Keep a copy")
                        .on_hover_text("Keep a copy in StowLock's reports folder")
                        .clicked()
                    {
                        match report::save_report(&this.paths, "bugreport", &this.bug.preview) {
                            Ok(p) => this.toast(&format!("Saved {}", util::tilde(&p))),
                            Err(e) => this.fail(&e, "report"),
                        }
                    }
                    if ui::button(ui, ui::icon::EXTERNAL, "Open on GitHub…")
                        .on_hover_text("Opens a public GitHub issue (asks first)")
                        .clicked()
                    {
                        this.bug.confirm_issue = true;
                        this.bug.issue_private = None;
                    }
                    if this.bug.saving {
                        ui.spinner();
                    }
                });
                if let Some(p) = this.bug.saved_to.clone() {
                    ui::banner(ui, ui::SUCCESS(), |ui| {
                        ui.add(
                            egui::Label::new(format!("Downloaded to {}", util::tilde(&p))).wrap(),
                        );
                        if ui::button(ui, ui::icon::FOLDER_OPEN, "Open folder").clicked() {
                            let folder = p
                                .parent()
                                .map(|f| f.display().to_string())
                                .unwrap_or_default();
                            let _ = proc::xdg_open(&folder);
                        }
                    });
                }
                if this.bug.confirm_issue {
                    this.confirm_issue_banner(ui);
                }
            });
            ui.add_space(8.0);
            ui::card(ui, |ui| {
                let crashes = std::fs::read_dir(this.paths.reports_dir())
                    .map(|rd| {
                        rd.flatten()
                            .filter(|e| e.file_name().to_string_lossy().starts_with("crash-"))
                            .count()
                    })
                    .unwrap_or(0);
                ui.label(ui::semibold("Crash reports", 14.5));
                ui.add(egui::Label::new(ui::muted(if crashes == 0 {
                    "No crashes recorded. If StowLock ever closes unexpectedly, a report with your details hidden is kept here for you to send.".to_string()
                } else {
                    format!("{crashes} crash report(s) kept, with your details hidden.")
                }).size(13.0)).wrap());
                if crashes > 0 && ui::button(ui, ui::icon::FOLDER_OPEN, "Open folder").clicked() {
                    let _ = proc::xdg_open(&this.paths.reports_dir().display().to_string());
                }
            });
        };
        if wide {
            ui.horizontal_top(|ui| {
                ui.vertical(|ui| {
                    ui.set_width(left_w);
                    left(self, ui);
                });
                ui.add_space(8.0);
                ui.vertical(|ui| {
                    right(self, ui);
                });
            });
        } else {
            left(self, ui);
            ui.add_space(8.0);
            right(self, ui);
        }
        if rebuild {
            self.bug.preview = self.build_bug_report();
        }
        if download {
            self.download_bug_report();
        }
    }
}
