// SPDX-License-Identifier: GPL-3.0-or-later
//! The App's jobs code (part of the StowLock window).
use super::*;

impl App {
    // ------------------------------------------------------------ plumbing

    pub(super) fn pctx(&self) -> pipeline::Ctx {
        pipeline::Ctx {
            paths: self.paths.clone(),
            config: self.config.clone(),
            gh: self.gh.clone(),
            log: self.log.clone(),
            cancel: self.cancel.clone(),
            trash_dir: None,
        }
    }

    pub(super) fn send(tx: &Sender<Msg>, ctx: &egui::Context, m: Msg) {
        let _ = tx.send(m);
        ctx.request_repaint();
    }

    /// Run a pipeline job on a worker thread. Only one at a time.
    pub(super) fn spawn_job(
        &mut self,
        label: &str,
        f: impl FnOnce(pipeline::Ctx) -> Msg + Send + 'static,
    ) {
        if self.busy.is_some() {
            self.toast("Please wait for the current step to finish.");
            return;
        }
        self.cancel.store(false, Ordering::Relaxed);
        self.busy = Some(label.to_string());
        self.error = None;
        let pctx = self.pctx();
        let tx = self.tx.clone();
        let ctx = self.egui_ctx.clone();
        std::thread::spawn(move || {
            let m = f(pctx);
            Self::send(&tx, &ctx, m);
            Self::send(&tx, &ctx, Msg::Done);
        });
    }

    /// Background work that doesn't block the pipeline (checks, scans).
    pub(super) fn spawn_bg(&self, f: impl FnOnce() -> Msg + Send + 'static) {
        let tx = self.tx.clone();
        let ctx = self.egui_ctx.clone();
        std::thread::spawn(move || Self::send(&tx, &ctx, f()));
    }

    pub(super) fn toast(&mut self, s: &str) {
        self.toast = Some((s.to_string(), Instant::now()));
    }

    pub(super) fn fail(&mut self, raw: &str, step: &str) {
        let f = errors::explain(raw, step);
        self.log.error(step, raw);
        self.error = Some((f, raw.to_string()));
    }

    pub(super) fn save_config(&mut self) {
        if self.config != self.saved_config {
            match self.config.save(&self.paths) {
                Ok(_) => {
                    self.saved_config = self.config.clone();
                    HealthState::record(&self.paths, "Settings & state", true, "");
                }
                Err(e) => {
                    HealthState::record(&self.paths, "Settings & state", false, &e);
                    self.fail(&e, "settings");
                }
            }
        }
    }

    pub(super) fn refresh_env(&mut self) {
        let gh = self.gh.clone();
        self.spawn_bg(move || {
            let mut e = EnvStatus {
                checked: true,
                git: proc::which("git").is_some(),
                gh: proc::which("gh").is_some(),
                ..Default::default()
            };
            let (n, m) = git::global_identity();
            e.name = n;
            e.email = m;
            if e.gh {
                e.accounts = gh.accounts();
                if let Ok(login) = gh.login() {
                    e.orgs = gh.orgs();
                    match gh.list_repos(&login) {
                        Ok(r) => {
                            e.repos = r.iter().map(|x| (x.name.clone(), x.private)).collect();
                            e.repo_names = Some(r.into_iter().map(|r| r.name).collect());
                        }
                        Err(_) => e.offline = true,
                    }
                    e.login = Some(login);
                } else if !e.accounts.is_empty() {
                    e.offline = true;
                }
            }
            Msg::Env(Box::new(e))
        });
    }

    pub(super) fn refresh_projects(&self) {
        let paths = self.paths.clone();
        let config = self.config.clone();
        self.spawn_bg(move || Msg::Projects(pipeline::scan_projects(&paths, &config)));
    }

