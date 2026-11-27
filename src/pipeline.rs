// SPDX-License-Identifier: GPL-3.0-or-later
//! The core pipeline, independent of the UI: stage (new / update), checks,
//! push, archive only, and the workspace project list. Runs on worker
//! threads; the self-test drives the same functions against a fake GitHub.

use crate::archive;
use crate::cleanup::{self, CleanupReport};
use crate::config::{AppPaths, Config};
use crate::detect::{self, Stack};
use crate::extract::{self, ExtractReport};
use crate::git::{self, Changes};
use crate::github::GitHubApi;
use crate::history::{History, PushRecord};
use crate::logbook::{Logger, StepStatus};
use crate::preflight::{self, TestEnv, TestResult};
use crate::secrets::{self, Hit};
use crate::state::{ProjectsState, Status};
use crate::util::{self, Ctx as _, Res};
use crate::version::Channel;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Instant;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    New,
    Update,
    ArchiveOnly,
}

impl Mode {
    pub fn badge(&self) -> &'static str {
        match self {
            Mode::New => "NEW REPO",
            Mode::Update => "UPDATE",
            Mode::ArchiveOnly => "ARCHIVE ONLY",
        }
    }
}

/// Everything a pipeline step needs.
#[derive(Clone)]
pub struct Ctx {
    pub paths: AppPaths,
    pub config: Config,
    pub gh: Arc<dyn GitHubApi>,
    pub log: Logger,
    pub cancel: Arc<AtomicBool>,
    /// Tests and the self-test use a pretend Trash inside their own folder,
    /// so they never put anything in the user's real Trash. None = real Trash.
    pub trash_dir: Option<PathBuf>,
}

impl Ctx {
    pub fn workspace(&self) -> PathBuf {
        self.config.workspace()
    }

    /// Move a folder to the Trash (the pretend one in tests).
    pub fn trash(&self, path: &Path) -> Res<String> {
        let Some(dir) = &self.trash_dir else {
            return util::move_to_trash(path);
        };
        std::fs::create_dir_all(dir).ctx("creating the test Trash")?;
        let base = path.file_name().and_then(|n| n.to_str()).unwrap_or("item");
        let mut dest = dir.join(base);
        let mut n = 1;
        while dest.exists() {
            n += 1;
            dest = dir.join(format!("{base}.{n}"));
        }
        std::fs::rename(path, &dest).ctx("moving to the Trash")?;
        Ok("Trash".into())
    }
    fn timed<T>(&self, name: &str, f: impl FnOnce() -> Res<T>) -> Res<T> {
        let t = Instant::now();
        self.log.step(name, StepStatus::Running, 0, "");
        let r = f();
        match &r {
            Ok(_) => self
                .log
                .step(name, StepStatus::Passed, t.elapsed().as_millis(), ""),
            Err(e) => {
                self.log
                    .step(name, StepStatus::Failed, t.elapsed().as_millis(), e);
                self.log.error(name, e);
            }
        }
        r
    }
}

/// Decide the mode for a slug from what exists locally and on GitHub.
/// `remote` is None when GitHub couldn't be checked (offline).
#[cfg(test)]
pub fn decide_mode(
    workspace: &Path,
    slug: &str,
    owner: &str,
    remote: Option<&[String]>,
) -> (Mode, bool, Option<bool>) {
    let r = route(workspace, slug, owner, remote);
    (r.mode, r.local, r.on_github)
}

/// Where a project goes: new or update, and whether its repo was deleted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Route {
    pub mode: Mode,
    /// A folder with this name is in the workspace.
    pub local: bool,
    /// On GitHub right now (None = GitHub's list isn't known).
    pub on_github: Option<bool>,
    /// The local folder pushes to this repo, but GitHub no longer has it.
    pub deleted_on_github: bool,
}

pub fn route(workspace: &Path, slug: &str, owner: &str, remote: Option<&[String]>) -> Route {
    let dir = detect::project_dir(workspace, slug);
    let local = dir.exists();
    let on_github = remote.map(|names| names.iter().any(|n| n.eq_ignore_ascii_case(slug)));
    let local_linked = local
        && git::remote_url(&dir)
            .map(|u| git::origin_matches(&u, owner, slug))
            .unwrap_or(false);
    // Only trust the local folder's link when GitHub's list isn't known:
    // a repo deleted on GitHub isn't an update any more.
    let mode = match on_github {
        Some(true) => Mode::Update,
        Some(false) => Mode::New,
        None if local_linked => Mode::Update,
        None => Mode::New,
    };
    Route {
        mode,
        local,
        on_github,
        deleted_on_github: local_linked && on_github == Some(false),
    }
}

#[derive(Debug, Clone)]
pub struct StageRequest {
    pub zip: PathBuf,
    pub slug: String,
    pub owner: String,
    pub mode: Mode,
    /// NEW mode only: replace an existing local folder that isn't on GitHub.
    pub replace_local: bool,
    pub description: String,
    /// "none", "MIT" or "GPL-3.0" (NEW repos only)
    pub license: String,
    pub license_holder: String,
}

#[derive(Debug, Clone, Default)]
pub struct StageResult {
    pub dir: PathBuf,
    pub stack: Option<Stack>,
    pub files: usize,
    pub size: u64,
    pub extract: ExtractReport,
    pub cleanup: CleanupReport,
    pub changes: Option<Changes>,
    pub warn_files: Vec<(String, u64)>,
    pub block_files: Vec<(String, u64)>,
    pub notes: Vec<String>,
    pub default_branch: Option<String>,
    pub run_command: String,
}

impl StageResult {
    pub fn stack(&self) -> Stack {
        self.stack.unwrap_or(Stack::Other)
    }
}

fn temp_dir_in(ws: &Path) -> Res<PathBuf> {
    util::ensure_private_dir(ws)
        .or_else(|_| std::fs::create_dir_all(ws).ctx("creating workspace"))?;
    let t = tempfile::Builder::new()
        .prefix(".stowlock-tmp-")
        .tempdir_in(ws)
        .ctx("creating a temp folder")?;
    Ok(t.keep())
}

fn license_text(kind: &str, holder: &str) -> Option<String> {
    let year = chrono::Local::now().format("%Y").to_string();
    match kind {
        "MIT" => Some(
            include_str!("../assets/licenses/MIT.txt")
                .replace("<year>", &year)
                .replace(
                    "<copyright holders>",
                    if holder.is_empty() {
                        "the authors"
                    } else {
                        holder
                    },
                ),
        ),
        "GPL-3.0" => Some(include_str!("../assets/licenses/GPL-3.0.txt").to_string()),
        _ => None,
    }
}

/// Extract, clean and prepare a project folder. Nothing touches GitHub
/// except cloning in UPDATE mode when the project isn't on this computer yet.
pub fn stage(ctx: &Ctx, req: &StageRequest) -> Res<StageResult> {
    if !detect::valid_slug(&req.slug) {
        return Err(detect::name_problem(&req.slug).unwrap_or_default());
    }
    let ws = ctx.workspace();
    std::fs::create_dir_all(&ws).ctx("creating the workspace folder")?;
    let target = detect::project_dir(&ws, &req.slug);
    let mut result = StageResult {
        dir: target.clone(),
        ..Default::default()
    };
    match req.mode {
        Mode::ArchiveOnly => return Err("Archive Only doesn't stage a project.".into()),
        Mode::New => ctx.timed("Extract & clean", || {
            stage_new(ctx, req, &target, &mut result)
        })?,
        Mode::Update => ctx.timed("Extract & clean", || {
            stage_update(ctx, req, &target, &mut result)
        })?,
    }
    // Common: what would be committed, sizes, counts.
    let files = git::files_to_commit(&target);
    let (warn, block) = cleanup::size_check(&target, &files);
    result.warn_files = warn;
    result.block_files = block;
    result.files = files.len();
    result.size = files
        .iter()
        .filter_map(|f| std::fs::metadata(target.join(f)).ok())
        .map(|m| m.len())
        .sum();
    let stack = detect::detect_stack(&target);
    result.stack = Some(stack);
    let st = ProjectsState::load(&ctx.paths).get(&req.slug);
    result.run_command = st
        .run_command
        .clone()
        .unwrap_or_else(|| detect::default_run_command(&target, stack));
    if req.mode == Mode::Update {
        result.changes = Some(git::changes(&target));
    }
    let zip_s = req.zip.display().to_string();
    let owner = req.owner.clone();
    let run = result.run_command.clone();
    ProjectsState::update(&ctx.paths, &req.slug, |p| {
        p.status = Status::Staged;
        p.source_zip = Some(zip_s);
        p.owner = Some(owner);
        if p.run_command.is_none() && !run.is_empty() {
            p.run_command = Some(run);
        }
    })?;
    ctx.log.info(
        "stage",
        &format!(
            "Staged {} ({} files, {})",
            req.slug,
            result.files,
            util::human_size(result.size)
        ),
    );
    Ok(result)
}

