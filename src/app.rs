// SPDX-License-Identifier: GPL-3.0-or-later
//! The StowLock window: state, background work and every screen.

use crate::auth::{self, SignInEvent};
use crate::config::{AppPaths, Config, APP_VERSION};
use crate::detect;
use crate::errors::{self, Action, Friendly};
use crate::git;
use crate::github::{Account, GitHubApi, RealGh};
use crate::health::{self, Health, HealthState};
use crate::history::{History, PushRecord};
use crate::install::{self, InstallPaths};
use crate::instance;
use crate::logbook::{Level, Logger};
use crate::pipeline::{
    self, Checks, Mode, ProjectInfo, PushOutcome, PushRequest, StageRequest, StageResult,
};
use crate::preflight::TestStatus;
use crate::proc::{self, Output};
use crate::report::{self, RedactContext, ReportInput, SessionInfo};
use crate::selftest::{self, SelfTestReport};
use crate::state::ProjectsState;
use crate::ui::{self, ACCENT, BORDER, ERROR, MUTED, SUCCESS, TEXT, WARNING};
use crate::uninstall::{self, Level as UninstallLevel, Plan};
use crate::updater;
use crate::util;
use crate::version::{self, Channel};
use eframe::egui::{self, RichText};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

const STEPS: [&str; 6] = [
    "Extract & clean",
    "Pre-flight tests",
    "Secret scan",
    "Commit & push",
    "Tag & release",
    "Archive zip",
];

#[derive(Default, Clone)]
struct EnvStatus {
    checked: bool,
    git: bool,
    gh: bool,
    accounts: Vec<Account>,
    login: Option<String>,
    orgs: Vec<String>,
    name: Option<String>,
    email: Option<String>,
    repo_names: Option<Vec<String>>,
    /// (name, private) from GitHub, used to hide repo names in reports.
    repos: Vec<(String, bool)>,
    offline: bool,
}

impl EnvStatus {
    fn ready(&self) -> bool {
        self.git && self.gh && self.login.is_some()
    }
    fn secure(&self) -> Option<bool> {
        self.accounts.iter().find(|a| a.active).map(|a| a.secure)
    }
}

#[allow(clippy::large_enum_variant)]
enum Msg {
    Env(Box<EnvStatus>),
    Repos(String, Result<Vec<String>, String>),
    Picked(Option<PathBuf>),
    Staged(Result<StageResult, String>),
    Checked(Checks),
    Pushed(Result<PushOutcome, String>),
    Archived(Result<PathBuf, String>),
    Projects(Vec<ProjectInfo>),
    SelfTest(Box<SelfTestReport>, Option<PathBuf>),
    UpdateChecked(Result<Option<String>, String>),
    UpdateApplied(Result<PathBuf, String>),
    SignIn(SignInEvent),
    Tools(Output),
    Toast(String),
    BugDownloaded(Result<Option<PathBuf>, String>),
    Done,
}

#[derive(PartialEq, Clone, Copy)]
enum Tab {
    Stage,
    Projects,
}

enum UpdateState {
    None,
    Available(String),
    Working(String),
    Ready(String, PathBuf),
    Failed(String),
}

#[derive(Default)]
struct SignInUi {
    active: bool,
    code: Option<String>,
    lines: Vec<String>,
    result: Option<Result<String, String>>,
    cancel: Arc<AtomicBool>,
}

#[derive(Clone)]
struct Session {
    zip: Option<PathBuf>,
    slug: String,
    owner: String,
    mode: Mode,
    archive_only: bool,
    local_exists: bool,
    /// The existing folder is StowLock's own earlier staging that was never pushed.
    local_is_ours: bool,
    on_github: Option<bool>,
    replace_local: bool,
    description: String,
    topics: String,
    license: String,
    private: bool,
    tag: String,
    channel: Channel,
    commit_msg: String,
    commit_edited: bool,
    archive_after: bool,
    archive_move: bool,
    stage: Option<StageResult>,
    checks: Option<Checks>,
    override_tests: bool,
    run_cmd: String,
    pass_env: bool,
    confirm: bool,
    outcome: Option<PushOutcome>,
    show_changes: bool,
    /// Version found in the zip (Archive Only), and whether `tag` was filled from it.
    found_version: Option<detect::FoundVersion>,
    tag_auto: bool,
}

impl Session {
    fn new(config: &Config) -> Self {
        Session {
            zip: None,
            slug: String::new(),
            owner: String::new(),
            mode: Mode::New,
            archive_only: false,
            local_exists: false,
            local_is_ours: false,
            on_github: None,
            replace_local: false,
            description: String::new(),
            topics: String::new(),
            license: config.default_license.clone(),
            private: config.default_private,
            tag: String::new(),
            channel: Channel::Stable,
            commit_msg: String::new(),
            commit_edited: false,
            archive_after: config.archive_after_push,
            archive_move: config.move_archives(),
            stage: None,
            checks: None,
            override_tests: false,
            run_cmd: String::new(),
            pass_env: false,
            confirm: false,
            outcome: None,
            show_changes: false,
            found_version: None,
            tag_auto: false,
        }
    }
    fn effective_mode(&self) -> Mode {
        if self.archive_only {
            Mode::ArchiveOnly
        } else {
            self.mode
        }
    }
}

struct BugForm {
    what: String,
    expected: String,
    include_public: bool,
    include_username: bool,
    preview: String,
    confirm_issue: bool,
    /// When this report was created (used in the download file name).
    created: chrono::DateTime<chrono::Local>,
    /// Where the last download went.
    saved_to: Option<PathBuf>,
    saving: bool,
}

struct UninstallUi {
    level: UninstallLevel,
    typed: String,
    permanent: bool,
    plan: Option<Plan>,
    result: Option<Result<uninstall::Outcome, String>>,
}

pub struct App {
    paths: AppPaths,
    config: Config,
    saved_config: Config,
    log: Logger,
    gh: Arc<dyn GitHubApi>,
    tx: Sender<Msg>,
    rx: Receiver<Msg>,
    egui_ctx: egui::Context,
    busy: Option<String>,
    cancel: Arc<AtomicBool>,
    tab: Tab,
    env: EnvStatus,
    session: Session,
    projects: Vec<ProjectInfo>,
    history: History,
    health: HealthState,
    error: Option<(Friendly, String)>,
    toast: Option<(String, Instant)>,
    update: UpdateState,
    signin: SignInUi,
    tools_installing: bool,
    tools_result: Option<String>,
    identity_name: String,
    identity_email: String,
    show_settings: bool,
    show_health: bool,
    show_bug: bool,
    show_today: bool,
    show_uninstall: bool,
    show_log: bool,
    bug: BugForm,
    uninstall: UninstallUi,
    selftest_online: bool,
    last_selftest: Option<(SelfTestReport, Option<PathBuf>)>,
    run_child: Option<std::process::Child>,
    crash_report: Option<PathBuf>,
    installed: bool,
    install_inst: InstallPaths,
    last_pending_poll: Instant,
    archive_note_seen: bool,
    _guard: Option<instance::InstanceGuard>,
}