    pub(super) fn record_health(&mut self, update_mode: bool) {
        for s in self.log.steps() {
            for area in health::areas_for_step(&s.name, update_mode) {
                match s.status {
                    crate::logbook::StepStatus::Passed => {
                        HealthState::record(&self.paths, area, true, "")
                    }
                    crate::logbook::StepStatus::Failed => {
                        let why = s.detail.lines().next().unwrap_or("");
                        let broken = health::counts_against_stowlock(&s.name, why);
                        HealthState::record(
                            &self.paths,
                            area,
                            !broken,
                            if broken { why } else { "" },
                        )
                    }
                    _ => {}
                }
            }
        }
        self.health = HealthState::load(&self.paths);
    }

    // ------------------------------------------------------------ actions

    pub(super) fn pick_zip(&mut self) {
        let start = dirs::download_dir().unwrap_or_else(util::home_dir);
        self.spawn_bg(move || Msg::Picked(pick_file_dialog(&start)));
    }

    pub(super) fn load_zip(&mut self, zip: PathBuf) {
        if self.busy.is_some() {
            self.toast("Please wait for the current step to finish.");
            return;
        }
        if !zip
            .extension()
            .map(|x| x.eq_ignore_ascii_case("zip"))
            .unwrap_or(false)
        {
            self.toast("That isn't a .zip file.");
            return;
        }
        let name = zip
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        let mut s = Session::new(&self.config);
        s.slug = detect::slug_from_filename(&name);
        s.owner = self.env.login.clone().unwrap_or_default();
        // A longer zip name ("gimbot-v0.4.2-house-standard") still belongs to
        // the existing project it starts with.
        {
            let mut known: Vec<String> = self.projects.iter().map(|p| p.slug.clone()).collect();
            if let Some(r) = self.github_repos(&s.owner) {
                known.extend(r.iter().cloned());
            }
            if let Some(m) = detect::existing_match(&s.slug, known.iter().map(String::as_str)) {
                s.slug = m;
            }
        }
        s.description = format!("{}, a vibe-coded project", detect::title_from_slug(&s.slug));
        s.zip = Some(zip.clone());
        s.zip_info = crate::zipinfo::read(&zip);
        s.same_zip = s
            .zip_info
            .as_ref()
            .is_some_and(|i| crate::zipinfo::same_as_last(&self.paths, &s.slug, &i.sha));
        s.newer_copy = crate::zipinfo::newer_copy(&zip);
        s.found_version = detect::detect_version(&zip, &self.config.tag_prefix);
        // Fill in the version (and Stable/Beta) from the zip's own files.
        if let Some(fv) = &s.found_version {
            if let Some(v) = version::parse_tag(&fv.tag, &self.config.tag_prefix) {
                s.tag = fv.tag.clone();
                s.channel = if v.pre.is_empty() {
                    Channel::Stable
                } else {
                    Channel::Beta
                };
            }
        }
        let label = format!("Stage {}", s.slug);
        self.session = s;
        self.error = None;
        self.log.begin_run(&label);
        let about = self
            .session
            .zip_info
            .as_ref()
            .map(|i| format!(" ({})", i.describe()))
            .unwrap_or_default();
        self.log
            .info("intake", &format!("Loaded {}{about}", util::tilde(&zip)));
        if self.session.same_zip {
            self.toast("This is the same zip as last time.");
        }
        self.tab = Tab::Stage;
        self.recompute_mode();
    }

    /// Load an existing workspace project (to try, re-check and push edits made by hand).
    pub(super) fn load_project(&mut self, p: &ProjectInfo) {
        let mut s = Session::new(&self.config);
        s.slug = p.slug.clone();
        s.owner = self
            .project_owner_repo(p)
            .map(|o| o.0)
            .or(ProjectsState::load(&self.paths).get(&p.slug).owner)
            .or(self.env.login.clone())
            .unwrap_or_default();
        s.description = format!("{}, a vibe-coded project", detect::title_from_slug(&s.slug));
        s.archive_after = false;
        let st = StageResult {
            dir: p.dir.clone(),
            stack: Some(p.stack),
            run_command: ProjectsState::load(&self.paths)
                .get(&p.slug)
                .run_command
                .unwrap_or_else(|| detect::default_run_command(&p.dir, p.stack)),
            ..Default::default()
        };
        s.run_cmd = st.run_command.clone();
        s.pass_env = ProjectsState::load(&self.paths).get(&p.slug).pass_env;
        s.stage = Some(st);
        self.session = s;
        self.recompute_mode();
        if self.session.mode == Mode::Update {
            if let Some(st) = self.session.stage.as_mut() {
                st.changes = Some(git::changes(&p.dir));
            }
        }
        self.log
            .begin_run(&format!("Re-check {}", self.session.slug));
        self.tab = Tab::Stage;
        self.recheck();
    }

