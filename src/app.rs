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

/// What each step tile says before it runs.
const WAITING: [&str; 6] = [
    "Waiting",
    "Waiting",
    "Waiting",
    "After checks pass",
    "After push",
    "After release",
];

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
    /// GitHub's build for a pushed commit: (watch id, status).
    Build(u64, Result<crate::github::Build, String>),
    /// Finish the release / Remove the tag / Delete release finished.
    ReleaseFixed(Result<String, String>),
    Archived(Result<PathBuf, String>),
    Projects(Vec<ProjectInfo>),
    SelfTest(Box<SelfTestReport>, Option<PathBuf>),
    UpdateChecked(Result<updater::CheckResult, String>),
    UpdateApplied(Result<PathBuf, String>),
    RolledBack(Result<(PathBuf, String), String>),
    SignIn(SignInEvent),
    Tools(Output),
    BugDownloaded(Result<Option<PathBuf>, String>),
    Done,
}

#[derive(PartialEq, Clone, Copy)]
enum Tab {
    Stage,
    Projects,
    History,
    Health,
    Report,
    Settings,
}

enum UpdateState {
    None,
    Available(updater::Release),
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
    /// The local folder pushes to a repo that was deleted on GitHub.
    deleted_on_github: bool,
    /// The project's own release workflow publishes its releases (checked once).
    workflow_publishes: Option<bool>,
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
    /// When the review window opened; its push button ignores clicks for
    /// half a second so a double-click can't push by accident.
    confirm_at: Option<std::time::Instant>,
    /// "I understand this will be public" for a new public repo.
    public_ack: bool,
    /// The tag as it was last frame (to react to edits and one-click picks).
    last_tag: String,
    /// "Push it anyway" for a version that counts as older than a release.
    older_ack: bool,
    /// Open the review window by itself once the checks pass.
    review_when_ready: bool,
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
            deleted_on_github: false,
            workflow_publishes: None,
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
            confirm_at: None,
            public_ack: false,
            last_tag: String::new(),
            older_ack: false,
            review_when_ready: false,
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

/// GitHub's build for the last push, followed in the background.
#[derive(Debug, Clone)]
struct BuildWatch {
    id: u64,
    owner: String,
    slug: String,
    tag: Option<String>,
    status: Option<crate::github::Build>,
    /// The local checks passed (so a failure on GitHub is a mismatch).
    local_passed: bool,
    dismissed: bool,
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
    /// Report sections the user unticked.
    excluded: Vec<String>,
    /// The "Also hide these words" box.
    hide_input: String,
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
    /// The screen shown last frame (to notice opening Projects).
    prev_tab: Tab,
    env: EnvStatus,
    session: Session,
    projects: Vec<ProjectInfo>,
    /// Earlier attempts StowLock set aside (Projects > Saved earlier attempts).
    attempts: Vec<crate::attempts::Attempt>,
    confirm_delete_attempt: Option<usize>,
    logo: Option<egui::TextureHandle>,
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
    show_uninstall: bool,
    show_log: bool,
    bug: BugForm,
    uninstall: UninstallUi,
    selftest_online: bool,
    last_selftest: Option<(SelfTestReport, Option<PathBuf>)>,
    run_child: Option<std::process::Child>,
    crash_report: Option<PathBuf>,
    /// A version to go back to, offered after this one crashed.
    rollback_offer: Option<String>,
    installed: bool,
    /// Version of the installed copy when the running copy isn't it.
    installed_version: Option<String>,
    /// Set when pacman installed StowLock (the AUR package): updates go through paru/yay.
    pacman_pkg: Option<String>,
    /// Following GitHub's build after a push.
    build: Option<BuildWatch>,
    /// Seconds before the first build check, and between checks.
    build_poll: (u64, u64),
    /// "Delete release…" in History: (owner, slug, tag, what the user typed).
    deleting_release: Option<(String, String, String, String)>,
    /// Confirming "Remove the tag" / "Delete the release and tag" after a push.
    confirm_remove_tag: bool,
    /// Each account's repos on GitHub, as last loaded (lower-case owner).
    repo_lists: std::collections::HashMap<String, Vec<String>>,
    /// Accounts whose list is loading now / when each was last asked for.
    repos_loading: std::collections::HashSet<String>,
    repos_asked: std::collections::HashMap<String, Instant>,
    /// Accounts whose last reload failed (the list shown may be out of date).
    repos_stale: std::collections::HashSet<String>,
    /// The "Existing…" menu was open last frame / the window was focused.
    existing_open: bool,
    was_focused: bool,
    /// Projects: the folder waiting for "Remove the folder" confirmation.
    confirm_trash_project: Option<String>,
    /// The last update check's result, shown in Settings.
    update_note: Option<String>,
    /// The running check was started with Check now (so say the result).
    update_manual: bool,
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
        ui::install_fonts(&cc.egui_ctx);
        let config = Config::load(&paths);
        let light = wants_light(&config.theme, cc.egui_ctx.system_theme());
        ui::apply_theme(&cc.egui_ctx, light);
        let (tx, rx) = channel();
        let crash_report = std::fs::read_to_string(paths.crash_marker())
            .ok()
            .map(|s| PathBuf::from(s.trim()));
        let _ = std::fs::remove_file(paths.crash_marker());
        let rollback_offer = updater::take_rollback_offer(&paths);
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
        let pacman_pkg = std::env::current_exe()
            .ok()
            .and_then(|e| std::fs::canonicalize(e).ok())
            .and_then(|e| crate::uninstall::pacman_owner(&e));
        let mut installed = install::is_installed(&inst) || pacman_pkg.is_some();
        let installed_version = if installed {
            None
        } else {
            install::program_version(&inst.bin)
        };
        if installed_version.as_deref() == Some(APP_VERSION) {
            installed = true;
        }
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
            prev_tab: Tab::Stage,
            env: EnvStatus::default(),
            projects: Vec::new(),
            attempts: Vec::new(),
            confirm_delete_attempt: None,
            logo: None,
            error: None,
            toast: None,
            update: UpdateState::None,
            signin: SignInUi::default(),
            tools_installing: false,
            tools_result: None,
            identity_name: String::new(),
            identity_email: String::new(),
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
                excluded: Vec::new(),
                hide_input: String::new(),
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
            rollback_offer,
            installed,
            installed_version,
            pacman_pkg,
            build: None,
            build_poll: (8, 15),
            deleting_release: None,
            confirm_remove_tag: false,
            repo_lists: Default::default(),
            repos_loading: Default::default(),
            repos_asked: Default::default(),
            repos_stale: Default::default(),
            existing_open: false,
            was_focused: true,
            confirm_trash_project: None,
            update_note: None,
            update_manual: false,
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
        // --open-screen NAME: open on a given screen (used for layout checks).
        let args: Vec<String> = std::env::args().collect();
        if let Some(name) = args
            .iter()
            .position(|a| a == "--open-screen")
            .and_then(|i| args.get(i + 1))
        {
            app.tab = match name.as_str() {
                "projects" => Tab::Projects,
                "history" => Tab::History,
                "health" => Tab::Health,
                "report" => {
                    app.open_bug_report();
                    Tab::Report
                }
                "settings" => Tab::Settings,
                _ => Tab::Stage,
            };
            if app.tab == Tab::Projects {
                app.refresh_attempts();
            }
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
            trash_dir: None,
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
        self.log
            .info("intake", &format!("Loaded {}", util::tilde(&zip)));
        self.tab = Tab::Stage;
        self.recompute_mode();
    }