fn stage_new(ctx: &Ctx, req: &StageRequest, target: &Path, result: &mut StageResult) -> Res<()> {
    let ws = ctx.workspace();
    if target.exists() {
        // A folder StowLock staged earlier and never pushed (for example, the
        // tests failed) is replaced without asking; it goes to the Trash.
        let ours = ProjectsState::load(&ctx.paths).staged_not_pushed(&req.slug);
        if !req.replace_local && !ours {
            return Err(format!(
                "A folder named \"{}\" already exists in your workspace but isn't on GitHub. Tick \"Replace the existing folder\" to replace it, or pick another name.",
                req.slug
            ));
        }
        match ctx.trash(target) {
            Ok(_) => result.notes.push(if ours {
                "Moved the earlier unpushed staging to the Trash".into()
            } else {
                "Moved the old local folder to the Trash".into()
            }),
            Err(_) => {
                let aside = ws.join(format!(
                    "{}{}-{}",
                    crate::attempts::FOLDER_PREFIX,
                    req.slug,
                    util::now_stamp_secs()
                ));
                std::fs::rename(target, &aside).ctx("moving the old folder aside")?;
                result.notes.push(format!(
                    "Moved the {} to {}",
                    if ours {
                        "earlier unpushed staging"
                    } else {
                        "old local folder"
                    },
                    util::tilde(&aside)
                ));
            }
        }
    }
    let tmp = temp_dir_in(&ws)?;
    let ex = match extract::extract_zip(&req.zip, &tmp) {
        Ok(r) => r,
        Err(e) => {
            let _ = std::fs::remove_dir_all(&tmp);
            return Err(e);
        }
    };
    if let Err(e) = std::fs::rename(&tmp, target) {
        let _ = std::fs::remove_dir_all(&tmp);
        return Err(format!("moving the project into place: {e}"));
    }
    log_extract(ctx, &ex, result);
    result.extract = ex;
    let stack = detect::detect_stack(target);
    result.cleanup = cleanup::cleanup(target, stack)?;
    if !git::is_repo(target) {
        git::git_ok(target, &["init", "-b", "main"], &ctx.log, "stage")?;
    }
    // New-repo polish.
    if !target.join("README.md").exists()
        && !target.join("readme.md").exists()
        && !target.join("README").exists()
    {
        let readme = detect::generate_readme(target, &req.slug, &req.description, stack);
        std::fs::write(target.join("README.md"), readme).ctx("writing README.md")?;
        result.notes.push("Created a starter README.md".into());
    }
    let has_license = ["LICENSE", "LICENSE.md", "LICENSE.txt", "COPYING"]
        .iter()
        .any(|n| target.join(n).exists());
    if !has_license {
        if let Some(text) = license_text(&req.license, &req.license_holder) {
            std::fs::write(target.join("LICENSE"), text).ctx("writing LICENSE")?;
            result.notes.push(format!("Added {} LICENSE", req.license));
        }
    }
    Ok(())
}

fn log_extract(ctx: &Ctx, ex: &ExtractReport, result: &mut StageResult) {
    if let Some(w) = &ex.unwrapped {
        result.notes.push(format!("Unwrapped folder {w}"));
    }
    if !ex.skipped_unsafe.is_empty() {
        result.notes.push(format!(
            "Skipped {} unsafe path{}",
            ex.skipped_unsafe.len(),
            if ex.skipped_unsafe.len() == 1 {
                ""
            } else {
                "s"
            }
        ));
        for p in &ex.skipped_unsafe {
            ctx.log
                .warn("extract", &format!("skipped unsafe path: {p}"));
        }
    }
    if !ex.skipped_symlinks.is_empty() {
        result.notes.push(format!(
            "Skipped {} symlink{}",
            ex.skipped_symlinks.len(),
            if ex.skipped_symlinks.len() == 1 {
                ""
            } else {
                "s"
            }
        ));
    }
}

fn stage_update(ctx: &Ctx, req: &StageRequest, target: &Path, result: &mut StageResult) -> Res<()> {
    let log = &ctx.log;
    // 1. Clone if the project isn't on this computer yet.
    if !target.exists() {
        if !ctx.gh.repo_exists(&req.owner, &req.slug)? {
            return Err(format!(
                "{}/{} isn't on GitHub and isn't in your workspace.",
                req.owner, req.slug
            ));
        }
        ctx.gh.clone_repo(&req.owner, &req.slug, target, log)?;
        result
            .notes
            .push(format!("Cloned {}/{} from GitHub", req.owner, req.slug));
    }
    if !git::is_repo(target) {
        return Err(format!(
            "The folder {} exists but isn't a git repo. Rename it or pick another name.",
            util::tilde(target)
        ));
    }
    // 2. Origin must point at owner/slug.
    let url = git::remote_url(target).unwrap_or_default();
    if !git::origin_matches(&url, &req.owner, &req.slug) {
        return Err(format!(
            "This folder pushes to \"{}\", not to {}/{}. StowLock stopped so nothing goes to the wrong repo.",
            if url.is_empty() { "nowhere" } else { &url },
            req.owner,
            req.slug
        ));
    }
    // 3. No unsaved local work. Loose files left by StowLock's own earlier
    // staging (never pushed, e.g. tests failed) are saved in a git stash
    // so the new zip can go in and nothing is lost.
    if !git::status_lines(target).is_empty() {
        if !ProjectsState::load(&ctx.paths).staged_not_pushed(&req.slug) {
            return Err(
                "This project has unsaved changes that aren't on GitHub. Push or discard them first."
                    .into(),
            );
        }
        let label = format!(
            "{} {}",
            crate::attempts::STASH_PREFIX,
            util::now_stamp_secs()
        );
        let fb = (
            ctx.config.fallback_git_name.as_str(),
            ctx.config.fallback_git_email.as_str(),
        );
        // A stash is local only, so a placeholder name is fine if git has none.
        let mut args = git::identity_args(target, fb).unwrap_or_else(|_| {
            vec![
                "-c".into(),
                "user.name=StowLock".into(),
                "-c".into(),
                "user.email=stowlock@localhost".into(),
            ]
        });
        for a in ["stash", "push", "--include-untracked", "-m", &label] {
            args.push(a.to_string());
        }
        let refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
        git::git_ok(target, &refs, log, "stage")
            .map_err(|e| format!("Couldn't set aside the earlier unpushed staging: {e}"))?;
        if crate::attempts::drop_if_duplicate(target) {
            log.info(
                "stage",
                "Same as the last saved attempt, so it wasn't saved twice",
            );
        } else {
            result.notes.push(format!(
                "Saved the earlier unpushed staging in git stash (\"{label}\")"
            ));
        }
    }
    // 4. Default branch, fetch, fast-forward.
    let branch = ctx
        .gh
        .default_branch(&req.owner, &req.slug)
        .or_else(|_| git::current_branch(target).ok_or("no branch".to_string()))?;
    result.default_branch = Some(branch.clone());
    git::git_ok(target, &["fetch", "origin"], log, "stage")?;
    git::git_ok(target, &["checkout", &branch], log, "stage")?;
    if let Err(e) = git::git_ok(
        target,
        &["pull", "--ff-only", "origin", &branch],
        log,
        "stage",
    ) {
        return Err(format!("Your copy and GitHub have both changed since they split (pull --ff-only failed). Nothing was changed. Details: {e}"));
    }
    // 5. Sync the new files in, keeping .git.
    let had: Vec<&str> = [".gitignore", "LICENSE", "README.md"]
        .into_iter()
        .filter(|n| target.join(n).exists())
        .collect();
    let tmp = temp_dir_in(&ctx.workspace())?;
    let ex = match extract::extract_zip(&req.zip, &tmp) {
        Ok(r) => r,
        Err(e) => {
            let _ = std::fs::remove_dir_all(&tmp);
            return Err(e);
        }
    };
    for e in std::fs::read_dir(target).ctx("reading project")?.flatten() {
        if e.file_name() == ".git" {
            continue;
        }
        let p = e.path();
        if p.is_dir() && !p.is_symlink() {
            std::fs::remove_dir_all(&p).ctx("clearing old files")?;
        } else {
            std::fs::remove_file(&p).ctx("clearing old files")?;
        }
    }
    for e in std::fs::read_dir(&tmp)
        .ctx("reading extracted files")?
        .flatten()
    {
        if e.file_name() == ".git" {
            let _ = std::fs::remove_dir_all(e.path());
            continue;
        }
        std::fs::rename(e.path(), target.join(e.file_name())).ctx("moving new files in")?;
    }
    let _ = std::fs::remove_dir_all(&tmp);
    for f in had {
        if !target.join(f).exists() {
            let _ = git::git_ok(target, &["checkout", "HEAD", "--", f], log, "stage");
            result.notes.push(format!("Kept your existing {f}"));
        }
    }
    log_extract(ctx, &ex, result);
    result.extract = ex;
    let stack = detect::detect_stack(target);
    result.cleanup = cleanup::cleanup(target, stack)?;
    Ok(())
}

