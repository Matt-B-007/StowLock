// SPDX-License-Identifier: GPL-3.0-or-later
//! The App's after push code (part of the StowLock window).
use super::*;

impl App {
    /// The GitHub build card (and the failed-release card) after a push.
    pub(super) fn after_push_cards(&mut self, ui: &mut egui::Ui) {
        // The tag is on GitHub but the release failed.
        if let Some(out) = self.session.outcome.clone() {
            if let (Some(why), Some(tag)) = (out.release_failed.clone(), out.tag.clone()) {
                let (owner, slug, beta) = (
                    self.session.owner.clone(),
                    self.session.slug.clone(),
                    self.session.channel == Channel::Beta,
                );
                ui::banner(ui, WARNING(), |ui| {
                    ui.label(ui::semibold(
                        format!("The tag {tag} is on GitHub, but the release wasn't created"),
                        14.5,
                    ));
                    ui.add(
                        egui::Label::new(ui::muted(why.lines().next().unwrap_or("")).size(13.0))
                            .wrap(),
                    );
                    ui.horizontal_wrapped(|ui| {
                        if ui::primary(ui, "Finish the release", self.busy.is_none()).clicked() {
                            self.fix_release(true, owner.clone(), slug.clone(), tag.clone(), beta);
                        }
                        if self.confirm_remove_tag {
                            if ui
                                .add(
                                    egui::Button::new(
                                        ui::semibold(format!("Remove {tag} from GitHub"), 13.5)
                                            .color(ui::CARD()),
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
                                    beta,
                                );
                            }
                            if ui::button(ui, "", "Keep it").clicked() {
                                self.confirm_remove_tag = false;
                            }
                        } else if ui::button(ui, ui::icon::TRASH, "Remove the tag…").clicked() {
                            self.confirm_remove_tag = true;
                        }
                    });
                });
                ui.add_space(8.0);
            }
        }
        let Some(w) = self.build.clone() else { return };
        if w.dismissed || !w.slug.eq_ignore_ascii_case(&self.session.slug) {
            return;
        }
        use crate::github::Build;
        match &w.status {
            None | Some(Build::Running { .. }) => {
                ui::banner(ui, ACCENT(), |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.spinner();
                        let by_workflow = self
                            .session
                            .outcome
                            .as_ref()
                            .is_some_and(|o| o.release_by_workflow);
                        ui.label(match (&w.tag, by_workflow) {
                            (Some(t), true) => format!("GitHub is building and publishing {t}…"),
                            (Some(t), false) => format!("GitHub is building {t}…"),
                            (None, _) => "GitHub is building this push…".to_string(),
                        });
                        if let Some(Build::Running { url }) = &w.status {
                            if ui.link("Watch on GitHub").clicked() {
                                let _ = proc::xdg_open(url);
                            }
                        }
                    });
                });
            }
            Some(Build::None) => {}
            Some(Build::Passed { url }) => {
                let url = url.clone();
                ui::banner(ui, ui::SUCCESS(), |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(RichText::new(ui::icon::CIRCLE_CHECK).color(ui::SUCCESS()));
                        let by_workflow = self
                            .session
                            .outcome
                            .as_ref()
                            .is_some_and(|o| o.release_by_workflow);
                        ui.label(match (&w.tag, by_workflow) {
                            (Some(t), true) => format!("Built and published {t} on GitHub"),
                            (Some(_), false) => "Built on GitHub: the release is ready".to_string(),
                            (None, _) => "Built on GitHub".to_string(),
                        });
                        if ui.link("View the build").clicked() {
                            let _ = proc::xdg_open(&url);
                        }
                    });
                });
            }
            Some(Build::Failed { url, step, log }) => {
                let (url, step, log) = (url.clone(), step.clone(), log.clone());
                ui::banner(ui, ERROR(), |ui| {
                    ui.label(
                        ui::semibold(format!("GitHub's build failed at {step}"), 14.5)
                            .color(ERROR()),
                    );
                    let transient = crate::github::looks_transient(&log);
                    if transient {
                        ui.add(egui::Label::new(RichText::new("This looks like a passing problem on GitHub's side (a download or network error), not your project. Re-run it first.").color(WARNING())).wrap());
                    } else if w.local_passed {
                        ui.add(egui::Label::new(ui::muted("It passed here but failed on GitHub: GitHub runs on a different machine, often with newer tools.").size(13.0)).wrap());
                    }
                    // What GitHub said, without opening the log.
                    let key = crate::logbook::key_lines(&log, 12);
                    if !key.is_empty() {
                        egui::CollapsingHeader::new("What GitHub's log says")
                            .id_salt("gh_fail_lines")
                            .default_open(true)
                            .show(ui, |ui| {
                                for l in &key {
                                    ui.add(
                                        egui::Label::new(
                                            ui::mono(report::redact(l, &self.redact_ctx()))
                                                .size(12.0),
                                        )
                                        .wrap(),
                                    );
                                }
                            });
                    }
                    ui.horizontal_wrapped(|ui| {
                        if ui::button_enabled(ui, self.busy.is_none(), ui::icon::REFRESH, "Re-run on GitHub")
                            .on_hover_text("Re-runs only the failed jobs, then StowLock watches again")
                            .clicked()
                        {
                            self.rerun_build(url.clone());
                        }
                        if ui.link("Check again").on_hover_text("If you re-ran it on GitHub yourself").clicked() {
                            self.rewatch_build();
                        }
                        if ui::button(ui, ui::icon::EXTERNAL, "Open the build log").clicked() {
                            let _ = proc::xdg_open(&url);
                        }
                        if ui::button(ui, ui::icon::COPY, "Copy for AI").clicked() {
                            let lines: Vec<String> = log.lines().map(|l| l.to_string()).collect();
                            let prompt = format!(
                                "My project built and passed its tests on my computer, but GitHub's build failed at: {step}\n\n{}",
                                report::project_fix_prompt("GitHub Actions build failed", &lines)
                            );
                            ui.ctx().copy_text(report::redact(&prompt, &self.redact_ctx()));
                            self.toast("Copied. Paste it into your AI chat, then load the fixed zip.");
                        }
                        if let Some(tag) = w.tag.clone() {
                            if self.confirm_remove_tag {
                                if ui.add(egui::Button::new(ui::semibold(format!("Delete {tag} from GitHub"), 13.5).color(ui::CARD())).fill(ERROR())).clicked() {
                                    self.fix_release(false, w.owner.clone(), w.slug.clone(), tag.clone(), false);
                                }
                            } else if ui::button(ui, ui::icon::TRASH, "Delete the release and tag…").on_hover_text("So you can fix it and reuse the same version").clicked() {
                                self.confirm_remove_tag = true;
                            }
                        }
                        if ui::button(ui, "", "Keep it").clicked() {
                            self.confirm_remove_tag = false;
                            if let Some(b) = self.build.as_mut() {
                                b.dismissed = true;
                            }
                        }
                    });
                });
            }
        }
        ui.add_space(8.0);
    }

    pub(super) fn start_rollback(&mut self) {
        let paths = self.paths.clone();
        let log = self.log.clone();
        self.spawn_bg(move || {
            Msg::RolledBack(
                std::env::current_exe()
                    .map_err(|e| e.to_string())
                    .and_then(|exe| updater::rollback(&paths, &exe, &log)),
            )
        });
    }

    pub(super) fn start_sign_in(&mut self) {
        self.signin = SignInUi {
            active: true,
            cancel: Arc::new(AtomicBool::new(false)),
            ..Default::default()
        };
        let (etx, erx) = channel::<SignInEvent>();
        let cancel = self.signin.cancel.clone();
        let ctx = self.egui_ctx.clone();
        let tx = self.tx.clone();
        let ctx2 = ctx.clone();
        std::thread::spawn(move || auth::sign_in(etx, cancel, move || ctx.request_repaint()));
        std::thread::spawn(move || {
            while let Ok(ev) = erx.recv() {
                Self::send(&tx, &ctx2, Msg::SignIn(ev));
            }
        });
    }
}