    /// Load an existing workspace project (to try, re-check and push edits made by hand).
    fn load_project(&mut self, p: &ProjectInfo) {
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

    fn recompute_mode(&mut self) {
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
    fn github_repos(&self, owner: &str) -> Option<&Vec<String>> {
        self.repo_lists.get(&owner.to_ascii_lowercase())
    }

    /// Reload an account's repo list from GitHub (at most every few seconds).
    fn refresh_repos(&mut self, owner: &str, force: bool) {
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
    fn project_owner_repo(&self, p: &ProjectInfo) -> Option<(String, String)> {
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
    fn gone_from_github(&self, p: &ProjectInfo) -> bool {
        let Some((owner, repo)) = self.project_owner_repo(p) else {
            return false;
        };
        self.github_repos(&owner)
            .is_some_and(|names| !names.iter().any(|n| n.eq_ignore_ascii_case(&repo)))
    }

    /// Reload the lists for every account the projects push to.
    fn refresh_project_owners(&mut self, force: bool) {
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
        self.session.review_when_ready = true;
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

    fn apply_update(&mut self, tag: String) {
        self.update = UpdateState::Working(tag.clone());
        let paths = self.paths.clone();
        let repo = self.config.update_repo.clone();
        let log = self.log.clone();
        self.spawn_bg(move || Msg::UpdateApplied(updater::apply(&paths, &repo, &tag, &log)));
    }

    /// Follow GitHub's build for a pushed commit: check every 15 seconds for
    /// up to 40 minutes. No build within 90 seconds means the project has none.
    fn watch_build(&mut self, commit: String, tag: Option<String>) {
        let id = self.build.as_ref().map(|w| w.id + 1).unwrap_or(1);
        let local_passed = self
            .session
            .checks
            .as_ref()
            .map(|c| c.tests.status == TestStatus::Passed)
            .unwrap_or(false);
        self.build = Some(BuildWatch {
            id,
            owner: self.session.owner.clone(),
            slug: self.session.slug.clone(),
            tag,
            status: None,
            local_passed,
            dismissed: false,
        });
        let gh = self.gh.clone();
        let (owner, slug) = (self.session.owner.clone(), self.session.slug.clone());
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

    fn fix_release(&mut self, finish: bool, owner: String, slug: String, tag: String, beta: bool) {
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

    /// The GitHub build card (and the failed-release card) after a push.
    fn after_push_cards(&mut self, ui: &mut egui::Ui) {
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
                    if w.local_passed {
                        ui.add(egui::Label::new(ui::muted("It passed here but failed on GitHub: GitHub runs on a different machine, often with newer tools.").size(13.0)).wrap());
                    }
                    ui.horizontal_wrapped(|ui| {
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

    fn start_rollback(&mut self) {
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
        rc.extra_words = self.config.report_hide_words.clone();
        rc
    }

    fn open_bug_report(&mut self) {
        self.tab = Tab::Report;
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
                Some(path) => util::atomic_write(&path, text.as_bytes())
                    .map(|_| Some(path.clone()))
                    .map_err(|e| format!("Couldn't save {}: {e}", util::tilde(&path))),
            };
            Msg::BugDownloaded(r)
        });
    }

    fn excluded(&self, section: &str) -> bool {
        self.bug.excluded.iter().any(|x| x == section)
    }

    fn refresh_attempts(&mut self) {
        self.attempts = crate::attempts::list(&self.config.workspace());
    }

    fn build_bug_report(&self) -> String {
        let s = &self.session;
        let input = ReportInput {
            title: "bug report".into(),
            what_doing: self.bug.what.clone(),
            expected: self.bug.expected.clone(),
            env: report::collect_env(Some(self.env.login.is_some()), self.env.secure()),
            session: if s.slug.is_empty()
                || self.bug.excluded.iter().any(|x| x == "Current project")
            {
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
            runs: if self.excluded("Recent runs") {
                vec![]
            } else {
                self.log.runs()
            },
            failing_command: self.log.failing_command(),
            health: if self.excluded("Health") {
                vec![]
            } else {
                self.health.lines()
            },
            selftest_summary: self.last_selftest.as_ref().map(|(r, _)| r.summary_line()),
            problem_lines: if self.excluded("Warnings and errors") {
                vec![]
            } else {
                self.log.report_lines(40, 100).0
            },
            log_lines: if self.excluded("Latest log lines") {
                vec![]
            } else {
                self.log.report_lines(40, 100).1
            },
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
            Msg::Build(id, r) => {
                let Some(w) = self.build.as_mut().filter(|w| w.id == id) else {
                    return;
                };
                match r {
                    Ok(b) => {
                        let passed = matches!(b, crate::github::Build::Passed { .. });
                        let failed = matches!(b, crate::github::Build::Failed { .. });
                        w.status = Some(b);
                        if failed {
                            self.log.warn("build", "GitHub's build failed");
                        }
                        // Pushing StowLock itself: offer the new version now.
                        let ours = format!("{}/{}", w.owner, w.slug)
                            .eq_ignore_ascii_case(&self.config.update_repo);
                        if passed && ours && w.tag.is_some() && self.pacman_pkg.is_none() {
                            self.check_updates(true);
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
            Msg::Projects(p) => {
                self.projects = p;
                self.refresh_attempts();
            }
            Msg::SelfTest(r, path) => {
                self.health = HealthState::load(&self.paths);
                self.toast(&format!("Self-test: {}", r.summary_line()));
                self.last_selftest = Some((*r, path));
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
/// Light theme? "light"/"dark" in Settings, otherwise follow the desktop.
fn wants_light(choice: &str, system: Option<egui::Theme>) -> bool {
    match choice {
        "light" => true,
        "dark" => false,
        _ => system == Some(egui::Theme::Light),
    }
}

/// How many details were hidden in a report (placeholders and removals).
fn count_hidden(text: &str) -> usize {
    let re = regex::Regex::new(r"<(account|repo|org|user|host|email|hidden|workspace)(-\d+)?>|\[removed\]|\[REDACTED[^\]]*\]|left out\]").unwrap();
    re.find_iter(text).count()
}

/// "2026-11-23-111459" -> "Nov 23, 11:14".
fn nice_stamp(stamp: &str) -> String {
    chrono::NaiveDateTime::parse_from_str(stamp, "%Y-%m-%d-%H%M%S")
        .map(|t| t.format("%b %-d, %H:%M").to_string())
        .unwrap_or_else(|_| stamp.to_string())
}

/// Report sections the user can leave out.
const REPORT_SECTIONS: [&str; 5] = [
    "Current project",
    "Recent runs",
    "Health",
    "Warnings and errors",
    "Latest log lines",
];

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
        // Back in front: GitHub may have changed meanwhile (a repo deleted, say).
        let focused = ctx.input(|i| i.viewport().focused.unwrap_or(true));
        if focused && !self.was_focused && self.env.login.is_some() {
            let owner = self.session.owner.clone();
            self.refresh_repos(&owner, false);
            if self.tab == Tab::Projects {
                self.refresh_project_owners(false);
            }
        }
        self.was_focused = focused;
        if self.tab == Tab::Projects && self.prev_tab != Tab::Projects {
            self.refresh_project_owners(false);
        }
        self.prev_tab = self.tab;
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

        // Light or dark, following the desktop unless chosen in Settings.
        let light = wants_light(&self.config.theme, ctx.system_theme());
        if light != ui::is_light() {
            ui::apply_theme(ctx, light);
        }

        self.sidebar(ctx);
        if self.tab == Tab::Stage {
            self.action_bar(ctx);
        }
        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(ui::BG())
                    .inner_margin(egui::Margin {
                        left: 28,
                        right: 28,
                        top: 22,
                        bottom: 8,
                    }),
            )
            .show(ctx, |ui| {
                self.banners(ui);
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        let w = ui.available_width().min(1100.0);
                        ui.set_max_width(w);
                        match self.tab {
                            Tab::Stage => {
                                ui::title(
                                    ui,
                                    "Stage",
                                    "Turn a project zip into a tested GitHub release.",
                                );
                                ui.add_space(8.0);
                                self.stage_tab(ui, hovering)
                            }
                            Tab::Projects => {
                                ui::title(
                                    ui,
                                    "Projects",
                                    &format!(
                                        "Everything in {} and on GitHub.",
                                        util::tilde(&self.config.workspace())
                                    ),
                                );
                                ui.add_space(8.0);
                                self.projects_tab(ui)
                            }
                            Tab::History => self.history_screen(ui),
                            Tab::Health => self.health_screen(ui),
                            Tab::Report => self.report_screen(ui),
                            Tab::Settings => self.settings_screen(ui),
                        }
                        ui.add_space(16.0);
                    });
            });
        self.windows(ctx);
        if let Some((t, at)) = &self.toast {
            let gap = if self.tab == Tab::Stage { 88.0 } else { 24.0 };
            if !ui::toast(ctx, t, at.elapsed().as_secs_f32(), gap) {
                self.toast = None;
            }
        }
        self.save_config();
    }
}

impl App {
    // ------------------------------------------------------------ sidebar

    fn sidebar(&mut self, ctx: &egui::Context) {
        let compact = ctx.screen_rect().width() < 820.0;
        let logo = self
            .logo
            .get_or_insert_with(|| {
                ctx.load_texture(
                    "stowlock-logo",
                    egui::ColorImage::from_rgba_unmultiplied([64, 64], &crate::icon::rgba(64)),
                    egui::TextureOptions::LINEAR,
                )
            })
            .clone();
        egui::SidePanel::left("nav")
            .resizable(false)
            .exact_width(if compact { 68.0 } else { 236.0 })
            .frame(
                egui::Frame::new()
                    .fill(ui::SIDE())
                    .inner_margin(egui::Margin::symmetric(12, 18))
                    .stroke(egui::Stroke::new(1.0_f32, BORDER())),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.add_space(6.0);
                    ui.add(egui::Image::new(&logo).fit_to_exact_size(egui::vec2(30.0, 30.0)));
                    if !compact {
                        ui.vertical(|ui| {
                            ui.spacing_mut().item_spacing.y = 0.0;
                            ui.label(ui::semibold("StowLock", 16.0));
                            ui.label(ui::muted(format!("v{APP_VERSION}")).size(12.0));
                        });
                    }
                });
                ui.add_space(18.0);
                let today = self.history.today().len();
                let (broken, attn) = self.health.counts();
                let saved = self.attempts.len();
                let update = matches!(self.update, UpdateState::Available(_));
                let items: [(Tab, &str, &str, Option<String>); 6] = [
                    (Tab::Stage, ui::icon::UPLOAD, "Stage", None),
                    (
                        Tab::Projects,
                        ui::icon::LAYERS,
                        "Projects",
                        (saved > 0).then(|| format!("{saved} saved")),
                    ),
                    (
                        Tab::History,
                        ui::icon::HISTORY,
                        "History",
                        (today > 0).then(|| format!("{today} today")),
                    ),
                    (
                        Tab::Health,
                        ui::icon::ACTIVITY,
                        "Health",
                        (broken + attn > 0).then(|| format!("{}", broken + attn)),
                    ),
                    (Tab::Report, ui::icon::BUG, "Bug report", None),
                    (
                        Tab::Settings,
                        ui::icon::SLIDERS,
                        "Settings",
                        update.then(|| "Update".to_string()),
                    ),
                ];
                ui.spacing_mut().item_spacing.y = 4.0;
                for (tab, icon, label, badge) in items {
                    if ui::nav_item(ui, icon, label, self.tab == tab, badge.as_deref(), compact)
                        .clicked()
                    {
                        if tab == Tab::Report {
                            self.open_bug_report();
                        } else if tab == Tab::Projects {
                            self.refresh_attempts();
                        }
                        self.tab = tab;
                    }
                }
                ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
                    let signed_in = self.env.login.clone();
                    let dot = if signed_in.is_some() {
                        ui::SUCCESS()
                    } else {
                        WARNING()
                    };
                    if compact {
                        let label = signed_in.clone().unwrap_or_else(|| "Not signed in".into());
                        if ui::nav_item(ui, ui::icon::USER, &label, false, None, true).clicked() {
                            self.tab = Tab::Settings;
                        }
                        return;
                    }
                    let resp = egui::Frame::new()
                        .fill(ui::CARD())
                        .stroke(egui::Stroke::new(1.0_f32, BORDER()))
                        .corner_radius(egui::CornerRadius::same(12))
                        .inner_margin(egui::Margin::same(if compact { 8 } else { 12 }))
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            if compact {
                                ui.vertical_centered(|ui| {
                                    ui.label(RichText::new(ui::icon::USER).size(18.0).color(dot));
                                });
                            } else {
                                ui.horizontal(|ui| {
                                    let (r, _) = ui.allocate_exact_size(
                                        egui::vec2(10.0, 10.0),
                                        egui::Sense::hover(),
                                    );
                                    ui.painter().circle_filled(r.center(), 4.0, dot);
                                    ui.add(
                                        egui::Label::new(ui::semibold(
                                            signed_in
                                                .clone()
                                                .unwrap_or_else(|| "Not signed in".into()),
                                            13.0,
                                        ))
                                        .truncate(),
                                    );
                                });
                                ui.label(
                                    ui::muted(if self.config.beta_updates() {
                                        "Beta updates"
                                    } else {
                                        "Stable updates"
                                    })
                                    .size(12.0),
                                );
                            }
                        })
                        .response
                        .interact(egui::Sense::click())
                        .on_hover_cursor(egui::CursorIcon::PointingHand)
                        .on_hover_text("Account and updates");
                    if resp.clicked() {
                        self.tab = Tab::Settings;
                    }
                });
            });
    }

    // ------------------------------------------------------------ history

    fn history_screen(&mut self, ui: &mut egui::Ui) {
        ui::title(ui, "History", "Every push and archive, newest first.");
        ui.add_space(8.0);
        let mut pushes = self.history.pushes.clone();
        pushes.reverse();
        if pushes.is_empty() {
            ui::card(ui, |ui| {
                ui.vertical_centered(|ui| {
                    ui.add_space(16.0);
                    ui.label(RichText::new(ui::icon::HISTORY).size(30.0).color(MUTED()));
                    ui.label(ui::semibold("Nothing pushed yet", 16.0));
                    ui.label(ui::muted(
                        "Pushes and archives show up here, so you can see what went where.",
                    ));
                    ui.add_space(16.0);
                });
            });
            return;
        }
        let mut last_date = String::new();
        ui::card(ui, |ui| {
            for r in pushes.iter().take(200) {
                if r.date() != last_date {
                    last_date = r.date().to_string();
                    if ui.cursor().top() > ui.min_rect().top() + 30.0 {
                        ui.add_space(6.0);
                    }
                    ui::section_title(ui, &last_date);
                }
                self.today_row(ui, r);
            }
        });
    }

    // ------------------------------------------------------------ settings parts

    fn appearance_and_updates(&mut self, ui: &mut egui::Ui) {
        ui::card(ui, |ui| {
            ui::section_title(ui, "Updates");
            ui.horizontal_wrapped(|ui| {
                ui.vertical(|ui| {
                    ui.set_max_width((ui.available_width() - 250.0).max(220.0));
                    ui.label(ui::semibold("Update channel", 14.5));
                    ui.add(egui::Label::new(ui::muted(if self.config.beta_updates() {
                        "Beta: every newer version, betas included. Never offers an older one."
                    } else {
                        "Stable: full releases only. Betas are skipped."
                    }).size(13.0)).wrap());
                });
                let sel = usize::from(self.config.beta_updates());
                if let Some(i) = ui::segmented(ui, &["Stable", "Beta"], sel, 220.0) {
                    self.config.update_channel = if i == 1 {
                        "beta".into()
                    } else {
                        "stable".into()
                    };
                    if matches!(self.update, UpdateState::Available(_)) {
                        self.update = UpdateState::None;
                    }
                }
            });
            ui.add_space(6.0);
            ui.horizontal_wrapped(|ui| {
                ui.checkbox(
                    &mut self.config.check_for_updates,
                    "Check for updates when StowLock opens",
                );
                if ui::button_enabled(
                    ui,
                    self.pacman_pkg.is_none(),
                    ui::icon::REFRESH,
                    "Check now",
                )
                .clicked()
                {
                    self.check_updates(true);
                }
            });
            match &self.update {
                UpdateState::Available(rel) => {
                    let label = rel.label();
                    let tag = rel.tag.clone();
                    ui::banner(ui, ACCENT(), |ui| {
                        ui.horizontal_wrapped(|ui| {
                            ui.label(format!("StowLock {label} is available."));
                            if self.pacman_pkg.is_none()
                                && ui::primary(ui, &format!("Update to {label}"), true).clicked()
                            {
                                self.apply_update(tag.clone());
                            }
                        });
                    });
                }
                _ => {
                    if let Some(n) = &self.update_note {
                        ui.label(ui::muted(n).size(13.0));
                    }
                }
            }
            if let Some(pkg) = &self.pacman_pkg {
                ui::inset(ui, |ui| {
                    ui.add(egui::Label::new(format!("Installed from the AUR ({pkg}). Updates come through your AUR helper, for example paru or yay.")).wrap());
                });
            } else if let Some(prev) = updater::previous_version(&self.paths) {
                ui.horizontal_wrapped(|ui| {
                    ui.label(ui::muted(format!(
                        "Version {prev} is kept from before the last update."
                    )));
                    if ui::button(ui, ui::icon::UNDO, &format!("Go back to {prev}")).clicked() {
                        self.start_rollback();
                    }
                });
            }
        });
        ui.add_space(8.0);
        ui::card(ui, |ui| {
            ui::section_title(ui, "Appearance");
            let sel = match self.config.theme.as_str() {
                "light" => 1,
                "dark" => 2,
                _ => 0,
            };
            if let Some(i) = ui::segmented(ui, &["Follow system", "Light", "Dark"], sel, 340.0) {
                self.config.theme = ["", "light", "dark"][i].to_string();
            }
        });
    }

    // ------------------------------------------------------------ bug report

    fn report_screen(&mut self, ui: &mut egui::Ui) {
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
        let wide = ui.available_width() > 860.0;
        let left_w = if wide { 340.0 } else { ui.available_width() };
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
                let r = ui.add(
                    egui::TextEdit::singleline(&mut this.bug.hide_input)
                        .hint_text("Type a word and press Enter")
                        .desired_width(f32::INFINITY),
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
                    .corner_radius(egui::CornerRadius::same(10))
                    .inner_margin(egui::Margin::same(12))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        egui::ScrollArea::both()
                            .max_height(440.0)
                            .id_salt("bugprev")
                            .show(ui, |ui| {
                                let mut p = this.bug.preview.clone();
                                let w = ui.available_width();
                                ui.add(
                                    egui::TextEdit::multiline(&mut p)
                                        .code_editor()
                                        .frame(false)
                                        .desired_width(w),
                                );
                            });
                    });
                ui.add_space(4.0);
                ui.horizontal_wrapped(|ui| {
                    if ui::primary(
                        ui,
                        &ui::with_icon(ui::icon::DOWNLOAD, "Download as .txt…"),
                        !this.bug.saving,
                    )
                    .clicked()
                    {
                        download = true;
                    }
                    if ui::button(ui, ui::icon::COPY, "Copy").clicked() {
                        ui.ctx().copy_text(this.bug.preview.clone());
                        this.toast("Copied. Paste it into a GitHub issue or an AI chat.");
                    }
                    if ui::button(ui, ui::icon::SAVE, "Save copy")
                        .on_hover_text("Keep a copy in StowLock's reports folder")
                        .clicked()
                    {
                        match report::save_report(&this.paths, "bugreport", &this.bug.preview) {
                            Ok(p) => this.toast(&format!("Saved {}", util::tilde(&p))),
                            Err(e) => this.fail(&e, "report"),
                        }
                    }
                    if ui::button(ui, ui::icon::EXTERNAL, "Open GitHub issue…").clicked() {
                        this.bug.confirm_issue = true;
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
            right(self, ui);
            ui.add_space(8.0);
            left(self, ui);
        }
        if rebuild {
            self.bug.preview = self.build_bug_report();
        }
        if download {
            self.download_bug_report();
        }
    }

    fn banners(&mut self, ui: &mut egui::Ui) {
        if self.env.checked && !self.env.ready() {
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
        if self.env.secure() == Some(false) {
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
                        if ui.small_button("✖").clicked() {
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
                    if ui.small_button("✖").clicked() {
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
                    if ui.small_button("✖").clicked() {
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

    fn error_card(&mut self, ui: &mut egui::Ui) {
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

    fn stage_tab(&mut self, ui: &mut egui::Ui, hovering: bool) {
        self.error_card(ui);
        // ---- drop zone
        let has_zip = self.session.zip.is_some() || self.session.stage.is_some();
        let (fill, stroke) = if hovering {
            (ACCENT().linear_multiply(0.12), ACCENT())
        } else {
            (ui::CARD(), BORDER())
        };
        let mut pick = false;
        egui::Frame::new()
            .fill(fill)
            .stroke(egui::Stroke::new(
                if hovering { 2.0_f32 } else { 1.0 },
                stroke,
            ))
            .corner_radius(egui::CornerRadius::same(14))
            .inner_margin(egui::Margin::same(if has_zip { 16 } else { 28 }))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                if has_zip {
                    ui.horizontal(|ui| {
                        let (r, _) =
                            ui.allocate_exact_size(egui::vec2(48.0, 48.0), egui::Sense::hover());
                        ui.painter().rect_filled(
                            r,
                            egui::CornerRadius::same(12),
                            ui::ACCENT_SOFT(),
                        );
                        let glyph = if self.session.zip.is_some() {
                            ui::icon::PACKAGE
                        } else {
                            ui::icon::FOLDER
                        };
                        ui.painter().text(
                            r.center(),
                            egui::Align2::CENTER_CENTER,
                            glyph,
                            egui::FontId::proportional(22.0),
                            ACCENT(),
                        );
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            pick |=
                                ui::button_enabled(ui, self.busy.is_none(), "", "Choose another…")
                                    .clicked();
                            ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
                                ui.spacing_mut().item_spacing.y = 2.0;
                                let (name, sub) = if let Some(z) = &self.session.zip {
                                    (
                                        z.file_name()
                                            .map(|n| n.to_string_lossy().to_string())
                                            .unwrap_or_default(),
                                        util::tilde(z),
                                    )
                                } else if let Some(st) = &self.session.stage {
                                    (
                                        util::tilde(&st.dir),
                                        "Loaded from your workspace".to_string(),
                                    )
                                } else {
                                    (String::new(), String::new())
                                };
                                ui.add(egui::Label::new(ui::semibold(&name, 15.0)).truncate())
                                    .on_hover_text(&name);
                                let meta = if let Some(st) = &self.session.stage {
                                    let n = crate::archive::count_archives(
                                        &self.config.archive_root(),
                                        &self.session.slug,
                                    );
                                    let mut m = format!(
                                        "{} · {} files · {}",
                                        st.stack().name(),
                                        st.files,
                                        util::human_size(st.size)
                                    );
                                    if n > 0 {
                                        m.push_str(&format!(
                                            " · {n} archived version{}",
                                            if n == 1 { "" } else { "s" }
                                        ));
                                    }
                                    m
                                } else {
                                    format!("{sub} · drop another zip here to replace it")
                                };
                                ui.add(egui::Label::new(ui::muted(meta).size(13.0)).truncate());
                            });
                        });
                    });
                } else {
                    ui.vertical_centered(|ui| {
                        let (r, _) =
                            ui.allocate_exact_size(egui::vec2(56.0, 56.0), egui::Sense::hover());
                        ui.painter().rect_filled(
                            r,
                            egui::CornerRadius::same(14),
                            ui::ACCENT_SOFT(),
                        );
                        ui.painter().text(
                            r.center(),
                            egui::Align2::CENTER_CENTER,
                            ui::icon::UPLOAD,
                            egui::FontId::proportional(26.0),
                            ACCENT(),
                        );
                        ui.add_space(6.0);
                        ui.label(ui::semibold(
                            if hovering {
                                "Drop the zip to load it"
                            } else {
                                "Choose a project .zip to get started"
                            },
                            17.0,
                        ));
                        ui.label(ui::muted("or drag it onto this window · Ctrl+O").size(13.0));
                        ui.add_space(6.0);
                        pick |= ui::primary(ui, "Choose a .zip…", self.busy.is_none()).clicked();
                    });
                }
            });
        if pick {
            self.pick_zip();
        }
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
        if let Some(out) = self.session.outcome.clone() {
            ui.add_space(8.0);
            self.after_push_cards(ui);
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

    fn details_card(&mut self, ui: &mut egui::Ui) {
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
                            ui.add(
                                egui::TextEdit::singleline(&mut self.session.tag)
                                    .hint_text("optional, e.g. v0.1.0")
                                    .desired_width(170.0)
                                    .font(egui::TextStyle::Monospace),
                            );
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
                        ui.horizontal_wrapped(|ui| {
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

    fn results_card(&mut self, ui: &mut egui::Ui) {
        let steps = self.log.steps();
        let Some(st) = self.session.stage.clone() else {
            ui::card(ui, |ui| {
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
            ui::section_title(ui, "PIPELINE");
            ui::stepper(ui, &STEPS, &steps, &WAITING);
            for n in st.notes.iter().chain(st.cleanup.lines().iter()) {
                ui.label(RichText::new(format!("✔ {n}")).color(SUCCESS()).size(13.0));
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
                        TestStatus::Passed => ("Tests passed", SUCCESS()),
                        TestStatus::Failed => ("Tests failed", ERROR()),
                        TestStatus::Skipped if c.tests.summary.starts_with("Can't run here") => {
                            ("Can't run here", WARNING())
                        }
                        TestStatus::Skipped => ("Tests skipped", MUTED()),
                        TestStatus::Cancelled => ("Tests cancelled", WARNING()),
                    };
                    let cant_run = txt == "Can't run here";
                    ui.horizontal_wrapped(|ui| {
                        ui::badge(ui, txt, col);
                        ui.label(ui::muted(format!(
                            "{} · {:.1}s",
                            c.tests.summary,
                            c.tests.duration.as_secs_f32()
                        )));
                    });
                    if cant_run {
                        ui.add(
                            egui::Label::new(
                                "Something this step needs isn't on this computer, so it couldn't run here. That doesn't mean your project is broken. Install what's missing and Re-check, or push: GitHub runs this step with everything installed, and StowLock watches the build.",
                            )
                            .wrap(),
                        );
                        if c.tests.summary.contains("`") {
                            ui.add(
                                egui::Label::new(ui::muted(
                                    "If it's a Python tool, listing it in the project's requirements file lets StowLock install it here too, in its own environment.",
                                ).size(12.5))
                                .wrap(),
                            );
                        }
                    }
                    if c.tests.status == TestStatus::Passed
                        && c.tests.summary.contains("can't run here")
                    {
                        ui.add(
                            egui::Label::new(ui::muted(
                                "Everything that could run here passed. GitHub runs the rest, and StowLock watches the build. Nothing to install unless you want to run those steps here too.",
                            ).size(12.5))
                            .wrap(),
                        );
                    }
                    if let Some(h) = &c.tests.hint {
                        ui.horizontal_wrapped(|ui| {
                            if c.tests.status == TestStatus::Passed || cant_run {
                                ui.label(ui::muted("Optional, to run it here too:").size(12.5));
                            }
                            ui.add(egui::Label::new(ui::mono(h)).wrap());
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
                        // Safer choices first; pushing anyway comes last.
                        let mut pick = false;
                        let mut recheck = false;
                        let mut report = false;
                        let mut open = false;
                        let mut copied = false;
                        ui::banner(ui, WARNING(), |ui| {
                            ui.label(ui::semibold("What to do next", 14.5));
                            if c.tests.summary.contains(crate::preflight::MACHINE_SPECIFIC) {
                                ui.add(egui::Label::new(RichText::new(
                                    "These failures look specific to this computer, not the project: they mention a running program or your home folder. They'll likely pass on GitHub. Quit the running copy and Re-check, or push anyway.",
                                ).color(WARNING())).wrap());
                            }
                            ui.add(egui::Label::new(
                                "1. Copy for AI, paste it into your AI chat, and load the fixed zip it gives you. The failed attempt is kept, never lost.",
                            ).wrap());
                            ui.add(
                                egui::Label::new(
                                    "2. Or fix it in the project folder yourself, then Re-check.",
                                )
                                .wrap(),
                            );
                            ui.add(
                                egui::Label::new(
                                    "3. If it looks like StowLock's fault, send a bug report.",
                                )
                                .wrap(),
                            );
                            ui.horizontal_wrapped(|ui| {
                                if ui::primary(ui, &ui::with_icon(ui::icon::COPY, "Copy for AI"), true)
                                    .on_hover_text("Copies the failed tests and their messages, with a request to fix the project")
                                    .clicked()
                                {
                                    let lines: Vec<String> = c.tests.tail.lines().map(|l| l.to_string()).collect();
                                    let prompt = report::project_fix_prompt(&c.tests.summary, &lines);
                                    let prompt = report::redact(&prompt, &self.redact_ctx());
                                    ui.ctx().copy_text(prompt);
                                    copied = true;
                                }
                                pick |=
                                    ui::button(ui, ui::icon::UPLOAD, "Load a fixed zip…").clicked();
                                open |=
                                    ui::button(ui, ui::icon::FOLDER_OPEN, "Open folder").clicked();
                                recheck |= ui::button(ui, ui::icon::REFRESH, "Re-check").clicked();
                                report |= ui::button(ui, ui::icon::BUG, "Bug report").clicked();
                            });
                            ui.add_space(2.0);
                            ui.checkbox(
                                &mut self.session.override_tests,
                                "Push anyway: I know why the tests fail and it's safe",
                            );
                        });
                        if copied {
                            self.toast(
                                "Copied. Paste it into your AI chat, then load the fixed zip.",
                            );
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
                    ui.add_space(4.0);
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
                    } else {
                        ui::badge(
                            ui,
                            &format!(
                                "{} possible secret{}",
                                c.secrets.len(),
                                if c.secrets.len() == 1 { "" } else { "s" }
                            ),
                            ERROR(),
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
                        ui.label(RichText::new("If this key was ever pushed before, removing it isn't enough. Revoke it and make a new one.").color(WARNING()).size(12.5));
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

    fn completion_card(&mut self, ui: &mut egui::Ui, out: &PushOutcome) {
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
                                Level::Info => MUTED(),
                                Level::Warn => WARNING(),
                                Level::Error => ERROR(),
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
                    .fill(ui::CARD())
                    .inner_margin(egui::Margin::symmetric(16, 12))
                    .stroke(egui::Stroke::new(1.0_f32, BORDER())),
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
                if s.outcome.is_none() {
                    if let Err(e) =
                        version::validate(&s.tag, s.channel, &self.config.tag_prefix, &tags)
                    {
                        blockers.push(e);
                    }
                }
                if s.outcome
                    .as_ref()
                    .map(|o| !o.nothing_to_push)
                    .unwrap_or(false)
                    && s.mode == Mode::New
                {
                    blockers.push("Already pushed".into());
                }
                let pushed = s.outcome.as_ref().map(|o| !o.nothing_to_push).unwrap_or(false);
                // When the checks you started pass, go straight to the review.
                if s.review_when_ready && busy.is_none() && s.checks.is_some() {
                    self.session.review_when_ready = false;
                    if blockers.is_empty() && s.outcome.is_none() {
                        self.session.confirm = true;
                        self.session.confirm_at = Some(std::time::Instant::now());
                        self.session.public_ack = false;
                        self.session.older_ack = false;
                    }
                }
                ui.horizontal(|ui| {
                    Self::action_text(ui, BTN + 140.0, |ui| {
                        if let Some(b) = &busy {
                            ui.horizontal(|ui| {
                                ui.spinner();
                                ui.label(b);
                                if ui.small_button("Cancel").clicked() {
                                    self.cancel.store(true, Ordering::Relaxed);
                                }
                            });
                        } else if pushed {
                            ui.horizontal(|ui| {
                                ui.label(RichText::new(ui::icon::CIRCLE_CHECK).size(16.0).color(ui::SUCCESS()));
                                ui.add(egui::Label::new(ui::semibold(
                                    format!("Pushed to github.com/{}/{}{}", s.owner, s.slug,
                                        s.outcome.as_ref().and_then(|o| o.tag.clone()).map(|t| format!(" · {t}")).unwrap_or_default()),
                                    14.0,
                                )).truncate());
                            });
                        } else if s.checks.is_none() {
                            ui.label(ui::muted("Run checks first: Stage & check tests the project before anything is pushed."));
                        } else if let Some(b) = blockers.first() {
                            ui.horizontal(|ui| {
                                ui.label(RichText::new(ui::icon::CIRCLE_X).size(16.0).color(ERROR()));
                                ui.add(egui::Label::new(RichText::new(format!("Blocked: {b}")).color(ERROR())).wrap());
                            });
                            for more in blockers.iter().skip(1).take(2) {
                                ui.add(egui::Label::new(ui::muted(format!("Also: {more}")).size(12.5)).wrap());
                            }
                        } else {
                            ui.horizontal(|ui| {
                                ui.label(RichText::new(ui::icon::CIRCLE_CHECK).size(16.0).color(ui::SUCCESS()));
                                ui.label(ui::semibold("Ready to push", 14.0).color(ui::SUCCESS()));
                                ui.label(ui::muted(format!("· github.com/{}/{}", s.owner, s.slug)).size(12.5));
                            });
                        }
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if pushed {
                            if ui::primary(ui, "Stage another zip", busy.is_none()).clicked() {
                                self.pick_zip();
                            }
                            if ui::button(ui, ui::icon::EXTERNAL, "View on GitHub").clicked() {
                                let url = format!("https://github.com/{}/{}", s.owner, s.slug);
                                let _ = proc::xdg_open(&url);
                            }
                            return;
                        }
                        let ok = busy.is_none() && blockers.is_empty();
                        if ui::primary(ui, &ui::with_icon(ui::icon::UPLOAD, "Push to GitHub…"), ok).clicked() {
                            self.session.confirm = true;
                            self.session.confirm_at = Some(std::time::Instant::now());
                            self.session.public_ack = false;
                            self.session.older_ack = false;
                        }
                    });
                });
            });
        if self.session.confirm {
            self.push_review(ctx);
        }
    }

    /// The "Ready to push" window: everything about the push in one place,
    /// and a confirm button that says exactly what it will do.
    fn push_review(&mut self, ctx: &egui::Context) {
        if self.session.workflow_publishes.is_none() {
            if let Some(st) = &self.session.stage {
                self.session.workflow_publishes =
                    Some(crate::workflows::publishes_releases(&st.dir));
            }
        }
        let s = self.session.clone();
        let Some(stage) = s.stage.clone() else {
            self.session.confirm = false;
            return;
        };
        let public_new = s.mode == Mode::New && !s.private;
        let tag = s.tag.trim().to_string();
        let armed = s
            .confirm_at
            .map(|t| t.elapsed().as_secs_f32() >= 0.5)
            .unwrap_or(true);
        if !armed {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
        let mut close = false;
        let mut go = false;
        let mut show_files = false;
        let modal = egui::Modal::new(egui::Id::new("push_review")).show(ctx, |ui| {
            ui.set_width(520.0_f32.min(ctx.screen_rect().width() - 80.0));
            ui.horizontal(|ui| {
                let (r, _) = ui.allocate_exact_size(egui::vec2(34.0, 34.0), egui::Sense::hover());
                ui.painter()
                    .rect_filled(r, egui::CornerRadius::same(10), ui::ACCENT_SOFT());
                ui.painter().text(
                    r.center(),
                    egui::Align2::CENTER_CENTER,
                    ui::icon::UPLOAD,
                    egui::FontId::proportional(18.0),
                    ACCENT(),
                );
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 0.0;
                    ui.label(ui::semibold("Ready to push", 18.0));
                    ui.label(ui::muted("Check the details, then push.").size(13.0));
                });
            });
            ui.add_space(8.0);
            egui::Grid::new("review")
                .num_columns(2)
                .spacing([14.0, 10.0])
                .max_col_width(400.0)
                .show(ui, |ui| {
                    ui.label(ui::muted("Where"));
                    ui.add(
                        egui::Label::new(ui::mono(format!("github.com/{}/{}", s.owner, s.slug)))
                            .truncate(),
                    );
                    ui.end_row();
                    ui.label(ui::muted("Type"));
                    ui.horizontal_wrapped(|ui| {
                        ui::badge(
                            ui,
                            if s.mode == Mode::New {
                                "New repo"
                            } else {
                                "Update"
                            },
                            ui::SUCCESS(),
                        );
                        if s.mode == Mode::New {
                            if s.private {
                                ui::badge(ui, "Private", MUTED());
                            } else {
                                ui::badge(ui, "Public", WARNING());
                            }
                        }
                    });
                    ui.end_row();
                    ui.label(ui::muted("Version"));
                    ui.label(if tag.is_empty() {
                        "No release (code only)".to_string()
                    } else if s.channel == Channel::Beta {
                        format!("{tag} · Beta (pre-release)")
                    } else {
                        format!("{tag} · Stable")
                    });
                    ui.end_row();
                    ui.label(ui::muted("Changes"));
                    ui.horizontal_wrapped(|ui| match &stage.changes {
                        Some(c) if c.total() > 0 => {
                            ui.label(format!(
                                "{} file{}:",
                                c.total(),
                                if c.total() == 1 { "" } else { "s" }
                            ));
                            if !c.added.is_empty() {
                                ui.label(
                                    RichText::new(format!("{} added", c.added.len()))
                                        .color(ui::SUCCESS()),
                                );
                            }
                            if !c.changed.is_empty() {
                                ui.label(
                                    RichText::new(format!("{} changed", c.changed.len()))
                                        .color(WARNING()),
                                );
                            }
                            if !c.removed.is_empty() {
                                ui.label(
                                    RichText::new(format!("{} removed", c.removed.len()))
                                        .color(ERROR()),
                                );
                            }
                            show_files = ui
                                .link(if s.show_changes {
                                    "hide list"
                                } else {
                                    "show list"
                                })
                                .clicked();
                        }
                        Some(_) => {
                            ui.label("No file changes (a new version tag only)");
                        }
                        None => {
                            ui.label(format!("{} files (first push)", stage.files));
                        }
                    });
                    ui.end_row();
                    ui.label(ui::muted("Checks"));
                    let tests = s
                        .checks
                        .as_ref()
                        .map(|c| c.tests.summary.clone())
                        .unwrap_or_default();
                    ui.horizontal_wrapped(|ui| {
                        if s.override_tests {
                            ui.label(
                                RichText::new(format!(
                                    "{} Tests failed, pushing anyway",
                                    ui::icon::TRIANGLE_ALERT
                                ))
                                .color(WARNING()),
                            );
                        } else {
                            ui.label(
                                RichText::new(format!(
                                    "{} {}",
                                    ui::icon::CIRCLE_CHECK,
                                    if tests.is_empty() {
                                        "Passed".to_string()
                                    } else {
                                        tests
                                    }
                                ))
                                .color(ui::SUCCESS()),
                            );
                        }
                    });
                    ui.end_row();
                    ui.label(ui::muted("Who gets this"));
                let older = version::newer_existing(&tag, &self.config.tag_prefix, &self.existing_tags());
                ui.add(egui::Label::new(match (&older, tag.is_empty(), s.channel) {
                    (_, true, _) => "Nobody is offered an update (no version tag)".to_string(),
                    (Some(n), _, _) => format!("Nobody: this counts as older than {n}"),
                    (None, _, Channel::Beta) => "People on the Beta update channel".to_string(),
                    (None, _, Channel::Stable) => "Everyone using an older version".to_string(),
                }).wrap());
                ui.end_row();
                ui.label(ui::muted("After"));
                    let mut after = Vec::new();
                    if !tag.is_empty() {
                        if s.workflow_publishes == Some(true) {
                            after.push("the project's release workflow builds and publishes the release on GitHub");
                        } else {
                            after.push("release created");
                        }
                    }
                    if s.archive_after {
                        after.push(if s.archive_move {
                            "zip moved to the archive"
                        } else {
                            "zip copied to the archive"
                        });
                    }
                    ui.label(if after.is_empty() {
                        "Nothing else".to_string()
                    } else {
                        util::capitalize(&after.join(" · "))
                    });
                    ui.end_row();
                });
            if s.show_changes {
                if let Some(c) = &stage.changes {
                    egui::ScrollArea::vertical()
                        .max_height(160.0)
                        .id_salt("review_files")
                        .show(ui, |ui| {
                            for (list, sign, col) in [
                                (&c.added, "+", ui::SUCCESS()),
                                (&c.changed, "~", WARNING()),
                                (&c.removed, "−", ERROR()),
                            ] {
                                for f in list.iter() {
                                    ui.label(ui::mono(format!("{sign} {f}")).color(col));
                                }
                            }
                        });
                }
            }
            let older_tag = version::newer_existing(&tag, &self.config.tag_prefix, &self.existing_tags());
            if let Some(n) = &older_tag {
                ui.add_space(4.0);
                ui::banner(ui, WARNING(), |ui| {
                    ui.add(egui::Label::new(format!("{tag} counts as older than {n}, so nobody on {n} will be offered it. That's only right for tagging an old fix.")).wrap());
                    ui.checkbox(&mut self.session.older_ack, "Push it anyway");
                });
            }
            if public_new {
                ui.add_space(4.0);
                ui::banner(ui, WARNING(), |ui| {
                    ui.add(
                        egui::Label::new(
                            "This creates a public repository: anyone can see this code.",
                        )
                        .wrap(),
                    );
                    ui.checkbox(
                        &mut self.session.public_ack,
                        "I understand this will be public",
                    );
                });
            }
            ui.add_space(6.0);
            ui.separator();
            ui.horizontal(|ui| {
                if ui::button(ui, "", "Back").clicked() {
                    close = true;
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let label = if tag.is_empty() {
                        format!("Push to {}", s.slug)
                    } else {
                        format!("Push {tag} to {}", s.slug)
                    };
                    let ready = armed
                        && (!public_new || self.session.public_ack)
                        && (older_tag.is_none() || self.session.older_ack);
                    let clicked =
                        ui::primary(ui, &ui::with_icon(ui::icon::UPLOAD, &label), ready).clicked();
                    let keyed =
                        ready && ui.input(|i| i.modifiers.ctrl && i.key_pressed(egui::Key::Enter));
                    if clicked || keyed {
                        go = true;
                    }
                });
            });
        });
        if show_files {
            self.session.show_changes = !self.session.show_changes;
        }
        if modal.should_close() || close {
            self.session.confirm = false;
        }
        if go {
            self.session.confirm = false;
            self.push();
        }
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
                    self.refresh_project_owners(true);
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
    fn attempts_card(&mut self, ui: &mut egui::Ui) {
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

    fn windows(&mut self, ctx: &egui::Context) {
        if self.show_uninstall {
            self.uninstall_window(ctx);
        }
    }

    fn today_row(&mut self, ui: &mut egui::Ui, r: &PushRecord) {
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
                        ui.add(
                            egui::TextEdit::singleline(&mut typed)
                                .desired_width(160.0)
                                .hint_text(tag.as_str()),
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

    fn health_screen(&mut self, ui: &mut egui::Ui) {
        let mut run = false;
        {
            ui::title(ui, "Health", &self.health.summary());
            ui.add_space(8.0);
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
                                ui.add(egui::Label::new(ui::muted(note).size(12.5)).truncate())
                                    .on_hover_text(format!("{} · {}", s.source, s.checked_at));
                            })
                        });
                        if (i + 1) % cols == 0 {
                            ui.end_row();
                        }
                    }
                });
            ui.add_space(8.0);
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
                ui.checkbox(&mut self.selftest_online, "Include read-only GitHub checks");
                if self.busy.as_deref() == Some("Running self-test") {
                    ui.spinner();
                }
            });
            ui.label(ui::muted("The self-test runs in a sandbox. It never touches your projects and never writes to GitHub.").size(12.0));
            if let Some((r, path)) = &self.last_selftest {
                ui.label(RichText::new(r.summary_line()).strong());
                for c in r
                    .checks
                    .iter()
                    .filter(|c| c.outcome != selftest::Outcome::Pass)
                {
                    let col = if c.outcome == selftest::Outcome::Fail {
                        ERROR()
                    } else {
                        MUTED()
                    };
                    ui.label(
                        RichText::new(format!("{} {} — {}", c.id, c.name, c.detail))
                            .color(col)
                            .size(12.5),
                    );
                }
                ui.horizontal_wrapped(|ui| {
                    if ui.button("Copy summary").clicked() {
                        let text = report::redact(&r.to_markdown(), &self.redact_ctx());
                        ui.ctx().copy_text(text);
                    }
                    if r.counts().1 > 0 && ui.button("Copy AI fix prompt").clicked() {
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
        }
        if run {
            self.run_selftest();
        }
    }

    fn settings_screen(&mut self, ui: &mut egui::Ui) {
        {
            ui::title(ui, "Settings", "");
            ui.add_space(8.0);
            self.appearance_and_updates(ui);
            ui.add_space(8.0);
            ui::card(ui, |ui| self.account_panel(ui));
            ui.add_space(8.0);
            ui::card(ui, |ui| {
                ui::section_title(ui, "GENERAL");
                egui::Grid::new("general")
                    .max_col_width((ui.available_width() - 130.0).max(160.0))
                    .num_columns(2)
                    .spacing([12.0, 8.0])
                    .show(ui, |ui| {
                        ui.label("Workspace folder");
                        ui.add(
                            egui::TextEdit::singleline(&mut self.config.workspace_dir)
                                .font(egui::TextStyle::Monospace)
                                .desired_width(300.0),
                        );
                        ui.end_row();
                        ui.label("New repos");
                        ui.checkbox(&mut self.config.default_private, "Private by default");
                        ui.end_row();
                        ui.label("Default license");
                        egui::ComboBox::from_id_salt("deflic")
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
                        ui.end_row();
                        ui.label("License holder");
                        ui.add(
                            egui::TextEdit::singleline(&mut self.config.license_holder)
                                .hint_text("Your name")
                                .desired_width(300.0),
                        );
                        ui.end_row();
                        ui.label("After push");
                        ui.horizontal_wrapped(|ui| {
                            ui.checkbox(&mut self.config.archive_after_push, "Archive the zip");
                            ui.radio_value(
                                &mut self.config.archive_mode,
                                "move".to_string(),
                                "Move",
                            );
                            ui.radio_value(
                                &mut self.config.archive_mode,
                                "copy".to_string(),
                                "Copy",
                            );
                        });
                        ui.end_row();
                        ui.label("Tag prefix");
                        ui.add(
                            egui::TextEdit::singleline(&mut self.config.tag_prefix)
                                .desired_width(60.0),
                        );
                        ui.end_row();
                    });
                ui.separator();
                ui::section_title(ui, "CHECKS");
                ui.checkbox(
                    &mut self.config.run_tests_before_push,
                    "Run tests before pushing",
                );
                ui.checkbox(
                    &mut self.config.match_github_checks,
                    "Match GitHub's checks: run the project's own GitHub workflow steps",
                )
                .on_hover_text("Uses the same commands and settings as the project's .github/workflows files, and lists the steps only GitHub can run.");
                ui.checkbox(
                    &mut self.config.clean_test_home,
                    "Like GitHub's clean machine: a fresh home folder, without your desktop session",
                )
                .on_hover_text("Tests don't see your settings, app data or running desktop, as on GitHub. Tool caches are kept, so it stays fast.");
                let act_ready = proc::which("act").is_some() && proc::which("docker").is_some();
                ui.add_enabled_ui(act_ready, |ui| {
                    ui.checkbox(
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
                ui.checkbox(
                    &mut self.config.block_on_secrets,
                    "Block pushes when possible secrets are found",
                );
                ui.separator();
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
                ui.separator();
                ui::section_title(ui, "UPDATES");
                ui.horizontal_wrapped(|ui| {
                    ui.checkbox(
                        &mut self.config.check_for_updates,
                        "Check for updates automatically",
                    );
                    if ui.button("Check now").clicked() {
                        self.check_updates(true);
                    }
                });
                egui::CollapsingHeader::new(ui::muted("Advanced"))
                    .id_salt("adv")
                    .show(ui, |ui| {
                        ui.horizontal_wrapped(|ui| {
                            ui.label("Update source");
                            ui.add(
                                egui::TextEdit::singleline(&mut self.config.update_repo)
                                    .font(egui::TextStyle::Monospace)
                                    .desired_width(240.0),
                            );
                        });
                    });
                ui.separator();
                ui::section_title(ui, "ABOUT");
                ui.label(format!(
                    "StowLock {APP_VERSION} · Free software under the GNU GPL v3 · github.com/{}",
                    crate::config::UPDATE_REPO
                ));
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
                        RichText::new(if a.active { "⏺" } else { "○" }).color(if a.active {
                            SUCCESS()
                        } else {
                            MUTED()
                        }),
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

    fn confirm_issue_banner(&mut self, ui: &mut egui::Ui) {
        let who = self
            .env
            .login
            .clone()
            .unwrap_or_else(|| "your account".into());
        ui::banner(ui, WARNING(), |ui| {
            ui.add(
                egui::Label::new(format!(
                    "This will be public on GitHub under the account {who}, on {}.",
                    self.config.update_repo
                ))
                .wrap(),
            );
            ui.horizontal_wrapped(|ui| {
                if ui.button("Open issue").clicked() {
                    match report::save_report(&self.paths, "bugreport", &self.bug.preview) {
                        Ok(p) => {
                            let o = proc::run(
                                "gh",
                                &[
                                    "issue",
                                    "create",
                                    "--repo",
                                    &self.config.update_repo,
                                    "--title",
                                    "Bug report",
                                    "--body-file",
                                    &p.display().to_string(),
                                ],
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
            std::process::exit(0);
        }
        if !open {
            self.show_uninstall = false;
        }
    }
}

/// Click-through tests: the real app, driven by clicking its buttons (through
/// the accessibility tree, as a screen reader would). GitHub is the local
/// stand-in, so nothing ever reaches the real GitHub.
#[cfg(test)]
mod ui_tests {
    use super::*;
    use egui_kittest::kittest::Queryable;
    use egui_kittest::Harness;

    fn wait_until(h: &mut Harness<'_, App>, what: &str, f: impl Fn(&App) -> bool) {
        for _ in 0..1500 {
            h.step();
            if f(h.state()) {
                h.step();
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        panic!("timed out waiting for {what}");
    }

    fn steps(h: &mut Harness<'_, App>, n: usize) {
        for _ in 0..n {
            h.step();
        }
    }

    /// Click the button whose name contains `text`.
    fn click_button(h: &mut Harness<'_, App>, text: &str) {
        let n = h
            .get_all_by_label_contains(text)
            .find(|n| {
                n.role() == egui::accesskit::Role::Button
                    || n.role() == egui::accesskit::Role::CheckBox
            })
            .unwrap_or_else(|| panic!("no button named like \"{text}\""));
        n.click();
        steps(h, 2);
    }

    fn click_nav(h: &mut Harness<'_, App>, label: &str) {
        h.get_by_role_and_label(egui::accesskit::Role::Button, label)
            .click();
        steps(h, 3);
    }

    #[test]
    fn click_through_the_app() {
        let d = tempfile::tempdir().unwrap();
        let paths = AppPaths::under(&d.path().join("app"));
        paths.ensure().unwrap();
        let mut cfg = Config::load(&paths);
        cfg.workspace_dir = d.path().join("ws").display().to_string();
        cfg.check_for_updates = false;
        cfg.theme = "dark".into();
        cfg.fallback_git_name = "StowLock Test".into();
        cfg.fallback_git_email = "test@example.invalid".into();
        cfg.save(&paths).unwrap();
        let zip = crate::fixtures::Sandbox::new(&d.path().join("sb"))
            .unwrap()
            .zip(
                "demo-app-v0.2.0.zip",
                &[("main.py", b"print('hi')\n", 0o644)],
            );

        let p2 = paths.clone();
        let mut h = Harness::builder()
            .with_size(egui::vec2(1280.0, 860.0))
            .build_eframe(move |cc| App::new(cc, p2, Logger::new(None), None, None));
        // Let the start-up checks finish, then sign in to the stand-in GitHub.
        wait_until(&mut h, "start-up checks", |a| a.env.checked);
        {
            let a = h.state_mut();
            a.env.git = true;
            a.env.gh = true;
            a.env.login = Some("selftest-user".into());
            a.gh = Arc::new(crate::github::FakeGh::new(&d.path().join("gh")));
            a.build_poll = (0, 1);
        }
        steps(&mut h, 2);

        // Every screen opens from the sidebar.
        for (nav, marker) in [
            ("Projects", "projects in"),
            ("History", "Nothing pushed yet"),
            ("Health", "Run self-test"),
            ("Bug report", "Also hide these words"),
            ("Settings", "Update channel"),
        ] {
            click_nav(&mut h, nav);
            assert!(
                h.query_by_label_contains(marker).is_some(),
                "{nav} screen didn't show \"{marker}\""
            );
        }

        // Settings: the update channel and the theme switch.
        h.get_by_role_and_label(egui::accesskit::Role::Button, "Stable")
            .click();
        steps(&mut h, 2);
        assert_eq!(h.state().config.update_channel, "stable");
        h.get_by_role_and_label(egui::accesskit::Role::Button, "Light")
            .click();
        steps(&mut h, 3);
        assert!(ui::is_light(), "Light theme didn't apply");
        h.get_by_role_and_label(egui::accesskit::Role::Button, "Dark")
            .click();
        steps(&mut h, 3);
        assert!(!ui::is_light());

        // Stage & check a zip.
        click_nav(&mut h, "Stage");
        h.state_mut().load_zip(zip);
        steps(&mut h, 3);
        // The version comes from the zip: no typing.
        assert_eq!(h.state().session.tag, "v0.2.0");
        assert_eq!(h.state().session.channel, Channel::Stable);
        h.state_mut().session.private = false; // a new public repo
        click_button(&mut h, "Stage & check");
        wait_until(&mut h, "checks", |a| {
            a.session.checks.is_some() && a.busy.is_none()
        });
        steps(&mut h, 2);

        // The review window opens by itself; an instant click can't push.
        assert!(
            h.state().session.confirm,
            "review window didn't open by itself"
        );
        assert!(
            h.query_by_label_contains("Everyone using an older version")
                .is_some(),
            "Who gets this"
        );
        assert!(h
            .query_by_label_contains("I understand this will be public")
            .is_some());
        click_button(&mut h, "to demo-app");
        steps(&mut h, 2);
        assert!(
            h.state().session.outcome.is_none(),
            "pushed on a double-click"
        );
        // Armed after half a second, but a public repo still needs the tick.
        std::thread::sleep(std::time::Duration::from_millis(600));
        steps(&mut h, 2);
        click_button(&mut h, "to demo-app");
        steps(&mut h, 2);
        assert!(
            h.state().session.outcome.is_none(),
            "pushed a public repo without the tick"
        );
        click_button(&mut h, "I understand this will be public");
        steps(&mut h, 2);
        // GitHub's build will fail, to check the build card.
        std::fs::create_dir_all(d.path().join("gh")).unwrap();
        std::fs::write(
            d.path().join("gh/build.json"),
            r#"{"state":"failed","step":"arch (cargo test --locked)","log":"test x ... FAILED"}"#,
        )
        .unwrap();
        click_button(&mut h, "to demo-app");
        wait_until(&mut h, "push", |a| {
            a.session.outcome.is_some() && a.busy.is_none()
        });

        // Finished state: no "already exists" warning, the next steps instead.
        assert!(h.query_by_label_contains("View on GitHub").is_some());
        assert!(h.query_by_label_contains("Stage another zip").is_some());
        assert!(h.query_by_label_contains("already exists").is_none());
        assert!(
            d.path().join("gh").exists(),
            "nothing reached the stand-in GitHub"
        );

        // GitHub's build fails: say where, and offer to delete the release.
        wait_until(&mut h, "GitHub build", |a| {
            matches!(
                a.build.as_ref().and_then(|b| b.status.clone()),
                Some(crate::github::Build::Failed { .. })
            )
        });
        steps(&mut h, 2);
        assert!(h
            .query_by_label_contains("GitHub's build failed at arch (cargo test --locked)")
            .is_some());
        assert!(h
            .query_by_label_contains("It passed here but failed on GitHub")
            .is_some());
        click_button(&mut h, "Delete the release and tag");
        click_button(&mut h, "Delete v0.2.0 from GitHub");
        wait_until(&mut h, "release removed", |a| {
            a.busy.is_none() && a.session.outcome.as_ref().is_some_and(|o| o.tag.is_none())
        });
        let fake = crate::github::FakeGh::new(&d.path().join("gh"));
        assert!(
            fake.releases("selftest-user", "demo-app").is_empty(),
            "release still on GitHub"
        );

        // The push shows in History.
        click_nav(&mut h, "History");
        assert!(
            h.query_by_label_contains("demo-app").is_some(),
            "History is empty"
        );

        // The repo is deleted on GitHub: Projects flags the folder.
        std::fs::remove_dir_all(fake.bare("selftest-user", "demo-app")).unwrap();
        click_nav(&mut h, "Projects");
        click_button(&mut h, "Refresh");
        wait_until(&mut h, "repo list reloaded", |a| {
            a.repos_loading.is_empty()
                && a.github_repos("selftest-user")
                    .is_some_and(|n| n.is_empty())
        });
        steps(&mut h, 2);
        assert!(
            h.query_by_label_contains("Not on GitHub anymore").is_some(),
            "deleted repo not flagged"
        );
        click_button(&mut h, "Remove the folder");
        assert!(h
            .query_by_label_contains("Move demo-app to the Trash")
            .is_some());
        click_button(&mut h, "Cancel");

        // Push it again: not an update any more, and the repo comes back.
        click_button(&mut h, "Push it again as a new repo");
        wait_until(&mut h, "re-check", |a| {
            a.session.checks.is_some() && a.busy.is_none()
        });
        assert_eq!(h.state().session.mode, Mode::New);
        assert!(h.state().session.deleted_on_github);
        assert!(h.state().session.confirm, "review window didn't open");
        std::thread::sleep(std::time::Duration::from_millis(600));
        steps(&mut h, 2);
        if h.query_by_label_contains("I understand this will be public")
            .is_some()
        {
            click_button(&mut h, "I understand this will be public");
        }
        click_button(&mut h, "to demo-app");
        wait_until(&mut h, "push again", |a| {
            a.session.outcome.is_some() && a.busy.is_none()
        });
        assert!(
            fake.bare("selftest-user", "demo-app").exists(),
            "repo not created again"
        );
        wait_until(&mut h, "repo list after push", |a| {
            a.github_repos("selftest-user")
                .is_some_and(|n| n.iter().any(|x| x == "demo-app"))
        });
    }
}