// ------------------------------------------------------------------ checks

#[derive(Debug, Clone)]
pub struct Checks {
    pub tests: TestResult,
    /// Only some jobs were re-checked: the full check must run before a push.
    pub partial: bool,
    /// Unresolved hits (those marked "Not a secret" are removed).
    pub secrets: Vec<Hit>,
    pub ignored_secrets: usize,
}

/// Where StowLock keeps each project's check cache (builds, its Python
/// environment, the fresh home) and its shared tools folder.
pub fn caches_dir(paths: &crate::config::AppPaths) -> PathBuf {
    paths.cache_dir.join("builds")
}

/// The shared Cargo tools folder: kept when old project caches are cleared.
const SHARED_TOOLS: &str = "stowlock-tools";
/// Marks when a project's check cache was last used.
const LAST_USED: &str = ".stowlock-last-used";

fn dir_size(p: &Path) -> u64 {
    walkdir::WalkDir::new(p)
        .follow_links(false)
        .into_iter()
        .flatten()
        .filter_map(|e| e.metadata().ok())
        .filter(|m| m.is_file())
        .map(|m| m.len())
        .sum()
}

/// How much space the check caches use, in bytes.
pub fn cache_size(paths: &crate::config::AppPaths) -> u64 {
    dir_size(&caches_dir(paths))
}

/// Remove every check cache (they're rebuilt when needed). Returns bytes freed.
pub fn clear_caches(paths: &crate::config::AppPaths) -> Res<u64> {
    let dir = caches_dir(paths);
    let size = dir_size(&dir);
    if dir.exists() {
        std::fs::remove_dir_all(&dir)
            .map_err(|e| format!("Couldn't clear the check caches: {e}"))?;
    }
    Ok(size)
}

/// Remove project caches not used for `days` days (the shared tools folder
/// stays). Returns the projects cleared and the bytes freed.
pub fn clean_old_caches(paths: &crate::config::AppPaths, days: u64) -> (Vec<String>, u64) {
    let mut cleared = Vec::new();
    let mut freed = 0;
    let Ok(rd) = std::fs::read_dir(caches_dir(paths)) else {
        return (cleared, freed);
    };
    let limit = std::time::Duration::from_secs(days * 24 * 60 * 60);
    for e in rd.flatten() {
        let p = e.path();
        let name = e.file_name().to_string_lossy().to_string();
        if !p.is_dir() || name == SHARED_TOOLS {
            continue;
        }
        let used = std::fs::metadata(p.join(LAST_USED))
            .or_else(|_| std::fs::metadata(&p))
            .and_then(|m| m.modified())
            .ok();
        let old = used
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age > limit);
        if old {
            let size = dir_size(&p);
            if std::fs::remove_dir_all(&p).is_ok() {
                freed += size;
                cleared.push(name);
            }
        }
    }
    (cleared, freed)
}

pub fn run_checks(ctx: &Ctx, slug: &str, dir: &Path, stack: Stack) -> Checks {
    run_checks_of(ctx, slug, dir, stack, &[])
}

/// The checks, for only some workflow jobs when `only_jobs` isn't empty.
pub fn run_checks_of(
    ctx: &Ctx,
    slug: &str,
    dir: &Path,
    stack: Stack,
    only_jobs: &[String],
) -> Checks {
    let st = ProjectsState::load(&ctx.paths).get(slug);
    // Mark this project's check cache as used (for the 30-day clean-up).
    let cache = caches_dir(&ctx.paths).join(slug.to_ascii_lowercase());
    if std::fs::create_dir_all(&cache).is_ok() {
        let _ = std::fs::write(cache.join(LAST_USED), util::now_stamp_secs());
    }
    let tests = if ctx.config.run_tests_before_push {
        let t = Instant::now();
        ctx.log.step("Pre-flight tests", StepStatus::Running, 0, "");
        let env = TestEnv {
            timeout_secs: ctx.config.test_timeout_secs,
            cancel: ctx.cancel.clone(),
            pass_env: st.pass_env,
            log: &ctx.log,
            build_cache: Some(
                ctx.paths
                    .cache_dir
                    .join("builds")
                    .join(slug.to_ascii_lowercase()),
            ),
            match_github: ctx.config.match_github_checks,
            exact: ctx.config.exact_github_checks,
            clean_home: ctx.config.clean_test_home,
            github_fonts: ctx.config.github_fonts,
            sandbox: ctx.config.sandbox_checks,
            offline: ctx.config.offline_checks,
            only_jobs: only_jobs.to_vec(),
        };
        let r = preflight::run_tests(dir, stack, &env);
        let status = match r.status {
            preflight::TestStatus::Passed => StepStatus::Passed,
            preflight::TestStatus::Skipped => StepStatus::Skipped,
            _ => StepStatus::Failed,
        };
        ctx.log.step(
            "Pre-flight tests",
            status,
            t.elapsed().as_millis(),
            &r.summary,
        );
        r
    } else {
        TestResult {
            status: preflight::TestStatus::Skipped,
            summary: "Skipped: tests are turned off in Settings".into(),
            tail: String::new(),
            duration: Default::default(),
            hint: None,
        }
    };
    let t = Instant::now();
    let all = secrets::scan_files(dir, &git::files_to_commit(dir));
    let before = all.len();
    let unresolved: Vec<Hit> = all
        .into_iter()
        .filter(|h| !st.not_secret.contains(&h.id))
        .collect();
    let ignored = before - unresolved.len();
    ctx.log.step(
        "Secret scan",
        if unresolved.is_empty() {
            StepStatus::Passed
        } else {
            StepStatus::Failed
        },
        t.elapsed().as_millis(),
        &format!("{} possible secret(s)", unresolved.len()),
    );
    for h in &unresolved {
        ctx.log.warn(
            "secrets",
            &format!("{}:{} {} {}", h.file, h.line, h.kind, h.masked),
        );
    }
    Checks {
        tests,
        partial: !only_jobs.is_empty(),
        secrets: unresolved,
        ignored_secrets: ignored,
    }
}

/// Why Push is blocked, if it is. `override_tests` = "Push anyway".
pub fn push_blockers(
    ctx: &Ctx,
    stage: &StageResult,
    checks: Option<&Checks>,
    public: bool,
    override_tests: bool,
) -> Vec<String> {
    let mut v = Vec::new();
    if !stage.block_files.is_empty() {
        v.push(format!(
            "{} file(s) are over GitHub's 100 MB limit",
            stage.block_files.len()
        ));
    }
    match checks {
        None => v.push("Checks haven't run yet".into()),
        Some(c) => {
            if c.tests.status == preflight::TestStatus::Failed
                && ctx.config.run_tests_before_push
                && !override_tests
            {
                v.push("Tests failed. See \"What to do next\" above".into());
            }
            if c.tests.status == preflight::TestStatus::Cancelled && !override_tests {
                v.push("Tests were cancelled".into());
            }
            if c.partial && !override_tests {
                v.push("Only some jobs were re-checked: run the full check before pushing".into());
            }
            if !c.secrets.is_empty() && (ctx.config.block_on_secrets || public) {
                v.push(format!("{} possible secret(s) found", c.secrets.len()));
            }
        }
    }
    v
}

