// SPDX-License-Identifier: GPL-3.0-or-later
//! The App's projects code (part of the StowLock window).
use super::*;

impl App {
    pub(super) fn projects_tab(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(ui::muted(format!(
                "{} project{} in {}",
                self.projects.len(),
                if self.projects.len() == 1 { "" } else { "s" },
                util::tilde(&self.config.workspace())
            )));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("↻ Refresh").clicked() {
                    self.refresh_projects();
                    self.refresh_project_owners(true);
                }
                if ui.button("Open archive").clicked() {
                    let _ = std::fs::create_dir_all(self.config.archive_root());
                    let _ = proc::xdg_open(&self.config.archive_root().display().to_string());
                }
            });
        });
        // GitHub's build of the last push, while it runs.
        if let Some(w) = self.build.clone().filter(|w| {
            !w.dismissed && matches!(w.status, None | Some(crate::github::Build::Running { .. }))
        }) {
            let left = self.build_countdown();
            let mut show = false;
            ui::card_active(ui, true, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(ui::icon::TIMER).size(18.0).color(ACCENT()));
                    ui.label(ui::semibold(
                        match &left {
                            Some(l) => format!("GitHub is building {} · {l} left, about", w.slug),
                            None => format!("GitHub is building {}", w.slug),
                        },
                        14.0,
                    ));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        show |= ui.button("Show").clicked();
                    });
                });
            });
            if show {
                self.tab = Tab::Stage;
            }
            ui.add_space(4.0);
        }
        let today = self.history.today();
        if !today.is_empty() {
            ui::card(ui, |ui| {
                ui::section_title(ui, "PUSHED TODAY");
                ui.horizontal_wrapped(|ui| {
                    for r in &today {
                        if ui
                            .button(RichText::new(r.chip()).monospace().color(
                                if r.channel == "beta" {
                                    WARNING()
                                } else {
                                    SUCCESS()
                                },
                            ))
                            .clicked()
                        {
                            let _ = proc::xdg_open(&r.github_url);
                        }
                    }
                });
            });
        }
        if self.projects.is_empty() {
            ui.add_space(20.0);
            ui.vertical_centered(|ui| {
                ui.label(ui::muted(
                    "No projects yet. Stage a zip to create your first one.",
                ))
            });
            return;
        }
        let mut load: Option<ProjectInfo> = None;
        let mut trash: Option<ProjectInfo> = None;
        let mut keep: Option<String> = None;
        let saved = ProjectsState::load(&self.paths);
        for p in self.projects.clone() {
            let gone = self.gone_from_github(&p) && !saved.get(&p.slug).keep_without_github;
            ui::card(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.add(
                        egui::Label::new(ui::mono(&p.slug).strong().size(15.0).color(TEXT()))
                            .truncate(),
                    )
                    .on_hover_text(&p.slug);
                    ui::badge(ui, p.stack.name(), ACCENT());
                    let col = if p.badge.starts_with("Beta") {
                        WARNING()
                    } else if p.badge.starts_with("Pushed") || p.badge == "On GitHub" {
                        SUCCESS()
                    } else if p.badge == "Local changes" {
                        ERROR()
                    } else {
                        MUTED()
                    };
                    if gone {
                        ui::badge(ui, "Not on GitHub anymore", ERROR());
                    } else {
                        ui::badge(ui, &p.badge, col);
                    }
                });
                if gone {
                    ui.add(
                        egui::Label::new(ui::muted(
                            "Its repo was deleted on GitHub. The folder and its history are still on this computer.",
                        ))
                        .wrap(),
                    );
                    ui.horizontal_wrapped(|ui| {
                        if self.confirm_trash_project.as_deref() == Some(p.slug.as_str()) {
                            if ui
                                .add(
                                    egui::Button::new(
                                        ui::semibold(format!("Move {} to the Trash", p.slug), 13.5)
                                            .color(ui::CARD()),
                                    )
                                    .fill(ERROR()),
                                )
                                .clicked()
                            {
                                trash = Some(p.clone());
                            }
                            if ui.button("Cancel").clicked() {
                                self.confirm_trash_project = None;
                            }
                        } else if ui::button(ui, ui::icon::TRASH, "Remove the folder…")
                            .on_hover_text("Moves it to the Trash, so it can be restored")
                            .clicked()
                        {
                            self.confirm_trash_project = Some(p.slug.clone());
                        }
                        if ui::button_enabled(
                            ui,
                            self.busy.is_none(),
                            ui::icon::UPLOAD,
                            "Push it again as a new repo",
                        )
                        .on_hover_text(
                            "Creates the repo on GitHub again, with this folder's history",
                        )
                        .clicked()
                        {
                            load = Some(p.clone());
                        }
                        if ui
                            .button("Keep it")
                            .on_hover_text("Stop flagging this folder")
                            .clicked()
                        {
                            keep = Some(p.slug.clone());
                        }
                    });
                }
                ui.horizontal_wrapped(|ui| {
                    if let Some(t) = &p.last_tag {
                        ui.label(ui::muted(format!("Latest tag {t}")));
                    }
                    if let Some(lp) = &p.last_push {
                        ui.label(ui::muted(format!("· Last push {lp}")));
                    }
                    ui.label(ui::muted(format!(
                        "· {} archived zip{}",
                        p.archives,
                        if p.archives == 1 { "" } else { "s" }
                    )));
                });
                ui.horizontal_wrapped(|ui| {
                    if ui.button("Open Folder").clicked() {
                        let _ = proc::xdg_open(&p.dir.display().to_string());
                    }
                    if let Some(url) = &p.remote {
                        if ui.button("Open on GitHub").clicked() {
                            let web = url
                                .trim_end_matches(".git")
                                .replace("git@github.com:", "https://github.com/");
                            let _ = proc::xdg_open(&web);
                        }
                    }
                    if p.archives > 0 && ui.button("Open Archive").clicked() {
                        let _ = proc::xdg_open(
                            &self
                                .config
                                .archive_root()
                                .join(&p.slug)
                                .display()
                                .to_string(),
                        );
                    }
                    if ui
                        .add_enabled(self.busy.is_none(), egui::Button::new("Load"))
                        .on_hover_text(
                            "Open in Stage & Push to try, re-check and push edits made by hand",
                        )
                        .clicked()
                    {
                        load = Some(p.clone());
                    }
                });
            });
            ui.add_space(4.0);
        }
        if let Some(p) = load {
            self.load_project(&p);
        }
        if let Some(p) = trash {
            self.confirm_trash_project = None;
            match self.pctx().trash(&p.dir) {
                Ok(_) => {
                    self.log
                        .info("projects", &format!("Moved {} to the Trash", p.slug));
                    self.toast(&format!("Moved {} to the Trash", p.slug));
                }
                Err(e) => self.fail(&e, "projects"),
            }
            self.refresh_projects();
        }
        if let Some(slug) = keep {
            let _ = ProjectsState::update(&self.paths, &slug, |s| s.keep_without_github = true);
        }
        self.attempts_card(ui);
    }

    /// Projects > Saved earlier attempts: stagings that were never pushed,
    /// with Restore and Delete (Delete asks first).
    pub(super) fn attempts_card(&mut self, ui: &mut egui::Ui) {
        if self.attempts.is_empty() {
            return;
        }
        ui.add_space(8.0);
        let mut action: Option<(usize, bool)> = None;
        ui::card(ui, |ui| {
            ui::section_title(ui, "Saved earlier attempts");
            ui.label(
                ui::muted("Stagings that never got pushed. Nothing is lost until you delete it.")
                    .size(13.0),
            );
            ui.add_space(4.0);
            for (i, a) in self.attempts.iter().enumerate() {
                ui::inset(ui, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.vertical(|ui| {
                            ui.set_max_width((ui.available_width() - 260.0).max(200.0));
                            ui.add(egui::Label::new(ui::semibold(&a.project, 14.0)).truncate());
                            ui.label(
                                ui::muted(format!("{} · {}", nice_stamp(&a.when), a.where_kept()))
                                    .size(12.5),
                            );
                        });
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let confirming = self.confirm_delete_attempt == Some(i);
                            if confirming {
                                if ui
                                    .add(
                                        egui::Button::new(
                                            ui::semibold("Delete it", 13.5).color(ui::CARD()),
                                        )
                                        .fill(ERROR()),
                                    )
                                    .clicked()
                                {
                                    action = Some((i, false));
                                }
                                if ui::button(ui, "", "Keep").clicked() {
                                    self.confirm_delete_attempt = None;
                                }
                            } else {
                                if ui::button(ui, ui::icon::TRASH, "Delete…").clicked() {
                                    self.confirm_delete_attempt = Some(i);
                                }
                                if ui::button(ui, ui::icon::ROTATE_CCW, "Restore").clicked() {
                                    action = Some((i, true));
                                }
                            }
                        });
                    });
                });
            }
        });
        if let Some((i, restore)) = action {
            self.confirm_delete_attempt = None;
            let a = self.attempts[i].clone();
            let ws = self.config.workspace();
            let r = if restore {
                crate::attempts::restore(&a, &ws)
            } else {
                let ctx = self.pctx();
                crate::attempts::delete(&a, |p| ctx.trash(p))
            };
            match r {
                Ok(msg) => {
                    self.log.info("attempts", &msg);
                    self.toast(&msg);
                }
                Err(e) => self.fail(&e, "attempts"),
            }
            self.refresh_attempts();
            self.refresh_projects();
        }
    }

    // ------------------------------------------------------------ windows

    pub(super) fn windows(&mut self, ctx: &egui::Context) {
        if self.show_uninstall {
            self.uninstall_window(ctx);
        }
    }

    pub(super) fn today_row(&mut self, ui: &mut egui::Ui, r: &PushRecord) {
        ui.horizontal_wrapped(|ui| {
            ui.label(ui::mono(r.time()).color(MUTED()));
            if ui.link(RichText::new(&r.slug).monospace()).clicked() {
                let _ = proc::xdg_open(&r.github_url);
            }
            if let Some(t) = &r.tag {
                ui::badge(
                    ui,
                    t,
                    if r.channel == "beta" {
                        WARNING()
                    } else {
                        SUCCESS()
                    },
                );
            }
            ui.label(ui::muted(format!("{} · {}", r.mode, r.visibility)));
            if let Some(t) = &r.tag {
                let watching = self
                    .build
                    .as_ref()
                    .filter(|w| w.slug == r.slug && w.tag.as_deref() == Some(t.as_str()));
                if let Some(w) = watching {
                    use crate::github::Build;
                    let (txt, col) = match &w.status {
                        None | Some(Build::Running { .. }) => ("building on GitHub…", MUTED()),
                        Some(Build::Passed { .. }) => ("built", ui::SUCCESS()),
                        Some(Build::Failed { .. }) => ("build failed", ERROR()),
                        Some(Build::None) => ("", MUTED()),
                    };
                    if !txt.is_empty() {
                        ui.label(RichText::new(txt).size(12.5).color(col));
                    }
                }
                if self.deleting_release.is_none() && ui.small_button("Delete release…").clicked()
                {
                    self.deleting_release =
                        Some((r.owner.clone(), r.slug.clone(), t.clone(), String::new()));
                }
            }
        });
        if let Some((owner, slug, tag, mut typed)) = self.deleting_release.clone() {
            if slug == r.slug && r.tag.as_deref() == Some(tag.as_str()) {
                ui::banner(ui, ERROR(), |ui| {
                    ui.add(egui::Label::new(format!("Delete the release {tag} and its tag from GitHub? Your code and commits stay. Type {tag} to confirm.")).wrap());
                    ui.horizontal_wrapped(|ui| {
                        ui::named(
                            ui.add(
                                egui::TextEdit::singleline(&mut typed)
                                    .desired_width(160.0)
                                    .hint_text(tag.as_str()),
                            ),
                            "Type the tag to confirm",
                        );
                        let ok = typed.trim() == tag;
                        if ui
                            .add_enabled(
                                ok,
                                egui::Button::new(
                                    ui::semibold("Delete release", 13.5).color(ui::CARD()),
                                )
                                .fill(ERROR()),
                            )
                            .clicked()
                        {
                            self.fix_release(
                                false,
                                owner.clone(),
                                slug.clone(),
                                tag.clone(),
                                false,
                            );
                        }
                        if ui::button(ui, "", "Cancel").clicked() {
                            self.deleting_release = None;
                        }
                    });
                });
                if let Some(d) = self.deleting_release.as_mut() {
                    d.3 = typed;
                }
            }
        }
    }
}
