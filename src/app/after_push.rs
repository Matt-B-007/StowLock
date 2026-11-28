// SPDX-License-Identifier: GPL-3.0-or-later
//! The App's after push code (part of the StowLock window).
use super::*;

/// Seconds as "3:48".
pub(super) fn clock(secs: u64) -> String {
    format!("{}:{:02}", secs / 60, secs % 60)
}

fn now_secs() -> i64 {
    chrono::Local::now().timestamp()
}

impl App {
    /// "3:48" while GitHub builds the last push, when there's a history to
    /// estimate from (for the sidebar).
    pub(super) fn build_countdown(&self) -> Option<String> {
        let w = self.build.as_ref()?;
        if w.dismissed || !matches!(w.status, None | Some(crate::github::Build::Running { .. })) {
            return None;
        }
        let left = w.progress.as_ref()?.secs_left(now_secs())?;
        Some(if left == 0 {
            "soon".into()
        } else {
            clock(left)
        })
    }

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
                let by_workflow = self
                    .session
                    .outcome
                    .as_ref()
                    .is_some_and(|o| o.release_by_workflow);
                let title = match (&w.tag, by_workflow) {
                    (Some(t), true) => format!("GITHUB IS BUILDING AND PUBLISHING {t}"),
                    (Some(t), false) => format!("GITHUB IS BUILDING {t}"),
                    (None, _) => "GITHUB IS BUILDING THIS PUSH".to_string(),
                };
                let url = match &w.status {
                    Some(Build::Running { url }) => Some(url.clone()),
                    _ => None,
                };
                let progress = w.progress.clone().unwrap_or_default();
                let now = now_secs();
                ui::card_active(ui, true, |ui| {
                    ui::section_title(ui, &title);
                    ui.horizontal(|ui| {
                        let left = progress.secs_left(now);
                        let (big, small) = match left {
                            Some(0) => ("soon".to_string(), "any moment".to_string()),
                            Some(s) => (clock(s), "left, about".to_string()),
                            None => (
                                format!("{}%", (progress.fraction(now) * 100.0) as u32),
                                "of the steps".to_string(),
                            ),
                        };
                        ui::ring(ui, 120.0, progress.fraction(now), &big, &small);
                        ui.add_space(12.0);
                        ui.vertical(|ui| {
                            match (left, progress.started) {
                                (Some(s), _) if s > 0 => {
                                    let at = chrono::Local::now()
                                        + chrono::Duration::seconds(s as i64);
                                    ui.label(ui::semibold(
                                        format!("Ready around {}", at.format("%H:%M")),
                                        17.0,
                                    ));
                                }
                                _ => {
                                    ui.label(ui::semibold("Building on GitHub", 17.0));
                                }
                            }
                            let started = progress
                                .started
                                .map(|t| format!("Started {} min ago. ", ((now - t).max(0)) / 60))
                                .unwrap_or_default();
                            let basis = match progress.typical_secs {
                                Some(t) => format!("This project's recent builds took about {} min, so this is a good guess, not a promise: GitHub sometimes queues a build first.", (t + 30) / 60),
                                None => "There's no earlier build to go by yet, so this shows the steps instead of a time.".to_string(),
                            };
                            ui.add(
                                egui::Label::new(ui::muted(format!("{started}{basis}")).size(13.0))
                                    .wrap(),
                            );
                            ui.label(
                                ui::muted("You'll get a desktop notification when it's done.")
                                    .size(12.5),
                            );
                            if let Some(u) = &url {
                                if ui.link("Watch on GitHub").clicked() {
                                    let _ = proc::xdg_open(u);
                                }
                            }
                        });
                    });
                    if progress.jobs.is_empty() {
                        ui.horizontal_wrapped(|ui| {
                            ui.spinner();
                            ui.label(ui::muted("Waiting for GitHub to start…"));
                        });
                    }
                    for j in &progress.jobs {
                        let lane = match (j.status.as_str(), j.conclusion.as_str()) {
                            ("completed", "success") => ui::Lane::Passed,
                            ("completed", "skipped") => ui::Lane::GitHubOnly,
                            ("completed", _) => ui::Lane::Failed,
                            ("in_progress", _) => ui::Lane::Running,
                            _ => ui::Lane::Waiting,
                        };
                        let detail = if lane == ui::Lane::Running && j.steps > 0 {
                            format!(
                                "Step {} of {} · {}",
                                (j.done + 1).min(j.steps),
                                j.steps,
                                j.current
                            )
                        } else if j.steps > 0 {
                            format!("{} steps", j.steps)
                        } else {
                            String::new()
                        };
                        ui::job_lane(ui, &j.name, &detail, lane, "");
                    }
                    ui.ctx()
                        .request_repaint_after(std::time::Duration::from_secs(1));
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