// ------------------------------------------------------------------ push

#[derive(Debug, Clone)]
pub struct PushRequest {
    pub slug: String,
    pub owner: String,
    pub mode: Mode,
    pub private: bool,
    pub description: String,
    pub topics: Vec<String>,
    pub tag: String,
    pub channel: Channel,
    pub commit_message: String,
    pub archive_zip: Option<PathBuf>,
    pub archive_move: bool,
}

#[derive(Debug, Clone, Default)]
pub struct PushOutcome {
    pub url: String,
    pub tag: Option<String>,
    pub archived: Option<PathBuf>,
    pub warnings: Vec<String>,
    pub nothing_to_push: bool,
    /// The pushed commit (to follow GitHub's build for it).
    pub commit: Option<String>,
    /// The tag reached GitHub but creating the release failed: the reason.
    /// The UI offers Finish the release or Remove the tag.
    pub release_failed: Option<String>,
    /// The project's own release workflow builds and publishes the release
    /// (StowLock pushed the tag only).
    pub release_by_workflow: bool,
}

/// Create the GitHub release for `tag`, with the CHANGELOG notes if any.
fn create_release_with_notes(
    ctx: &Ctx,
    dir: &Path,
    owner: &str,
    slug: &str,
    tag: &str,
    beta: bool,
) -> Res<()> {
    let notes = crate::version::changelog_notes(dir, tag);
    if notes.is_some() {
        ctx.log.info(
            "release",
            "Using the CHANGELOG notes for this version as the release notes",
        );
    }
    match ctx
        .gh
        .create_release(owner, slug, tag, beta, notes.as_deref(), &ctx.log)
    {
        Err(e) if !e.to_ascii_lowercase().contains("already exists") => Err(e),
        _ => Ok(()),
    }
}

/// Finish the release for a tag that's already on GitHub (after creating the
/// release failed). Not blocked by "already exists": it's this push's tag.
pub fn finish_release(ctx: &Ctx, owner: &str, slug: &str, tag: &str, beta: bool) -> Res<()> {
    let dir = detect::project_dir(&ctx.workspace(), slug);
    create_release_with_notes(ctx, &dir, owner, slug, tag, beta)?;
    ctx.log
        .info("release", &format!("Finished the release {tag}"));
    Ok(())
}

/// Remove a release and its tag from GitHub and from this computer, so the
/// version can be used again. The code and commits stay.
pub fn remove_release(ctx: &Ctx, owner: &str, slug: &str, tag: &str) -> Res<()> {
    ctx.gh.delete_release(owner, slug, tag, &ctx.log)?;
    let dir = detect::project_dir(&ctx.workspace(), slug);
    if dir.is_dir() {
        let _ = git::git(&dir, &["tag", "-d", tag]);
    }
    ctx.log
        .info("release", &format!("Removed the release and tag {tag}"));
    Ok(())
}

pub fn push(ctx: &Ctx, req: &PushRequest) -> Res<PushOutcome> {
    let dir = detect::project_dir(&ctx.workspace(), &req.slug);
    if !dir.is_dir() {
        return Err("The staged project folder is missing. Stage the zip again.".into());
    }
    let log = &ctx.log;
    let fb = (
        ctx.config.fallback_git_name.as_str(),
        ctx.config.fallback_git_email.as_str(),
    );
    let tag = req.tag.trim().to_string();
    let mut outcome = PushOutcome {
        url: ctx.gh.repo_url(&req.owner, &req.slug),
        ..Default::default()
    };
    if !tag.is_empty() {
        let existing = git::tags(&dir);
        if existing.contains(&tag)
            && git::git(&dir, &["ls-remote", "--tags", "origin", &tag])
                .stdout
                .contains(&tag)
        {
            return Err(format!(
                "The tag {tag} already exists on GitHub. Pick a new version."
            ));
        }
    }
    match req.mode {
        Mode::ArchiveOnly => return Err("Archive Only doesn't push.".into()),
        Mode::New => ctx.timed("Commit & push", || {
            if !git::is_repo(&dir) {
                git::git_ok(&dir, &["init", "-b", "main"], log, "push")?;
            }
            let mut remote = git::remote_url(&dir);
            // The folder still points at a repo that was deleted on GitHub:
            // create it again (history and tags are kept).
            if let Some(url) = remote.clone() {
                if git::origin_matches(&url, &req.owner, &req.slug)
                    && !ctx.gh.repo_exists(&req.owner, &req.slug)?
                {
                    git::git_ok(&dir, &["remote", "remove", "origin"], log, "push")?;
                    log.info("push", "The repo was deleted on GitHub; creating it again");
                    remote = None;
                }
            }
            if remote.is_none() && ctx.gh.repo_exists(&req.owner, &req.slug)? {
                return Err(format!(
                    "Name already exists on this account: {}/{}",
                    req.owner, req.slug
                ));
            }
            git::git_ok(&dir, &["add", "-A"], log, "push")?;
            if !git::status_lines(&dir).is_empty() || !git::has_commits(&dir) {
                git::commit(&dir, &req.commit_message, fb, log, "push")?;
            }
            if remote.is_none() {
                ctx.gh.create_repo(
                    &dir,
                    &req.owner,
                    &req.slug,
                    req.private,
                    &req.description,
                    log,
                )?;
            } else {
                let branch = git::current_branch(&dir).unwrap_or_else(|| "main".into());
                git::git_ok(&dir, &["push", "-u", "origin", &branch], log, "push")?;
            }
            if !req.topics.is_empty() {
                if let Err(e) = ctx.gh.add_topics(&req.owner, &req.slug, &req.topics, log) {
                    outcome
                        .warnings
                        .push(format!("Topics weren't added: {}", first_line(&e)));
                }
            }
            Ok(())
        })?,
        Mode::Update => ctx.timed("Commit & push", || {
            let url = git::remote_url(&dir).unwrap_or_default();
            if !git::origin_matches(&url, &req.owner, &req.slug) {
                return Err(format!(
                    "This folder pushes to \"{url}\", not {}/{}. Stopped.",
                    req.owner, req.slug
                ));
            }
            let branch = git::current_branch(&dir).unwrap_or_else(|| "main".into());
            git::git_ok(&dir, &["add", "-A"], log, "push")?;
            let dirty = !git::status_lines(&dir).is_empty();
            let unpushed = git::unpushed_commits(&dir).unwrap_or(1);
            if !dirty && unpushed == 0 && tag.is_empty() {
                outcome.nothing_to_push = true;
                return Ok(());
            }
            if dirty {
                git::commit(&dir, &req.commit_message, fb, log, "push")?;
            }
            git::git_ok(&dir, &["push", "origin", &branch], log, "push")?;
            Ok(())
        })?,
    }
    if outcome.nothing_to_push {
        log.info("push", "No changes to push");
        return Ok(outcome);
    }
    outcome.commit = git::git(&dir, &["rev-parse", "HEAD"]).ok().then(|| {
        git::git(&dir, &["rev-parse", "HEAD"])
            .stdout
            .trim()
            .to_string()
    });
    if !tag.is_empty() {
        let r = ctx.timed("Tag & release", || {
            let created = !git::tags(&dir).contains(&tag);
            if created {
                git::with_identity(
                    &dir,
                    &["tag", "-a", &tag, "-m", &format!("Release {tag}")],
                    fb,
                    log,
                    "release",
                )?;
            }
            if let Err(e) = git::git_ok(&dir, &["push", "origin", &tag], log, "release") {
                // Nothing reached GitHub: take the tag back off this computer
                // too, so trying again isn't blocked by "already exists".
                if created {
                    let _ = git::git(&dir, &["tag", "-d", &tag]);
                    log.info(
                        "release",
                        &format!("Removed the local tag {tag} so you can try again"),
                    );
                }
                return Err(e);
            }
            if crate::workflows::publishes_releases(&dir) {
                log.info(
                    "release",
                    &format!("The project's release workflow builds and publishes {tag} on GitHub"),
                );
                outcome.release_by_workflow = true;
                return Ok(());
            }
            if let Err(e) = create_release_with_notes(
                ctx,
                &dir,
                &req.owner,
                &req.slug,
                &tag,
                req.channel == Channel::Beta,
            ) {
                outcome.release_failed = Some(e.clone());
                return Err(format!(
                    "The tag {tag} is on GitHub, but creating the release failed: {e}"
                ));
            }
            Ok(())
        });
        match r {
            Ok(()) => outcome.tag = Some(tag.clone()),
            // The code and tag are on GitHub; offer Finish/Remove instead of failing.
            Err(_) if outcome.release_failed.is_some() => outcome.tag = Some(tag.clone()),
            Err(e) => return Err(e),
        }
    }
    if let Some(zip) = &req.archive_zip {
        let t = Instant::now();
        match archive::archive_zip(
            zip,
            &ctx.config.archive_root(),
            &req.slug,
            outcome.tag.as_deref(),
            req.archive_move,
        ) {
            Ok(p) => {
                log.step(
                    "Archive zip",
                    StepStatus::Passed,
                    t.elapsed().as_millis(),
                    "",
                );
                log.info("archive", &format!("Archived to {}", util::tilde(&p)));
                outcome.archived = Some(p);
            }
            Err(e) => {
                log.step(
                    "Archive zip",
                    StepStatus::Failed,
                    t.elapsed().as_millis(),
                    &e,
                );
                outcome
                    .warnings
                    .push(format!("The push worked, but the zip wasn't archived: {e}"));
            }
        }
    }
    History::append(
        &ctx.paths,
        PushRecord {
            slug: req.slug.clone(),
            owner: req.owner.clone(),
            github_url: outcome.url.clone(),
            mode: if req.mode == Mode::New {
                "new".into()
            } else {
                "update".into()
            },
            tag: outcome.tag.clone(),
            channel: req.channel.key().into(),
            visibility: if req.private {
                "private".into()
            } else {
                "public".into()
            },
            pushed_at: util::now_iso(),
            archive_path: outcome.archived.as_ref().map(|p| util::tilde(p)),
        },
    )?;
    let tag_c = outcome.tag.clone();
    let chan = req.channel.key().to_string();
    ProjectsState::update(&ctx.paths, &req.slug, |p| {
        p.status = Status::Pushed;
        if tag_c.is_some() {
            p.last_tag = tag_c;
            p.last_channel = Some(chan);
        }
    })?;
    log.info("push", &format!("Pushed {}/{}", req.owner, req.slug));
    Ok(outcome)
}

