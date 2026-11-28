// SPDX-License-Identifier: GPL-3.0-or-later
//! The App's settings code (part of the StowLock window).
use super::*;

/// The Settings sections, in the order they're on the page.
const SETTINGS_SECTIONS: [&str; 8] = [
    "Updates",
    "Appearance",
    "GitHub account",
    "Folders & defaults",
    "Checks",
    "Privacy",
    "Help",
    "About",
];

impl App {
    /// Scroll to this place when its section was picked from the list.
    pub(super) fn jump_here(&mut self, ui: &mut egui::Ui, name: &str) {
        if self.settings_jump == Some(name) {
            ui.scroll_to_cursor(Some(egui::Align::TOP));
            self.settings_jump = None;
        }
    }

    /// Help: shortcuts, the tour, bug reports, what's new (what the Help menu had).
    fn help_card(&mut self, ui: &mut egui::Ui) {
        self.jump_here(ui, "Help");
        let mut cmd = None;
        ui::card(ui, |ui| {
            ui::section_title(ui, "HELP");
            ui.horizontal_wrapped(|ui| {
                if ui::button(ui, ui::icon::SEARCH, "Find a command  Ctrl+K").clicked() {
                    self.palette = Some(help::Palette::default());
                }
                if ui::button(ui, ui::icon::KEYBOARD, "Keyboard shortcuts  F1").clicked() {
                    cmd = Some(help::Cmd::Shortcuts);
                }
                if ui::button(ui, ui::icon::PLAY, "Take the tour").clicked() {
                    cmd = Some(help::Cmd::Tour);
                }
                if ui::button(ui, ui::icon::BUG, "Write a bug report").clicked() {
                    cmd = Some(help::Cmd::Go(Tab::Report));
                }
                if ui::button(ui, ui::icon::EXTERNAL, "What's new").clicked() {
                    let _ = proc::xdg_open(&format!(
                        "https://github.com/{}/releases",
                        crate::config::UPDATE_REPO
                    ));
                }
            });
        });
        if let Some(c) = cmd {
            self.run_cmd(c);
        }
    }