    pub(super) fn recompute_mode(&mut self) {
        let ws = self.config.workspace();
        let names = self.github_repos(&self.session.owner).cloned();
        let s = &mut self.session;
        // An existing GitHub repo keeps its exact name (capitals included).
        if let Some(names) = &names {
            if let Some(exact) = names.iter().find(|n| n.eq_ignore_ascii_case(&s.slug)) {
                s.slug = exact.clone();
            }
        }
        let r = pipeline::route(&ws, &s.slug, &s.owner, names.as_deref());
        let mode = r.mode;
        s.mode = mode;
        s.local_exists = r.local;
        s.local_is_ours = r.local && ProjectsState::load(&self.paths).staged_not_pushed(&s.slug);
        s.on_github = r.on_github;
        s.deleted_on_github = r.deleted_on_github;
        if !s.commit_edited {
            s.commit_msg = version::default_commit_message(mode == Mode::New, &s.tag);
        }
    }

    /// This account's repos on GitHub, if they've been loaded.
    pub(super) fn github_repos(&self, owner: &str) -> Option<&Vec<String>> {
        self.repo_lists.get(&owner.to_ascii_lowercase())
    }

    /// Reload an account's repo list from GitHub (at most every few seconds).
    pub(super) fn refresh_repos(&mut self, owner: &str, force: bool) {
        let key = owner.to_ascii_lowercase();
        if key.is_empty() || !self.env.gh || self.repos_loading.contains(&key) {
            return;
        }
        if !force
            && self
                .repos_asked
                .get(&key)
                .is_some_and(|t| t.elapsed() < Duration::from_secs(5))
        {
            return;
        }
        self.repos_loading.insert(key.clone());
        self.repos_asked.insert(key, Instant::now());
        let owner = owner.to_string();
        let gh = self.gh.clone();
        self.spawn_bg(move || {
            let r = gh
                .list_repos(&owner)
                .map(|v| v.into_iter().map(|r| r.name).collect());
            Msg::Repos(owner, r)
        });
    }

    /// The GitHub owner and repo a project pushes to.
    pub(super) fn project_owner_repo(&self, p: &ProjectInfo) -> Option<(String, String)> {
        let url = p.remote.as_deref()?;
        git::owner_repo(url).or_else(|| {
            let owner = ProjectsState::load(&self.paths)
                .get(&p.slug)
                .owner
                .or(self.env.login.clone())?;
            git::origin_matches(url, &owner, &p.slug).then(|| (owner, p.slug.clone()))
        })
    }

    /// The project's repo is gone from GitHub (only when that account's list is known).
    pub(super) fn gone_from_github(&self, p: &ProjectInfo) -> bool {
        let Some((owner, repo)) = self.project_owner_repo(p) else {
            return false;
        };
        self.github_repos(&owner)
            .is_some_and(|names| !names.iter().any(|n| n.eq_ignore_ascii_case(&repo)))
    }

    /// Reload the lists for every account the projects push to.
    pub(super) fn refresh_project_owners(&mut self, force: bool) {
        let mut owners: Vec<String> = self
            .projects
            .iter()
            .filter_map(|p| self.project_owner_repo(p).map(|o| o.0))
            .collect();
        if let Some(l) = self.env.login.clone() {
            owners.push(l);
        }
        owners.sort_by_key(|o| o.to_ascii_lowercase());
        owners.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
        for o in owners {
            self.refresh_repos(&o, force);
        }
    }

    pub(super) fn existing_tags(&self) -> Vec<String> {
        let dir = self.config.workspace().join(&self.session.slug);
        if git::is_repo(&dir) {
            git::tags(&dir)
        } else {
            Vec::new()
        }
    }