fn first_line(s: &str) -> String {
    s.lines().next().unwrap_or("").to_string()
}

/// Archive Only: file the zip with its project. Nothing goes to GitHub.
pub fn archive_only(
    ctx: &Ctx,
    zip: &Path,
    slug: &str,
    tag: Option<&str>,
    move_it: bool,
) -> Res<PathBuf> {
    if !detect::valid_slug(slug) {
        return Err(detect::name_problem(slug).unwrap_or_default());
    }
    let p = ctx.timed("Archive zip", || {
        archive::archive_zip(zip, &ctx.config.archive_root(), slug, tag, move_it)
    })?;
    ProjectsState::update(&ctx.paths, slug, |s| {
        if s.status == Status::Unknown {
            s.status = Status::Archived;
        }
    })?;
    ctx.log.info(
        "archive",
        &format!("Archived {} to {}", util::tilde(zip), util::tilde(&p)),
    );
    Ok(p)
}

// ------------------------------------------------------------------ projects

#[derive(Debug, Clone)]
pub struct ProjectInfo {
    pub slug: String,
    pub dir: PathBuf,
    pub stack: Stack,
    pub badge: String,
    pub last_tag: Option<String>,
    pub last_push: Option<String>,
    pub archives: usize,
    pub remote: Option<String>,
}

pub fn scan_projects(paths: &AppPaths, config: &Config) -> Vec<ProjectInfo> {
    let ws = config.workspace();
    let state = ProjectsState::load(paths);
    let history = History::load(paths);
    let archive_root = config.archive_root();
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(&ws) else {
        return out;
    };
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        if name.starts_with('.') || name == "_archive" || !e.path().is_dir() {
            continue;
        }
        let dir = e.path();
        let st = state.get(&name);
        let is_repo = git::is_repo(&dir);
        let local_changes = is_repo && !git::status_lines(&dir).is_empty();
        let last_tag =
            st.last_tag
                .clone()
                .or_else(|| if is_repo { git::latest_tag(&dir) } else { None });
        let badge = if local_changes && st.status != Status::Staged {
            "Local changes".to_string()
        } else {
            match st.status {
                Status::Staged => "Staged".into(),
                Status::Pushed => match (&st.last_tag, st.last_channel.as_deref()) {
                    (Some(t), Some("beta")) => format!("Beta {t}"),
                    (Some(t), _) => format!("Pushed {t}"),
                    _ => "Pushed".into(),
                },
                _ => {
                    if is_repo && git::remote_url(&dir).is_some() {
                        "On GitHub".into()
                    } else {
                        "Local only".into()
                    }
                }
            }
        };
        out.push(ProjectInfo {
            stack: detect::detect_stack(&dir),
            remote: if is_repo { git::remote_url(&dir) } else { None },
            last_push: history
                .last_for(&name)
                .map(|r| format!("{} {}", r.date(), r.time())),
            archives: archive::count_archives(&archive_root, &name),
            slug: name,
            dir,
            badge,
            last_tag,
        });
    }
    out.sort_by(|a, b| a.slug.cmp(&b.slug));
    out
}

/// Remove leftover temp folders older than a day (after a crash).
pub fn clean_stale_temp(config: &Config) {
    let ws = config.workspace();
    let Ok(rd) = std::fs::read_dir(&ws) else {
        return;
    };
    for e in rd.flatten() {
        let n = e.file_name().to_string_lossy().to_string();
        if n.starts_with(".stowlock-tmp-") {
            let old = e
                .metadata()
                .and_then(|m| m.modified())
                .map(|t| t.elapsed().map(|d| d.as_secs() > 86_400).unwrap_or(false))
                .unwrap_or(false);
            if old {
                let _ = std::fs::remove_dir_all(e.path());
            }
        }
    }
}

/// Start the project's run command in a terminal window.
pub fn run_in_terminal(dir: &Path, command: &str, pass_env: bool) -> Res<std::process::Child> {
    if command.trim().is_empty() {
        return Err("Set a run command first.".into());
    }
    let script =
        format!("{command}\necho\necho '[StowLock] Finished. Press Enter to close.'\nread _");
    let opts = crate::proc::Opts {
        cwd: Some(dir.to_path_buf()),
        scrub_env: !pass_env,
        ..Default::default()
    };
    let wd = dir.display().to_string();
    let attempts: Vec<(&str, Vec<&str>)> = vec![
        (
            "konsole",
            vec!["--workdir", wd.as_str(), "-e", "sh", "-c", script.as_str()],
        ),
        ("xdg-terminal-exec", vec!["sh", "-c", script.as_str()]),
        (
            "x-terminal-emulator",
            vec!["-e", "sh", "-c", script.as_str()],
        ),
        ("gnome-terminal", vec!["--", "sh", "-c", script.as_str()]),
        ("xterm", vec!["-e", "sh", "-c", script.as_str()]),
    ];
    for (prog, args) in attempts {
        if crate::proc::which(prog).is_some() {
            return crate::proc::spawn_detached(prog, &args, &opts)
                .map_err(|e| format!("couldn't start {prog}: {e}"));
        }
    }
    Err("No terminal app was found (tried konsole, xdg-terminal-exec, x-terminal-emulator).".into())
}

/// Which terminal launcher would be used (for the self-test).
pub fn terminal_available() -> Option<&'static str> {
    [
        "konsole",
        "xdg-terminal-exec",
        "x-terminal-emulator",
        "gnome-terminal",
        "xterm",
    ]
    .into_iter()
    .find(|p| crate::proc::which(p).is_some())
}