    pub(super) fn settings_screen(&mut self, ui: &mut egui::Ui) {
        ui::title(
            ui,
            "Settings",
            "Saved as you change them · Ctrl+, opens this page",
        );
        ui.add_space(6.0);
        // The sections, to jump straight to one.
        ui.horizontal_wrapped(|ui| {
            for s in SETTINGS_SECTIONS {
                if ui.selectable_label(false, s).clicked() {
                    self.settings_jump = Some(s);
                }
            }
        });
        ui.add_space(8.0);
        self.appearance_and_updates(ui);
        ui.add_space(8.0);
        self.jump_here(ui, "GitHub account");
        ui::card(ui, |ui| self.account_panel(ui));
        ui.add_space(8.0);
        self.jump_here(ui, "Folders & defaults");
        ui::card(ui, |ui| {
            ui::section_title(ui, "FOLDERS & DEFAULTS");
            egui::Grid::new("general")
                .max_col_width((ui.available_width() - 130.0).max(160.0))
                .num_columns(2)
                .spacing([12.0, 8.0])
                .show(ui, |ui| {
                    ui.label("Workspace folder");
                    ui::named(
                        ui.add(
                            egui::TextEdit::singleline(&mut self.config.workspace_dir)
                                .font(egui::TextStyle::Monospace)
                                .desired_width(300.0),
                        ),
                        "Workspace folder",
                    );
                    ui.end_row();
                    ui.label("New repos");
                    ui::switch(ui, &mut self.config.default_private, "Private by default");
                    ui.end_row();
                    ui.label("Default license");
                    let cb = egui::ComboBox::from_id_salt("deflic")
                        .selected_text(self.config.default_license.clone())
                        .show_ui(ui, |ui| {
                            ui.selectable_value(
                                &mut self.config.default_license,
                                "none".to_string(),
                                "none",
                            );
                            ui.selectable_value(
                                &mut self.config.default_license,
                                "MIT".to_string(),
                                "MIT",
                            );
                            ui.selectable_value(
                                &mut self.config.default_license,
                                "GPL-3.0".to_string(),
                                "GPL-3.0",
                            );
                        });
                    ui::named(cb.response, "Default license");
                    ui.end_row();
                    ui.label("License holder");
                    ui::named(
                        ui.add(
                            egui::TextEdit::singleline(&mut self.config.license_holder)
                                .hint_text("Your name")
                                .desired_width(300.0),
                        ),
                        "License holder",
                    );
                    ui.end_row();
                    ui.label("After push");
                    ui.horizontal_wrapped(|ui| {
                        ui::switch(ui, &mut self.config.archive_after_push, "Archive the zip");
                        ui.radio_value(&mut self.config.archive_mode, "move".to_string(), "Move");
                        ui.radio_value(&mut self.config.archive_mode, "copy".to_string(), "Copy");
                    });
                    ui.end_row();
                    ui.label("Tag prefix");
                    ui::named(
                        ui.add(
                            egui::TextEdit::singleline(&mut self.config.tag_prefix)
                                .desired_width(60.0),
                        ),
                        "Tag prefix",
                    );
                    ui.end_row();
                });
        });
        ui.add_space(8.0);
        self.jump_here(ui, "Checks");
        ui::card(ui, |ui| {
            ui::section_title(ui, "CHECKS");
            ui::switch(
                ui,
                &mut self.config.run_tests_before_push,
                "Run tests before pushing",
            );
            ui::switch(ui, &mut self.config.match_github_checks, "Match GitHub's checks: run the project's own GitHub workflow steps")
            .on_hover_text("Uses the same commands and settings as the project's .github/workflows files, and lists the steps only GitHub can run.");
            ui::switch(ui, &mut self.config.clean_test_home, "Like GitHub's clean machine: a fresh home folder, without your desktop session")
            .on_hover_text("Tests don't see your settings, app data or running desktop, as on GitHub. Tool caches are kept, so it stays fast.");
            ui::switch(ui, &mut self.config.github_fonts, "Only GitHub's fonts: run checks with just the fonts GitHub's machine has (DejaVu, Liberation, emoji)")
            .on_hover_text("Text that only fits with your own fonts fails here too, before you push. Fonts a project bundles and loads itself still work.");
            let bwrap = crate::sandbox::available();
            ui.add_enabled_ui(bwrap && self.config.clean_test_home, |ui| {
                ui::switch(ui, &mut self.config.sandbox_checks, "Sandbox: checks can't see your home folder, keys or running apps (bubblewrap)")
                .on_hover_text("The project's scripts and tests see only the project, the build tools and a fresh home, like GitHub's machine. Stopping a check stops everything in it.");
                ui.add_enabled_ui(self.config.sandbox_checks, |ui| {
                    ui::switch(ui, &mut self.config.offline_checks, "Offline: no network during checks (finds steps that quietly download)");
                });
            });
            if !bwrap {
                ui.add(
                    egui::Label::new(
                        ui::muted("The sandbox needs bubblewrap: sudo pacman -S bubblewrap (or Install missing tools in Health)")
                            .size(12.5),
                    )
                    .wrap(),
                );
            }
            let act_ready = proc::which("act").is_some() && proc::which("docker").is_some();
            ui.add_enabled_ui(act_ready, |ui| {
                ui::switch(
                    ui,
                    &mut self.config.exact_github_checks,
                    "Exact mode: run the workflows exactly as GitHub would (act)",
                );
            });
            if !act_ready {
                ui.add(
                    egui::Label::new(
                        ui::muted("Exact mode needs act and Docker: sudo pacman -S act docker")
                            .size(12.5),
                    )
                    .wrap(),
                );
            }
            ui.horizontal_wrapped(|ui| {
                ui.label("Test time limit");
                ui.add(
                    egui::DragValue::new(&mut self.config.test_timeout_secs)
                        .range(30..=3600)
                        .suffix(" s"),
                );
            });
            ui::switch(
                ui,
                &mut self.config.block_on_secrets,
                "Block pushes when possible secrets are found",
            );
            ui.horizontal_wrapped(|ui| {
                if self.cache_size.is_none() {
                    self.cache_size = Some(u64::MAX); // measuring
                    let paths = self.paths.clone();
                    self.spawn_bg(move || Msg::CacheSize(pipeline::cache_size(&paths)));
                }
                let size = match self.cache_size {
                    Some(u64::MAX) | None => "measuring…".to_string(),
                    Some(n) => util::human_size(n),
                };
                ui.label(format!("Check caches: {size}"))
                    .on_hover_text("Builds, Python environments and tools StowLock keeps so checks are fast. A project's cache is removed after 30 days without a check.");
                if ui
                    .add_enabled(self.busy.is_none(), egui::Button::new("Clear").small())
                    .clicked()
                {
                    let paths = self.paths.clone();
                    self.spawn_bg(move || Msg::CachesCleared(pipeline::clear_caches(&paths)));
                }
            });
        });
        ui.add_space(8.0);
        self.jump_here(ui, "Privacy");
        ui::card(ui, |ui| {
            ui::section_title(ui, "PRIVACY");
            ui.horizontal_wrapped(|ui| {
                ui.label("Delete logs and reports after");
                ui.add(
                    egui::DragValue::new(&mut self.config.report_retention_days)
                        .range(1..=365)
                        .suffix(" days"),
                );
            });
            ui.horizontal_wrapped(|ui| {
                if ui.button("Clear logs and reports").clicked() {
                    match report::clear_logs_and_reports(&self.paths, &self.log) {
                        Ok(n) => self.toast(&format!("Removed {n} file(s).")),
                        Err(e) => self.fail(&e, "privacy"),
                    }
                }
                if ui.button("Clear push history").clicked() {
                    let _ = History::clear(&self.paths);
                    self.history = History::default();
                    self.toast("Push history cleared.");
                }
            });
            egui::CollapsingHeader::new("What StowLock stores and sends").id_salt("privacy").show(ui, |ui| {
            ui.label("Sends to GitHub (through gh and git): the repos you choose to push, sign-in, your repo list.");
            ui.label(format!("Checks {} for new versions at most every 6 hours (no personal data sent).", self.config.update_repo));
            ui.label("Bug reports leave your computer only when you copy them or open an issue, after a preview.");
            ui.label("No tracking, analytics or automatic crash uploads.");
            ui.label(format!("Stores settings in {} and history, logs and reports in {} (only your account can read them).", util::tilde(&self.paths.config_dir), util::tilde(&self.paths.state_dir)));
        });
        });
        ui.add_space(8.0);
        self.help_card(ui);
        ui.add_space(8.0);
        self.jump_here(ui, "About");
        ui::card(ui, |ui| {
            ui::section_title(ui, "ABOUT");
            ui.add(
                egui::Label::new(format!(
                    "StowLock {APP_VERSION} · Free software under the GNU GPL v3 · github.com/{}",
                    crate::config::UPDATE_REPO
                ))
                .wrap(),
            );
            ui.add_space(8.0);
            if ui
                .button(RichText::new("Uninstall StowLock…").color(ERROR()))
                .clicked()
            {
                self.show_uninstall = true;
                self.uninstall.result = None;
                self.uninstall.plan = None;
                self.uninstall.typed.clear();
            }
        });
    }

