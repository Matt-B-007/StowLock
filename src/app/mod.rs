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

mod action_bar;
mod after_push;
mod bug_report;
mod handle;
mod health_ui;
mod help;
mod jobs;
mod projects;
mod report_ui;
mod settings;
mod sidebar;
mod stage;
#[cfg(test)]
mod ui_tests;

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
    /// GitHub started (or refused) a re-run of the failed jobs.
    Rerun(Result<(), String>),
    /// Finish the release / Remove the tag / Delete release finished.
    ReleaseFixed(Result<String, String>),
    Archived(Result<PathBuf, String>),
    Projects(Vec<ProjectInfo>),
    SelfTest(Box<SelfTestReport>, Option<PathBuf>),
    UpdateChecked(Result<updater::CheckResult, String>),
    /// The check caches' size in bytes (for Settings).
    CacheSize(u64),
    /// Check caches cleared: bytes freed, or why not.
    CachesCleared(Result<u64, String>),
    /// The family's tools and whether they're installed (Health).
    FamilyTools(Vec<crate::tools::ToolInfo>),
    /// pacman finished installing tools; re-check the staged project after.
    FamilyToolsInstalled(Result<String, String>, bool),
    /// What this computer's Rust is like (rustup check).
    Rust(crate::rustcheck::RustState),
    /// `rustup update` finished: (version, what happened), or why not.
    RustUpdated(Result<(String, String), String>),
    UpdateApplied(Result<PathBuf, String>),
    RolledBack(Result<(PathBuf, String), String>),
    SignIn(SignInEvent),
    Tools(Output),
    BugDownloaded(Result<Option<PathBuf>, String>),
    Done,
}

#[derive(PartialEq, Clone, Copy, Debug)]
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
    /// The loaded zip's size, time and fingerprint.
    zip_info: Option<crate::zipinfo::ZipInfo>,
    /// It's byte for byte the zip staged last time for this project.
    same_zip: bool,
    /// The browser saved a newer copy of this download beside it.
    newer_copy: Option<PathBuf>,
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
            zip_info: None,
            same_zip: false,
            newer_copy: None,
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
    /// The pushed commit being watched (for Check again and re-runs).
    commit: String,
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
    /// The command palette (Ctrl+K), while open.
    palette: Option<help::Palette>,
    show_shortcuts: bool,
    /// Which tour card is showing, if any.
    tour_step: Option<usize>,
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
    /// This computer's Rust (from the daily rustup check) and whether an update
    /// is running / was put off for now.
    rust: Option<crate::rustcheck::RustState>,
    /// The family's tools (None until surveyed) and an install in progress.
    tools: Option<Vec<crate::tools::ToolInfo>>,
    tools_busy: bool,
    /// The check caches' size (None until measured).
    cache_size: Option<u64>,
    rust_busy: bool,
    rust_later: bool,
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
        } else {
            // Settings read fine and the last run ended cleanly.
            HealthState::record(&paths, "Settings & state", true, "");
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
            palette: None,
            show_shortcuts: false,
            tour_step: None,
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
            rust: None,
            cache_size: None,
            tools: None,
            tools_busy: false,
            rust_busy: false,
            rust_later: false,
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
        // The tour starts by itself only on a new install.
        if !app.config.tour_seen {
            app.tour_step = Some(0);
        }
        app
    }
}

/// A pop-up window that always fits inside the app window and scrolls
/// inside itself instead of growing past the screen.
fn ui_window(ctx: &egui::Context, title: &str, width: f32) -> egui::Window<'static> {
    // Keep pop-ups below the top bar so its buttons stay visible.
    let top = ctx
        .data(|d| d.get_temp::<f32>(egui::Id::unique("top_bar_bottom")))
        .unwrap_or(56.0);
    let full = ctx.content_rect();
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
    fn ui(&mut self, root: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = &root.ctx().clone();
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
                .map(|f| f.path().to_path_buf())
                .filter(|p| !p.as_os_str().is_empty())
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
        self.shortcuts(ctx);

        // Light or dark, following the desktop unless chosen in Settings.
        let light = wants_light(&self.config.theme, ctx.system_theme());
        if light != ui::is_light() {
            ui::apply_theme(ctx, light);
        }

        self.sidebar(root);
        if self.tab == Tab::Stage {
            self.action_bar(root);
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
            .show(root, |ui| {
                // A faint dot grid behind empty areas (House Standard v1.3).
                if !self.config.plain_look {
                    ui::dot_grid(ui.painter(), ui.max_rect());
                }
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
        self.help_windows(ctx);
        if let Some((t, at)) = &self.toast {
            let gap = if self.tab == Tab::Stage { 88.0 } else { 24.0 };
            if !ui::toast(ctx, t, at.elapsed().as_secs_f32(), gap) {
                self.toast = None;
            }
        }
        self.save_config();
    }
}