pub fn open_in_editor(dir: &Path) -> Res<()> {
    let d = dir.display().to_string();
    if let Ok(v) = std::env::var("VISUAL") {
        if !v.trim().is_empty() {
            return crate::proc::spawn_detached(v.trim(), &[d.as_str()], &Default::default())
                .map(|_| ())
                .map_err(|e| e.to_string());
        }
    }
    if let Ok(ed) = std::env::var("EDITOR") {
        if !ed.trim().is_empty() {
            return run_in_terminal(dir, &format!("{} .", ed.trim()), true).map(|_| ());
        }
    }
    crate::proc::xdg_open(&d)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::{fake_token, Sandbox};
    use std::os::unix::fs::PermissionsExt;

    fn req(sb: &Sandbox, zip: &Path, mode: Mode) -> StageRequest {
        StageRequest {
            zip: zip.to_path_buf(),
            slug: "demo-app".into(),
            owner: sb.owner.clone(),
            mode,
            replace_local: false,
            description: "Demo app, a vibe-coded project".into(),
            license: "GPL-3.0".into(),
            license_holder: String::new(),
        }
    }

    fn preq(
        sb: &Sandbox,
        mode: Mode,
        tag: &str,
        channel: Channel,
        zip: Option<PathBuf>,
    ) -> PushRequest {
        PushRequest {
            slug: "demo-app".into(),
            owner: sb.owner.clone(),
            mode,
            private: true,
            description: "Demo".into(),
            topics: vec!["rust".into()],
            tag: tag.into(),
            channel,
            commit_message: crate::version::default_commit_message(mode == Mode::New, tag),
            archive_zip: zip,
            archive_move: true,
        }
    }

    #[test]
    fn full_new_update_archive_flow() {
        let d = tempfile::tempdir().unwrap();
        let sb = Sandbox::new(d.path()).unwrap();
        let ws = sb.ctx.workspace();
        let zip1 = sb.python_zip(
            "demo-app-main.zip",
            "demo-app-main",
            &[("old.py", b"x = 1\n")],
        );
        let (mode, local, remote) = decide_mode(&ws, "demo-app", &sb.owner, Some(&[]));
        assert_eq!((mode, local, remote), (Mode::New, false, Some(false)));

        let st = stage(&sb.ctx, &req(&sb, &zip1, Mode::New)).unwrap();
        let dir = ws.join("demo-app");
        assert!(dir.join("main.py").exists(), "wrapper folder unwrapped");
        assert!(dir.join(".env").exists(), ".env kept locally");
        assert!(!dir.join(".DS_Store").exists());
        assert!(std::fs::read_to_string(dir.join(".gitignore"))
            .unwrap()
            .contains(".env"));
        assert!(dir.join("README.md").exists() && dir.join("LICENSE").exists());
        assert!(
            std::fs::metadata(dir.join("run.sh"))
                .unwrap()
                .permissions()
                .mode()
                & 0o111
                != 0
        );
        assert_eq!(st.stack(), Stack::Python);
        assert!(
            !git::files_to_commit(&dir)
                .iter()
                .any(|f| f.ends_with(".env")),
            ".env must not be committed"
        );

        let checks = run_checks(&sb.ctx, "demo-app", &dir, st.stack());
        assert!(checks.secrets.is_empty());
        assert!(
            push_blockers(&sb.ctx, &st, Some(&checks), false, false).is_empty()
                || crate::proc::which("python3").is_none()
        );

        let out = push(
            &sb.ctx,
            &preq(
                &sb,
                Mode::New,
                "v0.1.0",
                Channel::Stable,
                Some(zip1.clone()),
            ),
        )
        .unwrap();
        let bare = sb.fake.bare(&sb.owner, "demo-app");
        assert!(bare.exists());
        assert_eq!(
            git::git_bare(&bare, &["rev-list", "--count", "main"]).stdout_trim(),
            "1",
            "single initial commit"
        );
        assert_eq!(
            sb.fake.releases(&sb.owner, "demo-app"),
            vec![("v0.1.0".to_string(), false)]
        );
        assert!(out
            .archived
            .as_ref()
            .unwrap()
            .ends_with("_archive/demo-app/demo-app-v0.1.0.zip"));
        assert!(!zip1.exists(), "zip moved out of downloads");
        assert_eq!(History::load(&sb.ctx.paths).today().len(), 1);

        // Update: old.py removed, util.py changed.
        let zip2 = sb.zip(
            "demo-app-v2.zip",
            &[
                ("main.py", b"print('v2')\n", 0o644),
                ("util.py", b"def add(a, b):\n    return b + a\n", 0o644),
                ("run.sh", b"#!/bin/sh\npython3 main.py\n", 0o755),
            ],
        );
        let names: Vec<String> = sb
            .ctx
            .gh
            .list_repos(&sb.owner)
            .unwrap()
            .into_iter()
            .map(|r| r.name)
            .collect();
        let (mode2, _, _) = decide_mode(
            &ws,
            &detect::slug_from_filename("demo-app-v2.zip"),
            &sb.owner,
            Some(&names),
        );
        assert_eq!(mode2, Mode::Update);
        let st2 = stage(&sb.ctx, &req(&sb, &zip2, Mode::Update)).unwrap();
        let ch = st2.changes.clone().unwrap();
        assert!(ch.removed.iter().any(|f| f == "old.py"), "{ch:?}");
        assert!(ch.changed.iter().any(|f| f == "main.py"));
        assert!(dir.join(".git").exists());
        assert!(
            dir.join("LICENSE").exists() && dir.join("README.md").exists(),
            "kept existing files"
        );
        let out2 = push(
            &sb.ctx,
            &preq(
                &sb,
                Mode::Update,
                "v0.2.0-beta.1",
                Channel::Beta,
                Some(zip2),
            ),
        )
        .unwrap();
        assert!(!out2.nothing_to_push);
        assert!(sb
            .fake
            .releases(&sb.owner, "demo-app")
            .contains(&("v0.2.0-beta.1".to_string(), true)));
        assert_eq!(
            git::git_bare(&bare, &["rev-list", "--count", "main"]).stdout_trim(),
            "2"
        );

        // Nothing changed → nothing to push.
        let out3 = push(&sb.ctx, &preq(&sb, Mode::Update, "", Channel::Stable, None)).unwrap();
        assert!(out3.nothing_to_push);

        // Dirty tree is refused.
        std::fs::write(dir.join("scratch.txt"), "wip").unwrap();
        let zip3 = sb.zip("demo-app-v3.zip", &[("main.py", b"print('v3')\n", 0o644)]);
        let err = stage(&sb.ctx, &req(&sb, &zip3, Mode::Update)).unwrap_err();
        assert!(err.contains("unsaved changes"), "{err}");
        std::fs::remove_file(dir.join("scratch.txt")).unwrap();

        // Diverged history is refused, never force-pushed.
        let other = d.path().join("other-clone");
        git::git(
            d.path(),
            &[
                "clone",
                &bare.display().to_string(),
                &other.display().to_string(),
            ],
        );
        std::fs::write(other.join("remote.txt"), "from elsewhere").unwrap();
        git::git(&other, &["add", "-A"]);
        git::git(
            &other,
            &[
                "-c",
                "user.name=x",
                "-c",
                "user.email=x@y",
                "commit",
                "-m",
                "remote change",
            ],
        );
        assert!(git::git(&other, &["push", "origin", "main"]).ok());
        std::fs::write(dir.join("local.txt"), "local").unwrap();
        git::git(&dir, &["add", "-A"]);
        git::git(
            &dir,
            &[
                "-c",
                "user.name=x",
                "-c",
                "user.email=x@y",
                "commit",
                "-m",
                "local change",
            ],
        );
        let err2 = stage(&sb.ctx, &req(&sb, &zip3, Mode::Update)).unwrap_err();
        assert!(err2.contains("both changed"), "{err2}");
        assert_eq!(
            git::git_bare(&bare, &["log", "-1", "--format=%s", "main"]).stdout_trim(),
            "remote change"
        );

        // Archive Only never pushes.
        let before = git::git_bare(&bare, &["rev-list", "--count", "main"]).stdout_trim();
        let p = archive_only(&sb.ctx, &zip3, "demo-app", Some("v0.0.9"), true).unwrap();
        assert!(p.exists());
        assert_eq!(
            git::git_bare(&bare, &["rev-list", "--count", "main"]).stdout_trim(),
            before
        );
    }

    #[test]
    fn capitalised_github_repo_routes_as_update() {
        let d = tempfile::tempdir().unwrap();
        let sb = Sandbox::new(d.path()).unwrap();
        // A repo called "Sluice" already exists on (fake) GitHub, with one commit.
        let seed = d.path().join("seed");
        std::fs::create_dir_all(&seed).unwrap();
        std::fs::write(seed.join("main.py"), "print(1)\n").unwrap();
        git::git(&seed, &["init", "-b", "main"]);
        git::git(&seed, &["add", "-A"]);
        git::git(
            &seed,
            &[
                "-c",
                "user.name=s",
                "-c",
                "user.email=s@x",
                "commit",
                "-m",
                "first",
            ],
        );
        sb.fake
            .create_repo(&seed, &sb.owner, "Sluice", true, "", &sb.ctx.log)
            .unwrap();
        let names: Vec<String> = sb
            .ctx
            .gh
            .list_repos(&sb.owner)
            .unwrap()
            .into_iter()
            .map(|r| r.name)
            .collect();
        let slug = detect::slug_from_filename("sluice-v2.zip");
        let (mode, _, on_gh) = decide_mode(&sb.ctx.workspace(), &slug, &sb.owner, Some(&names));
        assert_eq!(
            (mode, on_gh),
            (Mode::Update, Some(true)),
            "case doesn't matter"
        );
        let zip = sb.zip("sluice-v2.zip", &[("main.py", b"print(2)\n", 0o644)]);
        let mut r = req(&sb, &zip, Mode::Update);
        r.slug = "Sluice".into(); // the app adopts GitHub's exact name
        let st = stage(&sb.ctx, &r).unwrap();
        assert!(st.dir.ends_with("Sluice"));
        // Staging again with lowercase finds the same folder, no duplicate.
        assert_eq!(detect::project_dir(&sb.ctx.workspace(), "sluice"), st.dir);
    }

    #[test]
    fn secrets_and_local_conflicts_block() {
        let d = tempfile::tempdir().unwrap();
        let sb = Sandbox::new(d.path()).unwrap();
        let tok = fake_token();
        let zip = sb.python_zip(
            "leaky.zip",
            "leaky",
            &[("config.py", format!("TOKEN = '{tok}'\n").as_bytes())],
        );
        let mut r = req(&sb, &zip, Mode::New);
        r.slug = "leaky".into();
        let st = stage(&sb.ctx, &r).unwrap();
        let checks = run_checks(&sb.ctx, "leaky", &st.dir, st.stack());
        assert_eq!(checks.secrets.len(), 1);
        assert_eq!(checks.secrets[0].file, "config.py");
        let blockers = push_blockers(&sb.ctx, &st, Some(&checks), true, true);
        assert!(
            blockers.iter().any(|b| b.contains("secret")),
            "secrets can't be overridden while public"
        );
        assert!(!sb.ctx.log.text().contains(&tok), "token never logged");
        // Marking it as not a secret clears it.
        let id = checks.secrets[0].id.clone();
        ProjectsState::update(&sb.ctx.paths, "leaky", |p| p.not_secret.push(id)).unwrap();
        assert!(run_checks(&sb.ctx, "leaky", &st.dir, st.stack())
            .secrets
            .is_empty());
        // A local-only folder StowLock didn't stage (here: the record says
        // it isn't StowLock's unpushed staging) needs an explicit replace.
        ProjectsState::update(&sb.ctx.paths, "leaky", |p| p.status = Status::Unknown).unwrap();
        let err = stage(&sb.ctx, &r).unwrap_err();
        assert!(err.contains("already exists"));
    }

    #[test]
    fn failed_update_can_be_restaged_and_nothing_is_lost() {
        let d = tempfile::tempdir().unwrap();
        let sb = Sandbox::new(d.path()).unwrap();
        let z1 = sb.zip("demo-app.zip", &[("main.py", b"print(1)\n", 0o644)]);
        stage(&sb.ctx, &req(&sb, &z1, Mode::New)).unwrap();
        push(&sb.ctx, &preq(&sb, Mode::New, "", Channel::Stable, None)).unwrap();
        // An update is staged but never pushed (say the tests failed).
        let z2 = sb.zip(
            "demo-app-v2.zip",
            &[("main.py", b"print(2 broken\n", 0o644)],
        );
        let st = stage(&sb.ctx, &req(&sb, &z2, Mode::Update)).unwrap();
        // The fixed zip stages cleanly over it.
        let z3 = sb.zip("demo-app-v3.zip", &[("main.py", b"print(3)\n", 0o644)]);
        let st3 = stage(&sb.ctx, &req(&sb, &z3, Mode::Update)).unwrap();
        assert!(st3.notes.iter().any(|n| n.contains("git stash")));
        assert_eq!(
            std::fs::read_to_string(st.dir.join("main.py")).unwrap(),
            "print(3)\n"
        );
        // The broken attempt is kept in the stash, not thrown away.
        let stash = git::git(&st.dir, &["stash", "show", "-p", "stash@{0}"]).stdout;
        assert!(stash.contains("print(2 broken"));
    }

    #[test]
    fn real_local_edits_still_block_an_update() {
        let d = tempfile::tempdir().unwrap();
        let sb = Sandbox::new(d.path()).unwrap();
        let z1 = sb.zip("demo-app.zip", &[("main.py", b"print(1)\n", 0o644)]);
        let st = stage(&sb.ctx, &req(&sb, &z1, Mode::New)).unwrap();
        push(&sb.ctx, &preq(&sb, Mode::New, "", Channel::Stable, None)).unwrap();
        // The person edits the pushed project by hand.
        std::fs::write(st.dir.join("main.py"), "print('mine')\n").unwrap();
        let z2 = sb.zip("demo-app-v2.zip", &[("main.py", b"print(2)\n", 0o644)]);
        let e = stage(&sb.ctx, &req(&sb, &z2, Mode::Update)).unwrap_err();
        assert!(e.contains("unsaved changes"));
        assert_eq!(
            std::fs::read_to_string(st.dir.join("main.py")).unwrap(),
            "print('mine')\n"
        );
    }

    #[test]
    fn failed_new_repo_can_be_restaged() {
        let d = tempfile::tempdir().unwrap();
        let sb = Sandbox::new(d.path()).unwrap();
        let z1 = sb.zip("demo-app.zip", &[("main.py", b"print(1 broken\n", 0o644)]);
        stage(&sb.ctx, &req(&sb, &z1, Mode::New)).unwrap();
        let z2 = sb.zip("demo-app-v2.zip", &[("main.py", b"print(2)\n", 0o644)]);
        let st = stage(&sb.ctx, &req(&sb, &z2, Mode::New)).unwrap();
        assert!(st
            .notes
            .iter()
            .any(|n| n.contains("earlier unpushed staging")));
        assert_eq!(
            std::fs::read_to_string(st.dir.join("main.py")).unwrap(),
            "print(2)\n"
        );
    }

    #[test]
    fn failed_release_can_be_finished_or_removed() {
        let d = tempfile::tempdir().unwrap();
        let sb = Sandbox::new(d.path()).unwrap();
        let z = sb.zip("demo-app.zip", &[("main.py", b"print(1)\n", 0o644)]);
        stage(&sb.ctx, &req(&sb, &z, Mode::New)).unwrap();
        // GitHub accepts the tag but creating the release fails.
        std::fs::create_dir_all(&sb.fake.root).unwrap();
        std::fs::write(sb.fake.root.join("fail-release"), "HTTP 502").unwrap();
        let out = push(
            &sb.ctx,
            &preq(&sb, Mode::New, "v0.1.0", Channel::Stable, None),
        )
        .unwrap();
        assert!(out.release_failed.as_deref().unwrap_or("").contains("502"));
        assert_eq!(out.tag.as_deref(), Some("v0.1.0"));
        assert!(out.commit.is_some());
        let fake = sb.fake.clone();
        assert!(fake.releases(&sb.owner, "demo-app").is_empty());
        // Finish the release once GitHub works again.
        std::fs::remove_file(sb.fake.root.join("fail-release")).unwrap();
        finish_release(&sb.ctx, &sb.owner, "demo-app", "v0.1.0", false).unwrap();
        assert_eq!(fake.releases(&sb.owner, "demo-app").len(), 1);
        // Remove it: gone from GitHub and this computer, so v0.1.0 is free.
        remove_release(&sb.ctx, &sb.owner, "demo-app", "v0.1.0").unwrap();
        assert!(fake.releases(&sb.owner, "demo-app").is_empty());
        let dir = detect::project_dir(&sb.ctx.workspace(), "demo-app");
        assert!(!git::tags(&dir).contains(&"v0.1.0".to_string()));
    }

    #[test]
    fn a_repo_deleted_on_github_is_created_again() {
        let d = tempfile::tempdir().unwrap();
        let sb = Sandbox::new(d.path()).unwrap();
        let ws = sb.ctx.workspace();
        let zip = sb.python_zip("demo-app.zip", "demo-app", &[("app.py", b"x = 1\n")]);
        stage(&sb.ctx, &req(&sb, &zip, Mode::New)).unwrap();
        push(
            &sb.ctx,
            &preq(&sb, Mode::New, "v0.1.0", Channel::Stable, None),
        )
        .unwrap();
        let names = |sb: &Sandbox| -> Vec<String> {
            sb.ctx
                .gh
                .list_repos(&sb.owner)
                .unwrap()
                .into_iter()
                .map(|r| r.name)
                .collect()
        };
        let r = route(&ws, "demo-app", &sb.owner, Some(&names(&sb)));
        assert_eq!((r.mode, r.deleted_on_github), (Mode::Update, false));

        // Deleted on GitHub: no longer an update, and flagged.
        std::fs::remove_dir_all(sb.fake.bare(&sb.owner, "demo-app")).unwrap();
        let r = route(&ws, "demo-app", &sb.owner, Some(&names(&sb)));
        assert_eq!(
            (r.mode, r.on_github, r.deleted_on_github),
            (Mode::New, Some(false), true)
        );
        // GitHub's list unknown (offline): the folder's link still counts.
        let r = route(&ws, "demo-app", &sb.owner, None);
        assert_eq!((r.mode, r.deleted_on_github), (Mode::Update, false));

        // Push it again as a new repo: created again, with its history.
        let commits_before =
            git::git(&ws.join("demo-app"), &["rev-list", "--count", "HEAD"]).stdout;
        push(
            &sb.ctx,
            &preq(&sb, Mode::New, "v0.1.1", Channel::Stable, None),
        )
        .unwrap();
        assert!(
            names(&sb).iter().any(|n| n == "demo-app"),
            "repo not created again"
        );
        let bare = sb.fake.bare(&sb.owner, "demo-app");
        let on_gh = crate::git::git_bare(&bare, &["rev-list", "--count", "main"]).stdout;
        assert_eq!(on_gh.trim(), commits_before.trim(), "history wasn't kept");
    }

    #[test]
    fn owner_and_repo_from_remote_urls() {
        for u in [
            "https://github.com/Matt-B-007/StowLock.git",
            "git@github.com:Matt-B-007/StowLock.git",
            "https://github.com/Matt-B-007/StowLock/",
        ] {
            assert_eq!(
                git::owner_repo(u),
                Some(("Matt-B-007".into(), "StowLock".into())),
                "{u}"
            );
        }
        assert_eq!(git::owner_repo("/tmp/fake/x.git"), None);
    }

    #[test]
    fn a_project_that_publishes_its_own_releases_gets_the_tag_only() {
        let d = tempfile::tempdir().unwrap();
        let sb = Sandbox::new(d.path()).unwrap();
        let wf = b"on:\n  push:\n    tags: ['v*']\njobs:\n  release:\n    runs-on: ubuntu-latest\n    steps:\n      - run: gh release create \"$GITHUB_REF_NAME\" --draft\n";
        let zip = sb.python_zip(
            "demo-app.zip",
            "demo-app",
            &[
                ("app.py", b"x = 1\n"),
                (".github/workflows/release.yml", wf),
            ],
        );
        stage(&sb.ctx, &req(&sb, &zip, Mode::New)).unwrap();
        let out = push(
            &sb.ctx,
            &preq(&sb, Mode::New, "v1.0.0", Channel::Stable, None),
        )
        .unwrap();
        assert!(out.release_by_workflow);
        assert_eq!(out.tag.as_deref(), Some("v1.0.0"));
        assert!(
            sb.fake.releases(&sb.owner, "demo-app").is_empty(),
            "StowLock made the release"
        );
        let tags = crate::git::git_bare(&sb.fake.bare(&sb.owner, "demo-app"), &["tag"]).stdout;
        assert!(tags.contains("v1.0.0"), "tag not pushed: {tags}");
    }

    #[test]
    fn restaging_the_same_zip_doesnt_pile_up_saved_attempts() {
        let d = tempfile::tempdir().unwrap();
        let sb = Sandbox::new(d.path()).unwrap();
        let ws = sb.ctx.workspace();
        let v1 = sb.python_zip("demo-app.zip", "demo-app", &[("app.py", b"x = 1\n")]);
        stage(&sb.ctx, &req(&sb, &v1, Mode::New)).unwrap();
        push(
            &sb.ctx,
            &preq(&sb, Mode::New, "v0.1.0", Channel::Stable, None),
        )
        .unwrap();
        let v2 = sb.python_zip(
            "demo-app-2.zip",
            "demo-app",
            &[("app.py", b"x = 2\n"), ("new.py", b"y = 1\n")],
        );
        let v3 = sb.python_zip("demo-app-3.zip", "demo-app", &[("app.py", b"x = 3\n")]);
        let stashes = || {
            git::git(&ws.join("demo-app"), &["stash", "list"])
                .stdout
                .lines()
                .count()
        };
        let restage = |zip: &std::path::Path| {
            crate::state::ProjectsState::update(&sb.ctx.paths, "demo-app", |p| {
                p.status = crate::state::Status::Staged
            })
            .unwrap();
            stage(&sb.ctx, &req(&sb, zip, Mode::Update)).unwrap();
        };
        restage(&v2); // nothing to save yet
        restage(&v2); // saves the v2 staging
        assert_eq!(stashes(), 1);
        restage(&v2); // the same v2 staging again: not saved twice
        restage(&v3); // still the same v2 staging being set aside
        assert_eq!(stashes(), 1, "identical attempts were saved twice");
        restage(&v2); // now the v3 staging is set aside: a different attempt
        assert_eq!(stashes(), 2);
    }

    #[test]
    fn old_check_caches_are_removed_but_shared_tools_stay() {
        let d = tempfile::tempdir().unwrap();
        let sb = Sandbox::new(d.path()).unwrap();
        let root = caches_dir(&sb.ctx.paths);
        for p in ["old-project", "new-project", SHARED_TOOLS] {
            std::fs::create_dir_all(root.join(p)).unwrap();
            std::fs::write(root.join(p).join("big.bin"), vec![0u8; 1000]).unwrap();
            std::fs::write(root.join(p).join(LAST_USED), "x").unwrap();
        }
        let long_ago =
            std::time::SystemTime::now() - std::time::Duration::from_secs(40 * 24 * 3600);
        for p in ["old-project", SHARED_TOOLS] {
            let f = std::fs::File::options()
                .write(true)
                .open(root.join(p).join(LAST_USED))
                .unwrap();
            f.set_modified(long_ago).unwrap();
        }
        assert!(cache_size(&sb.ctx.paths) >= 3000);
        let (cleared, freed) = clean_old_caches(&sb.ctx.paths, 30);
        assert_eq!(cleared, ["old-project"]);
        assert!(freed >= 1000);
        assert!(root.join("new-project").exists() && root.join(SHARED_TOOLS).exists());
        assert!(clear_caches(&sb.ctx.paths).unwrap() >= 2000);
        assert!(!root.exists());
    }
}