impl App {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        paths: AppPaths,
        log: Logger,
        guard: Option<instance::InstanceGuard>,
        zip_arg: Option<PathBuf>,
    ) -> Self {
        ui::apply_theme(&cc.egui_ctx);
        let config = Config::load(&paths);
        let (tx, rx) = channel();
        let crash_report = std::fs::read_to_string(paths.crash_marker())
            .ok()
            .map(|s| PathBuf::from(s.trim()));
        let _ = std::fs::remove_file(paths.crash_marker());
        if crash_report.is_some() {
            HealthState::record(
                &paths,
                "Settings & state",
                false,
                "StowLock closed unexpectedly last time",
            );
        }
        report::apply_retention(&paths, config.report_retention_days);
        pipeline::clean_stale_temp(&config);
        let inst = InstallPaths::detect();
        let installed = install::is_installed(&inst);
        let mut app = App {
            history: History::load(&paths),
            health: HealthState::load(&paths),
            session: Session::new(&config),
            saved_config: config.clone(),
            config,
            log,
            gh: Arc::new(RealGh),
            tx,
            rx,
            egui_ctx: cc.egui_ctx.clone(),
            busy: None,
            cancel: Arc::new(AtomicBool::new(false)),
            tab: Tab::Stage,
            env: EnvStatus::default(),
            projects: Vec::new(),
            error: None,
            toast: None,
            update: UpdateState::None,
            signin: SignInUi::default(),
            tools_installing: false,
            tools_result: None,
            identity_name: String::new(),
            identity_email: String::new(),
            show_settings: false,
            show_health: false,
            show_bug: false,
            show_today: false,
            show_uninstall: false,
            show_log: false,
            bug: BugForm {
                what: String::new(),
                expected: String::new(),
                include_public: false,
                include_username: false,
                preview: String::new(),
                confirm_issue: false,
                created: chrono::Local::now(),
                saved_to: None,
                saving: false,
            },
            uninstall: UninstallUi {
                level: UninstallLevel::AppOnly,
                typed: String::new(),
                permanent: false,
                plan: None,
                result: None,
            },
            selftest_online: false,
            last_selftest: None,
            run_child: None,
            crash_report,
            installed,
            install_inst: inst,
            last_pending_poll: Instant::now(),
            archive_note_seen: false,
            paths,
            _guard: guard,
        };
        app.log
            .info("app", &format!("StowLock {APP_VERSION} started"));
        app.refresh_env();
        app.refresh_projects();
        if let Some(z) = zip_arg {
            app.load_zip(z);
        }
        app
    }

    // ------------------------------------------------------------ plumbing

    fn pctx(&self) -> pipeline::Ctx {
        pipeline::Ctx {
            paths: self.paths.clone(),
            config: self.config.clone(),
            gh: self.gh.clone(),
            log: self.log.clone(),
            cancel: self.cancel.clone(),
        }
    }

    fn send(tx: &Sender<Msg>, ctx: &egui::Context, m: Msg) {
        let _ = tx.send(m);
        ctx.request_repaint();
    }

    /// Run a pipeline job on a worker thread. Only one at a time.
    fn spawn_job(&mut self, label: &str, f: impl FnOnce(pipeline::Ctx) -> Msg + Send + 'static) {
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
    fn spawn_bg(&self, f: impl FnOnce() -> Msg + Send + 'static) {
        let tx = self.tx.clone();
        let ctx = self.egui_ctx.clone();
        std::thread::spawn(move || Self::send(&tx, &ctx, f()));
    }

    fn toast(&mut self, s: &str) {
        self.toast = Some((s.to_string(), Instant::now()));
    }

    fn fail(&mut self, raw: &str, step: &str) {
        let f = errors::explain(raw, step);
        self.log.error(step, raw);
        self.error = Some((f, raw.to_string()));
    }

    fn save_config(&mut self) {
        if self.config != self.saved_config {
            match self.config.save(&self.paths) {
                Ok(_) => self.saved_config = self.config.clone(),
                Err(e) => self.fail(&e, "settings"),
            }
        }
    }

    fn refresh_env(&mut self) {
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

    fn refresh_projects(&self) {
        let paths = self.paths.clone();
        let config = self.config.clone();
        self.spawn_bg(move || Msg::Projects(pipeline::scan_projects(&paths, &config)));
    }

    fn record_health(&mut self, update_mode: bool) {
        for s in self.log.steps() {
            for area in health::areas_for_step(&s.name, update_mode) {
                match s.status {
                    crate::logbook::StepStatus::Passed => {
                        HealthState::record(&self.paths, area, true, "")
                    }
                    crate::logbook::StepStatus::Failed => HealthState::record(
                        &self.paths,
                        area,
                        false,
                        s.detail.lines().next().unwrap_or(""),
                    ),
                    _ => {}
                }
            }
        }
        self.health = HealthState::load(&self.paths);
    }

    // ------------------------------------------------------------ actions

    fn pick_zip(&mut self) {
        let start = dirs::download_dir().unwrap_or_else(util::home_dir);
        self.spawn_bg(move || Msg::Picked(pick_file_dialog(&start)));
    }

    fn load_zip(&mut self, zip: PathBuf) {
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
        s.description = format!("{}, a vibe-coded project", detect::title_from_slug(&s.slug));
        s.zip = Some(zip.clone());
        s.found_version = detect::detect_version(&zip, &self.config.tag_prefix);
        let label = format!("Stage {}", s.slug);
        self.session = s;
        self.error = None;
        self.log.begin_run(&label);
        self.log
            .info("intake", &format!("Loaded {}", util::tilde(&zip)));
        self.tab = Tab::Stage;
        self.recompute_mode();
    }

    /// Load an existing workspace project (to try, re-check and push edits made by hand).
    fn load_project(&mut self, p: &ProjectInfo) {
        let mut s = Session::new(&self.config);
        s.slug = p.slug.clone();
        s.owner = ProjectsState::load(&self.paths)
            .get(&p.slug)
            .owner
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

    fn recompute_mode(&mut self) {
        let ws = self.config.workspace();
        let s = &mut self.session;
        // An existing GitHub repo keeps its exact name (capitals included).
        if let Some(names) = &self.env.repo_names {
            if let Some(exact) = names.iter().find(|n| n.eq_ignore_ascii_case(&s.slug)) {
                s.slug = exact.clone();
            }
        }
        let (mode, local, remote) =
            pipeline::decide_mode(&ws, &s.slug, &s.owner, self.env.repo_names.as_deref());
        s.mode = mode;
        s.local_exists = local;
        s.local_is_ours = local && ProjectsState::load(&self.paths).staged_not_pushed(&s.slug);
        s.on_github = remote;
        if !s.commit_edited {
            s.commit_msg = version::default_commit_message(mode == Mode::New, &s.tag);
        }
    }

    fn existing_tags(&self) -> Vec<String> {
        let dir = self.config.workspace().join(&self.session.slug);
        if git::is_repo(&dir) {
            git::tags(&dir)
        } else {
            Vec::new()
        }
    }

    fn stage(&mut self) {
        let Some(zip) = self.session.zip.clone() else {
            return;
        };
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

    fn recheck(&mut self) {
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

    fn push(&mut self) {
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

    fn archive_only(&mut self) {
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

    fn run_selftest(&mut self) {
        let paths = self.paths.clone();
        let online = self.selftest_online;
        self.spawn_job("Running self-test", move |_| {
            let r = selftest::run(&paths, online);
            r.apply_to_health(&paths);
            let saved = report::save_report(&paths, "selftest", &r.to_markdown()).ok();
            Msg::SelfTest(Box::new(r), saved)
        });
    }

    fn check_updates(&mut self, manual: bool) {
        if !self.env.gh {
            if manual {
                self.toast("GitHub's gh tool is needed to check for updates.");
            }
            return;
        }
        let paths = self.paths.clone();
        let repo = self.config.update_repo.clone();
        let tx = self.tx.clone();
        let ctx = self.egui_ctx.clone();
        std::thread::spawn(move || {
            let r = updater::check(&paths, &repo);
            if manual {
                if let Ok(None) = &r {
                    Self::send(
                        &tx,
                        &ctx,
                        Msg::Toast(format!("StowLock {APP_VERSION} is the latest version.")),
                    );
                }
            }
            Self::send(&tx, &ctx, Msg::UpdateChecked(r));
        });
    }

    fn apply_update(&mut self, tag: String) {
        self.update = UpdateState::Working(tag.clone());
        let paths = self.paths.clone();
        let repo = self.config.update_repo.clone();
        let log = self.log.clone();
        self.spawn_bg(move || Msg::UpdateApplied(updater::apply(&paths, &repo, &tag, &log)));
    }

    fn start_sign_in(&mut self) {
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

    fn redact_ctx(&self) -> RedactContext {
        let mut rc = RedactContext::local(&self.config.workspace());
        rc.logins = self.env.accounts.iter().map(|a| a.login.clone()).collect();
        if let Some(l) = &self.env.login {
            if !rc.logins.contains(l) {
                rc.logins.push(l.clone());
            }
        }
        rc.orgs = self.env.orgs.clone();
        // Everything that could name a project: GitHub repos (with their real
        // visibility), workspace folders, archive folders, push history and the
        // loaded zip. Unknown visibility is treated as private.
        let mut repos: Vec<(String, bool)> = self.env.repos.clone();
        let mut add = |name: &str| {
            if !name.is_empty() && !repos.iter().any(|(n, _)| n.eq_ignore_ascii_case(name)) {
                repos.push((name.to_string(), true));
            }
        };
        for p in &self.projects {
            add(&p.slug);
        }
        if let Ok(rd) = std::fs::read_dir(self.config.archive_root()) {
            for e in rd.flatten() {
                add(&e.file_name().to_string_lossy());
            }
        }
        for h in &self.history.pushes {
            add(&h.slug);
        }
        add(&self.session.slug);
        if let Some(z) = &self.session.zip {
            let name = z
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            add(&detect::slug_from_filename(&name));
        }
        for e in self.log.entries() {
            if let Some(rest) = e.message.strip_prefix("Loaded ") {
                let name = rest.rsplit('/').next().unwrap_or(rest);
                add(&detect::slug_from_filename(name));
            }
        }
        rc.repos = repos;
        rc.include_public_repo_names = self.bug.include_public;
        rc.include_username = self.bug.include_username;
        rc
    }

    fn open_bug_report(&mut self) {
        self.show_bug = true;
        self.bug.created = chrono::Local::now();
        self.bug.saved_to = None;
        self.bug.confirm_issue = false;
        self.bug.preview = self.build_bug_report();
    }

    /// Download the previewed report as a .txt file, to a folder the user picks.
    fn download_bug_report(&mut self) {
        let text = self.bug.preview.clone();
        let name = report::bug_report_file_name(&self.bug.created);
        let start = dirs::download_dir().unwrap_or_else(util::home_dir);
        self.bug.saving = true;
        self.spawn_bg(move || {
            let r = match save_file_dialog(&start, &name) {
                None => Ok(None),
                Some(path) => std::fs::write(&path, text.as_bytes())
                    .map(|_| Some(path.clone()))
                    .map_err(|e| format!("Couldn't save {}: {e}", util::tilde(&path))),
            };
            Msg::BugDownloaded(r)
        });
    }

    fn build_bug_report(&self) -> String {
        let s = &self.session;
        let input = ReportInput {
            title: "bug report".into(),
            what_doing: self.bug.what.clone(),
            expected: self.bug.expected.clone(),
            env: report::collect_env(Some(self.env.login.is_some()), self.env.secure()),
            session: if s.slug.is_empty() {
                None
            } else {
                Some(SessionInfo {
                    owner: s.owner.clone(),
                    slug: s.slug.clone(),
                    mode: s.effective_mode().badge().into(),
                    channel: s.channel.label().into(),
                    tag: s.tag.clone(),
                    stack: s
                        .stage
                        .as_ref()
                        .map(|x| x.stack().name().to_string())
                        .unwrap_or_else(|| "not staged".into()),
                })
            },
            steps: Vec::new(),
            runs: self.log.runs(),
            failing_command: self.log.failing_command(),
            health: self.health.lines(),
            selftest_summary: self.last_selftest.as_ref().map(|(r, _)| r.summary_line()),
            problem_lines: self.log.report_lines(40, 100).0,
            log_lines: self.log.report_lines(40, 100).1,
            panic: None,
        };
        let text = report::build_report(&input, &self.redact_ctx());
        let created = format!(
            "- **Created:** {}\n",
            self.bug.created.format("%Y-%m-%d %H:%M:%S")
        );
        match text.find("\n\n") {
            Some(i) => format!("{}{}{}", &text[..i + 2], created, &text[i + 2..]),
            None => text,
        }
    }

    // ------------------------------------------------------------ messages

    fn handle(&mut self, m: Msg) {
        match m {
            Msg::Env(e) => {
                let first = !self.env.checked;
                self.env = *e;
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
                    self.show_settings = true;
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
                if first
                    && self.config.check_for_updates
                    && self.env.gh
                    && updater::check_due(&self.paths)
                {
                    self.check_updates(false);
                }
            }
            Msg::Repos(owner, r) => {
                if owner == self.session.owner {
                    if let Ok(names) = r {
                        self.env.repo_names = Some(names);
                        if self.session.stage.is_none() {
                            self.recompute_mode();
                        }
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
            Msg::Checked(c) => {
                self.session.checks = Some(c);
                self.record_health(self.session.mode == Mode::Update);
            }
            Msg::Pushed(r) => {
                self.record_health(self.session.mode == Mode::Update);
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
                        self.session.outcome = Some(out);
                        self.history = History::load(&self.paths);
                        self.health = HealthState::load(&self.paths);
                        self.refresh_projects();
                        self.refresh_env();
                    }
                    Err(e) => self.fail(&e, "push"),
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
            Msg::Projects(p) => self.projects = p,
            Msg::SelfTest(r, path) => {
                self.health = HealthState::load(&self.paths);
                self.toast(&format!("Self-test: {}", r.summary_line()));
                self.last_selftest = Some((*r, path));
            }
            Msg::UpdateChecked(r) => match r {
                Ok(Some(tag)) => {
                    HealthState::record(&self.paths, "Self-update", true, "");
                    self.update = UpdateState::Available(tag);
                }
                Ok(None) => HealthState::record(&self.paths, "Self-update", true, ""),
                Err(e) => self
                    .log
                    .warn("update", &format!("Update check failed: {e}")),
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
            Msg::Toast(s) => self.toast(&s),
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

/// A pop-up window that always fits inside the app window and scrolls
/// inside itself instead of growing past the screen.
fn ui_window(ctx: &egui::Context, title: &str, width: f32) -> egui::Window<'static> {
    // Keep pop-ups below the top bar so its buttons stay visible.
    let top = ctx
        .data(|d| d.get_temp::<f32>(egui::Id::new("top_bar_bottom")))
        .unwrap_or(56.0);
    let full = ctx.screen_rect();
    let screen = egui::Rect::from_min_max(egui::pos2(full.left(), top), full.max);
    let w = width.min(screen.width() - 24.0).max(280.0);
    egui::Window::new(title.to_string())
        .constrain_to(screen.shrink(4.0))
        .collapsible(false)
        .default_width(w)
        .max_width(screen.width() - 24.0)
        .max_height(screen.height() - 32.0)
        .default_pos(egui::pos2(screen.center().x - w / 2.0, screen.top() + 12.0))
        .vscroll(true)
}

/// Save dialog: KDE's dialog, then zenity, then the desktop portal. Each asks
/// before replacing an existing file. None = cancelled.
fn save_file_dialog(start: &Path, default_name: &str) -> Option<PathBuf> {
    let suggested = start.join(default_name).display().to_string();
    if proc::which("kdialog").is_some() {
        let o = proc::run(
            "kdialog",
            &[
                "--title",
                "Save bug report",
                "--getsavefilename",
                &suggested,
                "*.txt|Text files",
            ],
            &proc::Opts::default().with_timeout(3600),
        );
        return if o.ok() && !o.stdout_trim().is_empty() {
            Some(PathBuf::from(o.stdout_trim()))
        } else {
            None
        };
    }
    if proc::which("zenity").is_some() {
        let o = proc::run(
            "zenity",
            &[
                "--file-selection",
                "--save",
                "--confirm-overwrite",
                "--title=Save bug report",
                &format!("--filename={suggested}"),
            ],
            &proc::Opts::default().with_timeout(3600),
        );
        return if o.ok() && !o.stdout_trim().is_empty() {
            Some(PathBuf::from(o.stdout_trim()))
        } else {
            None
        };
    }
    rfd::FileDialog::new()
        .set_title("Save bug report")
        .add_filter("Text files", &["txt"])
        .set_directory(start)
        .set_file_name(default_name)
        .save_file()
}

/// File picker: KDE's dialog, then zenity, then the desktop portal.
fn pick_file_dialog(start: &Path) -> Option<PathBuf> {
    let s = start.display().to_string();
    if proc::which("kdialog").is_some() {
        let o = proc::run(
            "kdialog",
            &[
                "--title",
                "Choose a project zip",
                "--getopenfilename",
                &s,
                "*.zip|Zip archives",
            ],
            &proc::Opts::default().with_timeout(3600),
        );
        return if o.ok() && !o.stdout_trim().is_empty() {
            Some(PathBuf::from(o.stdout_trim()))
        } else {
            None
        };
    }
    if proc::which("zenity").is_some() {
        let o = proc::run(
            "zenity",
            &[
                "--file-selection",
                "--title=Choose a project zip",
                "--file-filter=Zip archives | *.zip",
                &format!("--filename={s}/"),
            ],
            &proc::Opts::default().with_timeout(3600),
        );
        return if o.ok() && !o.stdout_trim().is_empty() {
            Some(PathBuf::from(o.stdout_trim()))
        } else {
            None
        };
    }
    rfd::FileDialog::new()
        .set_title("Choose a project zip")
        .add_filter("Zip archives", &["zip"])
        .set_directory(start)
        .pick_file()
}

// ================================================================== drawing

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        while let Ok(m) = self.rx.try_recv() {
            self.handle(m);
        }
        if self.last_pending_poll.elapsed() > Duration::from_secs(1) {
            self.last_pending_poll = Instant::now();
            if let Some(z) = instance::take_pending(&self.paths).into_iter().last() {
                self.load_zip(z);
                ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
            }
        }
        ctx.request_repaint_after(Duration::from_secs(1));
        if let Some(child) = self.run_child.as_mut() {
            if let Ok(Some(_)) = child.try_wait() {
                self.run_child = None;
            }
        }
        // Drag and drop (may not work on every Wayland setup; the button always does).
        let dropped: Vec<PathBuf> = ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .filter_map(|f| f.path.clone())
                .collect()
        });
        if let Some(z) = dropped.into_iter().find(|p| {
            p.extension()
                .map(|x| x.eq_ignore_ascii_case("zip"))
                .unwrap_or(false)
        }) {
            self.load_zip(z);
        }
        let hovering = ctx.input(|i| !i.raw.hovered_files.is_empty());
        if ctx.input(|i| i.modifiers.ctrl && i.key_pressed(egui::Key::O)) {
            self.pick_zip();
        }

        self.top_bar(ctx);
        if self.tab == Tab::Stage {
            self.action_bar(ctx);
        }
        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(ui::BG)
                    .inner_margin(egui::Margin::symmetric(16, 10)),
            )
            .show(ctx, |ui| {
                self.banners(ui);
                ui.horizontal_wrapped(|ui| {
                    ui.selectable_value(
                        &mut self.tab,
                        Tab::Stage,
                        RichText::new("  Stage & Push  ").strong(),
                    );
                    ui.selectable_value(
                        &mut self.tab,
                        Tab::Projects,
                        RichText::new(format!("  Projects ({})  ", self.projects.len())).strong(),
                    );
                });
                ui.add_space(4.0);
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| match self.tab {
                        Tab::Stage => self.stage_tab(ui, hovering),
                        Tab::Projects => self.projects_tab(ui),
                    });
            });
        self.windows(ctx);
        if let Some((t, at)) = &self.toast {
            if at.elapsed() < Duration::from_secs(4) {
                let t = t.clone();
                egui::Area::new(egui::Id::new("toast"))
                    .anchor(egui::Align2::CENTER_BOTTOM, [0.0, -90.0])
                    .show(ctx, |ui| {
                        ui::banner(ui, ACCENT, |ui| ui.label(t));
                    });
            } else {
                self.toast = None;
            }
        }
        self.save_config();
    }
}

impl App {
    fn top_buttons(&mut self, ui: &mut egui::Ui, right_to_left: bool) {
        let n = self.history.today().len();
        let (broken, attn) = self.health.counts();
        let (htxt, hcol) = if broken > 0 {
            (format!("⏺ {broken} broken"), ERROR)
        } else if attn > 0 {
            (format!("⏺ {attn} need attention"), WARNING)
        } else {
            ("⏺ All working".to_string(), SUCCESS)
        };
        let mut order: Vec<u8> = vec![0, 1, 2, 3];
        if right_to_left {
            order.reverse();
        }
        for i in order {
            match i {
                0 => {
                    if ui
                        .button(RichText::new(format!("Pushed today: {n}")).color(if n > 0 {
                            SUCCESS
                        } else {
                            MUTED
                        }))
                        .clicked()
                    {
                        self.show_today = !self.show_today;
                    }
                }
                1 => {
                    if ui
                        .button(RichText::new(&htxt).color(hcol))
                        .on_hover_text("Health")
                        .clicked()
                    {
                        self.show_health = !self.show_health;
                    }
                }
                2 => {
                    if ui.button("Bug Report").clicked() {
                        self.open_bug_report();
                    }
                }
                _ => {
                    if ui.button("⚙").on_hover_text("Settings").clicked() {
                        self.show_settings = !self.show_settings;
                    }
                }
            }
        }
    }

    fn top_bar(&mut self, ctx: &egui::Context) {
        let resp = egui::TopBottomPanel::top("top")
            .frame(
                egui::Frame::new()
                    .fill(ui::CARD)
                    .inner_margin(egui::Margin::symmetric(16, 10))
                    .stroke(egui::Stroke::new(1.0_f32, BORDER)),
            )
            .show(ctx, |ui| {
                let title = |ui: &mut egui::Ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;
                    ui.label(RichText::new("Stow").strong().size(19.0).color(TEXT));
                    ui.label(RichText::new("Lock").strong().size(19.0).color(ACCENT));
                    ui.add_space(8.0);
                    ui.label(ui::muted(format!("v{APP_VERSION}")).size(11.5));
                };
                let ws = util::tilde(&self.config.workspace());
                // Wide: title, path (shortened with …), buttons on the right.
                // Narrow: buttons wrap onto a second line. Nothing overlaps.
                if ui.available_width() >= 640.0 {
                    ui.horizontal(|ui| {
                        title(ui);
                        ui.add_space(12.0);
                        let path_w = ui.available_width() - 440.0;
                        if path_w > 60.0 {
                            ui.allocate_ui_with_layout(
                                egui::vec2(path_w, 20.0),
                                egui::Layout::left_to_right(egui::Align::Center),
                                |ui| {
                                    ui.add(
                                        egui::Label::new(ui::mono(&ws).color(MUTED).size(12.0))
                                            .truncate(),
                                    )
                                    .on_hover_text(format!("Workspace: {ws}"));
                                },
                            );
                        }
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            self.top_buttons(ui, true);
                        });
                    });
                } else {
                    ui.horizontal(|ui| title(ui));
                    ui.horizontal_wrapped(|ui| self.top_buttons(ui, false));
                }
            });
        // Remember where the top bar ends so pop-ups open below it.
        let bottom = resp.response.rect.bottom();
        ctx.data_mut(|d| d.insert_temp(egui::Id::new("top_bar_bottom"), bottom));
    }

    fn banners(&mut self, ui: &mut egui::Ui) {
        if self.env.checked && !self.env.ready() {
            ui::banner(ui, WARNING, |ui| {
                ui.horizontal_wrapped(|ui| {
                    let what = if !self.env.git || !self.env.gh {
                        "Git and GitHub's gh tool need to be installed."
                    } else {
                        "Sign in to GitHub to push projects."
                    };
                    ui.label(RichText::new(format!("Finish setup: {what}")).color(WARNING));
                    if ui.button("Finish setup").clicked() {
                        self.show_settings = true;
                    }
                });
            });
        }
        if self.env.offline {
            ui.label(ui::muted("Offline: using local projects only.").size(12.0));
        }
        if self.env.secure() == Some(false) {
            ui::banner(ui, WARNING, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label("Your GitHub sign-in is stored in a plain file instead of the system keyring.");
                    if ui.button("Move to secure storage").clicked() {
                        self.show_settings = true;
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
            UpdateState::Available(t) => {
                ui::banner(ui, ACCENT, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(format!(
                            "StowLock {t} is available (you have v{APP_VERSION})."
                        ));
                        if ui
                            .button(RichText::new(format!("Update to {t}")).strong())
                            .clicked()
                        {
                            do_update = Some(t.clone());
                        }
                        if ui.small_button("✖").clicked() {
                            dismiss_update = true;
                        }
                    });
                });
            }
            UpdateState::Working(t) => {
                ui::banner(ui, ACCENT, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.spinner();
                        ui.label(format!("Downloading and checking StowLock {t}…"));
                    });
                });
            }
            UpdateState::Ready(t, p) => {
                ui::banner(ui, SUCCESS, |ui| {
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
                ui::banner(ui, WARNING, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(format!(
                            "The update didn't install: {}. Your current version keeps working.",
                            e.lines().next().unwrap_or("")
                        ));
                        if ui.small_button("✖").clicked() {
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
            if updater::restart(&p).is_ok() {
                std::process::exit(0);
            }
        }
        if let Some(p) = self.crash_report.clone() {
            ui::banner(ui, ERROR, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label("StowLock closed unexpectedly last time.");
                    if ui.button("View report").clicked() {
                        let _ = proc::xdg_open(&p.display().to_string());
                    }
                    if ui.small_button("✖").clicked() {
                        self.crash_report = None;
                    }
                });
            });
        }
        if !self.installed {
            ui::banner(ui, ACCENT, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label("StowLock isn't installed yet, so it won't appear in your app menu.");
                    if ui.button("Install StowLock").clicked() {
                        match install::install(&self.install_inst) {
                            Ok(r) => {
                                for l in r.lines.iter().chain(r.warnings.iter()) {
                                    self.log.info("install", l);
                                }
                                self.toast("Installed. StowLock is now in your app menu.");
                                self.installed = true;
                            }
                            Err(e) => self.fail(&e, "install"),
                        }
                    }
                    if ui.small_button("✖").clicked() {
                        self.installed = true;
                    }
                });
            });
        }
    }

    fn error_card(&mut self, ui: &mut egui::Ui) {
        let Some((f, raw)) = self.error.clone() else {
            return;
        };
        ui::banner(ui, ERROR, |ui| {
            ui.add(egui::Label::new(RichText::new(&f.message).color(ERROR).strong()).wrap());
            ui.horizontal_wrapped(|ui| {
                match f.action {
                    Some(Action::SignIn) => {
                        if ui.button("Sign in").clicked() {
                            self.show_settings = true;
                            self.start_sign_in();
                        }
                    }
                    Some(Action::SetIdentity) => {
                        if ui.button("Set name and email").clicked() {
                            self.show_settings = true;
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

    fn stage_tab(&mut self, ui: &mut egui::Ui, hovering: bool) {
        self.error_card(ui);
        // ---- drop zone
        let has_zip = self.session.zip.is_some() || self.session.stage.is_some();
        let (fill, stroke) = if hovering {
            (ACCENT.linear_multiply(0.12), ACCENT)
        } else {
            (ui::CARD, BORDER)
        };
        egui::Frame::new()
            .fill(fill)
            .stroke(egui::Stroke::new(
                if hovering { 2.0_f32 } else { 1.0 },
                stroke,
            ))
            .corner_radius(egui::CornerRadius::same(12))
            .inner_margin(egui::Margin::same(16))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.vertical_centered(|ui| {
                    if let Some(z) = &self.session.zip {
                        ui.label(RichText::new("📦").size(26.0));
                        ui.add(
                            egui::Label::new(
                                ui::mono(
                                    z.file_name()
                                        .map(|n| n.to_string_lossy().to_string())
                                        .unwrap_or_default(),
                                )
                                .size(15.0)
                                .strong(),
                            )
                            .truncate(),
                        );
                        ui.add(egui::Label::new(ui::muted(util::tilde(z)).size(11.5)).truncate())
                            .on_hover_text(util::tilde(z));
                    } else if let Some(st) = &self.session.stage {
                        ui.label(RichText::new("📁").size(26.0));
                        ui.add(
                            egui::Label::new(ui::mono(util::tilde(&st.dir)).size(14.0).strong())
                                .truncate(),
                        );
                        ui.label(ui::muted("Loaded from your workspace"));
                    } else {
                        ui.label(RichText::new("⬇").size(30.0).color(if hovering {
                            ACCENT
                        } else {
                            MUTED
                        }));
                        ui.label(
                            RichText::new(if hovering {
                                "Drop the zip to load it"
                            } else {
                                "Choose a project .zip to get started"
                            })
                            .size(15.0),
                        );
                        ui.label(ui::muted("or drag it onto this window · Ctrl+O").size(12.0));
                    }
                    ui.add_space(4.0);
                    if ui
                        .add_enabled(
                            self.busy.is_none(),
                            egui::Button::new(
                                RichText::new(if has_zip {
                                    "Choose another .zip…"
                                } else {
                                    "Choose .zip…"
                                })
                                .strong(),
                            ),
                        )
                        .clicked()
                    {
                        self.pick_zip();
                    }
                    if let Some(st) = &self.session.stage {
                        ui.horizontal_wrapped(|ui| {
                            ui::badge(ui, st.stack().name(), ACCENT);
                            ui.label(ui::muted(format!(
                                "{} files · {}",
                                st.files,
                                util::human_size(st.size)
                            )));
                            let n = crate::archive::count_archives(
                                &self.config.archive_root(),
                                &self.session.slug,
                            );
                            if n > 0 {
                                ui.label(ui::muted(format!(
                                    "· {n} archived version{}",
                                    if n == 1 { "" } else { "s" }
                                )));
                            }
                        });
                    }
                });
            });
        if !has_zip {
            ui.add_space(8.0);
            ui.label(ui::muted("StowLock unpacks your zip into its own project folder, checks it, lets you try it, then pushes it to the right GitHub repo and files the zip away."));
            self.log_drawer(ui);
            return;
        }
        ui.add_space(8.0);
        self.routing_card(ui);
        ui.add_space(8.0);
        if !self.session.archive_only {
            self.details_card(ui);
            ui.add_space(8.0);
            self.results_card(ui);
        } else {
            ui::card(ui, |ui| {
                ui::section_title(ui, "ARCHIVE ONLY");
                ui.add(egui::Label::new("The zip is filed with this project. Nothing is extracted, checked or pushed to GitHub.").wrap());
                ui.add_space(4.0);
                ui.horizontal_wrapped(|ui| {
                    ui.label("Version");
                    let r = ui.add(
                        egui::TextEdit::singleline(&mut self.session.tag)
                            .hint_text("optional")
                            .desired_width(140.0)
                            .font(egui::TextStyle::Monospace),
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
                    ui.add(egui::Label::new(RichText::new(format!("{tag} is already archived for this project. The new copy is kept alongside it (nothing is overwritten).")).color(WARNING).size(12.5)).wrap());
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
        if let Some(out) = self.session.outcome.clone() {
            ui.add_space(8.0);
            self.completion_card(ui, &out);
        }
        self.log_drawer(ui);
    }

    fn routing_card(&mut self, ui: &mut egui::Ui) {
        let mut slug_changed = false;
        let mut owner_changed = false;
        let staged = self.session.stage.is_some();
        ui::card(ui, |ui| {
            ui.horizontal(|ui| {
                ui::section_title(ui, "ROUTING");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let m = self.session.effective_mode();
                    let col = match m {
                        Mode::New => ACCENT,
                        Mode::Update => SUCCESS,
                        Mode::ArchiveOnly => MUTED,
                    };
                    ui::badge(ui, m.badge(), col);
                    if self.session.channel == Channel::Beta && m != Mode::ArchiveOnly {
                        ui::badge(ui, "BETA", WARNING);
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
                MUTED
            } else {
                ACCENT
            };
            egui::Grid::new("route_grid")
                .num_columns(2)
                .spacing([10.0, 4.0])
                .show(ui, |ui| {
                    for (label, text, color) in [
                        ("Zip", &src, TEXT),
                        ("Folder", &dest, TEXT),
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
                .num_columns(2)
                .spacing([12.0, 8.0])
                .show(ui, |ui| {
                    ui.label("Repo name");
                    ui.horizontal_wrapped(|ui| {
                        let mut names: Vec<String> =
                            self.projects.iter().map(|p| p.slug.clone()).collect();
                        if let Some(r) = &self.env.repo_names {
                            names.extend(r.iter().cloned());
                        }
                        names.sort_by_key(|n| n.to_lowercase());
                        names.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
                        let field_w = (ui.available_width() - 150.0).clamp(140.0, 560.0);
                        let r = ui.add_enabled(
                            !staged,
                            egui::TextEdit::singleline(&mut self.session.slug)
                                .font(egui::TextStyle::Monospace)
                                .desired_width(field_w),
                        );
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
                        ui.add_enabled_ui(!staged, |ui| {
                            egui::ComboBox::from_id_salt("existing")
                                .selected_text("Existing…")
                                .width(110.0)
                                .show_ui(ui, |ui| {
                                    for n in names {
                                        if ui.selectable_label(n == self.session.slug, &n).clicked()
                                        {
                                            self.session.slug = n;
                                            slug_changed = true;
                                        }
                                    }
                                });
                        });
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
                            egui::ComboBox::from_id_salt("owner")
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
                        });
                        ui.end_row();
                    }
                });
            if let Some(problem) = detect::name_problem(&self.session.slug) {
                ui.add(egui::Label::new(RichText::new(problem).color(ERROR).size(12.5)).wrap());
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
            } else if self.session.mode == Mode::New
                && self.session.local_exists
                && !self.session.archive_only
                && !staged
            {
                ui.colored_label(
                    WARNING,
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
                ui.colored_label(SUCCESS, txt);
            }
        });
        if slug_changed || owner_changed {
            self.recompute_mode();
            if owner_changed && !self.session.owner.is_empty() {
                let owner = self.session.owner.clone();
                let gh = self.gh.clone();
                self.spawn_bg(move || {
                    Msg::Repos(
                        owner.clone(),
                        gh.list_repos(&owner)
                            .map(|v| v.into_iter().map(|r| r.name).collect()),
                    )
                });
            }
        }
    }

    fn details_card(&mut self, ui: &mut egui::Ui) {
        let mode = self.session.mode;
        let tags = self.existing_tags();
        ui::card(ui, |ui| {
            ui::section_title(ui, "DETAILS");
            egui::Grid::new("details")
                .num_columns(2)
                .spacing([12.0, 8.0])
                .show(ui, |ui| {
                    if mode == Mode::New {
                        ui.label("Description");
                        ui.add(
                            egui::TextEdit::singleline(&mut self.session.description)
                                .desired_width(f32::INFINITY),
                        );
                        ui.end_row();
                        ui.label("Topics");
                        ui.add(
                            egui::TextEdit::singleline(&mut self.session.topics)
                                .hint_text("rust, cli, linux")
                                .desired_width(f32::INFINITY),
                        );
                        ui.end_row();
                        ui.label("License");
                        egui::ComboBox::from_id_salt("license")
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
                        ui.end_row();
                        ui.label("Visibility");
                        ui.horizontal_wrapped(|ui| {
                            ui.selectable_value(&mut self.session.private, true, "🔒 Private");
                            ui.selectable_value(&mut self.session.private, false, "🌐 Public");
                        });
                        ui.end_row();
                    }
                    ui.label("Version");
                    ui.horizontal_wrapped(|ui| {
                        ui.selectable_value(&mut self.session.channel, Channel::Stable, "Stable");
                        ui.selectable_value(&mut self.session.channel, Channel::Beta, "Beta");
                        let r = ui.add(
                            egui::TextEdit::singleline(&mut self.session.tag)
                                .hint_text("optional, e.g. v0.1.0")
                                .desired_width(150.0)
                                .font(egui::TextStyle::Monospace),
                        );
                        if r.changed() && !self.session.commit_edited {
                            self.session.commit_msg = version::default_commit_message(
                                mode == Mode::New,
                                &self.session.tag,
                            );
                        }
                        let sug =
                            version::suggest(&tags, self.session.channel, &self.config.tag_prefix);
                        if self.session.tag != sug
                            && ui.small_button(format!("Use {sug}")).clicked()
                        {
                            self.session.tag = sug;
                            if !self.session.commit_edited {
                                self.session.commit_msg = version::default_commit_message(
                                    mode == Mode::New,
                                    &self.session.tag,
                                );
                            }
                        }
                    });
                    ui.end_row();
                    ui.label("Commit message");
                    if ui
                        .add(
                            egui::TextEdit::singleline(&mut self.session.commit_msg)
                                .desired_width(f32::INFINITY),
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
            if let Err(e) = version::validate(
                &self.session.tag,
                self.session.channel,
                &self.config.tag_prefix,
                &tags,
            ) {
                ui.label(RichText::new(e).color(ERROR).size(12.5));
            }
            if mode == Mode::New && !self.session.private {
                ui.label(
                    RichText::new(
                        "Public: anyone can see this repo. Secret-scan hits can't be overridden.",
                    )
                    .color(WARNING)
                    .size(12.5),
                );
            }
        });
    }

    fn results_card(&mut self, ui: &mut egui::Ui) {
        let steps = self.log.steps();
        let Some(st) = self.session.stage.clone() else {
            ui::card(ui, |ui| {
                ui::section_title(ui, "PIPELINE");
                ui::stepper(ui, &STEPS, &steps);
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
            ui::section_title(ui, "PIPELINE");
            ui::stepper(ui, &STEPS, &steps);
            for n in st.notes.iter().chain(st.cleanup.lines().iter()) {
                ui.label(RichText::new(format!("✔ {n}")).color(SUCCESS).size(13.0));
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
                        ui.label(ui::mono(format!("+ {f}")).color(SUCCESS));
                    }
                    for f in &ch.changed {
                        ui.label(ui::mono(format!("~ {f}")).color(WARNING));
                    }
                    for f in &ch.removed {
                        ui.label(ui::mono(format!("− {f}")).color(ERROR));
                    }
                }
            }
            for (f, n) in &st.warn_files {
                ui.colored_label(
                    WARNING,
                    format!("Large file: {f} ({})", util::human_size(*n)),
                );
            }
            for (f, n) in &st.block_files {
                ui.colored_label(
                    ERROR,
                    format!(
                        "Over GitHub's 100 MB limit: {f} ({}). Remove it or add it to .gitignore.",
                        util::human_size(*n)
                    ),
                );
            }
        });
        ui.add_space(8.0);
        // ---- checks
        ui::card(ui, |ui| {
            ui::section_title(ui, "CHECKS");
            match self.session.checks.clone() {
                None => {
                    ui.horizontal_wrapped(|ui| {
                        if self.busy.is_some() {
                            ui.spinner();
                            ui.label("Running tests and the secret scan…");
                            if ui.button("Cancel").clicked() {
                                self.cancel.store(true, Ordering::Relaxed);
                            }
                        } else {
                            ui.label(ui::muted("Checks haven't run yet."));
                        }
                    });
                }
                Some(c) => {
                    let (txt, col) = match c.tests.status {
                        TestStatus::Passed => ("Tests passed", SUCCESS),
                        TestStatus::Failed => ("Tests failed", ERROR),
                        TestStatus::Skipped => ("Tests skipped", MUTED),
                        TestStatus::Cancelled => ("Tests cancelled", WARNING),
                    };
                    ui.horizontal_wrapped(|ui| {
                        ui::badge(ui, txt, col);
                        ui.label(ui::muted(format!(
                            "{} · {:.1}s",
                            c.tests.summary,
                            c.tests.duration.as_secs_f32()
                        )));
                    });
                    if let Some(h) = &c.tests.hint {
                        ui.horizontal_wrapped(|ui| {
                            ui.label(ui::mono(h));
                            if ui.small_button("Copy").clicked() {
                                ui.ctx().copy_text(h.clone());
                            }
                        });
                    }
                    if !c.tests.tail.is_empty() {
                        egui::CollapsingHeader::new("Test output")
                            .id_salt("testout")
                            .default_open(c.tests.status == TestStatus::Failed)
                            .show(ui, |ui| {
                                egui::ScrollArea::vertical()
                                    .max_height(180.0)
                                    .id_salt("testscroll")
                                    .show(ui, |ui| {
                                        ui.label(ui::mono(&c.tests.tail).size(12.0));
                                    });
                            });
                    }
                    if c.tests.status == TestStatus::Failed
                        || c.tests.status == TestStatus::Cancelled
                    {
                        ui.checkbox(
                            &mut self.session.override_tests,
                            "Push anyway (tests failed)",
                        );
                    }
                    ui.add_space(4.0);
                    if c.secrets.is_empty() {
                        ui.horizontal_wrapped(|ui| {
                            ui::badge(ui, "0 secrets found", SUCCESS);
                            if c.ignored_secrets > 0 {
                                ui.label(ui::muted(format!(
                                    "({} marked not a secret)",
                                    c.ignored_secrets
                                )));
                            }
                        });
                    } else {
                        ui::badge(
                            ui,
                            &format!(
                                "{} possible secret{}",
                                c.secrets.len(),
                                if c.secrets.len() == 1 { "" } else { "s" }
                            ),
                            ERROR,
                        );
                        let mut mark: Option<String> = None;
                        for h in &c.secrets {
                            ui.horizontal_wrapped(|ui| {
                                ui.label(ui::mono(format!("{}:{}", h.file, h.line)));
                                ui.label(ui::muted(format!("{} {}", h.kind, h.masked)));
                                if ui.small_button("Not a secret").clicked() {
                                    mark = Some(h.id.clone());
                                }
                            });
                        }
                        ui.label(RichText::new("If this key was ever pushed before, removing it isn't enough. Revoke it and make a new one.").color(WARNING).size(12.5));
                        if let Some(id) = mark {
                            let slug = self.session.slug.clone();
                            let _ = ProjectsState::update(&self.paths, &slug, |p| {
                                p.not_secret.push(id.clone())
                            });
                            if let Some(c) = self.session.checks.as_mut() {
                                c.secrets.retain(|h| h.id != id);
                                c.ignored_secrets += 1;
                            }
                        }
                    }
                }
            }
        });
        ui.add_space(8.0);
        // ---- try it
        ui::card(ui, |ui| {
            ui::section_title(ui, "TRY IT BEFORE PUSHING");
            ui.horizontal_wrapped(|ui| {
                ui.label("Run command");
                let r = ui.add(
                    egui::TextEdit::singleline(&mut self.session.run_cmd)
                        .font(egui::TextStyle::Monospace)
                        .desired_width(260.0),
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
                    if ui.button("▶ Run").clicked() {
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
                } else if ui.button("■ Stop").clicked() {
                    if let Some(mut c) = self.run_child.take() {
                        let _ = c.kill();
                    }
                }
                if ui.button("Open Folder").clicked() {
                    let _ = proc::xdg_open(&st.dir.display().to_string());
                }
                if ui.button("Open in Editor").clicked() {
                    if let Err(e) = pipeline::open_in_editor(&st.dir) {
                        self.fail(&e, "editor");
                    }
                }
                if ui
                    .add_enabled(self.busy.is_none(), egui::Button::new("↻ Re-check"))
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

    fn completion_card(&mut self, ui: &mut egui::Ui, out: &PushOutcome) {
        let color = if out.nothing_to_push { MUTED } else { SUCCESS };
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
                    .color(SUCCESS)
                    .strong()
                    .size(15.0),
                );
            }
            if let Some(a) = &out.archived {
                ui.label(ui::muted(format!("Zip archived to {}", util::tilde(a))));
            }
            for w in &out.warnings {
                ui.colored_label(WARNING, w);
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

    fn log_drawer(&mut self, ui: &mut egui::Ui) {
        ui.add_space(8.0);
        let r = egui::CollapsingHeader::new(ui::muted("Log"))
            .id_salt("log")
            .open(Some(self.show_log))
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    if ui.small_button("Copy log").clicked() {
                        ui.ctx()
                            .copy_text(report::redact(&self.log.text(), &self.redact_ctx()));
                        self.toast("Log copied (personal details removed).");
                    }
                });
                egui::ScrollArea::vertical()
                    .max_height(220.0)
                    .stick_to_bottom(true)
                    .id_salt("logscroll")
                    .show(ui, |ui| {
                        for e in self
                            .log
                            .entries()
                            .iter()
                            .rev()
                            .take(400)
                            .collect::<Vec<_>>()
                            .into_iter()
                            .rev()
                        {
                            let col = match e.level {
                                Level::Info => MUTED,
                                Level::Warn => WARNING,
                                Level::Error => ERROR,
                            };
                            ui.label(ui::mono(e.line()).size(11.5).color(col));
                        }
                    });
            });
        if r.header_response.clicked() {
            self.show_log = !self.show_log;
        }
    }

    /// The text side of the action bar: wraps inside its own width so it
    /// never runs under the button.
    fn action_text(ui: &mut egui::Ui, button_w: f32, add: impl FnOnce(&mut egui::Ui)) {
        let w = (ui.available_width() - button_w - 16.0).max(120.0);
        ui.vertical(|ui| {
            ui.set_max_width(w);
            ui.set_min_width(w);
            add(ui);
        });
    }

    fn action_bar(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::bottom("actions")
            .frame(
                egui::Frame::new()
                    .fill(ui::CARD)
                    .inner_margin(egui::Margin::symmetric(16, 12))
                    .stroke(egui::Stroke::new(1.0_f32, BORDER)),
            )
            .show(ctx, |ui| {
                let s = self.session.clone();
                let busy = self.busy.clone();
                const BTN: f32 = 180.0;
                if s.zip.is_none() && s.stage.is_none() {
                    ui.label(ui::muted("No project loaded."));
                    return;
                }
                let name_ok = detect::name_problem(&s.slug).is_none();
                if s.archive_only {
                    ui.horizontal(|ui| {
                        Self::action_text(ui, BTN, |ui| {
                            ui.add(
                                egui::Label::new(ui::muted(format!(
                                    "Will file the zip under _archive/{}/. Nothing will be pushed.",
                                    s.slug
                                )))
                                .wrap(),
                            );
                        });
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let ok = busy.is_none() && name_ok && s.zip.is_some();
                            if ui::primary(ui, "Archive Zip", ok).clicked() {
                                self.archive_note_seen = true;
                                self.archive_only();
                            }
                        });
                    });
                    return;
                }
                let Some(stage) = s.stage.clone() else {
                    ui.horizontal(|ui| {
                        Self::action_text(ui, BTN, |ui| {
                            if let Some(b) = &busy {
                                ui.horizontal(|ui| {
                                    ui.spinner();
                                    ui.label(b);
                                });
                            } else {
                                ui.add(
                                    egui::Label::new(ui::muted(
                                        "Unpack and check the project. Nothing goes to GitHub yet.",
                                    ))
                                    .wrap(),
                                );
                            }
                        });
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let ok = busy.is_none()
                                && name_ok
                                && (s.mode == Mode::Update
                                    || !s.local_exists
                                    || s.local_is_ours
                                    || s.replace_local);
                            if ui::primary(ui, "Stage & check", ok).clicked() {
                                self.stage();
                            }
                        });
                    });
                    return;
                };
                let pctx = self.pctx();
                let tags = self.existing_tags();
                let mut blockers = pipeline::push_blockers(
                    &pctx,
                    &stage,
                    s.checks.as_ref(),
                    !s.private && s.mode == Mode::New,
                    s.override_tests,
                );
                if !self.env.ready() {
                    blockers.push("Finish setup (sign in to GitHub)".into());
                }
                if let Err(e) = version::validate(&s.tag, s.channel, &self.config.tag_prefix, &tags)
                {
                    blockers.push(e);
                }
                if s.outcome
                    .as_ref()
                    .map(|o| !o.nothing_to_push)
                    .unwrap_or(false)
                    && s.mode == Mode::New
                {
                    blockers.push("Already pushed".into());
                }
                let changes = stage
                    .changes
                    .as_ref()
                    .map(|c| format!(", {}", c.short()))
                    .unwrap_or_default();
                let tag = if s.tag.trim().is_empty() {
                    String::new()
                } else {
                    format!(
                        ", {} {}{}",
                        s.channel.label(),
                        s.tag.trim(),
                        if s.channel == Channel::Beta {
                            " (pre-release)"
                        } else {
                            ""
                        }
                    )
                };
                let vis = if s.mode == Mode::New {
                    if s.private {
                        " (private)"
                    } else {
                        " (PUBLIC)"
                    }
                } else {
                    ""
                };
                let summary = format!(
                    "Will push to github.com/{}/{}{vis}: {}{tag}{changes}",
                    s.owner,
                    s.slug,
                    s.mode.badge()
                );
                if s.confirm {
                    ui.add(egui::Label::new(RichText::new(&summary).strong()).wrap());
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        if ui.button("Back").clicked() {
                            self.session.confirm = false;
                        }
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let ok = busy.is_none() && blockers.is_empty();
                            let go = ui::primary(ui, "Confirm push", ok).clicked()
                                || (ok
                                    && ui.input(|i| {
                                        i.modifiers.ctrl && i.key_pressed(egui::Key::Enter)
                                    }));
                            if go {
                                self.push();
                            }
                        });
                    });
                    return;
                }
                ui.horizontal(|ui| {
                    Self::action_text(ui, BTN, |ui| {
                        if let Some(b) = &busy {
                            ui.horizontal(|ui| {
                                ui.spinner();
                                ui.label(b);
                                if ui.small_button("Cancel").clicked() {
                                    self.cancel.store(true, Ordering::Relaxed);
                                }
                            });
                        } else if blockers.is_empty() {
                            ui.add(egui::Label::new(ui::muted(&summary).size(12.5)).wrap());
                        } else {
                            for b in blockers.iter().take(3) {
                                ui.add(
                                    egui::Label::new(
                                        RichText::new(format!("• {b}")).color(WARNING).size(12.5),
                                    )
                                    .wrap(),
                                );
                            }
                        }
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let ok = busy.is_none() && blockers.is_empty();
                        if ui::primary(ui, "Push to GitHub", ok).clicked() {
                            self.session.confirm = true;
                        }
                    });
                });
            });
    }

    fn projects_tab(&mut self, ui: &mut egui::Ui) {
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
                    self.refresh_env();
                }
                if ui.button("Open archive").clicked() {
                    let _ = std::fs::create_dir_all(self.config.archive_root());
                    let _ = proc::xdg_open(&self.config.archive_root().display().to_string());
                }
            });
        });
        let today = self.history.today();
        if !today.is_empty() {
            ui::card(ui, |ui| {
                ui::section_title(ui, "PUSHED TODAY");
                ui.horizontal_wrapped(|ui| {
                    for r in &today {
                        if ui
                            .button(RichText::new(r.chip()).monospace().color(
                                if r.channel == "beta" {
                                    WARNING
                                } else {
                                    SUCCESS
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
        for p in self.projects.clone() {
            ui::card(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.add(
                        egui::Label::new(ui::mono(&p.slug).strong().size(15.0).color(TEXT))
                            .truncate(),
                    )
                    .on_hover_text(&p.slug);
                    ui::badge(ui, p.stack.name(), ACCENT);
                    let col = if p.badge.starts_with("Beta") {
                        WARNING
                    } else if p.badge.starts_with("Pushed") || p.badge == "On GitHub" {
                        SUCCESS
                    } else if p.badge == "Local changes" {
                        ERROR
                    } else {
                        MUTED
                    };
                    ui::badge(ui, &p.badge, col);
                });
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
    }

    // ------------------------------------------------------------ windows

    fn windows(&mut self, ctx: &egui::Context) {
        if self.show_today {
            let mut open = true;
            let today = self.history.today();
            ui_window(ctx, "Pushed today", 360.0)
                .open(&mut open)
                .show(ctx, |ui| {
                    if today.is_empty() {
                        ui.label(ui::muted("Nothing pushed yet today."));
                    }
                    for r in &today {
                        self.today_row(ui, r);
                    }
                });
            self.show_today = open;
        }
        if self.show_health {
            self.health_window(ctx);
        }
        if self.show_settings {
            self.settings_window(ctx);
        }
        if self.show_bug {
            self.bug_window(ctx);
        }
        if self.show_uninstall {
            self.uninstall_window(ctx);
        }
    }

    fn today_row(&self, ui: &mut egui::Ui, r: &PushRecord) {
        ui.horizontal_wrapped(|ui| {
            ui.label(ui::mono(r.time()).color(MUTED));
            if ui.link(RichText::new(&r.slug).monospace()).clicked() {
                let _ = proc::xdg_open(&r.github_url);
            }
            if let Some(t) = &r.tag {
                ui::badge(
                    ui,
                    t,
                    if r.channel == "beta" {
                        WARNING
                    } else {
                        SUCCESS
                    },
                );
            }
            ui.label(ui::muted(format!("{} · {}", r.mode, r.visibility)));
        });
    }

    fn health_window(&mut self, ctx: &egui::Context) {
        let mut open = true;
        let mut run = false;
        ui_window(ctx, "Health", 560.0).open(&mut open).show(ctx, |ui| {
            ui.label(RichText::new(self.health.summary()).strong().size(16.0));
            egui::Grid::new("health").num_columns(3).max_col_width(240.0).striped(true).spacing([14.0, 6.0]).show(ui, |ui| {
                for a in health::AREAS {
                    let s = self.health.get(a);
                    let col = match s.status {
                        Health::Working => SUCCESS,
                        Health::NeedsAttention => WARNING,
                        Health::Broken => ERROR,
                        Health::NotUsed => MUTED,
                    };
                    ui.label(a);
                    ui.label(RichText::new(format!("⏺ {}", s.status.label())).color(col));
                    ui.label(ui::muted(if s.status == Health::NotUsed { String::new() } else { format!("{} · {}{}", s.source, s.checked_at, if s.hint.is_empty() { String::new() } else { format!(" · {}", s.hint) }) }).size(12.0));
                    ui.end_row();
                }
            });
            ui.separator();
            ui.horizontal_wrapped(|ui| {
                if ui.add_enabled(self.busy.is_none(), egui::Button::new(RichText::new("Run self-test").strong())).clicked() {
                    run = true;
                }
                ui.checkbox(&mut self.selftest_online, "Include read-only GitHub checks");
                if self.busy.as_deref() == Some("Running self-test") {
                    ui.spinner();
                }
            });
            ui.label(ui::muted("The self-test runs in a sandbox. It never touches your projects and never writes to GitHub.").size(12.0));
            if let Some((r, path)) = &self.last_selftest {
                ui.label(RichText::new(r.summary_line()).strong());
                for c in r.checks.iter().filter(|c| c.outcome != selftest::Outcome::Pass) {
                    let col = if c.outcome == selftest::Outcome::Fail { ERROR } else { MUTED };
                    ui.label(RichText::new(format!("{} {} — {}", c.id, c.name, c.detail)).color(col).size(12.5));
                }
                ui.horizontal_wrapped(|ui| {
                    if ui.button("Copy summary").clicked() {
                        let text = report::redact(&r.to_markdown(), &self.redact_ctx());
                        ui.ctx().copy_text(text);
                    }
                    if r.counts().1 > 0 && ui.button("Copy AI fix prompt").clicked() {
                        ui.ctx().copy_text(report::redact(&r.ai_prompts(), &self.redact_ctx()));
                    }
                    if let Some(p) = path {
                        if ui.button("Open report").clicked() {
                            let _ = proc::xdg_open(&p.display().to_string());
                        }
                    }
                });
            }
        });
        if run {
            self.run_selftest();
        }
        self.show_health = open;
    }

    fn settings_window(&mut self, ctx: &egui::Context) {
        let mut open = true;
        ui_window(ctx, "Settings", 560.0).open(&mut open).show(ctx, |ui| {
            self.account_panel(ui);
            ui.separator();
            ui::section_title(ui, "GENERAL");
            egui::Grid::new("general").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
                ui.label("Workspace folder");
                ui.add(egui::TextEdit::singleline(&mut self.config.workspace_dir).font(egui::TextStyle::Monospace).desired_width(300.0));
                ui.end_row();
                ui.label("New repos");
                ui.checkbox(&mut self.config.default_private, "Private by default");
                ui.end_row();
                ui.label("Default license");
                egui::ComboBox::from_id_salt("deflic").selected_text(self.config.default_license.clone()).show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.config.default_license, "none".to_string(), "none");
                    ui.selectable_value(&mut self.config.default_license, "MIT".to_string(), "MIT");
                    ui.selectable_value(&mut self.config.default_license, "GPL-3.0".to_string(), "GPL-3.0");
                });
                ui.end_row();
                ui.label("License holder");
                ui.add(egui::TextEdit::singleline(&mut self.config.license_holder).hint_text("Your name").desired_width(300.0));
                ui.end_row();
                ui.label("After push");
                ui.horizontal_wrapped(|ui| {
                    ui.checkbox(&mut self.config.archive_after_push, "Archive the zip");
                    ui.radio_value(&mut self.config.archive_mode, "move".to_string(), "Move");
                    ui.radio_value(&mut self.config.archive_mode, "copy".to_string(), "Copy");
                });
                ui.end_row();
                ui.label("Tag prefix");
                ui.add(egui::TextEdit::singleline(&mut self.config.tag_prefix).desired_width(60.0));
                ui.end_row();
            });
            ui.separator();
            ui::section_title(ui, "CHECKS");
            ui.checkbox(&mut self.config.run_tests_before_push, "Run tests before pushing");
            ui.horizontal_wrapped(|ui| {
                ui.label("Test time limit");
                ui.add(egui::DragValue::new(&mut self.config.test_timeout_secs).range(30..=3600).suffix(" s"));
            });
            ui.checkbox(&mut self.config.block_on_secrets, "Block pushes when possible secrets are found");
            ui.separator();
            ui::section_title(ui, "PRIVACY");
            ui.horizontal_wrapped(|ui| {
                ui.label("Delete logs and reports after");
                ui.add(egui::DragValue::new(&mut self.config.report_retention_days).range(1..=365).suffix(" days"));
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
            ui.separator();
            ui::section_title(ui, "UPDATES");
            ui.horizontal_wrapped(|ui| {
                ui.checkbox(&mut self.config.check_for_updates, "Check for updates automatically");
                if ui.button("Check now").clicked() {
                    self.check_updates(true);
                }
            });
            egui::CollapsingHeader::new(ui::muted("Advanced")).id_salt("adv").show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label("Update source");
                    ui.add(egui::TextEdit::singleline(&mut self.config.update_repo).font(egui::TextStyle::Monospace).desired_width(240.0));
                });
            });
            ui.separator();
            ui::section_title(ui, "ABOUT");
            ui.label(format!("StowLock {APP_VERSION} · Free software under the GNU GPL v3 · github.com/{}", crate::config::UPDATE_REPO));
            ui.add_space(8.0);
            if ui.button(RichText::new("Uninstall StowLock…").color(ERROR)).clicked() {
                self.show_uninstall = true;
                self.uninstall.result = None;
                self.uninstall.plan = None;
                self.uninstall.typed.clear();
            }
        });
        self.show_settings = open;
    }

    fn account_panel(&mut self, ui: &mut egui::Ui) {
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
            ui::banner(ui, WARNING, |ui| {
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
            ui::banner(ui, ACCENT, |ui| {
                if let Some(code) = &self.signin.code {
                    ui.label("Your one-time code (already copied):");
                    ui.label(
                        RichText::new(code)
                            .monospace()
                            .size(28.0)
                            .strong()
                            .color(ACCENT),
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
            ui.colored_label(WARNING, e);
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
                        RichText::new(if a.active { "⏺" } else { "○" }).color(if a.active {
                            SUCCESS
                        } else {
                            MUTED
                        }),
                    );
                    ui.label(ui::mono(&a.login).strong());
                    if a.active {
                        ui.label(ui::muted("signed in"));
                    } else if ui.small_button("Switch").clicked() {
                        switch = Some(a.login.clone());
                    }
                    if !a.secure {
                        ui::badge(ui, "plain-file token", WARNING);
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
            .num_columns(2)
            .spacing([12.0, 6.0])
            .show(ui, |ui| {
                ui.label("Name");
                ui.add(egui::TextEdit::singleline(&mut self.identity_name).desired_width(280.0));
                ui.end_row();
                ui.label("Email");
                ui.add(egui::TextEdit::singleline(&mut self.identity_email).desired_width(280.0));
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

    fn bug_window(&mut self, ctx: &egui::Context) {
        let mut open = true;
        let mut rebuild = false;
        let mut download = false;
        ui_window(ctx, "Bug report", 620.0).open(&mut open).show(ctx, |ui| {
            ui.add(egui::Label::new(ui::muted("Nothing is sent anywhere unless you copy, download or share this. Personal details are removed below.")).wrap());
            ui.add_space(4.0);
            ui.label("What were you doing? (optional)");
            rebuild |= ui.add(egui::TextEdit::multiline(&mut self.bug.what).desired_rows(2).desired_width(f32::INFINITY)).changed();
            ui.label("What did you expect? (optional)");
            rebuild |= ui.add(egui::TextEdit::multiline(&mut self.bug.expected).desired_rows(2).desired_width(f32::INFINITY)).changed();
            ui.horizontal_wrapped(|ui| {
                rebuild |= ui.checkbox(&mut self.bug.include_public, "Include public repo names").changed();
                rebuild |= ui.checkbox(&mut self.bug.include_username, "Include my GitHub username").changed();
            });
            ui.add_space(4.0);
            ui.label(RichText::new("Preview (exactly what will be shared)").strong());
            egui::ScrollArea::vertical().max_height(260.0).id_salt("bugprev").show(ui, |ui| {
                let mut p = self.bug.preview.clone();
                ui.add(egui::TextEdit::multiline(&mut p).code_editor().desired_width(f32::INFINITY).interactive(true));
            });
            ui.add_space(4.0);
            ui.horizontal_wrapped(|ui| {
                if ui.button(RichText::new("Copy to clipboard").strong()).clicked() {
                    ui.ctx().copy_text(self.bug.preview.clone());
                    self.toast("Bug report copied.");
                }
                if ui.add_enabled(!self.bug.saving, egui::Button::new("Download as .txt…")).on_hover_text(report::bug_report_file_name(&self.bug.created)).clicked() {
                    download = true;
                }
                if ui.button("Save").on_hover_text("Keep a copy in StowLock's reports folder").clicked() {
                    match report::save_report(&self.paths, "bugreport", &self.bug.preview) {
                        Ok(p) => self.toast(&format!("Saved {}", util::tilde(&p))),
                        Err(e) => self.fail(&e, "report"),
                    }
                }
                if ui.button("Open GitHub issue…").clicked() {
                    self.bug.confirm_issue = true;
                }
                if self.bug.saving {
                    ui.spinner();
                }
            });
            if let Some(p) = self.bug.saved_to.clone() {
                ui::banner(ui, SUCCESS, |ui| {
                    ui.add(egui::Label::new(format!("Downloaded to {}", util::tilde(&p))).wrap());
                    if ui.button("Open folder").clicked() {
                        let folder = p.parent().map(|f| f.display().to_string()).unwrap_or_default();
                        let _ = proc::xdg_open(&folder);
                    }
                });
            }
            if self.bug.confirm_issue {
                let who = self.env.login.clone().unwrap_or_else(|| "your account".into());
                ui::banner(ui, WARNING, |ui| {
                    ui.add(egui::Label::new(format!("This will be public on GitHub under the account {who}, on {}.", self.config.update_repo)).wrap());
                    ui.horizontal_wrapped(|ui| {
                        if ui.button("Open issue").clicked() {
                            match report::save_report(&self.paths, "bugreport", &self.bug.preview) {
                                Ok(p) => {
                                    let o = proc::run(
                                        "gh",
                                        &["issue", "create", "--repo", &self.config.update_repo, "--title", "Bug report", "--body-file", &p.display().to_string()],
                                        &proc::Opts::default_timeout(),
                                    );
                                    self.log.command("report", &o);
                                    if o.ok() {
                                        let url = o.stdout_trim();
                                        self.toast("Issue opened.");
                                        let _ = proc::xdg_open(&url);
                                    } else {
                                        self.fail(&o.combined(), "report");
                                    }
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
        });
        if rebuild {
            self.bug.preview = self.build_bug_report();
        }
        if download {
            self.download_bug_report();
        }
        self.show_bug = open;
    }

    fn uninstall_window(&mut self, ctx: &egui::Context) {
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
                                    .color(SUCCESS),
                            );
                            for r in &out.removed {
                                ui.label(ui::mono(format!("removed {r}")).size(12.0));
                            }
                            for t in &out.trashed {
                                ui.label(ui::mono(format!("moved to Trash: {t}")).size(12.0));
                            }
                            for p in &out.problems {
                                ui.colored_label(WARNING, p);
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
                            ui.colored_label(ERROR, e);
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
                        .color(ERROR),
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
                                    WARNING,
                                    "Some projects have work that isn't on GitHub.",
                                );
                                if ui.button("Open Folder").clicked() {
                                    let _ = proc::xdg_open(&plan.workspace.display().to_string());
                                }
                            });
                        }
                        ui.horizontal_wrapped(|ui| {
                            ui.label("Type DELETE to confirm:");
                            ui.add(
                                egui::TextEdit::singleline(&mut self.uninstall.typed)
                                    .desired_width(100.0)
                                    .font(egui::TextStyle::Monospace),
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
                                    .color(ERROR)
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
            std::process::exit(0);
        }
        if !open {
            self.show_uninstall = false;
        }
    }
}