    pub(super) fn stage(&mut self) {
        let Some(zip) = self.session.zip.clone() else {
            return;
        };
        self.session.review_when_ready = true;
        if let Some(i) = &self.session.zip_info {
            crate::zipinfo::remember(&self.paths, &self.session.slug, &i.sha);
        }
        let s = &self.session;
        let req = StageRequest {
            zip,
            slug: s.slug.clone(),
            owner: s.owner.clone(),
            mode: s.mode,
            replace_local: s.replace_local,
            description: s.description.clone(),
            license: s.license.clone(),
            license_holder: self.config.license_holder.clone(),
        };
        self.session.stage = None;
        self.session.checks = None;
        self.session.outcome = None;
        self.session.confirm = false;
        self.log.begin_run(&format!("Stage {}", self.session.slug));
        let tx = self.tx.clone();
        let ectx = self.egui_ctx.clone();
        self.spawn_job("Staging", move |pctx| {
            let r = pipeline::stage(&pctx, &req);
            if let Ok(st) = &r {
                Self::send(&tx, &ectx, Msg::Staged(Ok(st.clone())));
                let checks = pipeline::run_checks(&pctx, &req.slug, &st.dir, st.stack());
                return Msg::Checked(checks);
            }
            Msg::Staged(r)
        });
    }

    pub(super) fn recheck(&mut self) {
        self.session.review_when_ready = true;
        let Some(st) = self.session.stage.clone() else {
            return;
        };
        let slug = self.session.slug.clone();
        self.session.checks = None;
        self.spawn_job("Checking", move |pctx| {
            let _ = crate::cleanup::cleanup(&st.dir, st.stack());
            Msg::Checked(pipeline::run_checks(&pctx, &slug, &st.dir, st.stack()))
        });
        if let Some(st) = self.session.stage.as_mut() {
            let files = git::files_to_commit(&st.dir);
            let (w, b) = crate::cleanup::size_check(&st.dir, &files);
            st.warn_files = w;
            st.block_files = b;
            st.files = files.len();
            if self.session.mode == Mode::Update {
                st.changes = Some(git::changes(&st.dir));
            }
        }
    }

    pub(super) fn push(&mut self) {
        let s = &self.session;
        let req = PushRequest {
            slug: s.slug.clone(),
            owner: s.owner.clone(),
            mode: s.mode,
            private: s.private,
            description: s.description.clone(),
            topics: s
                .topics
                .split(',')
                .map(|t| detect::kebab(t.trim()))
                .filter(|t| !t.is_empty())
                .collect(),
            tag: s.tag.trim().to_string(),
            channel: s.channel,
            commit_message: if s.commit_msg.trim().is_empty() {
                version::default_commit_message(s.mode == Mode::New, &s.tag)
            } else {
                s.commit_msg.clone()
            },
            archive_zip: if s.archive_after { s.zip.clone() } else { None },
            archive_move: s.archive_move,
        };
        self.session.confirm = false;
        let label = format!(
            "Push {}/{}{}",
            req.owner,
            req.slug,
            if req.tag.is_empty() {
                String::new()
            } else {
                format!(" {}", req.tag)
            }
        );
        self.log.set_run_label(&label);
        self.spawn_job("Pushing", move |pctx| {
            Msg::Pushed(pipeline::push(&pctx, &req))
        });
    }

    pub(super) fn archive_only(&mut self) {
        let Some(zip) = self.session.zip.clone() else {
            return;
        };
        let slug = self.session.slug.clone();
        let tag = self.session.tag.trim().to_string();
        let mv = self.session.archive_move;
        self.log.begin_run(&format!("Archive Only {slug}"));
        self.spawn_job("Archiving", move |pctx| {
            Msg::Archived(pipeline::archive_only(
                &pctx,
                &zip,
                &slug,
                if tag.is_empty() { None } else { Some(&tag) },
                mv,
            ))
        });
    }

