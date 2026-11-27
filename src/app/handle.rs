// SPDX-License-Identifier: GPL-3.0-or-later
//! The App's handle code (part of the StowLock window).
use super::*;

impl App {
    // ------------------------------------------------------------ messages

    pub(super) fn handle(&mut self, m: Msg) {
        match m {
            Msg::Env(e) => {
                let first = !self.env.checked;
                let rebuild_report = self.tab == Tab::Report;
                self.env = *e;
                if let (Some(l), Some(names)) = (&self.env.login, &self.env.repo_names) {
                    let key = l.to_ascii_lowercase();
                    self.repo_lists.insert(key.clone(), names.clone());
                    self.repos_stale.remove(&key);
                    self.repos_asked.insert(key, Instant::now());
                } else if let Some(l) = &self.env.login {
                    self.repos_stale.insert(l.to_ascii_lowercase());
                }
                if self.identity_name.is_empty() {
                    self.identity_name = self.env.name.clone().unwrap_or_default();
                }
                if self.identity_email.is_empty() {
                    self.identity_email = self.env.email.clone().unwrap_or_default();
                }
                if self.session.owner.is_empty() {
                    self.session.owner = self.env.login.clone().unwrap_or_default();
                }
                if !self.session.slug.is_empty() && self.session.stage.is_none() {
                    self.recompute_mode();
                }
                if first && !self.env.ready() {
                    // First-run screen: the GitHub account panel.
                    self.tab = Tab::Settings;
                }
                if self.env.ready() {
                    HealthState::record(&self.paths, "Tools & sign-in", true, "");
                } else {
                    HealthState::attention(
                        &self.paths,
                        "Tools & sign-in",
                        "Finish setup in Settings → GitHub account",
                    );
                }
                self.health = HealthState::load(&self.paths);
                if rebuild_report {
                    // Sign-in details and account names arrived: refresh the preview.
                    self.bug.preview = self.build_bug_report();
                }
                if first && self.config.check_for_updates && updater::check_due(&self.paths) {
                    self.check_updates(false);
                }
                if first
                    && self.config.rust_update_check
                    && crate::rustcheck::check_due(&self.paths)
                {
                    self.check_rust();
                }
                if first {
                    // Project check caches unused for 30 days are removed.
                    let paths = self.paths.clone();
                    let log = self.log.clone();
                    std::thread::spawn(move || {
                        let (cleared, freed) = pipeline::clean_old_caches(&paths, 30);
                        if !cleared.is_empty() {
                            log.info(
                                "cache",
                                &format!(
                                    "Removed check caches unused for 30 days ({}, {})",
                                    cleared.len(),
                                    util::human_size(freed)
                                ),
                            );
                        }
                    });
                }
            }
            Msg::Repos(owner, r) => {
                let key = owner.to_ascii_lowercase();
                self.repos_loading.remove(&key);
                match r {
                    Ok(names) => {
                        self.repos_stale.remove(&key);
                        if self
                            .env
                            .login
                            .as_deref()
                            .is_some_and(|l| l.eq_ignore_ascii_case(&owner))
                        {
                            self.env.repo_names = Some(names.clone());
                        }
                        self.repo_lists.insert(key, names);
                        if owner.eq_ignore_ascii_case(&self.session.owner)
                            && self.session.stage.is_none()
                        {
                            self.recompute_mode();
                        }
                    }
                    Err(_) => {
                        self.repos_stale.insert(key);
                    }
                }
            }
            Msg::Picked(p) => {
                if let Some(p) = p {
                    self.load_zip(p);
                }
            }
            Msg::Staged(r) => match r {
                Ok(st) => {
                    self.session.run_cmd = st.run_command.clone();
                    self.session.pass_env = ProjectsState::load(&self.paths)
                        .get(&self.session.slug)
                        .pass_env;
                    if self.session.tag.is_empty() && self.session.mode == Mode::Update {
                        self.session.tag = String::new();
                    }
                    self.session.stage = Some(st);
                    self.record_health(self.session.mode == Mode::Update);
                    HealthState::record(&self.paths, "Zip intake", true, "");
                    HealthState::record(&self.paths, "Clean-up", true, "");
                    self.refresh_projects();
                }
                Err(e) => {
                    self.record_health(self.session.mode == Mode::Update);
                    self.fail(&e, "stage");
                }
            },
            Msg::Rerun(r) => match r {
                Ok(()) => {
                    self.log
                        .info("build", "Asked GitHub to re-run the failed jobs");
                    self.toast("GitHub is re-running the failed jobs. StowLock is watching.");
                    self.rewatch_build();
                }
                Err(e) => self.fail(&e, "build"),
            },
            Msg::Build(id, r) => {
                let Some(w) = self.build.as_mut().filter(|w| w.id == id) else {
                    return;
                };
                match r {
                    Ok(b) => {
                        let passed = matches!(b, crate::github::Build::Passed { .. });
                        let failed = matches!(b, crate::github::Build::Failed { .. });
                        let what = match &w.tag {
                            Some(t) => format!("{}/{} {t}", w.owner, w.slug),
                            None => format!("{}/{}", w.owner, w.slug),
                        };
                        w.status = Some(b);
                        let note = if failed {
                            self.log.warn("build", "GitHub's build failed");
                            Some((
                                "GitHub's build failed",
                                format!("{what}: open StowLock to see where, or re-run it."),
                            ))
                        } else if passed {
                            Some((
                                "GitHub's build passed",
                                format!("{what} is built on GitHub."),
                            ))
                        } else {
                            None
                        };
                        // Pushing StowLock itself: offer the new version now.
                        let ours = format!("{}/{}", w.owner, w.slug)
                            .eq_ignore_ascii_case(&self.config.update_repo);
                        if passed && ours && w.tag.is_some() && self.pacman_pkg.is_none() {
                            self.check_updates(true);
                        }
                        if let Some((title, body)) = note {
                            self.notify(title, &body);
                        }
                    }
                    Err(e) => self
                        .log
                        .warn("build", &format!("Couldn't follow GitHub's build: {e}")),
                }
            }
            Msg::ReleaseFixed(r) => match r {
                Ok(msg) => {
                    self.toast(&msg);
                    if let Some(o) = self.session.outcome.as_mut() {
                        o.release_failed = None;
                        if msg.starts_with("Removed") {
                            o.tag = None;
                        }
                    }
                    self.deleting_release = None;
                    self.confirm_remove_tag = false;
                    self.history = History::load(&self.paths);
                }
                Err(e) => self.fail(&e, "release"),
            },
            Msg::Checked(c) => {
                self.session.checks = Some(c);
                self.record_health(self.session.mode == Mode::Update);
            }
            Msg::Pushed(r) => {
                self.record_health(self.session.mode == Mode::Update);
                let owner = self.session.owner.clone();
                self.refresh_repos(&owner, true);
                match r {
                    Ok(out) => {
                        if out.archived.is_some() && self.session.archive_move {
                            self.session.zip = None;
                        }
                        for w in &out.warnings {
                            self.log.warn("push", w);
                        }
                        if !out.nothing_to_push {
                            HealthState::record(&self.paths, "History", true, "");
                        }
                        if let (false, Some(commit)) = (out.nothing_to_push, out.commit.clone()) {
                            self.watch_build(commit, out.tag.clone());
                        }
                        self.notify(
                            "Pushed to GitHub",
                            &format!(
                                "{}/{}: StowLock is watching GitHub's build.",
                                self.session.owner, self.session.slug
                            ),
                        );
                        self.session.outcome = Some(out);
                        self.history = History::load(&self.paths);
                        self.health = HealthState::load(&self.paths);
                        self.refresh_projects();
                        self.refresh_env();
                    }
                    Err(e) => {
                        self.notify("The push didn't finish", "Open StowLock to see why.");
                        self.fail(&e, "push");
                    }
                }
            }
            Msg::Archived(r) => {
                match r {
                    Ok(p) => {
                        HealthState::record(&self.paths, "Archive", true, "");
                        self.toast(&format!("Archived to {}", util::tilde(&p)));
                        self.session = Session::new(&self.config);
                    }
                    Err(e) => {
                        HealthState::record(&self.paths, "Archive", false, &e);
                        self.fail(&e, "archive");
                    }
                }
                self.health = HealthState::load(&self.paths);
                self.refresh_projects();
            }
            Msg::Projects(p) => {
                self.projects = p;
                self.refresh_attempts();
            }
            Msg::SelfTest(r, path) => {
                self.health = HealthState::load(&self.paths);
                self.toast(&format!("Self-test: {}", r.summary_line()));
                self.last_selftest = Some((*r, path));
            }
            Msg::FamilyTools(t) => self.tools = Some(t),
            Msg::FamilyToolsInstalled(r, recheck) => {
                self.tools_busy = false;
                match r {
                    Ok(m) => {
                        self.log.info("tools", &m);
                        self.toast(&m);
                        self.survey_tools();
                        if recheck && self.session.stage.is_some() && self.busy.is_none() {
                            self.recheck();
                        }
                    }
                    Err(e) => self.fail(&e, "tools"),
                }
            }
            Msg::CacheSize(n) => self.cache_size = Some(n),
            Msg::CachesCleared(r) => match r {
                Ok(n) => {
                    self.cache_size = Some(0);
                    self.toast(&format!(
                        "Cleared the check caches ({})",
                        util::human_size(n)
                    ));
                }
                Err(e) => self.fail(&e, "cache"),
            },
            Msg::Rust(state) => {
                self.log.info("rust", &state.line());
                self.rust = Some(state);
            }
            Msg::RustUpdated(r) => {
                self.rust_busy = false;
                match r {
                    Ok((v, what)) => {
                        self.log.info("rust", &what);
                        self.toast(&what);
                        self.rust = Some(crate::rustcheck::RustState::UpToDate { version: v });
                    }
                    Err(e) => self.fail(&e, "rust"),
                }
            }
            Msg::UpdateChecked(r) => match r {
                Ok(c) => {
                    HealthState::record(&self.paths, "Self-update", true, "");
                    let when = chrono::Local::now().format("%H:%M");
                    let newest = c.newest_tag.clone().unwrap_or_else(|| "none yet".into());
                    let how = if c.via_sign_in {
                        " (through your GitHub sign-in)"
                    } else {
                        ""
                    };
                    let newest_older = c.newest_tag.as_deref().is_some_and(|t| {
                        instance::is_older(t.trim_start_matches('v'), APP_VERSION)
                    });
                    let mut note = match &c.newer {
                        Some(rel) => format!("{} is available. You have {APP_VERSION}. Checked {when}{how}.", rel.label()),
                        None if newest_older => format!(
                            "You have the newest version ({APP_VERSION}). {newest} on GitHub counts as older than yours, so it isn't offered. Checked {when}{how}."
                        ),
                        None => format!(
                            "You have the newest version ({APP_VERSION}). Newest on GitHub: {newest}. Checked {when}{how}."
                        ),
                    };
                    if let Some(b) = &c.newer_beta {
                        note.push_str(&format!(
                            " A beta ({}) is available: switch the Update channel to Beta to try it.",
                            b.trim_start_matches('v')
                        ));
                    }
                    self.update_note = Some(note);
                    match c.newer {
                        Some(rel) => self.update = UpdateState::Available(rel),
                        None if self.update_manual => {
                            self.toast(&format!("You have the newest version ({APP_VERSION})."))
                        }
                        None => {}
                    }
                }
                Err(e) => {
                    self.log
                        .warn("update", &format!("Update check failed: {e}"));
                    self.update_note = Some(e.clone());
                    if self.update_manual {
                        self.toast(&e);
                    }
                }
            },
            Msg::UpdateApplied(r) => match r {
                Ok(p) => {
                    let tag = match &self.update {
                        UpdateState::Working(t) => t.clone(),
                        _ => String::new(),
                    };
                    self.update = UpdateState::Ready(tag, p);
                }
                Err(e) => {
                    HealthState::record(&self.paths, "Self-update", false, &e);
                    self.update = UpdateState::Failed(e);
                }
            },
            Msg::RolledBack(r) => match r {
                Ok((p, v)) => {
                    self.rollback_offer = None;
                    self.update = UpdateState::Ready(format!("v{v}"), p);
                }
                Err(e) => self.toast(&format!("Couldn't go back: {e}")),
            },
            Msg::SignIn(ev) => match ev {
                SignInEvent::Code(c) => {
                    self.egui_ctx.copy_text(c.clone());
                    self.signin.code = Some(c);
                    let _ = proc::xdg_open(auth::DEVICE_URL);
                }
                SignInEvent::Line(l) => {
                    self.signin.lines.push(crate::secrets::redact_secrets(&l));
                    if self.signin.lines.len() > 30 {
                        self.signin.lines.remove(0);
                    }
                }
                SignInEvent::Done(r) => {
                    self.signin.active = false;
                    match &r {
                        Ok(l) => self.log.info("auth", &format!("Signed in as {l}")),
                        Err(e) => self.log.warn("auth", e),
                    }
                    self.signin.result = Some(r);
                    self.refresh_env();
                }
            },
            Msg::Tools(o) => {
                self.tools_installing = false;
                self.log.command("setup", &o);
                self.tools_result = Some(if o.ok() {
                    "git and gh are installed.".into()
                } else {
                    format!(
                        "Install didn't finish: {}",
                        o.combined().lines().last().unwrap_or("")
                    )
                });
                self.refresh_env();
            }
            Msg::BugDownloaded(r) => {
                self.bug.saving = false;
                match r {
                    Ok(Some(p)) => {
                        self.log.info("report", "Bug report downloaded");
                        self.bug.saved_to = Some(p);
                    }
                    Ok(None) => {}
                    Err(e) => self.fail(&e, "report"),
                }
            }
            Msg::Done => self.busy = None,
        }
    }
}