    pub(super) fn account_panel(&mut self, ui: &mut egui::Ui) {
        ui::section_title(ui, "GITHUB ACCOUNT");
        if !self.env.checked {
            ui.horizontal_wrapped(|ui| {
                ui.spinner();
                ui.label("Checking…");
            });
            return;
        }
        // 1. tools
        if !self.env.git || !self.env.gh {
            ui::banner(ui, WARNING(), |ui| {
                ui.label(format!(
                    "Missing: {}",
                    [
                        (!self.env.git).then_some("git"),
                        (!self.env.gh).then_some("GitHub's gh tool")
                    ]
                    .into_iter()
                    .flatten()
                    .collect::<Vec<_>>()
                    .join(" and ")
                ));
                ui.horizontal_wrapped(|ui| {
                    if self.tools_installing {
                        ui.spinner();
                        ui.label("Installing… (enter your password in the dialog)");
                    } else if ui
                        .button(RichText::new("Install missing tools").strong())
                        .clicked()
                    {
                        self.tools_installing = true;
                        self.spawn_bg(|| Msg::Tools(auth::install_tools()));
                    }
                });
                egui::CollapsingHeader::new(ui::muted("Show details"))
                    .id_salt("tooldetails")
                    .show(ui, |ui| {
                        ui.label(ui::mono("sudo pacman -S --needed git github-cli"));
                    });
            });
            if let Some(r) = &self.tools_result {
                ui.label(r);
            }
            return;
        }
        // 2. sign in
        if self.signin.active || self.signin.code.is_some() && self.env.login.is_none() {
            ui::banner(ui, ACCENT(), |ui| {
                if let Some(code) = &self.signin.code {
                    ui.label("Your one-time code (already copied):");
                    ui.label(
                        RichText::new(code)
                            .monospace()
                            .size(28.0)
                            .strong()
                            .color(ACCENT()),
                    );
                    ui.horizontal_wrapped(|ui| {
                        if ui.button("Copy").clicked() {
                            ui.ctx().copy_text(code.clone());
                        }
                        if ui.button("Open GitHub").clicked() {
                            let _ = proc::xdg_open(auth::DEVICE_URL);
                        }
                    });
                    ui.label(
                        "Paste it on the GitHub page, then approve. This window updates by itself.",
                    );
                } else {
                    ui.horizontal_wrapped(|ui| {
                        ui.spinner();
                        ui.label("Starting sign-in…");
                    });
                }
                if self.signin.active {
                    ui.horizontal_wrapped(|ui| {
                        ui.spinner();
                        ui.label("Approve StowLock in your browser…");
                        if ui.button("Cancel").clicked() {
                            self.signin.cancel.store(true, Ordering::Relaxed);
                        }
                    });
                }
                if self.signin.code.is_none() && !self.signin.lines.is_empty() {
                    ui.label(ui::muted(format!(
                        "Couldn't read the code automatically. Open {} and follow gh's message:",
                        auth::DEVICE_URL
                    )));
                    ui.label(ui::mono(self.signin.lines.join("\n")).size(12.0));
                }
            });
        }
        if let Some(Err(e)) = &self.signin.result {
            ui.colored_label(WARNING(), e);
        }
        if self.env.accounts.is_empty() && self.env.login.is_none() {
            if !self.signin.active
                && ui
                    .button(RichText::new("Sign in with GitHub").strong())
                    .clicked()
            {
                self.start_sign_in();
            }
        } else {
            let mut switch: Option<String> = None;
            let mut sign_out: Option<String> = None;
            for a in self.env.accounts.clone() {
                ui.horizontal_wrapped(|ui| {
                    ui.label(
                        RichText::new(if a.active {
                            ui::icon::CIRCLE_CHECK
                        } else {
                            ui::icon::CIRCLE
                        })
                        .color(if a.active { SUCCESS() } else { MUTED() }),
                    );
                    ui.label(ui::mono(&a.login).strong());
                    if a.active {
                        ui.label(ui::muted("signed in"));
                    } else if ui.small_button("Switch").clicked() {
                        switch = Some(a.login.clone());
                    }
                    if !a.secure {
                        ui::badge(ui, "plain-file token", WARNING());
                        if ui.small_button("Move to secure storage").clicked() {
                            self.start_sign_in();
                        }
                    }
                    if ui.small_button("Sign out").clicked() {
                        sign_out = Some(a.login.clone());
                    }
                });
            }
            if !self.env.orgs.is_empty() {
                ui.label(ui::muted(format!(
                    "Organizations: {}",
                    self.env.orgs.join(", ")
                )));
            }
            ui.horizontal_wrapped(|ui| {
                if !self.signin.active && ui.button("Add another account").clicked() {
                    self.start_sign_in();
                }
                if ui.button("Refresh").clicked() {
                    self.refresh_env();
                }
            });
            if let Some(l) = switch {
                let o = auth::switch_account(&l);
                self.log.command("auth", &o);
                self.refresh_env();
            }
            if let Some(l) = sign_out {
                let o = auth::sign_out(&l);
                self.log.command("auth", &o);
                self.refresh_env();
            }
        }
        // 3. identity
        ui.add_space(8.0);
        ui.label(RichText::new("Name and email for commits").strong());
        egui::Grid::new("ident")
            .max_col_width((ui.available_width() - 130.0).max(160.0))
            .num_columns(2)
            .spacing([12.0, 6.0])
            .show(ui, |ui| {
                ui.label("Name");
                ui::named(
                    ui.add(
                        egui::TextEdit::singleline(&mut self.identity_name).desired_width(280.0),
                    ),
                    "Name for commits",
                );
                ui.end_row();
                ui.label("Email");
                ui::named(
                    ui.add(
                        egui::TextEdit::singleline(&mut self.identity_email).desired_width(280.0),
                    ),
                    "Email for commits",
                );
                ui.end_row();
            });
        ui.horizontal_wrapped(|ui| {
            if ui.button("Use my private GitHub email").clicked() {
                if let Some(e) = auth::noreply_email() {
                    self.identity_email = e;
                } else {
                    self.toast("Sign in to GitHub first.");
                }
            }
            let changed = Some(self.identity_name.clone()) != self.env.name
                || Some(self.identity_email.clone()) != self.env.email;
            if ui
                .add_enabled(
                    changed
                        && !self.identity_name.trim().is_empty()
                        && self.identity_email.contains('@'),
                    egui::Button::new("Save"),
                )
                .clicked()
            {
                match git::set_global_identity(
                    self.identity_name.trim(),
                    self.identity_email.trim(),
                ) {
                    Ok(_) => {
                        self.toast("Saved.");
                        self.refresh_env();
                    }
                    Err(e) => self.fail(&e, "settings"),
                }
            }
        });
    }