    pub(super) fn run_selftest(&mut self) {
        let paths = self.paths.clone();
        let online = self.selftest_online;
        self.spawn_job("Running self-test", move |_| {
            let r = selftest::run(&paths, online);
            r.apply_to_health(&paths);
            let saved = report::save_report(&paths, "selftest", &r.to_markdown()).ok();
            Msg::SelfTest(Box::new(r), saved)
        });
    }

    pub(super) fn check_updates(&mut self, manual: bool) {
        self.update_manual = manual;
        if manual {
            self.update_note = Some("Checking for updates…".into());
        }

        let paths = self.paths.clone();
        let repo = self.config.update_repo.clone();
        let beta = self.config.beta_updates();
        let tx = self.tx.clone();
        let ctx = self.egui_ctx.clone();
        std::thread::spawn(move || {
            let r = updater::check(&paths, &repo, beta);
            let _ = manual;
            Self::send(&tx, &ctx, Msg::UpdateChecked(r));
        });
    }

    /// Look at which of the family's tools are installed (in the background).
    pub(super) fn survey_tools(&mut self) {
        self.spawn_bg(|| Msg::FamilyTools(crate::tools::survey()));
    }

    /// Install packages with pacman through the desktop's password prompt.
    pub(super) fn install_tools(&mut self, packages: Vec<String>, recheck: bool) {
        self.tools_busy = true;
        let log = self.log.clone();
        self.spawn_bg(move || {
            Msg::FamilyToolsInstalled(crate::tools::install(&packages, &log), recheck)
        });
    }

