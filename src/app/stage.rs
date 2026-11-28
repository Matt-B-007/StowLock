// SPDX-License-Identifier: GPL-3.0-or-later
//! The App's stage code (part of the StowLock window).
use super::*;

impl App {
    pub(super) fn banners(&mut self, ui: &mut egui::Ui) {
        // Setup and install reminders belong to Stage, where work starts;
        // Settings has its own account panel. Updates and crashes show anywhere.
        let home = self.tab == Tab::Stage;
        if home && self.env.checked && !self.env.ready() {
            ui::banner(ui, WARNING(), |ui| {
                ui.horizontal_wrapped(|ui| {
                    let what = if !self.env.git || !self.env.gh {
                        "Git and GitHub's gh tool need to be installed."
                    } else {
                        "Sign in to GitHub to push projects."
                    };
                    ui.label(RichText::new(format!("Finish setup: {what}")).color(WARNING()));
                    if ui.button("Finish setup").clicked() {
                        self.tab = Tab::Settings;
                    }
                });
            });
        }
        if self.env.offline {
            ui.label(ui::muted("Offline: using local projects only.").size(12.0));
        }
        if home && self.env.secure() == Some(false) {
            ui::banner(ui, WARNING(), |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label("Your GitHub sign-in is stored in a plain file instead of the system keyring.");
                    if ui.button("Move to secure storage").clicked() {
                        self.tab = Tab::Settings;
                        self.start_sign_in();
                    }
                });
            });
        }
        let mut dismiss_update = false;
        let mut do_update: Option<String> = None;
        let mut restart: Option<PathBuf> = None;
        match &self.update {
            UpdateState::None => {}
            UpdateState::Available(rel) => {
                ui::banner(ui, ACCENT(), |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(format!(
                            "StowLock {} is available (you have {APP_VERSION}).",
                            rel.label()
                        ));
                        if let Some(pkg) = &self.pacman_pkg {
                            ui.label(format!(
                                "Update it with your AUR helper ({pkg}), for example: paru -Syu"
                            ));
                        } else if ui
                            .button(RichText::new(format!("Update to {}", rel.label())).strong())
                            .clicked()
                        {
                            do_update = Some(rel.tag.clone());
                        }
                        if ui
                            .small_button(ui::icon::X)
                            .on_hover_text("Dismiss")
                            .clicked()
                        {
                            dismiss_update = true;
                        }
                    });
                    if !rel.notes.is_empty() {
                        egui::CollapsingHeader::new("What's new")
                            .id_salt("whats_new")
                            .show(ui, |ui| {
                                egui::ScrollArea::vertical()
                                    .max_height(200.0)
                                    .show(ui, |ui| {
                                        ui.add(egui::Label::new(rel.notes.as_str()).wrap());
                                    });
                            });
                    }
                });
            }
            UpdateState::Working(t) => {
                ui::banner(ui, ACCENT(), |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.spinner();
                        ui.label(format!("Downloading and checking StowLock {t}…"));
                    });
                });
            }
            UpdateState::Ready(t, p) => {
                ui::banner(ui, SUCCESS(), |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(format!("StowLock {t} is installed."));
                        if ui.button(RichText::new("Restart now").strong()).clicked() {
                            restart = Some(p.clone());
                        }
                    });
                });
            }
            UpdateState::Failed(e) => {
                let e = e.clone();
                ui::banner(ui, WARNING(), |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(format!(
                            "The update didn't install: {}. Your current version keeps working.",
                            e.lines().next().unwrap_or("")
                        ));
                        if ui
                            .small_button(ui::icon::X)
                            .on_hover_text("Dismiss")
                            .clicked()
                        {
                            dismiss_update = true;
                        }
                    });
                });
            }
        }
        if dismiss_update {
            self.update = UpdateState::None;
        }
        if let Some(t) = do_update {
            self.apply_update(t);
        }
        if let Some(p) = restart {
            // A clean exit: save settings and clear the "running" mark first,
            // so the new version doesn't think this one closed unexpectedly.
            self.save_config();
            report::mark_stopped(&self.paths);
            if updater::restart(&p).is_ok() {
                std::process::exit(0);
            }
            // The new version didn't start: this one keeps running.
            let _ = report::mark_running(&self.paths);
        }
        if let Some(prev) = self.rollback_offer.clone() {
            let mut go_back = false;
            ui::banner(ui, ERROR(), |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label(format!(
                        "StowLock {APP_VERSION} closed unexpectedly last time. Go back to {prev}?"
                    ));
                    if ui
                        .button(RichText::new(format!("Go back to {prev}")).strong())
                        .clicked()
                    {
                        go_back = true;
                    }
                    if ui
                        .small_button(ui::icon::X)
                        .on_hover_text("Dismiss")
                        .clicked()
                    {
                        self.rollback_offer = None;
                    }
                });
            });
            if go_back {
                self.start_rollback();
            }
        }
        if let Some(p) = self.crash_report.clone() {
            ui::banner(ui, ERROR(), |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label("StowLock closed unexpectedly last time.");
                    if ui::button(ui, ui::icon::BUG, "Report").clicked() {
                        self.tab = Tab::Report;
                    }
                    if ui.button("View the crash report").clicked() {
                        let _ = proc::xdg_open(&p.display().to_string());
                    }
                    if ui.button("Dismiss").clicked() {
                        self.crash_report = None;
                    }
                });
            });
        }
        if home && !self.installed {
            let older_installed = self
                .installed_version
                .clone()
                .filter(|v| instance::is_older(v, APP_VERSION));
            let newer_installed = self
                .installed_version
                .clone()
                .filter(|v| instance::is_older(APP_VERSION, v));
            let mut do_install = false;
            ui::banner(ui, ACCENT(), |ui| {
                ui.horizontal_wrapped(|ui| {
                    if let Some(v) = &newer_installed {
                        ui.label(format!(
                            "This is an older copy ({APP_VERSION}). The installed StowLock is {v}: open StowLock from your app menu to use it."
                        ));
                    } else if let Some(v) = &older_installed {
                        ui.label(format!(
                            "The installed StowLock is older ({v}). This copy is {APP_VERSION}."
                        ));
                        if ui.button("Replace the installed version").clicked() {
                            do_install = true;
                        }
                    } else {
                        ui.label("StowLock isn't installed yet, so it won't appear in your app menu.");
                        if ui.button("Install StowLock").clicked() {
                            do_install = true;
                        }
                    }
                    if ui.small_button(ui::icon::X).on_hover_text("Dismiss").clicked() {
                        self.installed = true;
                    }
                });
            });
            if do_install {
                match install::install(&self.install_inst) {
                    Ok(r) => {
                        for l in r.lines.iter().chain(r.warnings.iter()) {
                            self.log.info("install", l);
                        }
                        self.toast(
                            &r.lines
                                .first()
                                .cloned()
                                .unwrap_or_else(|| "Installed.".into()),
                        );
                        self.installed = true;
                    }
                    Err(e) => self.fail(&e, "install"),
                }
            }
        }
    }

    pub(super) fn error_card(&mut self, ui: &mut egui::Ui) {
        let Some((f, raw)) = self.error.clone() else {
            return;
        };
        ui::banner(ui, ERROR(), |ui| {
            ui.add(egui::Label::new(RichText::new(&f.message).color(ERROR()).strong()).wrap());
            ui.horizontal_wrapped(|ui| {
                match f.action {
                    Some(Action::SignIn) => {
                        if ui.button("Sign in").clicked() {
                            self.tab = Tab::Settings;
                            self.start_sign_in();
                        }
                    }
                    Some(Action::SetIdentity) => {
                        if ui.button("Set name and email").clicked() {
                            self.tab = Tab::Settings;
                        }
                    }
                    Some(Action::SwitchToUpdate) => {
                        if ui.button("Switch to Update").clicked() {
                            self.session.mode = Mode::Update;
                            self.error = None;
                        }
                    }
                    Some(Action::Retry) | Some(Action::BugReport) | None => {}
                }
                if ui.button("Bug Report").clicked() {
                    self.open_bug_report();
                }
                if ui.button("Dismiss").clicked() {
                    self.error = None;
                }
            });
            egui::CollapsingHeader::new(ui::muted("Details"))
                .id_salt("errdetails")
                .show(ui, |ui| {
                    ui.label(ui::mono(crate::secrets::redact_secrets(&raw)).size(12.0));
                });
        });
    }

    pub(super) fn stage_tab(&mut self, ui: &mut egui::Ui, hovering: bool) {
        self.error_card(ui);
        self.rust_notice(ui);
        let has_zip = self.session.zip.is_some() || self.session.stage.is_some();
        let mut pick = false;
        let mut use_newer: Option<PathBuf> = None;
        if !has_zip {
            self.welcome(ui, hovering);
            self.log_drawer(ui);
            return;
        }
        // ---- the project and where it is in the flow
        ui.horizontal(|ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                pick |=
                    ui::button_enabled(ui, self.busy.is_none(), "", "Choose another…").clicked();
                ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
                    let title = if self.session.slug.is_empty() {
                        "New project".to_string()
                    } else {
                        self.session.slug.clone()
                    };
                    // A very long name shortens with … (hover shows it all).
                    ui.add(egui::Label::new(ui::display(&title, 26.0).color(TEXT())).truncate())
                        .on_hover_text(&title);
                    ui.horizontal_wrapped(|ui| {
                        let tag = self.session.tag.trim().to_string();
                        if !tag.is_empty() {
                            ui.label(ui::mono(&tag).size(13.5));
                        }
                        let pushed = self
                            .session
                            .outcome
                            .as_ref()
                            .is_some_and(|o| !o.nothing_to_push);
                        if self.session.archive_only {
                            ui::pill(ui, "ARCHIVE ONLY", MUTED(), true);
                        } else {
                            if pushed {
                                ui::pill(ui, "PUSHED", SUCCESS(), false);
                            } else {
                                ui::pill(
                                    ui,
                                    if self.session.mode == Mode::New {
                                        "NEW REPO"
                                    } else {
                                        "UPDATE"
                                    },
                                    ACCENT(),
                                    false,
                                );
                            }
                            ui::pill(
                                ui,
                                if tag.is_empty() {
                                    "NO RELEASE"
                                } else if self.session.channel == Channel::Beta {
                                    "BETA"
                                } else {
                                    "STABLE"
                                },
                                MUTED(),
                                true,
                            );
                        }
                    });
                    let (zip_line, hover) = if let Some(z) = &self.session.zip {
                        let name = z
                            .file_name()
                            .map(|n| n.to_string_lossy().to_string())
                            .unwrap_or_default();
                        let about = self
                            .session
                            .zip_info
                            .as_ref()
                            .map(|i| format!(" · {}", i.describe()))
                            .unwrap_or_default();
                        (format!("{name}{about}"), util::tilde(z))
                    } else if let Some(st) = &self.session.stage {
                        (
                            format!("{} · loaded from your workspace", util::tilde(&st.dir)),
                            util::tilde(&st.dir),
                        )
                    } else {
                        (String::new(), String::new())
                    };
                    let mut line = zip_line;
                    if let Some(st) = &self.session.stage {
                        line.push_str(&format!(
                            " · {} · {} files · {}",
                            st.stack().name(),
                            st.files,
                            util::human_size(st.size)
                        ));
                    }
                    ui.add(egui::Label::new(ui::muted(line).size(13.0)).truncate())
                        .on_hover_text(hover);
                });
            });
        });
        ui.add_space(6.0);
        ui::step_rail(ui, &self.flow());
        ui.add_space(4.0);
        // Make sure it's the zip you meant.
        if self.session.stage.is_none() {
            if let Some(newer) = self.session.newer_copy.clone() {
                ui::banner(ui, WARNING(), |ui| {
                    ui.add(
                        egui::Label::new(format!(
                            "There's a newer copy of this download beside it: {}",
                            newer
                                .file_name()
                                .map(|n| n.to_string_lossy().to_string())
                                .unwrap_or_default()
                        ))
                        .wrap(),
                    );
                    if ui::button(ui, ui::icon::PACKAGE, "Use the newer copy").clicked() {
                        use_newer = Some(newer.clone());
                    }
                });
            } else if self.session.same_zip {
                ui::banner(ui, WARNING(), |ui| {
                    ui.add(
                        egui::Label::new(
                            "This is the same zip as last time (identical contents). If you downloaded a fixed one, choose it instead.",
                        )
                        .wrap(),
                    );
                });
            }
        }
        if pick {
            self.pick_zip();
        }
        if let Some(z) = use_newer {
            self.load_zip(z);
        }
        // Reviewing: the whole push on one page, in place of the work.
        if self.session.confirm && self.session.stage.is_some() {
            self.review_page(ui);
            self.log_drawer(ui);
            return;
        }
        // Wide windows (the default size and up): the work on the left, this
        // release on the right.
        let wide = ui.available_width() >= 800.0;
        if wide {
            let side = if ui.available_width() >= 1000.0 {
                300.0
            } else {
                260.0
            };
            let main_w = ui.available_width() - side - 16.0;
            ui.horizontal_top(|ui| {
                // The gap is the 16 px added below, nothing more: with the
                // usual spacing as well the panel ran past the edge.
                let usual = ui.spacing().item_spacing.x;
                ui.spacing_mut().item_spacing.x = 0.0;
                ui.allocate_ui_with_layout(
                    egui::vec2(main_w, 0.0),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        ui.set_width(main_w);
                        ui.spacing_mut().item_spacing.x = usual;
                        self.stage_body(ui);
                    },
                );
                ui.add_space(16.0);
                ui.allocate_ui_with_layout(
                    egui::vec2(side, 0.0),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        ui.set_width(side);
                        ui.set_max_width(side);
                        ui.spacing_mut().item_spacing.x = usual;
                        ui.add_space(8.0);
                        self.release_panel(ui);
                    },
                );
            });
        } else {
            self.stage_body(ui);
            ui.add_space(8.0);
            self.release_panel(ui);
        }
        self.log_drawer(ui);
    }

    /// Where the project is in the flow, for the steps along the top.
    pub(super) fn flow(&self) -> Vec<(&'static str, ui::Flow)> {
        use ui::Flow::*;
        let s = &self.session;
        let staged = s.stage.is_some();
        let checks = s.checks.as_ref().map(|c| c.tests.status);
        let pushed = s.outcome.is_some();
        let partial = s.checks.as_ref().is_some_and(|c| c.partial);
        let check = match (&checks, staged, pushed) {
            (_, _, true) => Done,
            (_, false, _) => Todo,
            (None, true, _) => Active,
            (Some(TestStatus::Failed), _, _) => Failed,
            (Some(TestStatus::Cancelled), _, _) => Stopped,
            (Some(TestStatus::Skipped), _, _) => Skipped,
            (Some(TestStatus::Passed), _, _) if partial => Active,
            (Some(TestStatus::Passed), _, _) => Done,
        };
        // After the checks, reviewing is what's next.
        let review = if pushed {
            Done
        } else if s.confirm || matches!(check, Done | Skipped) {
            Active
        } else {
            Todo
        };
        // A project with no workflows has nothing for GitHub to build.
        let builds = s
            .stage
            .as_ref()
            .is_none_or(|st| crate::workflows::has_workflows(&st.dir));
        let build = match self.build.as_ref().filter(|w| !w.dismissed && pushed) {
            Some(w) => match &w.status {
                Some(crate::github::Build::Passed { .. }) => Done,
                Some(crate::github::Build::Failed { .. }) => Failed,
                Some(crate::github::Build::None) => Skipped,
                _ => Active,
            },
            None if pushed => Done,
            None => Todo,
        };
        if s.archive_only {
            return vec![
                ("Zip", Done),
                ("Route", if pushed { Done } else { Active }),
                ("Archive", if pushed { Done } else { Todo }),
            ];
        }
        let mut steps = vec![
            ("Zip", Done),
            ("Route", if staged { Done } else { Active }),
            ("Check", check),
            ("Review", review),
        ];
        if builds {
            steps.push(("GitHub build", build));
        } else if pushed {
            steps.push(("Pushed", Done));
        }
        steps
    }

    /// No zip yet: a big place to drop one, and your projects to pick up again.
    fn welcome(&mut self, ui: &mut egui::Ui, hovering: bool) {
        ui::title(
            ui,
            "Stage",
            "Turn a project zip into a tested GitHub release.",
        );
        ui.add_space(8.0);
        let (fill, stroke) = if hovering {
            (ACCENT().linear_multiply(0.12), ACCENT())
        } else {
            (ACCENT().gamma_multiply(0.04), ACCENT().gamma_multiply(0.35))
        };
        let mut pick = false;
        egui::Frame::new()
            .fill(fill)
            .stroke(egui::Stroke::new(if hovering { 2.0_f32 } else { 1.5 }, stroke))
            .corner_radius(egui::CornerRadius::same(16))
            .inner_margin(egui::Margin::same(32))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.vertical_centered(|ui| {
                    let (r, _) = ui.allocate_exact_size(egui::vec2(56.0, 56.0), egui::Sense::hover());
                    ui.painter().rect_filled(r, egui::CornerRadius::same(16), ui::ACCENT_SOFT());
                    ui.painter().text(
                        r.center(),
                        egui::Align2::CENTER_CENTER,
                        ui::icon::UPLOAD,
                        egui::FontId::proportional(26.0),
                        ACCENT(),
                    );
                    ui.add_space(6.0);
                    ui.label(ui::semibold(
                        if hovering { "Drop the zip to load it" } else { "Drop a project zip anywhere in this window" },
                        17.0,
                    ));
                    ui.label(ui::muted("StowLock works out which project it belongs to from its name and version.").size(13.0));
                    ui.add_space(8.0);
                    pick |= ui::primary_glow(ui, "Choose a .zip…  Ctrl+O", self.busy.is_none()).clicked();
                });
            });
        if pick {
            self.pick_zip();
        }
        // Your projects, newest push first: pick one up again from its folder.
        let mut recent: Vec<ProjectInfo> = self.projects.clone();
        recent.sort_by(|a, b| b.last_push.cmp(&a.last_push));
        recent.truncate(5);
        if !recent.is_empty() {
            ui.add_space(12.0);
            let mut open: Option<ProjectInfo> = None;
            ui::card(ui, |ui| {
                ui::section_title(ui, "YOUR PROJECTS");
                for p in &recent {
                    ui.horizontal(|ui| {
                        ui.label(ui::semibold(&p.slug, 14.0));
                        if let Some(t) = &p.last_tag {
                            ui.label(ui::mono(t).size(12.5).color(MUTED()));
                        }
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui
                                .small_button("Open")
                                .on_hover_text("Load it from its folder to check and push again")
                                .clicked()
                            {
                                open = Some(p.clone());
                            }
                            if let Some(when) = &p.last_push {
                                ui.label(
                                    ui::muted(when.get(..16).unwrap_or(when).replace('T', " "))
                                        .size(12.5),
                                );
                            }
                        });
                    });
                }
            });
            if let Some(p) = open {
                self.load_project(&p);
            }
        }
        ui.add_space(8.0);
        ui.label(ui::muted("StowLock unpacks your zip into its own project folder, checks it like GitHub does, lets you try it, then pushes it to the right repo and files the zip away.").size(12.5));
    }

    /// One row per workflow job: waiting, running, passed, failed, or only
    /// on GitHub.
    fn job_lanes(&mut self, ui: &mut egui::Ui, dir: &Path) {
        if self.session.jobs.as_ref().is_none_or(|(d, _, _)| d != dir) {
            let (here, only) = match crate::workflows::plan(dir) {
                Some(plan) => {
                    let mut here: Vec<String> = Vec::new();
                    for st in &plan.steps {
                        if !here.contains(&st.job) {
                            here.push(st.job.clone());
                        }
                    }
                    (here, plan.github_only_jobs())
                }
                None => (Vec::new(), Vec::new()),
            };
            self.session.jobs = Some((dir.to_path_buf(), here, only));
        }
        let Some((_, here, only)) = self.session.jobs.clone() else {
            return;
        };
        if here.is_empty() {
            return;
        }
        let running = self.busy.is_some() && self.session.checks.is_none();
        let checks = self.session.checks.clone();
        let tail = checks
            .as_ref()
            .map(|c| c.tests.tail.clone())
            .unwrap_or_default();
        let stopped = checks
            .as_ref()
            .is_some_and(|c| c.tests.status == TestStatus::Cancelled);
        let alone = self.session.rechecking.clone();
        let live = proc::live_label();
        let current = live
            .strip_prefix('[')
            .and_then(|r| r.split(']').next())
            .map(str::to_string);
        let at = current
            .as_ref()
            .and_then(|c| here.iter().position(|j| j == c));
        // When the running job changes, its clock starts again.
        if running {
            if let Some(c) = &current {
                if self.session.job_clock.as_ref().is_none_or(|(j, _)| j != c) {
                    self.session.job_clock = Some((c.clone(), Instant::now()));
                }
            }
        }
        for (i, job) in here.iter().enumerate() {
            let tag = format!("[{job}]");
            let lines: Vec<&str> = tail.lines().filter(|l| l.contains(&tag)).collect();
            // A job left out of a re-check keeps the result it had.
            let left_out = !alone.is_empty() && !alone.contains(job);
            let mut right = String::new();
            let (lane, detail) = if left_out {
                self.session
                    .job_memo
                    .get(job)
                    .cloned()
                    .unwrap_or((ui::Lane::NotRun, "Not part of this re-check".into()))
            } else if running {
                match at {
                    Some(a) if i < a => (ui::Lane::Finished, String::new()),
                    Some(a) if i == a => {
                        if let Some((_, since)) = &self.session.job_clock {
                            right = super::after_push::clock(since.elapsed().as_secs());
                        }
                        (
                            ui::Lane::Running,
                            live.split_once("] ")
                                .map(|(_, s)| s.to_string())
                                .unwrap_or_default(),
                        )
                    }
                    _ => (ui::Lane::Waiting, String::new()),
                }
            } else if checks.is_none() {
                (ui::Lane::Waiting, String::new())
            } else if lines.iter().any(|l| l.starts_with("Failed: ")) {
                let step = lines
                    .iter()
                    .find(|l| l.starts_with("Failed: "))
                    .and_then(|l| l.split_once("] "))
                    .map(|(_, s)| s.to_string())
                    .unwrap_or_default();
                (ui::Lane::Failed, step)
            } else if stopped {
                (ui::Lane::Stopped, "You stopped the checks".into())
            } else if lines.iter().any(|l| l.starts_with("Passed: ")) {
                let n = lines.iter().filter(|l| l.starts_with("Passed: ")).count();
                (
                    ui::Lane::Passed,
                    format!("{n} step{} passed", if n == 1 { "" } else { "s" }),
                )
            } else if lines.iter().any(|l| l.starts_with("Can't run here")) {
                (
                    ui::Lane::CantRun,
                    "Something it needs isn't on this computer".into(),
                )
            } else {
                (ui::Lane::NotRun, "Nothing in this job ran here".into())
            };
            if !running
                && !left_out
                && matches!(
                    lane,
                    ui::Lane::Passed | ui::Lane::Failed | ui::Lane::CantRun
                )
            {
                self.session
                    .job_memo
                    .insert(job.clone(), (lane, detail.clone()));
            }
            ui::job_lane(ui, job, &detail, lane, &right);
            ui.add_space(4.0);
        }
        for job in &only {
            ui::job_lane(
                ui,
                job,
                "Runs in a container: on GitHub only",
                ui::Lane::GitHubOnly,
                "",
            );
            ui.add_space(4.0);
        }
        ui.add_space(4.0);
    }

    /// The side panel: this release, what to do when a check fails, and the
    /// project's last pushes.
    fn release_panel(&mut self, ui: &mut egui::Ui) {
        let s = self.session.clone();
        let failed = s
            .checks
            .as_ref()
            .is_some_and(|c| c.tests.status == TestStatus::Failed);
        if failed && s.outcome.is_none() {
            ui::card(ui, |ui| {
                ui::section_title(ui, "FIXING IT");
                for (i, t) in [
                    "Copy for AI, and paste it into the project's own chat.",
                    "Drop the fixed zip here (or anywhere on this window).",
                    "Re-check just the failed job: quicker while fixing.",
                    "One full check, then review and push.",
                ]
                .iter()
                .enumerate()
                {
                    ui.add(egui::Label::new(format!("{}. {t}", i + 1)).wrap());
                }
            });
            ui.add_space(8.0);
        }
        ui::card(ui, |ui| {
            ui::section_title(ui, "THIS RELEASE");
            // The release before this one (never this one's own tag).
            let prev = self
                .existing_tags()
                .into_iter()
                .chain(
                    self.projects
                        .iter()
                        .find(|p| p.slug.eq_ignore_ascii_case(&s.slug))
                        .and_then(|p| p.last_tag.clone()),
                )
                .find(|t| t.trim() != s.tag.trim());
            let col_w = (ui.available_width() - 100.0).max(80.0);
            egui::Grid::new("release_facts")
                .max_col_width(col_w)
                .num_columns(2)
                .spacing([14.0, 6.0])
                .show(ui, |ui| {
                    let tag = s.tag.trim();
                    ui.label(ui::muted("Version"));
                    ui.add(
                        egui::Label::new(
                            ui::mono(if tag.is_empty() { "no release" } else { tag }).size(13.0),
                        )
                        .truncate(),
                    );
                    ui.end_row();
                    if let Some(p) = &prev {
                        ui.label(ui::muted("After"));
                        ui.add(egui::Label::new(ui::mono(p).size(13.0)).truncate());
                        ui.end_row();
                    }
                    if let Some(ch) = s.stage.as_ref().and_then(|st| st.changes.as_ref()) {
                        ui.label(ui::muted("Changes"));
                        ui.add(egui::Label::new(ch.short()).truncate())
                            .on_hover_text(ch.summary());
                        ui.end_row();
                    }
                    ui.label(ui::muted("Secret scan"));
                    match &s.checks {
                        Some(c) if c.secrets.is_empty() => {
                            ui.label(RichText::new("None found").color(SUCCESS()));
                        }
                        Some(c) => {
                            ui.label(
                                RichText::new(format!("{} to look at", c.secrets.len()))
                                    .color(ERROR()),
                            );
                        }
                        None => {
                            ui.label(ui::muted("After the checks"));
                        }
                    }
                    ui.end_row();
                });
        });
        let pushes: Vec<PushRecord> = self
            .history
            .pushes
            .iter()
            .rev()
            .filter(|r| r.slug.eq_ignore_ascii_case(&s.slug))
            .take(3)
            .cloned()
            .collect();
        if !pushes.is_empty() {
            ui.add_space(8.0);
            ui::card(ui, |ui| {
                ui::section_title(ui, "LAST PUSHES");
                for r in &pushes {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(ui::icon::CHECK).color(SUCCESS()));
                        ui.label(r.tag.clone().unwrap_or_else(|| "(no tag)".into()));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(ui::muted(format!("{} {}", r.date(), r.time())).size(12.5));
                        });
                    });
                }
            });
        }
    }

    /// The work for the current step.
    fn stage_body(&mut self, ui: &mut egui::Ui) {
        ui.add_space(8.0);
        // Pushed: what GitHub is doing with it comes first.
        let pushed = self.session.outcome.is_some();
        if let Some(out) = self.session.outcome.clone() {
            self.completion_card(ui, &out);
            ui.add_space(8.0);
            self.after_push_cards(ui);
        }
        // Once staged, the route is done: a one-line summary you can reopen.
        let staged = self.session.stage.is_some();
        if pushed {
            self.session.edit_route = false;
        }
        if staged && !self.session.edit_route && !self.session.archive_only {
            let mut edit = false;
            ui::card(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(ui::muted("ROUTE").size(12.0).strong());
                    let to = if self.session.owner.is_empty() {
                        self.session.slug.clone()
                    } else {
                        format!("github.com/{}/{}", self.session.owner, self.session.slug)
                    };
                    ui.add(egui::Label::new(ui::mono(to).size(13.0)).truncate());
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if !pushed {
                            edit |= ui.small_button("Edit route and details").clicked();
                        }
                    });
                });
            });
            if edit {
                self.session.edit_route = true;
            }
            ui.add_space(8.0);
        } else {
            self.routing_card(ui);
            ui.add_space(8.0);
        }
        if !self.session.archive_only {
            if !staged || self.session.edit_route {
                self.details_card(ui);
                ui.add_space(8.0);
            }
            self.results_card(ui);
        } else {
            ui::card(ui, |ui| {
                ui::section_title(ui, "ARCHIVE ONLY");
                ui.add(egui::Label::new("The zip is filed with this project. Nothing is extracted, checked or pushed to GitHub.").wrap());
                ui.add_space(4.0);
                ui.horizontal_wrapped(|ui| {
                    ui.label("Version");
                    let r = ui::named(
                        ui.add(
                            egui::TextEdit::singleline(&mut self.session.tag)
                                .hint_text("optional")
                                .desired_width(140.0)
                                .font(egui::TextStyle::Monospace),
                        ),
                        "Version",
                    );
                    if r.changed() {
                        self.session.tag_auto = false;
                    }
                });
                let note = match (
                    &self.session.found_version,
                    self.session.tag_auto,
                    self.session.tag.trim().is_empty(),
                ) {
                    (Some(v), true, false) => format!("Found {} in {}.", v.tag, v.source),
                    (_, _, true) => "No version found; the date and time will be used.".to_string(),
                    _ => "Using the version you typed.".to_string(),
                };
                ui.add(egui::Label::new(ui::muted(note).size(12.5)).wrap());
                let tag = self.session.tag.trim().to_string();
                if !tag.is_empty()
                    && crate::archive::already_archived(
                        &self.config.archive_root(),
                        &self.session.slug,
                        &tag,
                    )
                {
                    ui.add(egui::Label::new(RichText::new(format!("{tag} is already archived for this project. The new copy is kept alongside it (nothing is overwritten).")).color(WARNING()).size(12.5)).wrap());
                }
                ui.add_space(4.0);
                ui.horizontal_wrapped(|ui| {
                    ui.radio_value(
                        &mut self.session.archive_move,
                        true,
                        "Move (clears Downloads)",
                    );
                    ui.radio_value(&mut self.session.archive_move, false, "Copy");
                });
                if !self.archive_note_seen {
                    ui.label(ui::muted("Archived zips are kept exactly as downloaded, including any .env files.").size(12.0));
                }
            });
        }
    }

    pub(super) fn routing_card(&mut self, ui: &mut egui::Ui) {
        let mut slug_changed = false;
        let mut owner_changed = false;
        let staged = self.session.stage.is_some();
        ui::card(ui, |ui| {
            ui.horizontal(|ui| {
                ui::section_title(ui, "ROUTING");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let m = self.session.effective_mode();
                    let col = match m {
                        Mode::New => ACCENT(),
                        Mode::Update => SUCCESS(),
                        Mode::ArchiveOnly => MUTED(),
                    };
                    ui::badge(ui, m.badge(), col);
                    if self.session.channel == Channel::Beta && m != Mode::ArchiveOnly {
                        ui::badge(ui, "BETA", WARNING());
                    }
                });
            });
            let src = self
                .session
                .zip
                .as_ref()
                .and_then(|z| z.file_name())
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| "workspace".into());
            let dest = if self.session.archive_only {
                format!("~/…/_archive/{}/", self.session.slug)
            } else {
                format!(
                    "{}/{}",
                    util::tilde(&self.config.workspace()),
                    self.session.slug
                )
            };
            let remote = if self.session.archive_only {
                "not pushed".to_string()
            } else {
                format!(
                    "github.com/{}/{}",
                    if self.session.owner.is_empty() {
                        "<sign in>"
                    } else {
                        &self.session.owner
                    },
                    self.session.slug
                )
            };
            // One line per hop so long names shorten with … instead of
            // breaking mid-word.
            let remote_color = if self.session.archive_only {
                MUTED()
            } else {
                ACCENT()
            };
            egui::Grid::new("route_grid")
                .max_col_width((ui.available_width() - 130.0).max(160.0))
                .num_columns(2)
                .spacing([10.0, 4.0])
                .show(ui, |ui| {
                    for (label, text, color) in [
                        ("Zip", &src, TEXT()),
                        ("Folder", &dest, TEXT()),
                        ("GitHub", &remote, remote_color),
                    ] {
                        ui.label(ui::muted(label));
                        ui.add(egui::Label::new(ui::mono(text).color(color)).truncate())
                            .on_hover_text(text.as_str());
                        ui.end_row();
                    }
                });
            ui.add_space(4.0);
            egui::Grid::new("routing")
                .max_col_width((ui.available_width() - 130.0).max(160.0))
                .num_columns(2)
                .spacing([12.0, 8.0])
                .show(ui, |ui| {
                    ui.label("Repo name");
                    ui.horizontal_wrapped(|ui| {
                        // Only repos that are on GitHub now; local folders stand in
                        // only when GitHub's list couldn't be loaded.
                        let gh_list = self.github_repos(&self.session.owner).cloned();
                        let from_github = gh_list.is_some();
                        let mut names: Vec<String> = gh_list.unwrap_or_else(|| {
                            self.projects.iter().map(|p| p.slug.clone()).collect()
                        });
                        names.sort_by_key(|n| n.to_lowercase());
                        names.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
                        let field_w = (ui.available_width() - 190.0).clamp(120.0, 560.0);
                        let r = ui.add_enabled(
                            !staged,
                            egui::TextEdit::singleline(&mut self.session.slug)
                                .font(egui::TextStyle::Monospace)
                                .desired_width(field_w),
                        );
                        let r = ui::named(r, "Repository name");
                        if r.changed() {
                            // Existing repos keep their exact name; anything else is
                            // formatted as it's typed (no capitals, spaces → hyphens).
                            let typed = self.session.slug.clone();
                            if !names.iter().any(|n| n == &typed) {
                                self.session.slug = detect::format_typing(&typed);
                            }
                            slug_changed = true;
                        }
                        if r.lost_focus() && !names.iter().any(|n| n == &self.session.slug) {
                            let fin = detect::finish_name(&self.session.slug);
                            if fin != self.session.slug {
                                self.session.slug = fin;
                                slug_changed = true;
                            }
                        }
                        let owner_key = self.session.owner.to_ascii_lowercase();
                        let loading = self.repos_loading.contains(&owner_key);
                        let stale = self.repos_stale.contains(&owner_key);
                        let mut opened = false;
                        let mut reload = false;
                        ui.add_enabled_ui(!staged, |ui| {
                            let r = egui::ComboBox::from_id_salt("existing")
                                .selected_text("Existing…")
                                .width(110.0)
                                .show_ui(ui, |ui| {
                                    let note = if loading {
                                        "Loading from GitHub…".to_string()
                                    } else if !from_github {
                                        "Couldn't reach GitHub: showing folders on this computer"
                                            .to_string()
                                    } else if stale {
                                        "Couldn't reach GitHub: this list may be out of date"
                                            .to_string()
                                    } else {
                                        format!("On GitHub ({})", self.session.owner)
                                    };
                                    ui.label(ui::muted(note).size(12.0));
                                    if names.is_empty() && from_github {
                                        ui.label(ui::muted("No repos yet").size(12.5));
                                    }
                                    for n in names {
                                        if ui.selectable_label(n == self.session.slug, &n).clicked()
                                        {
                                            self.session.slug = n;
                                            slug_changed = true;
                                        }
                                    }
                                });
                            ui::named(r.response.clone(), "Existing repository");
                            opened = r.inner.is_some();
                            reload = ui
                                .add_enabled(!loading, egui::Button::new(ui::icon::REFRESH).small())
                                .on_hover_text("Reload your repos from GitHub")
                                .clicked();
                        });
                        if opened && !self.existing_open {
                            let o = self.session.owner.clone();
                            self.refresh_repos(&o, true);
                        }
                        self.existing_open = opened;
                        if reload {
                            let o = self.session.owner.clone();
                            self.refresh_repos(&o, true);
                        }
                    });
                    ui.end_row();
                    if !self.session.archive_only {
                        ui.label("Push to");
                        let mut owners: Vec<String> =
                            self.env.accounts.iter().map(|a| a.login.clone()).collect();
                        if let Some(l) = &self.env.login {
                            if !owners.contains(l) {
                                owners.insert(0, l.clone());
                            }
                        }
                        owners.extend(self.env.orgs.iter().cloned());
                        ui.add_enabled_ui(!staged, |ui| {
                            let cb = egui::ComboBox::from_id_salt("owner")
                                .selected_text(if self.session.owner.is_empty() {
                                    "Sign in first".to_string()
                                } else {
                                    self.session.owner.clone()
                                })
                                .show_ui(ui, |ui| {
                                    for o in owners {
                                        if ui
                                            .selectable_label(o == self.session.owner, &o)
                                            .clicked()
                                        {
                                            self.session.owner = o;
                                            owner_changed = true;
                                        }
                                    }
                                });
                            ui::named(cb.response, "Owner");
                        });
                        ui.end_row();
                    }
                });
            if let Some(problem) = detect::name_problem(&self.session.slug) {
                ui.add(egui::Label::new(RichText::new(problem).color(ERROR()).size(12.5)).wrap());
            }
            ui.horizontal_wrapped(|ui| {
                ui.add_enabled_ui(!staged, |ui| {
                    let r = ui.checkbox(
                        &mut self.session.archive_only,
                        "Archive only (already uploaded; nothing is pushed)",
                    );
                    if r.changed() && self.session.archive_only {
                        // Fill in the version found in the zip, if the user hasn't typed one.
                        if self.session.tag.trim().is_empty() || self.session.tag_auto {
                            if let Some(v) = &self.session.found_version {
                                self.session.tag = v.tag.clone();
                                self.session.tag_auto = true;
                            }
                        }
                    }
                });
            });
            if self.session.mode == Mode::New
                && self.session.local_exists
                && self.session.local_is_ours
                && !self.session.archive_only
                && !staged
            {
                ui.add(
                    egui::Label::new(ui::muted(
                        "This replaces your earlier staging of this project that wasn't pushed. The old copy goes to the Trash.",
                    ))
                    .wrap(),
                );
            } else if self.session.deleted_on_github && !self.session.archive_only && !staged {
                ui::banner(ui, WARNING(), |ui| {
                    ui.label(ui::semibold(
                        format!(
                            "This repo was deleted on GitHub: github.com/{}/{}",
                            self.session.owner, self.session.slug
                        ),
                        14.0,
                    ));
                    ui.add(
                        egui::Label::new(ui::muted(
                            "It isn't an update any more. Create it again, or type another name above.",
                        ))
                        .wrap(),
                    );
                    ui.checkbox(
                        &mut self.session.replace_local,
                        "Create it again from this zip (the old folder goes to the Trash)",
                    );
                });
            } else if self.session.mode == Mode::New
                && self.session.local_exists
                && !self.session.archive_only
                && !staged
            {
                ui.colored_label(
                    WARNING(),
                    "A folder with this name already exists in your workspace but isn't on GitHub.",
                );
                ui.checkbox(
                    &mut self.session.replace_local,
                    "Replace the existing folder (moved to the Trash)",
                );
            }
            if self.session.mode == Mode::Update && !self.session.archive_only {
                let last = History::load(&self.paths).last_for(&self.session.slug);
                let tag = self.existing_tags().into_iter().next();
                let mut txt = String::from("Update detected: this goes to the existing repo.");
                if let Some(t) = tag {
                    txt.push_str(&format!(" Latest tag {t}."));
                }
                if let Some(l) = last {
                    txt.push_str(&format!(" Last pushed {} {}.", l.date(), l.time()));
                }
                ui.colored_label(SUCCESS(), txt);
            }
        });
        if slug_changed || owner_changed {
            self.recompute_mode();
            if owner_changed && !self.session.owner.is_empty() {
                let owner = self.session.owner.clone();
                self.refresh_repos(&owner, true);
            }
        }
    }

    pub(super) fn details_card(&mut self, ui: &mut egui::Ui) {
        let mode = self.session.mode;
        let tags = self.existing_tags();
        ui::card(ui, |ui| {
            ui::section_title(ui, "DETAILS");
            egui::Grid::new("details")
                .max_col_width((ui.available_width() - 130.0).max(160.0))
                .num_columns(2)
                .spacing([12.0, 8.0])
                .show(ui, |ui| {
                    if mode == Mode::New {
                        ui.label("Description");
                        ui::named(ui.add(
                            egui::TextEdit::singleline(&mut self.session.description)
                                .desired_width(f32::INFINITY),
                        ), "Description");
                        ui.end_row();
                        ui.label("Topics");
                        ui::named(ui.add(
                            egui::TextEdit::singleline(&mut self.session.topics)
                                .hint_text("rust, cli, linux")
                                .desired_width(f32::INFINITY),
                        ), "Topics");
                        ui.end_row();
                        ui.label("License");
                        let cb = egui::ComboBox::from_id_salt("license")
                            .selected_text(match self.session.license.as_str() {
                                "MIT" => "MIT",
                                "GPL-3.0" => "GNU GPL v3",
                                _ => "None",
                            })
                            .show_ui(ui, |ui| {
                                ui.selectable_value(
                                    &mut self.session.license,
                                    "none".to_string(),
                                    "None",
                                );
                                ui.selectable_value(
                                    &mut self.session.license,
                                    "MIT".to_string(),
                                    "MIT",
                                );
                                ui.selectable_value(
                                    &mut self.session.license,
                                    "GPL-3.0".to_string(),
                                    "GNU GPL v3",
                                );
                            });
ui::named(cb.response, "License");
                        ui.end_row();
                        ui.label("Visibility");
                        ui.horizontal_wrapped(|ui| {
                            ui.selectable_value(&mut self.session.private, true, "Private");
                            ui.selectable_value(&mut self.session.private, false, "Public");
                        });
                        ui.end_row();
                    }
                    ui.label("Version");
                    let mut new_tag: Option<String> = None;
                    ui.vertical(|ui| {
                        ui.horizontal_wrapped(|ui| {
                            let sel = usize::from(self.session.channel == Channel::Beta);
                            if let Some(i) = ui::segmented(ui, &["Stable", "Beta"], sel, 180.0) {
                                self.session.channel = if i == 1 { Channel::Beta } else { Channel::Stable };
                            }
                            ui::named(ui.add(
                                egui::TextEdit::singleline(&mut self.session.tag)
                                    .hint_text("optional, e.g. v0.1.0")
                                    .desired_width(170.0)
                                    .font(egui::TextStyle::Monospace),
                            ), "Version");
                        });
                        let prefix = self.config.tag_prefix.clone();
                        if let Some(fv) = &self.session.found_version {
                            if self.session.tag.trim() == fv.tag {
                                ui.label(ui::muted(format!("From {} in the zip", fv.source)).size(12.5));
                            }
                        }
                        let (ls, lb) = version::latest(&tags, &prefix);
                        let latest = match (&ls, &lb) {
                            (None, None) => "No releases on GitHub yet".to_string(),
                            (Some(s), None) => format!("Latest on GitHub: {s}"),
                            (None, Some(b)) => format!("Latest on GitHub: {b} (beta)"),
                            (Some(s), Some(b)) => format!("Latest on GitHub: {s} · {b} (beta)"),
                        };
                        ui.label(ui::muted(latest).size(12.5));
                        let (fix, feature) = version::next_choices(&tags, self.session.channel, &prefix);
                        // With no release yet there's nothing to count on from:
                        // the zip's own version (or what you type) stands.
                        let any = ls.is_some() || lb.is_some();
                        ui.horizontal_wrapped(|ui| {
                            if !any {
                                return;
                            }
                            if self.session.tag.trim() != fix
                                && ui.small_button(format!("Next fix: {fix}")).clicked()
                            {
                                new_tag = Some(fix.clone());
                            }
                            if self.session.tag.trim() != feature
                                && ui.small_button(format!("Next feature: {feature}")).clicked()
                            {
                                new_tag = Some(feature.clone());
                            }
                        });
                        if self.session.outcome.is_none() {
                            if let Some(newer) = version::newer_existing(&self.session.tag, &prefix, &tags) {
                                ui::banner(ui, WARNING(), |ui| {
                                    ui.add(egui::Label::new(format!(
                                        "{} counts as older than {newer}, so people on {newer} won't be offered it.",
                                        self.session.tag.trim()
                                    )).wrap());
                                    ui.horizontal_wrapped(|ui| {
                                        if ui::button(ui, "", &format!("Use {fix}")).clicked() {
                                            new_tag = Some(fix.clone());
                                        }
                                        if ui::button(ui, "", &format!("Use {feature}")).clicked() {
                                            new_tag = Some(feature.clone());
                                        }
                                    });
                                });
                            }
                        }
                    });
                    if let Some(t) = new_tag {
                        self.session.tag = t;
                    }
                    if self.session.tag != self.session.last_tag {
                        self.session.last_tag = self.session.tag.clone();
                        if let Some(v) = version::parse_tag(&self.session.tag, &self.config.tag_prefix) {
                            self.session.channel = if v.pre.is_empty() { Channel::Stable } else { Channel::Beta };
                        }
                        if !self.session.commit_edited {
                            self.session.commit_msg =
                                version::default_commit_message(mode == Mode::New, &self.session.tag);
                        }
                    }
                    ui.end_row();
                    ui.label("Commit message");
                    if ui::named(
                        ui.add(
                            egui::TextEdit::singleline(&mut self.session.commit_msg)
                                .desired_width(f32::INFINITY),
                        ),
                        "Commit message",
                    )
                    .changed()
                    {
                        self.session.commit_edited = true;
                    }
                    ui.end_row();
                    ui.label("After push");
                    ui.horizontal_wrapped(|ui| {
                        ui.checkbox(&mut self.session.archive_after, "Archive the zip");
                        ui.add_enabled_ui(self.session.archive_after, |ui| {
                            ui.radio_value(&mut self.session.archive_move, true, "Move");
                            ui.radio_value(&mut self.session.archive_move, false, "Copy");
                        });
                    });
                    ui.end_row();
                });
            if self.session.outcome.is_some() {
                // Pushed: the tag exists because this push just made it.
                let next = version::suggest(&tags, self.session.channel, &self.config.tag_prefix);
                ui.label(
                    RichText::new(format!(
                        "Pushed as {}. The next update would be {next}.",
                        self.session.tag
                    ))
                    .color(MUTED())
                    .size(12.5),
                );
            } else if let Err(e) = version::validate(
                &self.session.tag,
                self.session.channel,
                &self.config.tag_prefix,
                &tags,
            ) {
                ui.label(RichText::new(e).color(ERROR()).size(12.5));
            }
            if mode == Mode::New && !self.session.private {
                ui.label(
                    RichText::new(
                        "Public: anyone can see this repo. Secret-scan hits can't be overridden.",
                    )
                    .color(WARNING())
                    .size(12.5),
                );
            }
        });
    }

    pub(super) fn results_card(&mut self, ui: &mut egui::Ui) {
        let steps = self.log.steps();
        let Some(st) = self.session.stage.clone() else {
            let working = self.busy.is_some();
            ui::card_active(ui, working, |ui| {
                ui::section_title(ui, "PIPELINE");
                ui::stepper(ui, &STEPS, &steps, &WAITING);
                if self.busy.is_some() {
                    ui.horizontal_wrapped(|ui| {
                        ui.spinner();
                        ui.label(self.busy.clone().unwrap_or_default());
                    });
                } else {
                    ui.label(ui::muted("Click Stage & check to unpack the zip into its project folder and run the checks. Nothing goes to GitHub yet."));
                }
            });
            return;
        };
        ui::card(ui, |ui| {
            ui::section_title(ui, "UNPACKED");
            for n in st.notes.iter().chain(st.cleanup.lines().iter()) {
                ui.label(
                    RichText::new(format!("{} {n}", ui::icon::CHECK))
                        .color(SUCCESS())
                        .size(13.0),
                );
            }
            if let Some(ch) = &st.changes {
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new(format!("Changes: {}", ch.summary())).strong());
                    if ch.total() > 0
                        && ui
                            .small_button(if self.session.show_changes {
                                "Hide files"
                            } else {
                                "Show files"
                            })
                            .clicked()
                    {
                        self.session.show_changes = !self.session.show_changes;
                    }
                });
                if self.session.show_changes {
                    for f in &ch.added {
                        ui.label(ui::mono(format!("+ {f}")).color(SUCCESS()));
                    }
                    for f in &ch.changed {
                        ui.label(ui::mono(format!("~ {f}")).color(WARNING()));
                    }
                    for f in &ch.removed {
                        ui.label(ui::mono(format!("− {f}")).color(ERROR()));
                    }
                }
            }
            for (f, n) in &st.warn_files {
                ui.colored_label(
                    WARNING(),
                    format!("Large file: {f} ({})", util::human_size(*n)),
                );
            }
            for (f, n) in &st.block_files {
                ui.colored_label(
                    ERROR(),
                    format!(
                        "Over GitHub's 100 MB limit: {f} ({}). Remove it or add it to .gitignore.",
                        util::human_size(*n)
                    ),
                );
            }
        });
        ui.add_space(8.0);
        // ---- checks
        let working = self.busy.is_some();
        ui::card_active(ui, working, |ui| {
            ui.horizontal(|ui| {
                ui.label(ui::muted("CHECKS · LIKE GITHUB").size(12.0).strong());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    // How the checks run, at a glance.
                    let c = &self.config;
                    if c.github_fonts {
                        ui::pill(ui, "GitHub's fonts", MUTED(), true);
                    }
                    if c.clean_test_home {
                        if c.sandbox_checks && crate::sandbox::available() {
                            ui::pill(ui, "Sandboxed", MUTED(), true);
                        }
                        ui::pill(ui, "Clean home", MUTED(), true);
                    }
                });
            });
            ui.add_space(4.0);
            // Text here can't be selected with the mouse (that would copy it
            // unhidden): the Copy buttons hide private things first.
            ui.style_mut().interaction.selectable_labels = false;
            self.job_lanes(ui, &st.dir);
            match self.session.checks.clone() {
                None => {
                    ui.horizontal_wrapped(|ui| {
                        if self.busy.is_some() {
                            ui.spinner();
                            ui.label("Running the checks and the secret scan…");
                        } else {
                            ui.label(ui::muted("Checks haven't run yet."));
                        }
                    });
                    // The newest output, so a slow or stuck step is easy to see.
                    if self.busy.is_some() {
                        let lines = proc::live_lines();
                        if !lines.is_empty() {
                            ui::inset(ui, |ui| {
                                for l in &lines {
                                    ui.add(
                                        egui::Label::new(ui::mono(l).size(12.0).color(MUTED()))
                                            .truncate(),
                                    );
                                }
                            });
                        }
                        ui.ctx()
                            .request_repaint_after(std::time::Duration::from_millis(400));
                    }
                }
                Some(c) => self.check_results(ui, &st, &c),
            }
        });
        if self.session.outcome.is_some() {
            return;
        }
        ui.add_space(8.0);
        // ---- try it
        ui::card(ui, |ui| {
            ui::section_title(ui, "TRY IT BEFORE PUSHING");
            ui.horizontal_wrapped(|ui| {
                ui.label("Run command");
                let r = ui::named(
                    ui.add(
                        egui::TextEdit::singleline(&mut self.session.run_cmd)
                            .font(egui::TextStyle::Monospace)
                            .desired_width(260.0),
                    ),
                    "Run command",
                );
                if r.lost_focus() {
                    let slug = self.session.slug.clone();
                    let cmd = self.session.run_cmd.clone();
                    let _ =
                        ProjectsState::update(&self.paths, &slug, |p| p.run_command = Some(cmd));
                }
            });
            ui.horizontal_wrapped(|ui| {
                if self.run_child.is_none() {
                    if ui::button(ui, ui::icon::PLAY, "Run").clicked() {
                        match pipeline::run_in_terminal(
                            &st.dir,
                            &self.session.run_cmd,
                            self.session.pass_env,
                        ) {
                            Ok(c) => {
                                self.run_child = Some(c);
                                HealthState::record(&self.paths, "Run/Editor", true, "");
                            }
                            Err(e) => {
                                HealthState::record(&self.paths, "Run/Editor", false, &e);
                                self.fail(&e, "run");
                            }
                        }
                    }
                } else if ui::button(ui, ui::icon::X, "Stop").clicked() {
                    if let Some(mut c) = self.run_child.take() {
                        let _ = c.kill();
                    }
                }
                if ui::button(ui, ui::icon::FOLDER_OPEN, "Open folder").clicked() {
                    let _ = proc::xdg_open(&st.dir.display().to_string());
                }
                if ui::button(ui, ui::icon::PEN, "Open in editor").clicked() {
                    match pipeline::open_in_editor(&st.dir) {
                        Ok(_) => HealthState::record(&self.paths, "Run/Editor", true, ""),
                        Err(e) => {
                            HealthState::record(&self.paths, "Run/Editor", false, &e);
                            self.fail(&e, "editor");
                        }
                    }
                }
                if ui
                    .add_enabled(
                        self.busy.is_none(),
                        egui::Button::new(ui::semibold(
                            ui::with_icon(ui::icon::REFRESH, "Re-check"),
                            14.0,
                        ))
                        .min_size(egui::vec2(0.0, 36.0)),
                    )
                    .clicked()
                {
                    self.recheck();
                }
            });
            let mut pe = self.session.pass_env;
            if ui
                .checkbox(
                    &mut pe,
                    "Pass my environment (keys and tokens) to Run and tests",
                )
                .changed()
            {
                self.session.pass_env = pe;
                let slug = self.session.slug.clone();
                let _ = ProjectsState::update(&self.paths, &slug, |p| p.pass_env = pe);
            }
            ui.label(ui::muted("Run executes this project's code on your computer.").size(12.0));
        });
    }

    /// The checks' result: the verdict, anything secret-looking, what to do
    /// next, and the output.
    fn check_results(&mut self, ui: &mut egui::Ui, st: &StageResult, c: &Checks) {
        let failed = c.tests.status == TestStatus::Failed;
        let stopped = c.tests.status == TestStatus::Cancelled;
        let cant_run =
            c.tests.status == TestStatus::Skipped && c.tests.summary.starts_with("Can't run here");
        let (txt, col) = match c.tests.status {
            TestStatus::Passed => ("Checks passed", SUCCESS()),
            TestStatus::Failed => ("Checks failed", ERROR()),
            TestStatus::Skipped if cant_run => ("Can't run here", WARNING()),
            TestStatus::Skipped => ("Checks skipped", MUTED()),
            TestStatus::Cancelled => ("Checks stopped", WARNING()),
        };
        let rust_project = st.stack() == crate::detect::Stack::Rust;
        if let (true, Some(crate::rustcheck::RustState::UpdateAvailable { have, newest })) =
            (rust_project, &self.rust)
        {
            ui.add(
                egui::Label::new(ui::muted(format!(
                    "Your Rust ({have}) is older than the newest ({newest}), which GitHub uses: it may catch warnings this check didn't. Update Rust and Re-check to be sure."
                )).size(12.5))
                .wrap(),
            );
        }
        if failed {
            // A plain headline: what failed, and where.
            let tests = crate::preflight::failed_tests(&c.tests.tail);
            let jobs = crate::preflight::failed_jobs(&c.tests.tail);
            let what = match tests.len() {
                0 => "A check failed".to_string(),
                1 => "1 test failed".to_string(),
                n => format!("{n} tests failed"),
            };
            let place = match jobs.len() {
                0 => String::new(),
                1 => format!(" in the {} job", jobs[0]),
                _ => format!(" in {}", jobs.join(" and ")),
            };
            ui.add(
                egui::Label::new(ui::semibold(format!("{what}{place}"), 17.0).color(TEXT())).wrap(),
            );
        }
        ui::badge(ui, txt, col);
        // On its own line: a long job name in it wraps inside the card.
        let summary = if stopped {
            "You stopped the checks before they finished. Nothing was pushed.".to_string()
        } else {
            format!(
                "{} · {:.1}s",
                c.tests.summary,
                c.tests.duration.as_secs_f32()
            )
        };
        ui.add(egui::Label::new(ui::muted(summary)).wrap());
        if c.partial {
            ui::banner(ui, WARNING(), |ui| {
                ui.add(
                    egui::Label::new(
                        "Only the failed job was re-checked. Run the full check before pushing.",
                    )
                    .wrap(),
                );
                if self.busy.is_none()
                    && ui::primary(
                        ui,
                        &ui::with_icon(ui::icon::REFRESH, "Run the full check"),
                        true,
                    )
                    .clicked()
                {
                    self.recheck();
                }
            });
        }
        // Anything secret-looking comes before the advice: it blocks the push
        // whatever the tests say.
        ui.add_space(4.0);
        self.secrets_block(ui, c);
        if cant_run {
            ui.add(
                egui::Label::new(
                    "Something this step needs isn't on this computer, so it couldn't run here. That doesn't mean your project is broken. Install what's missing and Re-check, or push: GitHub runs this step with everything installed, and StowLock watches the build.",
                )
                .wrap(),
            );
            if c.tests.summary.contains('`') {
                ui.add(
                    egui::Label::new(ui::muted(
                        "If it's a Python tool, listing it in the project's requirements file lets StowLock install it here too, in its own environment.",
                    ).size(12.5))
                    .wrap(),
                );
            }
        }
        if c.tests.status == TestStatus::Passed && c.tests.summary.contains("can't run here") {
            ui.add(
                egui::Label::new(ui::muted(
                    "Everything that could run here passed. GitHub runs the rest, and StowLock watches the build. Nothing to install unless you want to run those steps here too.",
                ).size(12.5))
                .wrap(),
            );
        }
        if let Some(h) = c
            .tests
            .hint
            .clone()
            .filter(|h| h.starts_with("sudo pacman -S"))
        {
            let packages = crate::preflight::pacman_packages(&h);
            ui.horizontal_wrapped(|ui| {
                self.install_button(ui, &packages, true);
            });
        }
        if let Some(h) = &c.tests.hint {
            if is_command(h) {
                ui.horizontal_wrapped(|ui| {
                    if c.tests.status == TestStatus::Passed || cant_run {
                        ui.label(ui::muted("Optional, to run it here too:").size(12.5));
                    }
                    ui.add(egui::Label::new(ui::mono(h)).wrap());
                    if ui.small_button("Copy").clicked() {
                        ui.ctx().copy_text(h.clone());
                    }
                });
            } else {
                // An explanation, not something to paste into a terminal.
                ui::inset(ui, |ui| {
                    ui.add(egui::Label::new(RichText::new(h).size(13.0)).wrap());
                });
            }
        }
        if failed || stopped {
            // Safer choices first; pushing anyway comes last.
            let mut pick = false;
            let mut recheck = false;
            let mut report = false;
            let mut open = false;
            let mut copied = false;
            ui::banner(ui, WARNING(), |ui| {
                ui.label(ui::semibold("What to do next", 14.5));
                if stopped {
                    ui.add(
                        egui::Label::new(
                            "Run the checks again when you're ready, or load another zip.",
                        )
                        .wrap(),
                    );
                    ui.horizontal_wrapped(|ui| {
                        recheck |=
                            ui::primary(ui, &ui::with_icon(ui::icon::REFRESH, "Re-check"), true)
                                .clicked();
                        pick |= ui::button(ui, ui::icon::UPLOAD, "Load another zip…").clicked();
                        open |= ui::button(ui, ui::icon::FOLDER_OPEN, "Open folder").clicked();
                    });
                    ui.add_space(2.0);
                    ui.checkbox(
                        &mut self.session.override_tests,
                        "Push without the checks: I know it's safe",
                    );
                    return;
                }
                if c.tests.summary.contains(crate::preflight::MACHINE_SPECIFIC) {
                    ui.add(egui::Label::new(RichText::new(
                        "These failures look specific to this computer, not the project: they mention a running program or your home folder. They'll likely pass on GitHub. Quit the running copy and Re-check, or push anyway.",
                    ).color(WARNING())).wrap());
                }
                ui.add(egui::Label::new(
                    "1. Copy for AI, paste it into your AI chat, and load the fixed zip it gives you. The failed attempt is kept, never lost.",
                ).wrap());
                ui.add(
                    egui::Label::new("2. Or fix it in the project folder yourself, then Re-check.")
                        .wrap(),
                );
                ui.add(
                    egui::Label::new("3. If it looks like StowLock's fault, send a bug report.")
                        .wrap(),
                );
                ui.horizontal_wrapped(|ui| {
                    if ui::primary(ui, &ui::with_icon(ui::icon::COPY, "Copy for AI"), true)
                        .on_hover_text("Copies the failed checks and their messages, with a request to fix the project")
                        .clicked()
                    {
                        let lines: Vec<String> =
                            c.tests.tail.lines().map(|l| l.to_string()).collect();
                        let prompt = report::project_fix_prompt(&c.tests.summary, &lines);
                        let prompt = report::redact(&prompt, &self.redact_ctx());
                        ui.ctx().copy_text(prompt);
                        copied = true;
                    }
                    pick |= ui::button(ui, ui::icon::UPLOAD, "Load a fixed zip…").clicked();
                    open |= ui::button(ui, ui::icon::FOLDER_OPEN, "Open folder").clicked();
                    recheck |= ui::button(ui, ui::icon::REFRESH, "Re-check").clicked();
                    report |= ui::button(ui, ui::icon::BUG, "Bug report").clicked();
                });
                // Re-check only what failed: quicker while fixing.
                let jobs = crate::preflight::failed_jobs(&c.tests.tail);
                let several = c.tests.summary.contains(" · jobs: ") || c.partial;
                if several && self.busy.is_none() && !jobs.is_empty() {
                    ui.horizontal_wrapped(|ui| {
                        for job in &jobs {
                            if ui::button(ui, ui::icon::REFRESH, &format!("Re-check \"{job}\" only"))
                                .on_hover_text("Runs just this job again: quicker while you fix it. The full check runs before you push.")
                                .clicked()
                            {
                                self.recheck_jobs(vec![job.clone()]);
                            }
                        }
                    });
                }
                ui.add_space(2.0);
                ui.checkbox(
                    &mut self.session.override_tests,
                    "Push anyway: I know why the checks fail and it's safe",
                );
            });
            if copied {
                self.toast("Copied. Paste it into your AI chat, then load the fixed zip.");
            }
            if pick {
                self.pick_zip();
            }
            if open {
                let _ = proc::xdg_open(&st.dir.display().to_string());
            }
            if recheck {
                self.recheck();
            }
            if report {
                self.open_bug_report();
            }
        }
        if !c.tests.tail.is_empty() {
            ui.add_space(2.0);
            ui.horizontal_wrapped(|ui| {
                if ui
                    .small_button(ui::with_icon(ui::icon::COPY, "Copy output"))
                    .on_hover_text("Copies the result and output, with your name, folders and other private things hidden")
                    .clicked()
                {
                    let text = format!("{}\n{}", c.tests.summary, c.tests.tail);
                    ui.ctx()
                        .copy_text(report::redact(&text, &self.redact_ctx()));
                    self.toast("Copied, with private things hidden.");
                }
            });
            egui::CollapsingHeader::new("Output")
                .id_salt("testout")
                .default_open(failed)
                .show(ui, |ui| {
                    egui::ScrollArea::vertical()
                        .max_height(180.0)
                        .id_salt("testscroll")
                        .show(ui, |ui| {
                            ui.label(ui::mono(&c.tests.tail).size(12.0));
                        });
                });
        }
    }

    /// What the secret scan found, with a way to say "that isn't one".
    fn secrets_block(&mut self, ui: &mut egui::Ui, c: &Checks) {
        if c.secrets.is_empty() {
            ui.horizontal_wrapped(|ui| {
                ui::badge(ui, "0 secrets found", SUCCESS());
                if c.ignored_secrets > 0 {
                    ui.label(ui::muted(format!(
                        "({} marked not a secret)",
                        c.ignored_secrets
                    )));
                }
            });
            return;
        }
        let mut mark: Option<String> = None;
        ui::banner(ui, ERROR(), |ui| {
            ui.label(
                ui::semibold(
                    format!(
                        "{} possible secret{} in the files to push",
                        c.secrets.len(),
                        if c.secrets.len() == 1 { "" } else { "s" }
                    ),
                    14.5,
                )
                .color(ERROR()),
            );
            for h in &c.secrets {
                ui.horizontal_wrapped(|ui| {
                    ui.label(ui::mono(format!("{}:{}", h.file, h.line)));
                    ui.label(ui::muted(format!("{} {}", h.kind, h.masked)));
                    if ui.small_button("Not a secret").clicked() {
                        mark = Some(h.id.clone());
                    }
                });
            }
            ui.add(egui::Label::new(RichText::new("Take it out of the project and load a new zip. If this key was ever pushed before, removing it isn't enough: revoke it and make a new one.").color(WARNING()).size(12.5)).wrap());
        });
        if let Some(id) = mark {
            let slug = self.session.slug.clone();
            let _ = ProjectsState::update(&self.paths, &slug, |p| p.not_secret.push(id.clone()));
            if let Some(c) = self.session.checks.as_mut() {
                c.secrets.retain(|h| h.id != id);
                c.ignored_secrets += 1;
            }
        }
    }

    pub(super) fn completion_card(&mut self, ui: &mut egui::Ui, out: &PushOutcome) {
        let color = if out.nothing_to_push {
            MUTED()
        } else {
            SUCCESS()
        };
        ui::banner(ui, color, |ui| {
            if out.nothing_to_push {
                ui.label(
                    RichText::new("No changes to push. GitHub already has this version.").strong(),
                );
            } else {
                let t = out
                    .tag
                    .clone()
                    .map(|t| format!(" as {t}"))
                    .unwrap_or_default();
                ui.label(
                    RichText::new(format!(
                        "Pushed {}/{}{t}",
                        self.session.owner, self.session.slug
                    ))
                    .color(SUCCESS())
                    .strong()
                    .size(15.0),
                );
            }
            if let Some(a) = &out.archived {
                ui.label(ui::muted(format!("Zip archived to {}", util::tilde(a))));
            }
            for w in &out.warnings {
                ui.colored_label(WARNING(), w);
            }
            ui.horizontal_wrapped(|ui| {
                if ui
                    .button(RichText::new("Open on GitHub").strong())
                    .clicked()
                {
                    let _ = proc::xdg_open(&out.url);
                }
                if ui.button("Open Folder").clicked() {
                    let _ = proc::xdg_open(
                        &self
                            .config
                            .workspace()
                            .join(&self.session.slug)
                            .display()
                            .to_string(),
                    );
                }
                if ui.button("Start another").clicked() {
                    self.session = Session::new(&self.config);
                    self.log.begin_run("Ready");
                }
            });
        });
    }
}

/// Is this hint something to paste into a terminal (rather than a sentence)?
pub(super) fn is_command(hint: &str) -> bool {
    const STARTS: [&str; 16] = [
        "sudo ", "pacman ", "paru ", "yay ", "pip ", "pip3 ", "pipx ", "python ", "python3 ",
        "cargo ", "rustup ", "npm ", "pnpm ", "yarn ", "go ", "bash ",
    ];
    let h = hint.trim();
    !h.contains('\n') && STARTS.iter().any(|s| h.starts_with(s))
}