    pub(super) fn confirm_issue_banner(&mut self, ui: &mut egui::Ui) {
        let who = self
            .env
            .login
            .clone()
            .unwrap_or_else(|| "your account".into());
        let repo = self.config.update_repo.clone();
        // Public or private? Asked once, when the banner first shows.
        if self.bug.issue_private.is_none() {
            let o = proc::run(
                "gh",
                &[
                    "repo",
                    "view",
                    &repo,
                    "--json",
                    "visibility",
                    "-q",
                    ".visibility",
                ],
                &proc::Opts::default_timeout().with_timeout(10),
            );
            self.bug.issue_private =
                Some(o.ok() && o.stdout_trim().eq_ignore_ascii_case("private"));
        }
        let private = self.bug.issue_private == Some(true);
        ui::banner(ui, WARNING(), |ui| {
            ui.add(
                egui::Label::new(if private {
                    format!("This creates an issue on {repo}, a private repo: only the owner can see it. It's opened under the account {who}.")
                } else {
                    format!("This creates a PUBLIC issue on {repo}: anyone can read it. It's opened under the account {who}.")
                })
                .wrap(),
            );
            ui.add(
                egui::Label::new(ui::muted("GitHub's form is filled in with the summary and system details only. The full report is saved as a file for you to attach.").size(12.5))
                    .wrap(),
            );
            ui.horizontal_wrapped(|ui| {
                if ui.button("Open the issue form").clicked() {
                    match report::save_report(&self.paths, "bugreport", &self.bug.preview) {
                        Ok(p) => {
                            let name = p
                                .file_name()
                                .map(|n| n.to_string_lossy().to_string())
                                .unwrap_or_default();
                            let body = report::issue_body(&self.bug.preview, &name);
                            let url = report::new_issue_url(&repo, "Bug report", &body);
                            let _ = proc::xdg_open(&url);
                            // Show the file so it can be dragged into the form.
                            if let Some(dir) = p.parent() {
                                let _ = proc::xdg_open(&dir.display().to_string());
                            }
                            self.toast(&format!(
                                "Attach {name} to the issue (its folder is open)."
                            ));
                        }
                        Err(e) => self.fail(&e, "report"),
                    }
                    self.bug.confirm_issue = false;
                }
                if ui.button("Cancel").clicked() {
                    self.bug.confirm_issue = false;
                }
            });
        });
    }