    /// The Install button (or, without a password prompt, the command).
    pub(super) fn install_button(&mut self, ui: &mut egui::Ui, packages: &[String], recheck: bool) {
        if packages.is_empty() {
            return;
        }
        if self.tools_busy {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Installing (approve the password prompt)…");
            });
            return;
        }
        let label = format!("Install {}", packages.join(", "));
        if ui::button_enabled(ui, self.busy.is_none(), ui::icon::DOWNLOAD, &label)
            .on_hover_text(format!(
                "Runs {} after you approve your password prompt",
                crate::tools::pacman_command(packages)
            ))
            .clicked()
        {
            self.install_tools(packages.to_vec(), recheck);
        }
    }

    /// A desktop notification, only while StowLock isn't the window in front.
    pub(super) fn notify(&self, title: &str, body: &str) {
        if self.config.desktop_notifications && !self.was_focused {
            proc::notify(title, body);
        }
    }

    /// Ask rustup whether a newer stable Rust is out (in the background).
    pub(super) fn check_rust(&mut self) {
        let paths = self.paths.clone();
        self.spawn_bg(move || Msg::Rust(crate::rustcheck::check(&paths)));
    }

    /// Run `rustup update stable` (or `rustup default stable`) in the background.
    pub(super) fn update_rust(&mut self) {
        let Some(state) = self.rust.clone() else {
            return;
        };
        self.rust_busy = true;
        self.log.info("rust", "Updating Rust with rustup");
        self.spawn_bg(move || Msg::RustUpdated(crate::rustcheck::update(&state)));
    }

    /// The "newer Rust" notice on the Stage screen.
    pub(super) fn rust_notice(&mut self, ui: &mut egui::Ui) {
        use crate::rustcheck::RustState;
        if self.rust_later && !self.rust_busy {
            return;
        }
        let text = match &self.rust {
            Some(RustState::UpdateAvailable { have, newest }) => format!(
                "Rust {newest} is available (you have {have}). GitHub uses the newest, so update before checking Rust projects."
            ),
            Some(RustState::NoDefault) => {
                "rustup is installed but has no Rust version yet, so Rust projects can't be checked.".into()
            }
            _ => return,
        };
        let action = if self.rust == Some(RustState::NoDefault) {
            "Set up Rust"
        } else {
            "Update Rust"
        };
        ui::banner(ui, ACCENT(), |ui| {
            if self.rust_busy {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("Updating Rust with rustup (a minute or two)…");
                });
                return;
            }
            ui.add(egui::Label::new(text).wrap());
            ui.horizontal_wrapped(|ui| {
                if ui::primary(ui, action, self.busy.is_none()).clicked() {
                    self.update_rust();
                }
                if ui.button("Later").clicked() {
                    self.rust_later = true;
                }
            });
        });
        ui.add_space(6.0);
    }

    pub(super) fn apply_update(&mut self, tag: String) {
        self.update = UpdateState::Working(tag.clone());
        let paths = self.paths.clone();
        let repo = self.config.update_repo.clone();
        let log = self.log.clone();
        self.spawn_bg(move || Msg::UpdateApplied(updater::apply(&paths, &repo, &tag, &log)));
    }

    /// Follow GitHub's build for a pushed commit: check every 15 seconds for
    /// up to 40 minutes. No build within 90 seconds means the project has none.
    pub(super) fn watch_build(&mut self, commit: String, tag: Option<String>) {
        let local_passed = self
            .session
            .checks
            .as_ref()
            .map(|c| c.tests.status == TestStatus::Passed)
            .unwrap_or(false);
        let (owner, slug) = (self.session.owner.clone(), self.session.slug.clone());
        self.watch_build_for(owner, slug, commit, tag, local_passed);
    }

    /// Watch GitHub's build of `commit` until it passes or fails.
    pub(super) fn watch_build_for(
        &mut self,
        owner: String,
        slug: String,
        commit: String,
        tag: Option<String>,
        local_passed: bool,
    ) {
        let id = self.build.as_ref().map(|w| w.id + 1).unwrap_or(1);
        self.build = Some(BuildWatch {
            id,
            owner: owner.clone(),
            slug: slug.clone(),
            tag,
            commit: commit.clone(),
            status: None,
            local_passed,
            dismissed: false,
        });
        let gh = self.gh.clone();
        let tx = self.tx.clone();
        let ctx = self.egui_ctx.clone();
        let (first, every) = self.build_poll;
        std::thread::spawn(move || {
            let start = std::time::Instant::now();
            std::thread::sleep(std::time::Duration::from_secs(first));
            loop {
                let r = gh.build_status(&owner, &slug, &commit);
                let done = match &r {
                    Ok(crate::github::Build::Passed { .. })
                    | Ok(crate::github::Build::Failed { .. }) => true,
                    Ok(crate::github::Build::None) => start.elapsed().as_secs() > 90,
                    Ok(crate::github::Build::Running { .. }) => false,
                    Err(_) => start.elapsed().as_secs() > 300,
                };
                let quiet_none = matches!(r, Ok(crate::github::Build::None)) && !done;
                if !quiet_none {
                    Self::send(&tx, &ctx, Msg::Build(id, r));
                }
                if done || start.elapsed().as_secs() > 40 * 60 {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_secs(every));
            }
        });
    }

    /// Watch the same build again (after a re-run started here or on GitHub).
    pub(super) fn rewatch_build(&mut self) {
        if let Some(w) = &self.build {
            let (o, s, c, t, l) = (
                w.owner.clone(),
                w.slug.clone(),
                w.commit.clone(),
                w.tag.clone(),
                w.local_passed,
            );
            self.confirm_remove_tag = false;
            self.watch_build_for(o, s, c, t, l);
        }
    }

    /// Ask GitHub to re-run the failed jobs, then watch again.
    pub(super) fn rerun_build(&mut self, run_url: String) {
        let Some(w) = &self.build else { return };
        let (owner, slug) = (w.owner.clone(), w.slug.clone());
        let gh = self.gh.clone();
        self.spawn_bg(move || Msg::Rerun(gh.rerun_failed(&owner, &slug, &run_url)));
    }

    pub(super) fn fix_release(
        &mut self,
        finish: bool,
        owner: String,
        slug: String,
        tag: String,
        beta: bool,
    ) {
        let ctx = self.pctx();
        self.spawn_bg(move || {
            Msg::ReleaseFixed(if finish {
                pipeline::finish_release(&ctx, &owner, &slug, &tag, beta)
                    .map(|_| format!("Finished the release {tag}"))
            } else {
                pipeline::remove_release(&ctx, &owner, &slug, &tag)
                    .map(|_| format!("Removed the release and tag {tag}. You can use {tag} again."))
            })
        });
    }
}