    pub(super) fn uninstall_window(&mut self, ctx: &egui::Context) {
        let mut open = true;
        let mut do_exec = false;
        let mut quit = false;
        ui_window(ctx, "Uninstall StowLock", 580.0)
            .open(&mut open)
            .show(ctx, |ui| {
                if let Some(res) = &self.uninstall.result {
                    match res {
                        Ok(out) => {
                            ui.label(
                                RichText::new("StowLock was uninstalled.")
                                    .strong()
                                    .color(SUCCESS()),
                            );
                            for r in &out.removed {
                                ui.label(ui::mono(format!("removed {r}")).size(12.0));
                            }
                            for t in &out.trashed {
                                ui.label(ui::mono(format!("moved to Trash: {t}")).size(12.0));
                            }
                            for p in &out.problems {
                                ui.colored_label(WARNING(), p);
                            }
                            if !out.trashed.is_empty() {
                                ui.horizontal_wrapped(|ui| {
                                    ui.label("The Trash still holds those files.");
                                    if ui.link("Empty Trash").clicked() {
                                        let _ = proc::xdg_open("trash:/");
                                    }
                                });
                            }
                            if ui.button("Close StowLock").clicked() {
                                quit = true;
                            }
                        }
                        Err(e) => {
                            ui.colored_label(ERROR(), e);
                        }
                    }
                    return;
                }
                let before = self.uninstall.level;
                ui.radio_value(
                    &mut self.uninstall.level,
                    UninstallLevel::AppOnly,
                    "App only: the program, launcher entry, icon and cache",
                );
                ui.radio_value(
                    &mut self.uninstall.level,
                    UninstallLevel::AppAndData,
                    "App and data: also settings, push history, logs and reports",
                );
                ui.radio_value(
                    &mut self.uninstall.level,
                    UninstallLevel::Everything,
                    RichText::new("Everything: also your projects folder and archived zips")
                        .color(ERROR()),
                );
                if before != self.uninstall.level || self.uninstall.plan.is_none() {
                    self.uninstall.plan = Some(uninstall::plan(
                        self.uninstall.level,
                        &self.paths,
                        &self.config,
                        &self.install_inst,
                        &util::home_dir(),
                    ));
                    self.uninstall.typed.clear();
                }
                let plan = self.uninstall.plan.clone().unwrap();
                egui::ScrollArea::vertical()
                    .max_height(240.0)
                    .id_salt("uplan")
                    .show(ui, |ui| {
                        ui.label(ui::mono(plan.describe()).size(12.0));
                    });
                let mut allowed = true;
                if plan.level == UninstallLevel::Everything {
                    if !plan.refused.is_empty() {
                        allowed = false;
                    } else {
                        if !plan.unpushed.is_empty() {
                            ui.horizontal_wrapped(|ui| {
                                ui.colored_label(
                                    WARNING(),
                                    "Some projects have work that isn't on GitHub.",
                                );
                                if ui.button("Open Folder").clicked() {
                                    let _ = proc::xdg_open(&plan.workspace.display().to_string());
                                }
                            });
                        }
                        ui.horizontal_wrapped(|ui| {
                            ui.label("Type DELETE to confirm:");
                            ui::named(
                                ui.add(
                                    egui::TextEdit::singleline(&mut self.uninstall.typed)
                                        .desired_width(100.0)
                                        .font(egui::TextStyle::Monospace),
                                ),
                                "Type DELETE to confirm",
                            );
                        });
                        ui.checkbox(
                            &mut self.uninstall.permanent,
                            "Delete permanently instead of moving to the Trash",
                        );
                        allowed = self.uninstall.typed.trim() == "DELETE";
                    }
                }
                ui.horizontal_wrapped(|ui| {
                    if ui
                        .add_enabled(
                            allowed,
                            egui::Button::new(
                                RichText::new(format!("Uninstall ({})", plan.level.label()))
                                    .color(ERROR())
                                    .strong(),
                            ),
                        )
                        .clicked()
                    {
                        do_exec = true;
                    }
                    if ui.button("Cancel").clicked() {
                        self.show_uninstall = false;
                    }
                });
            });
        if do_exec {
            if let Some(plan) = &self.uninstall.plan {
                let confirmed = self.uninstall.typed.trim() == "DELETE";
                let r = uninstall::execute(plan, confirmed, self.uninstall.permanent);
                if let Ok(out) = &r {
                    if out.problems.is_empty() {
                        HealthState::record(&self.paths, "Uninstall", true, "");
                    }
                }
                self.uninstall.result = Some(r);
            }
        }
        if quit {
            report::mark_stopped(&self.paths);
            std::process::exit(0);
        }
        if !open {
            self.show_uninstall = false;
        }
    }
}
