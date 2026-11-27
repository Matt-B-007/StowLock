// SPDX-License-Identifier: GPL-3.0-or-later
//! Pre-flight tests: build/test commands per stack, with a timeout, cancel,
//! graceful handling of missing toolchains and dependencies, and a cleaned
//! environment (token-like variables removed) unless the user allows it.

use crate::detect::Stack;
use crate::logbook::Logger;
use crate::proc::{self, Opts, Output};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TestStatus {
    Passed,
    Failed,
    Skipped,
    Cancelled,
}

#[derive(Debug, Clone)]
pub struct TestResult {
    pub status: TestStatus,
    pub summary: String,
    /// Output with build progress collapsed and failure lines kept.
    pub tail: String,
    pub duration: Duration,
    pub hint: Option<String>,
}

impl TestResult {
    fn skipped(summary: &str, hint: Option<&str>) -> Self {
        TestResult {
            status: TestStatus::Skipped,
            summary: summary.into(),
            tail: String::new(),
            duration: Duration::ZERO,
            hint: hint.map(|s| s.into()),
        }
    }
    #[cfg(test)]
    pub fn passed_or_skipped(&self) -> bool {
        matches!(self.status, TestStatus::Passed | TestStatus::Skipped)
    }
}

pub struct TestEnv<'a> {
    pub timeout_secs: u64,
    pub cancel: Arc<AtomicBool>,
    pub pass_env: bool,
    pub log: &'a Logger,
    /// Where build files are kept between checks of the same project, so a
    /// second check doesn't rebuild everything. None = the project's own folder.
    pub build_cache: Option<PathBuf>,
    /// Run the project's own GitHub workflow steps when it has them.
    pub match_github: bool,
    /// Run the GitHub workflows exactly, with act (needs act and Docker).
    pub exact: bool,
    /// Run GitHub steps in a fresh home folder without the desktop session,
    /// like GitHub's clean machine (tool caches are kept).
    pub clean_home: bool,
}

/// Added to a test summary when the failures look specific to this computer.
pub const MACHINE_SPECIFIC: &str = "looks specific to this computer";

/// The names of failed tests (cargo, pytest, go, jest), shortest form,
/// in the order they appear, without repeats.
pub fn failed_tests(transcript: &str) -> Vec<String> {
    use regex::Regex;
    let pats = [
        Regex::new(r"(?m)^test (\S+) \.\.\. FAILED").unwrap(),
        Regex::new(r"(?m)^FAILED (\S+)").unwrap(),
        Regex::new(r"(?m)^\s*--- FAIL: (\S+)").unwrap(),
        Regex::new(r"(?m)^\s*[✕×] (.+?)(?: \(\d+ ?m?s\))?$").unwrap(),
    ];
    let mut out: Vec<String> = Vec::new();
    for re in &pats {
        for c in re.captures_iter(transcript) {
            let full = c[1].trim();
            // pipeline::tests::demo -> demo; tests/test_x.py::test_y -> test_y
            let short = full.rsplit("::").next().unwrap_or(full).to_string();
            if !out.contains(&short) {
                out.push(short);
            }
        }
    }
    out
}

/// "65 passed, 2 failed" plus the failed tests' names (up to 5).
pub fn with_failed_names(counts: String, transcript: &str) -> String {
    let names = failed_tests(transcript);
    if names.is_empty() {
        return counts;
    }
    let shown: Vec<&str> = names.iter().take(5).map(|s| s.as_str()).collect();
    let more = names.len().saturating_sub(5);
    let tail = if more > 0 {
        format!(" (+{more} more)")
    } else {
        String::new()
    };
    format!("{counts}: {}{tail}", shown.join(", "))
}

/// Pull the pass/fail counts out of test output (cargo, pytest, go, jest).
pub fn result_summary(transcript: &str) -> Option<String> {
    use regex::Regex;
    let cargo = Regex::new(r"test result: \w+\. (\d+) passed; (\d+) failed").unwrap();
    let (mut p, mut f, mut found) = (0u64, 0u64, false);
    for c in cargo.captures_iter(transcript) {
        p += c[1].parse::<u64>().unwrap_or(0);
        f += c[2].parse::<u64>().unwrap_or(0);
        found = true;
    }
    if found {
        return Some(format!("{p} passed, {f} failed"));
    }
    let jest = transcript
        .lines()
        .find(|l| l.trim_start().starts_with("Tests:"));
    if let Some(l) = jest {
        return Some(l.trim().trim_start_matches("Tests:").trim().to_string());
    }
    let py = Regex::new(r"(?m)^=+ (.*(?:passed|failed|error).*?) (?:in [\d.]+s )?=+$").unwrap();
    if let Some(c) = py.captures_iter(transcript).last() {
        return Some(c[1].to_string());
    }
    // pytest -q ends with a bare line: "5 failed, 1154 passed, 6 skipped in 116.08s"
    let quiet = Regex::new(r"(?m)^\s*(\d+ (?:failed|passed|errors?)\b[^\n]*?) in [\d.]+s").unwrap();
    if let Some(c) = quiet.captures_iter(transcript).last() {
        return Some(c[1].to_string());
    }
    let ok = transcript
        .lines()
        .filter(|l| l.starts_with("ok ") || l.starts_with("ok\t"))
        .count();
    let fail = transcript
        .lines()
        .filter(|l| l.starts_with("FAIL\t") || l.starts_with("--- FAIL"))
        .count();
    if ok + fail > 0 {
        return Some(format!("{ok} package(s) passed, {fail} failed"));
    }
    None
}

fn run_step(dir: &Path, program: &str, args: &[&str], env: &TestEnv) -> Output {
    run_step_with(dir, program, args, env, &[])
}

fn run_step_with(
    dir: &Path,
    program: &str,
    args: &[&str],
    env: &TestEnv,
    extra: &[(String, String)],
) -> Output {
    run_step_labelled(dir, program, args, env, extra, &[], None, env.timeout_secs)
}

#[allow(clippy::too_many_arguments)]
fn run_step_labelled(
    dir: &Path,
    program: &str,
    args: &[&str],
    env: &TestEnv,
    extra: &[(String, String)],
    remove: &[String],
    label: Option<&str>,
    timeout_secs: u64,
) -> Output {
    let opts = Opts {
        cwd: Some(dir.to_path_buf()),
        timeout: Some(Duration::from_secs(timeout_secs)),
        cancel: Some(env.cancel.clone()),
        scrub_env: !env.pass_env,
        env: {
            let mut e = vec![("CI".into(), "true".into())];
            if let Some(c) = &env.build_cache {
                e.push(("CARGO_TARGET_DIR".into(), c.display().to_string()));
            }
            e.extend(extra.iter().cloned());
            e
        },
        stdin: None,
        env_remove: remove.to_vec(),
    };
    let o = proc::run(program, args, &opts);
    match label {
        Some(l) => env.log.command_as("tests", l, &o),
        None => env.log.command("tests", &o),
    }
    o
}

/// The desktop session's variables: GitHub's machine has none, and apps read
/// them to decide how to look (effects, scaling, theme).
const SESSION_VARS: &[&str] = &[
    "DISPLAY",
    "WAYLAND_DISPLAY",
    "XAUTHORITY",
    "XDG_CURRENT_DESKTOP",
    "XDG_SESSION_TYPE",
    "XDG_SESSION_DESKTOP",
    "XDG_SESSION_ID",
    "XDG_SESSION_CLASS",
    "XDG_SEAT",
    "XDG_VTNR",
    "DESKTOP_SESSION",
    "SESSION_MANAGER",
    "DBUS_SESSION_BUS_ADDRESS",
    "KDE_FULL_SESSION",
    "KDE_SESSION_VERSION",
    "KDE_SESSION_UID",
    "KDE_APPLICATIONS_AS_SCOPE",
    "GNOME_DESKTOP_SESSION_ID",
    "GNOME_SETUP_DISPLAY",
    "QT_QPA_PLATFORMTHEME",
    "QT_STYLE_OVERRIDE",
    "QT_AUTO_SCREEN_SCALE_FACTOR",
    "QT_SCREEN_SCALE_FACTORS",
    "QT_SCALE_FACTOR",
    "PLASMA_USE_QT_SCALING",
    "GTK_THEME",
    "GTK_MODULES",
    "GDK_SCALE",
    "GDK_BACKEND",
    "XCURSOR_THEME",
    "XCURSOR_SIZE",
    "SSH_AUTH_SOCK",
    "GPG_AGENT_INFO",
];

/// A fresh home folder for GitHub steps: (variables to set, variables to drop).
/// Tool caches (Rust, pip, npm, Go) still point at the real ones, so it stays fast.
/// Variables to set, and variables to leave out.
type EnvChanges = (Vec<(String, String)>, Vec<String>);

fn clean_home(dir: &Path, env: &TestEnv, job: &str) -> Option<EnvChanges> {
    let real = std::env::var("HOME").ok().filter(|h| !h.is_empty())?;
    // Each job its own fresh home, like GitHub's separate machines.
    let home = match &env.build_cache {
        Some(c) => c.join("stowlock-home").join(job_folder(job)),
        None => python_env_dir(dir, env, job).with_extension("home"),
    };
    let _ = std::fs::remove_dir_all(&home);
    let runtime = home.join(".run");
    for d in [".config", ".local/share", ".local/state", ".cache"] {
        std::fs::create_dir_all(home.join(d)).ok()?;
    }
    std::fs::create_dir_all(&runtime).ok()?;
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&runtime, std::fs::Permissions::from_mode(0o700));
    }
    let h = |p: &str| home.join(p).display().to_string();
    let keep = |var: &str, default: String| std::env::var(var).unwrap_or(default);
    let set = vec![
        ("HOME".into(), home.display().to_string()),
        ("XDG_CONFIG_HOME".into(), h(".config")),
        ("XDG_DATA_HOME".into(), h(".local/share")),
        ("XDG_STATE_HOME".into(), h(".local/state")),
        ("XDG_CACHE_HOME".into(), h(".cache")),
        ("XDG_RUNTIME_DIR".into(), runtime.display().to_string()),
        (
            "CARGO_HOME".into(),
            keep("CARGO_HOME", format!("{real}/.cargo")),
        ),
        (
            "RUSTUP_HOME".into(),
            keep("RUSTUP_HOME", format!("{real}/.rustup")),
        ),
        (
            "PIP_CACHE_DIR".into(),
            keep("PIP_CACHE_DIR", format!("{real}/.cache/pip")),
        ),
        (
            "npm_config_cache".into(),
            keep("npm_config_cache", format!("{real}/.npm")),
        ),
        ("GOPATH".into(), keep("GOPATH", format!("{real}/go"))),
        (
            "GOCACHE".into(),
            keep("GOCACHE", format!("{real}/.cache/go-build")),
        ),
    ];
    Some((set, SESSION_VARS.iter().map(|v| v.to_string()).collect()))
}

/// The failures mention this computer itself (a running program, your real
/// home folder), not the project: they'd likely pass on GitHub.
/// Where cargo and rustup keep downloaded libraries and toolchains.
fn tool_homes() -> Vec<PathBuf> {
    let home = std::env::var("HOME").ok().filter(|h| h.len() > 1);
    let mut out = Vec::new();
    for (var, default) in [("CARGO_HOME", ".cargo"), ("RUSTUP_HOME", ".rustup")] {
        if let Some(v) = std::env::var_os(var).filter(|v| !v.is_empty()) {
            out.push(PathBuf::from(v));
        }
        if let Some(h) = &home {
            out.push(Path::new(h).join(default));
        }
    }
    out
}

fn machine_specific(output: &str, dir: &Path, env: &TestEnv) -> bool {
    let mut text = output.to_string();
    // The project's own folder and StowLock's cache are expected in paths.
    text = text.replace(&dir.display().to_string(), "<project>");
    if let Some(c) = &env.build_cache {
        text = text.replace(&c.display().to_string(), "<cache>");
    }
    // So are Rust's own folders: a panic inside a library names its source
    // under ~/.cargo/registry, which says nothing about this computer.
    for tool_home in tool_homes() {
        text = text.replace(&format!("{}/", tool_home.display()), "<tools>/");
    }
    let lower = text.to_ascii_lowercase();
    let running = regex::Regex::new(r"(?i)(still running|already running|\(process \d+|\bpid \d+)")
        .unwrap()
        .is_match(&text);
    let home = std::env::var("HOME")
        .ok()
        .filter(|h| h.len() > 1)
        .is_some_and(|h| lower.contains(&format!("{}/", h.to_ascii_lowercase())));
    running || home
}

/// Did any tests pass? ("1154 passed", "75 passed, 0 failed")
fn some_passed(counts: &str) -> bool {
    regex::Regex::new(r"(\d+) passed")
        .unwrap()
        .captures_iter(counts)
        .any(|c| c[1].parse::<u64>().unwrap_or(0) > 0)
}

/// Something really failed: failed tests, a panic, a compile error, a
/// formatting diff. Then a missing tool later on isn't the whole story.
fn real_failure(block: &str) -> bool {
    let failed = regex::Regex::new(r"(\d+) failed")
        .unwrap()
        .captures_iter(block)
        .any(|c| c[1].parse::<u64>().unwrap_or(0) > 0);
    failed
        || [
            "panicked at",
            " FAILED",
            "AssertionError",
            "error[E",
            "Diff in ",
            "could not compile",
        ]
        .iter()
        .any(|w| block.contains(w))
}

/// The package names in a `sudo pacman -S --needed …` command.
pub fn pacman_packages(cmd: &str) -> Vec<String> {
    cmd.split_whitespace()
        .skip_while(|w| !w.starts_with("-S"))
        .skip(1)
        .filter(|w| !w.starts_with('-'))
        .filter(|w| {
            w.chars()
                .all(|c| c.is_ascii_alphanumeric() || "@._+-".contains(c))
        })
        .map(|w| w.to_string())
        .collect()
}

/// The `sudo pacman -S …` command a check script suggests, if any.
fn pacman_line(output: &str) -> Option<String> {
    output.lines().find_map(|l| {
        let i = l.find("sudo pacman -S")?;
        let cmd = l[i..].trim().trim_end_matches(['.', ')', '`']);
        Some(cmd.to_string())
    })
}

/// The tool a "not installed" message names, and the project's own advice
/// for installing it ("Install it with: …"), if it gives one.
fn named_missing_tool(output: &str) -> (Option<String>, Option<String>) {
    let mut tool = missing_command(output);
    let mut hint = None;
    if output.contains("could not choose a version") || output.contains("no default toolchain") {
        return (
            Some("a Rust version (rustup has none chosen yet)".into()),
            Some("rustup default stable".into()),
        );
    }
    // "shellcheck isn't installed", "shellcheck 0.11.0 is needed (found: none)"
    let phrase = regex::Regex::new(
        r"(?i)([A-Za-z][\w.+-]*)(?: v?[\d.]+)? (?:isn't installed|is not installed|is needed)",
    )
    .unwrap();
    for l in output.lines() {
        let l = l.trim();
        let lower = l.to_ascii_lowercase();
        if tool.is_none() {
            if let Some(c) = phrase.captures(l) {
                let name = c[1].trim_matches('`');
                if !name.is_empty() && !name.eq_ignore_ascii_case("it") {
                    tool = Some(name.to_string());
                }
            }
            if let Some(i) = lower.find("no such command: ") {
                let name = l[i + 17..]
                    .trim()
                    .trim_matches(|c| c == '`' || c == '\'' || c == '"');
                if !name.is_empty() {
                    tool = Some(format!("cargo-{name}"));
                }
            }
        }
        if hint.is_none() {
            if let Some(i) = lower.find("install it with: ") {
                let h = l[i + 17..].trim().trim_end_matches('.');
                if !h.is_empty() {
                    hint = Some(h.to_string());
                }
            }
        }
    }
    (tool, hint)
}

/// The command a "command not found" line names.
fn missing_command(output: &str) -> Option<String> {
    output.lines().find_map(|l| {
        let l = l.trim();
        let rest = l.strip_suffix(": command not found")?;
        let name = rest.rsplit(": ").next()?.trim();
        (!name.is_empty() && !name.contains(' ')).then(|| name.to_string())
    })
}

/// How to get a missing tool on CachyOS / Arch.
fn install_hint(tool: &str) -> String {
    const PY: [&str; 8] = [
        "pytest", "ruff", "black", "mypy", "flake8", "pylint", "tox", "isort",
    ];
    if PY.contains(&tool) {
        format!("sudo pacman -S python-{tool}")
    } else {
        format!("sudo pacman -S {tool}")
    }
}

/// StowLock's shared folder for Cargo tools a workflow installs.
fn tools_dir(dir: &Path, env: &TestEnv) -> PathBuf {
    match env.build_cache.as_ref().and_then(|c| c.parent()) {
        Some(p) => p.join("stowlock-tools"),
        None => python_env_dir(dir, env, "tools").with_extension("tools"),
    }
}

/// StowLock's own Python environment for a project's pip installs.
/// A folder name for a job ("Tests (Ubuntu)" → "tests-ubuntu").
fn job_folder(job: &str) -> String {
    let mut out = String::new();
    for c in job.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    let out = out.trim_matches('-').to_string();
    if out.is_empty() {
        "job".into()
    } else {
        out
    }
}

/// StowLock's own Python environment for one job's pip installs.
fn python_env_dir(dir: &Path, env: &TestEnv, job: &str) -> PathBuf {
    match &env.build_cache {
        Some(c) => c.join("stowlock-venv").join(job_folder(job)),
        None => {
            use std::hash::{Hash, Hasher};
            let mut h = std::collections::hash_map::DefaultHasher::new();
            dir.hash(&mut h);
            std::env::temp_dir().join(format!(
                "stowlock-venv-{:x}-{}",
                h.finish(),
                job_folder(job)
            ))
        }
    }
}

fn missing(tool: &str, pkg: &str) -> TestResult {
    TestResult::skipped(
        &format!("Skipped: {tool} not installed"),
        Some(&format!("sudo pacman -S {pkg}")),
    )
}

/// Run the checks for a stack. Steps stop at the first failure.
/// A GitHub step that failed here (the check goes on with the other jobs).
struct StepFailure {
    step: String,
    what: String,
    counts: String,
    note: String,
    block: String,
    hint: Option<String>,
}

/// Run the project's GitHub workflow steps here, as GitHub would.
fn run_plan(dir: &Path, plan: &crate::workflows::Plan, env: &TestEnv) -> TestResult {
    let start = Instant::now();
    // What the earlier steps did, one line each; the failing step in full.
    let mut done: Vec<String> = Vec::new();
    let mut transcript = String::new();
    let skipped_note = || -> String {
        if plan.skipped.is_empty() {
            return String::new();
        }
        let mut n = String::from("\nSkipped on this computer (GitHub only):\n");
        for sk in &plan.skipped {
            n.push_str(&format!("  - {}: {} ({})\n", sk.job, sk.name, sk.why));
        }
        n
    };
    let finish =
        |done: &[String], status: TestStatus, summary: String, last: &str, hint: Option<String>| {
            let mut tail = done.join("\n");
            if !last.is_empty() {
                if !tail.is_empty() {
                    tail.push('\n');
                }
                tail.push_str(&crate::logbook::shorten(last).join("\n"));
            }
            tail.push_str(&skipped_note());
            TestResult {
                status,
                summary,
                tail,
                duration: start.elapsed(),
                hint,
            }
        };
    let jobs = plan.github_only_jobs();
    let jobs_note = match jobs.len() {
        0 => String::new(),
        1 => format!(" (\"{}\" runs only on GitHub)", jobs[0]),
        _ => format!(" ({} jobs run only on GitHub)", jobs.len()),
    };
    let ran_of = |n: usize| format!("Ran {n} of {} GitHub steps here{jobs_note}", plan.total());
    let mut py_env: Vec<(String, String)> = Vec::new();
    // Steps that couldn't run here (something missing): (what it needs, how
    // to get it). The rest still run; GitHub runs these ones.
    let mut cant_run: Vec<(String, Option<String>)> = Vec::new();
    // Tools StowLock installed for checks: its Python environment and its
    // shared Cargo tools folder, searched before the system's.
    let tools_root = tools_dir(dir, env);
    // The project's Python environment comes first (its packages), then the
    // system's own tools, and StowLock's tools folder only as a fallback, so a
    // copy StowLock built earlier never hides the version you installed.
    let mut tool_paths: Vec<PathBuf> = Vec::new();
    let mut ran = 0usize;
    // GitHub runs each job on its own clean machine, and a failing job doesn't
    // stop the others: here each job gets its own fresh home and Python
    // environment, and a failure only stops the rest of its own job.
    let mut current_job: Option<String> = None;
    let mut home_env: Vec<(String, String)> = Vec::new();
    let mut session_vars: Vec<String> = Vec::new();
    let mut failed_jobs: Vec<String> = Vec::new();
    let mut failures: Vec<StepFailure> = Vec::new();
    for (i, step) in plan.steps.iter().enumerate() {
        if current_job.as_deref() != Some(step.job.as_str()) {
            current_job = Some(step.job.clone());
            py_env.clear();
            tool_paths.clear();
            (home_env, session_vars) = if env.clean_home {
                clean_home(dir, env, &step.job).unwrap_or_default()
            } else {
                Default::default()
            };
        }
        if failed_jobs.contains(&step.job) {
            done.push(format!(
                "Not run: [{}] {} (an earlier step in this job failed)",
                step.job, step.name
            ));
            continue;
        }
        let mut cmd = match &env.build_cache {
            Some(c) => crate::workflows::use_build_cache(&step.run, c),
            None => step.run.clone(),
        };
        let wd = match &step.workdir {
            Some(w) if !w.contains("..") => dir.join(w),
            _ => dir.to_path_buf(),
        };
        let label = format!("[{}] {}", step.job, step.name);
        let mut timeout = env.timeout_secs;
        if step.python_deps {
            // pip installs go into StowLock's own environment, never system-wide.
            if py_env.is_empty() {
                let venv = python_env_dir(dir, env, &step.job);
                // A fresh environment every check, as on GitHub, so nothing
                // built by an earlier check (an older version of the project)
                // can stand in. pip's download cache keeps this quick.
                let _ = std::fs::remove_dir_all(&venv);
                if !venv.join("bin").join("python").exists() {
                    let v = venv.display().to_string();
                    let o = run_step_with(dir, "python3", &["-m", "venv", &v], env, &[]);
                    if !o.ok() {
                        return finish(&done, TestStatus::Skipped,
                            format!(
                                "Can't run here: StowLock couldn't make a Python environment for \"{}\" · {}",
                                step.name,
                                ran_of(i)
                            ),
                            &format!("$ python3 -m venv\n{}", o.combined()),
                            Some("sudo pacman -S python".into()),
                        );
                    }
                }
                py_env = vec![
                    ("VIRTUAL_ENV".into(), venv.display().to_string()),
                    ("PIP_DISABLE_PIP_VERSION_CHECK".into(), "1".into()),
                ];
                tool_paths.insert(0, venv.join("bin"));
            }
            cmd = cmd.replace(" --user", "");
            timeout = timeout.max(900);
        }
        if step.cargo_tools {
            // Cargo tools go into StowLock's own tools folder (built once,
            // then reused by every project); tools you already have are kept.
            let _ = std::fs::create_dir_all(&tools_root);
            py_env.retain(|(k, _)| k != "CARGO_INSTALL_ROOT");
            py_env.push((
                "CARGO_INSTALL_ROOT".into(),
                tools_root.display().to_string(),
            ));
            timeout = timeout.max(1800);
        }
        let mut step_env = home_env.clone();
        step_env.extend(step.env.iter().cloned());
        step_env.extend(py_env.iter().cloned());
        let path = tool_paths
            .iter()
            .map(|p| p.display().to_string())
            .chain(std::iter::once(if env.clean_home {
                proc::checks_path()
            } else {
                proc::full_path()
            }))
            .chain(std::iter::once(
                tools_root.join("bin").display().to_string(),
            ))
            .collect::<Vec<_>>()
            .join(":");
        step_env.push(("PATH".into(), path));
        let o = run_step_labelled(
            &wd,
            "bash",
            &["-e", "-o", "pipefail", "-c", &cmd],
            env,
            &step_env,
            &session_vars,
            Some(&label),
            timeout,
        );
        let out = crate::util::strip_ansi(&o.combined());
        let block = format!("$ {label}\n{out}\n");
        transcript.push_str(&block);
        if o.cancelled {
            return finish(
                &done,
                TestStatus::Cancelled,
                "Cancelled".into(),
                &block,
                None,
            );
        }
        if o.ok() {
            ran += 1;
            let counts = result_summary(&block)
                .map(|c| format!(" · {c}"))
                .unwrap_or_default();
            done.push(format!("Passed: {label}{counts}"));
            continue;
        }
        // Something this step needs isn't on this computer: it can't run here,
        // which isn't the same as the project's tests failing.
        let needs = plan
            .skipped
            .iter()
            .rev()
            .find(|sk| sk.job == step.job && sk.is_install() && sk.before <= i);
        let missing_evidence = out.lines().any(crate::logbook::is_missing_line);
        // House Standard: a check script exits 3 when only tools are missing.
        let tools_missing_exit =
            o.code == Some(3) && (step.run.contains("check.sh") || missing_evidence);
        if tools_missing_exit {
            let (tool, own_hint) = named_missing_tool(&out);
            let packages = pacman_line(&out)
                .map(|l| pacman_packages(&l))
                .unwrap_or_default();
            // The script's own list names tools, never the project's code:
            // keep all of it in the log (and so in bug reports).
            for l in out.lines().filter(|l| !l.trim().is_empty()).take(20) {
                env.log.warn("tests", &format!("  {}", l.trim()));
            }
            let shown = match (&tool, needs) {
                _ if !packages.is_empty() => packages
                    .iter()
                    .map(|p| format!("`{p}`"))
                    .collect::<Vec<_>>()
                    .join(", "),
                (Some(t), _) => format!("`{t}`"),
                (None, Some(sk)) => format!("what \"{}\" installs on GitHub", sk.name),
                (None, None) => "tools that aren't installed here".to_string(),
            };
            done.push(format!(
                "Can't run here: {label} (needs {shown}; GitHub runs it)"
            ));
            // The script's own list of what's missing, in full: it names
            // tools, never the project's code.
            done.extend(
                out.lines()
                    .map(str::trim_end)
                    .filter(|l| !l.trim().is_empty())
                    .take(12)
                    .map(|l| format!("  {}", l.trim())),
            );
            let hint = pacman_line(&out)
                .or(own_hint)
                .or_else(|| tool.as_deref().map(install_hint));
            cant_run.push((shown, hint));
            continue;
        }
        if o.code == Some(127) {
            let tool = missing_command(&out).unwrap_or_else(|| "a command it uses".into());
            let shown = if tool.contains(' ') {
                tool.clone()
            } else {
                format!("`{tool}`")
            };
            done.push(format!(
                "Can't run here: {label} (needs {shown}; GitHub runs it)"
            ));
            let hint = (!tool.contains(' ')).then(|| install_hint(&tool));
            cant_run.push((shown, hint));
            continue;
        }
        // A project's own script that stops because a tool isn't installed
        // ("shellcheck isn't installed") can't run here either, as long as
        // nothing actually failed before it.
        if missing_evidence && needs.is_none() && !real_failure(&block) {
            let (tool, own_hint) = named_missing_tool(&out);
            let shown = tool
                .as_ref()
                .map(|t| format!("`{t}`"))
                .unwrap_or_else(|| "a tool it uses".into());
            let counts = result_summary(&block)
                .map(|c| format!(", after {c}"))
                .unwrap_or_default();
            done.push(format!(
                "Can't run here: {label} (needs {shown}{counts}; GitHub runs it)"
            ));
            let hint = own_hint.or_else(|| tool.as_deref().map(install_hint));
            cant_run.push((shown, hint));
            continue;
        }
        if let (Some(sk), true) = (needs, missing_evidence && !real_failure(&block)) {
            let what = match named_missing_tool(&out).0 {
                Some(t) => format!("`{t}`"),
                None => format!("what \"{}\" installs on GitHub", sk.name),
            };
            done.push(format!(
                "Can't run here: {label} (needs {what}; GitHub runs it)"
            ));
            done.extend(
                crate::logbook::shorten(&out)
                    .into_iter()
                    .filter(|l| crate::logbook::is_missing_line(l))
                    .take(3)
                    .map(|l| format!("  {}", l.trim())),
            );
            let hint = sk.run.as_ref().map(|r| {
                format!(
                    "GitHub installs it with: {}",
                    r.lines()
                        .find(|l| !l.trim().is_empty() && !l.trim_start().starts_with('#'))
                        .unwrap_or(r)
                        .trim()
                )
            });
            cant_run.push((what, hint));
            continue;
        }
        let what = if o.timed_out { "timed out" } else { "failed" };
        let counts = result_summary(&block)
            .map(|c| format!(" · {}", with_failed_names(c, &block)))
            .unwrap_or_default();
        let machine = machine_specific(&out, dir, env);
        // Only mention a skipped install when it could be the cause: tests
        // that ran (and mostly passed) clearly had what they needed.
        let ran_tests =
            result_summary(&block).is_some_and(|c| some_passed(&c)) || real_failure(&block);
        let hint = if machine {
            Some("These failures mention things on this computer that GitHub's machine doesn't have, such as a running copy of the app or your home folder. They'll likely pass on GitHub. Quit the running copy and Re-check, or push anyway.".to_string())
        } else {
            needs.filter(|_| !ran_tests).map(|sk| {
                format!(
                    "This step comes after \"{}\", which only runs on GitHub. If it needs what that step installs, it can't pass here.",
                    sk.name
                )
            })
        };
        let note = if machine {
            format!(" · {MACHINE_SPECIFIC}")
        } else {
            String::new()
        };
        ran += 1;
        done.push(format!("Failed: {label}"));
        failed_jobs.push(step.job.clone());
        failures.push(StepFailure {
            step: step.name.clone(),
            what: what.to_string(),
            counts,
            note,
            block,
            hint,
        });
    }
    // Each job's result, when there's more than one ("test failed, rust passed").
    let mut job_order: Vec<&str> = Vec::new();
    for st in &plan.steps {
        if !job_order.contains(&st.job.as_str()) {
            job_order.push(&st.job);
        }
    }
    let jobs_line = if job_order.len() > 1 {
        let each: Vec<String> = job_order
            .iter()
            .map(|j| {
                let state = if failed_jobs.iter().any(|f| f == j) {
                    "failed"
                } else if done
                    .iter()
                    .any(|d| d.starts_with(&format!("Passed: [{j}]")))
                {
                    "passed"
                } else {
                    "couldn't run here"
                };
                format!("{j} {state}")
            })
            .collect();
        format!(" · jobs: {}", each.join(", "))
    } else {
        String::new()
    };
    if let Some(first) = failures.first() {
        let blocks: Vec<String> = failures.iter().map(|f| f.block.clone()).collect();
        return finish(
            &done,
            TestStatus::Failed,
            format!(
                "GitHub step \"{}\" {} · {}{}{}{jobs_line}",
                first.step,
                first.what,
                ran_of(ran),
                first.counts,
                failures
                    .iter()
                    .map(|f| f.note.as_str())
                    .find(|n| !n.is_empty())
                    .unwrap_or("")
            ),
            &blocks.join("\n"),
            failures.iter().find_map(|f| f.hint.clone()),
        );
    }
    let counts = result_summary(&transcript)
        .map(|c| format!(" · {c}"))
        .unwrap_or_default();
    let step_skips = plan.skipped.iter().filter(|s| !s.whole_job).count();
    let skipped = if step_skips == 0 {
        String::new()
    } else {
        format!(" · {step_skips} GitHub-only skipped")
    };
    if cant_run.is_empty() {
        return finish(
            &done,
            TestStatus::Passed,
            format!("Passed: {}{skipped}{counts}{jobs_line}", ran_of(ran)),
            "",
            None,
        );
    }
    // Everything that could run here passed; a few steps need something this
    // computer doesn't have. That's a pass with a note, not a problem.
    let needs: Vec<&str> = cant_run.iter().map(|(w, _)| w.as_str()).collect();
    let n = cant_run.len();
    let note = format!(
        "{n} step{} can't run here (needs {}; GitHub runs {})",
        if n == 1 { "" } else { "s" },
        needs.join(", "),
        if n == 1 { "it" } else { "them" }
    );
    let hint = cant_run.iter().find_map(|(_, h)| h.clone());
    if ran == 0 {
        return finish(
            &done,
            TestStatus::Skipped,
            format!("Can't run here: {note}{counts}"),
            "",
            hint,
        );
    }
    finish(
        &done,
        TestStatus::Passed,
        format!("Passed: {}{counts} · {note}{jobs_line}", ran_of(ran)),
        "",
        hint,
    )
}

/// Exact mode: run the GitHub workflows with act, in containers like GitHub's.
fn run_with_act(dir: &Path, env: &TestEnv) -> Option<TestResult> {
    if proc::which("act").is_none() || proc::which("docker").is_none() {
        return None;
    }
    let start = Instant::now();
    let long = TestEnv {
        timeout_secs: env.timeout_secs.max(1800),
        cancel: env.cancel.clone(),
        pass_env: env.pass_env,
        log: env.log,
        build_cache: None,
        match_github: false,
        exact: false,
        clean_home: false,
    };
    let o = run_step(dir, "act", &["push", "--rm"], &long);
    let transcript = format!("$ act push --rm\n{}\n", o.combined());
    let status = if o.cancelled {
        TestStatus::Cancelled
    } else if o.ok() {
        TestStatus::Passed
    } else {
        TestStatus::Failed
    };
    let counts = result_summary(&transcript)
        .map(|c| format!(" · {c}"))
        .unwrap_or_default();
    Some(TestResult {
        status,
        summary: match status {
            TestStatus::Passed => {
                format!("Passed: your GitHub workflows, run exactly (act){counts}")
            }
            _ => format!("GitHub workflows (act) failed{counts}"),
        },
        tail: crate::logbook::shorten(&transcript).join("\n"),
        duration: start.elapsed(),
        hint: None,
    })
}

pub fn run_tests(dir: &Path, stack: Stack, env: &TestEnv) -> TestResult {
    if env.exact {
        if let Some(r) = run_with_act(dir, env) {
            return r;
        }
    }
    if env.match_github {
        let plan = crate::workflows::plan(dir);
        if let Some(plan) = &plan {
            if !plan.steps.is_empty() {
                return run_plan(dir, plan, env);
            }
        }
        // Nothing in the workflows can run here, but the project has its own
        // check script (the House Standard): run that.
        if dir.join("scripts").join("check.sh").is_file() {
            let mut p = plan.unwrap_or_default();
            p.steps.push(crate::workflows::PlanStep {
                job: "check.sh".into(),
                name: "scripts/check.sh".into(),
                run: "sh scripts/check.sh".into(),
                python_deps: false,
                cargo_tools: false,
                env: Vec::new(),
                workdir: None,
            });
            return run_plan(dir, &p, env);
        }
        if let Some(plan) = &plan {
            if !plan.skipped.is_empty() {
                let start = Instant::now();
                let mut tail = String::from("Every workflow step needs GitHub:\n");
                for sk in &plan.skipped {
                    tail.push_str(&format!("  - {}: {} ({})\n", sk.job, sk.name, sk.why));
                }
                return TestResult {
                    status: TestStatus::Skipped,
                    summary: format!(
                        "Can't run here: all {} GitHub steps need GitHub; GitHub runs them",
                        plan.skipped.len()
                    ),
                    tail,
                    duration: start.elapsed(),
                    hint: None,
                };
            }
        }
    }
    let start = Instant::now();
    let mut transcript = String::new();
    let finish = |status: TestStatus, summary: String, transcript: &str| TestResult {
        status,
        summary,
        tail: crate::logbook::shorten(transcript).join("\n"),
        duration: start.elapsed(),
        hint: None,
    };
    let mut steps: Vec<(String, Vec<String>)> = Vec::new();
    match stack {
        Stack::Rust => {
            if proc::which("cargo").is_none() {
                return missing("cargo", "rust");
            }
            steps.push(("cargo".into(), vec!["check".into()]));
            steps.push(("cargo".into(), vec!["test".into(), "--no-fail-fast".into()]));
        }
        Stack::Node => {
            if proc::which("npm").is_none() {
                return missing("npm", "nodejs npm");
            }
            if !dir.join("node_modules").exists() {
                if dir.join("package-lock.json").is_file() {
                    steps.push((
                        "npm".into(),
                        vec![
                            "ci".into(),
                            "--ignore-scripts".into(),
                            "--no-audit".into(),
                            "--no-fund".into(),
                        ],
                    ));
                } else {
                    steps.push((
                        "npm".into(),
                        vec![
                            "install".into(),
                            "--ignore-scripts".into(),
                            "--no-audit".into(),
                            "--no-fund".into(),
                        ],
                    ));
                }
            }
            let pkg = std::fs::read_to_string(dir.join("package.json")).unwrap_or_default();
            let v: serde_json::Value = serde_json::from_str(&pkg).unwrap_or_default();
            let test_script = v
                .get("scripts")
                .and_then(|s| s.get("test"))
                .and_then(|t| t.as_str())
                .unwrap_or("");
            if !test_script.is_empty() && !test_script.contains("no test specified") {
                steps.push(("npm".into(), vec!["test".into()]));
            } else if dir.join("tsconfig.json").is_file() {
                steps.push((
                    "npx".into(),
                    vec!["--no-install".into(), "tsc".into(), "--noEmit".into()],
                ));
            } else if steps.is_empty() {
                return TestResult::skipped("Skipped: no test script in package.json", None);
            }
        }
        Stack::Python => {
            if proc::which("python3").is_none() {
                return missing("python3", "python");
            }
            steps.push((
                "python3".into(),
                vec!["-m".into(), "compileall".into(), "-q".into(), ".".into()],
            ));
            let has_pytest = proc::run(
                "python3",
                &["-m", "pytest", "--version"],
                &Opts::default_timeout(),
            )
            .ok();
            if has_pytest {
                steps.push((
                    "python3".into(),
                    vec!["-m".into(), "pytest".into(), "-q".into()],
                ));
            }
        }
        Stack::Go => {
            if proc::which("go").is_none() {
                return missing("go", "go");
            }
            steps.push(("go".into(), vec!["vet".into(), "./...".into()]));
            steps.push(("go".into(), vec!["test".into(), "./...".into()]));
        }
        Stack::Other => {
            return TestResult::skipped("Skipped: no tests for this kind of project", None)
        }
    }
    let mut ran = Vec::new();
    for (prog, args) in &steps {
        let refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
        let o = run_step(dir, prog, &refs, env);
        transcript.push_str(&format!("$ {}\n{}\n", o.command_line(), o.combined()));
        ran.push(o.command_line());
        if o.cancelled {
            return finish(TestStatus::Cancelled, "Cancelled".into(), &transcript);
        }
        // pytest exit 5 = no tests collected: the syntax check still passed.
        let pytest_empty =
            prog == "python3" && args.contains(&"pytest".to_string()) && o.code == Some(5);
        if pytest_empty {
            continue;
        }
        if prog == "npx" && !o.ok() && o.combined().contains("could not determine executable") {
            let mut r = finish(
                TestStatus::Skipped,
                "Skipped: TypeScript isn't installed in this project".into(),
                &transcript,
            );
            r.hint = Some("npm install --save-dev typescript".into());
            return r;
        }
        if !o.ok() {
            let what = if o.timed_out { "timed out" } else { "failed" };
            let counts = result_summary(&transcript)
                .map(|c| format!(" · {}", with_failed_names(c, &transcript)))
                .unwrap_or_default();
            return finish(
                TestStatus::Failed,
                format!("{} {what}{counts}", o.command_line()),
                &transcript,
            );
        }
    }
    let counts = result_summary(&transcript)
        .map(|c| format!(" · {c}"))
        .unwrap_or_default();
    finish(
        TestStatus::Passed,
        format!("Passed: {}{counts}", ran.join(", ")),
        &transcript,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(log: &Logger) -> TestEnv<'_> {
        TestEnv {
            timeout_secs: 60,
            cancel: Arc::new(AtomicBool::new(false)),
            pass_env: false,
            log,
            build_cache: None,
            match_github: true,
            exact: false,
            clean_home: true,
        }
    }

    #[test]
    fn python_pass_and_fail() {
        if proc::which("python3").is_none() {
            return;
        }
        let log = Logger::new(None);
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("main.py"), "print('ok')\n").unwrap();
        let r = run_tests(d.path(), Stack::Python, &env(&log));
        assert_eq!(r.status, TestStatus::Passed, "{}", r.tail);
        std::fs::write(d.path().join("broken.py"), "def oops(:\n").unwrap();
        let r2 = run_tests(d.path(), Stack::Python, &env(&log));
        assert_eq!(r2.status, TestStatus::Failed);
    }

    #[test]
    fn extracts_result_counts() {
        let cargo = "running 3 tests\ntest result: ok. 3 passed; 0 failed; 0 ignored\nrunning 51\ntest result: ok. 51 passed; 0 failed; 0 ignored\n";
        assert_eq!(
            result_summary(cargo).as_deref(),
            Some("54 passed, 0 failed")
        );
        let py = "....\n============ 4 passed, 1 failed in 0.12s ============\n";
        assert_eq!(result_summary(py).as_deref(), Some("4 passed, 1 failed"));
        assert_eq!(
            result_summary("Tests:       2 failed, 10 passed, 12 total").as_deref(),
            Some("2 failed, 10 passed, 12 total")
        );
        assert_eq!(
            result_summary("ok  \tgithub.com/x/y\t0.01s\nFAIL\tgithub.com/x/z").as_deref(),
            Some("1 package(s) passed, 1 failed")
        );
        assert_eq!(result_summary("nothing"), None);
    }

    #[test]
    fn names_failed_tests() {
        let cargo = "test a::ok_one ... ok\ntest instance::tests::lock_and_hand_off ... FAILED\ntest pipeline::tests::restage ... FAILED\n\nfailures:\n";
        assert_eq!(failed_tests(cargo), ["lock_and_hand_off", "restage"]);
        assert_eq!(
            with_failed_names("65 passed, 2 failed".into(), cargo),
            "65 passed, 2 failed: lock_and_hand_off, restage"
        );
        let py = "FAILED tests/test_x.py::test_parse - AssertionError\n";
        assert_eq!(failed_tests(py), ["test_parse"]);
        assert_eq!(failed_tests("--- FAIL: TestRoute (0.00s)\n"), ["TestRoute"]);
        let many: String = (0..8).map(|i| format!("test t{i} ... FAILED\n")).collect();
        assert!(with_failed_names("0 passed, 8 failed".into(), &many).ends_with("t4 (+3 more)"));
        assert_eq!(
            with_failed_names("3 passed, 0 failed".into(), "ok"),
            "3 passed, 0 failed"
        );
    }

    #[test]
    fn other_is_skipped_neutrally() {
        let log = Logger::new(None);
        let d = tempfile::tempdir().unwrap();
        let r = run_tests(d.path(), Stack::Other, &env(&log));
        assert_eq!(r.status, TestStatus::Skipped);
        assert!(r.passed_or_skipped());
    }

    #[test]
    fn runs_the_projects_github_steps() {
        let d = tempfile::tempdir().unwrap();
        let wf = d.path().join(".github/workflows");
        std::fs::create_dir_all(&wf).unwrap();
        std::fs::write(wf.join("ci.yml"), "on: [push]\nenv:\n  GREETING: hello\njobs:\n  test:\n    runs-on: ubuntu-latest\n    steps:\n      - uses: actions/checkout@v4\n      - name: Check greeting\n        run: test \"$GREETING\" = hello && test \"$CI\" = true\n      - run: sudo apt-get install foo\n").unwrap();
        std::fs::write(d.path().join("main.py"), "print(1)\n").unwrap();
        let log = Logger::new(None);
        let r = run_tests(d.path(), Stack::Python, &env(&log));
        assert_eq!(r.status, TestStatus::Passed, "{} {}", r.summary, r.tail);
        assert!(
            r.summary.contains("Ran 1 of 2 GitHub steps here"),
            "{}",
            r.summary
        );
        assert!(r.tail.contains("installs system packages"), "{}", r.tail);
        // A failing GitHub step fails the check, named.
        std::fs::write(wf.join("ci.yml"), "on: [push]\njobs:\n  t:\n    runs-on: ubuntu-latest\n    steps:\n      - name: Lint\n        run: exit 3\n").unwrap();
        let r = run_tests(d.path(), Stack::Python, &env(&log));
        assert_eq!(r.status, TestStatus::Failed);
        assert!(
            r.summary.contains("GitHub step \"Lint\" failed"),
            "{}",
            r.summary
        );
    }

    fn qt_like_project(check_step: &str) -> tempfile::TempDir {
        let d = tempfile::tempdir().unwrap();
        let wf = d.path().join(".github/workflows");
        std::fs::create_dir_all(&wf).unwrap();
        std::fs::write(
            wf.join("ci.yml"),
            format!(
                "on: [push]\njobs:\n  test:\n    runs-on: ubuntu-latest\n    steps:\n      - uses: actions/checkout@v4\n      - uses: actions/setup-python@v5\n      - name: Earlier step\n        run: echo EARLIER-OUTPUT\n      - name: Install Qt libraries\n        run: sudo apt-get install -y libegl1 libxkbcommon0\n{check_step}      - name: Install Python packages\n        run: pip install --user -r requirements.txt\n      - name: Uses the environment\n        run: test -n \"$VIRTUAL_ENV\" && command -v python | grep -q stowlock-venv\n      - name: Tests\n        run: QT_QPA_PLATFORM=offscreen stowlock-missing-tool -q\n"
            ),
        )
        .unwrap();
        std::fs::write(d.path().join("requirements.txt"), "# nothing to install\n").unwrap();
        std::fs::write(d.path().join("app.py"), "print(1)\n").unwrap();
        d
    }

    #[test]
    fn a_step_needing_a_github_only_install_cant_run_here() {
        let d = qt_like_project(
            "      - name: Check Qt libraries\n        run: |\n          echo 'libEGL.so.1: cannot open shared object file'\n          exit 1\n",
        );
        let log = Logger::new(None);
        let r = run_tests(d.path(), Stack::Python, &env(&log));
        // The later steps still ran; the library check is a note, not a failure.
        assert_eq!(r.status, TestStatus::Passed, "{}", r.summary);
        assert!(r.summary.contains("can't run here"), "{}", r.summary);
        assert!(r.summary.contains("Install Qt libraries"), "{}", r.summary);
        assert!(
            r.tail.contains("Can't run here: [test] Check Qt libraries"),
            "{}",
            r.tail
        );
        assert!(
            r.hint.as_deref().unwrap_or("").contains("apt-get install"),
            "{:?}",
            r.hint
        );
        // Only the failing step's output in full; earlier steps as one line.
        assert!(!r.tail.contains("EARLIER-OUTPUT"), "{}", r.tail);
        assert!(r.tail.contains("Passed: [test] Earlier step"), "{}", r.tail);
        assert!(
            r.tail.contains("cannot open shared object file"),
            "{}",
            r.tail
        );
    }

    #[test]
    fn pip_installs_run_privately_and_a_missing_tool_cant_run_here() {
        if proc::run(
            "python3",
            &["-c", "import venv, ensurepip"],
            &Opts::default_timeout(),
        )
        .code
            != Some(0)
        {
            return;
        }
        let d = qt_like_project("");
        let cache = tempfile::tempdir().unwrap();
        let log = Logger::new(None);
        let mut e = env(&log);
        e.build_cache = Some(cache.path().to_path_buf());
        e.timeout_secs = 300;
        let r = run_tests(d.path(), Stack::Python, &e);
        // pip ran in StowLock's own environment, and later steps use it.
        assert!(
            cache.path().join("stowlock-venv/test/bin/python").exists(),
            "{} {}",
            r.summary,
            r.tail
        );
        assert!(
            r.tail.contains("Passed: [test] Install Python packages"),
            "{}",
            r.tail
        );
        assert!(
            r.tail.contains("Passed: [test] Uses the environment"),
            "{}",
            r.tail
        );
        // pytest isn't installed: can't run here, not "your tests failed".
        assert_eq!(r.status, TestStatus::Passed, "{}", r.summary);
        assert!(
            r.summary.contains("needs `stowlock-missing-tool`"),
            "{}",
            r.summary
        );
        assert!(
            r.summary.contains("Ran 3 of 5 GitHub steps here"),
            "{}",
            r.summary
        );
        assert!(install_hint("pytest").contains("sudo pacman -S python-pytest"));
    }

    #[test]
    fn a_real_failure_after_a_skipped_install_still_fails() {
        let d = qt_like_project(
            "      - name: Lint\n        run: echo 'style problem in app.py' && exit 3\n",
        );
        let log = Logger::new(None);
        let r = run_tests(d.path(), Stack::Python, &env(&log));
        assert_eq!(r.status, TestStatus::Failed, "{}", r.summary);
        assert!(
            r.summary.contains("GitHub step \"Lint\" failed"),
            "{}",
            r.summary
        );
        assert!(
            r.hint
                .as_deref()
                .unwrap_or("")
                .contains("only runs on GitHub"),
            "{:?}",
            r.hint
        );
    }

    #[test]
    fn names_the_missing_command() {
        assert_eq!(
            missing_command("bash: line 1: pytest: command not found\n").as_deref(),
            Some("pytest")
        );
        assert_eq!(missing_command("all good").as_deref(), None);
    }

    fn one_step_project(run: &str) -> tempfile::TempDir {
        let d = tempfile::tempdir().unwrap();
        let wf = d.path().join(".github/workflows");
        std::fs::create_dir_all(&wf).unwrap();
        std::fs::write(
            wf.join("ci.yml"),
            format!("on: [push]\njobs:\n  test:\n    runs-on: ubuntu-latest\n    steps:\n      - name: Install system libraries\n        run: sudo apt-get install -y libegl1\n      - name: Tests\n        run: |\n{}\n", run.lines().map(|l| format!("          {l}")).collect::<Vec<_>>().join("\n")),
        )
        .unwrap();
        std::fs::write(d.path().join("app.py"), "print(1)\n").unwrap();
        d
    }

    #[test]
    fn github_steps_get_a_fresh_home_without_the_desktop_session() {
        std::env::set_var("WAYLAND_DISPLAY", "wayland-test-0");
        let real = std::env::var("HOME").unwrap();
        // Where the real Rust cache is (kept as it is, even in a fresh home).
        let cargo_home = std::env::var("CARGO_HOME").unwrap_or_else(|_| format!("{real}/.cargo"));
        let d = one_step_project(&format!(
            "test \"$HOME\" != \"{real}\"\ntest -d \"$XDG_CONFIG_HOME\"\ntest -z \"${{WAYLAND_DISPLAY:-}}\"\ntest -z \"${{DBUS_SESSION_BUS_ADDRESS:-}}\"\ntest \"$CARGO_HOME\" = \"{cargo_home}\""
        ));
        let cache = tempfile::tempdir().unwrap();
        let log = Logger::new(None);
        let mut e = env(&log);
        e.build_cache = Some(cache.path().to_path_buf());
        let r = run_tests(d.path(), Stack::Python, &e);
        assert_eq!(r.status, TestStatus::Passed, "{} {}", r.summary, r.tail);
        // Off: the real session is seen.
        let d = one_step_project("test -n \"${WAYLAND_DISPLAY:-}\"");
        e.clean_home = false;
        let r = run_tests(d.path(), Stack::Python, &e);
        assert_eq!(r.status, TestStatus::Passed, "{} {}", r.summary, r.tail);
    }

    #[test]
    fn failures_about_this_computer_are_flagged() {
        let d = one_step_project(
            "echo \"E  Left over: a demo program is still running (process 2206: /x/.venv/bin/python -m demo --minimized)\"\necho \"1 failed, 10 passed, 2 skipped in 0.52s\"\nexit 1",
        );
        let log = Logger::new(None);
        let r = run_tests(d.path(), Stack::Python, &env(&log));
        assert_eq!(r.status, TestStatus::Failed);
        assert!(r.summary.contains(MACHINE_SPECIFIC), "{}", r.summary);
        assert!(
            r.summary.contains("1 failed, 10 passed"),
            "pytest -q counts: {}",
            r.summary
        );
        assert!(
            r.hint.as_deref().unwrap_or("").contains("running copy"),
            "{:?}",
            r.hint
        );
    }

    #[test]
    fn a_panic_inside_a_rust_library_isnt_about_this_computer() {
        // From a real report: a test failed inside egui_kittest, whose source
        // lives under ~/.cargo/registry. That's a real failure, not this computer.
        let home = std::env::var("HOME").unwrap();
        let d = tempfile::tempdir().unwrap();
        let log = Logger::new(None);
        let e = env(&log);
        let lib = format!(
            "thread 'ui' panicked at {home}/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/egui_kittest-0.37.0/src/lib.rs:520:18:\n5 widget(s) lack accessible name"
        );
        assert!(!machine_specific(&lib, d.path(), &e), "{lib}");
        let mine = format!("Error: can't read {home}/Documents/settings.json");
        assert!(machine_specific(&mine, d.path(), &e), "{mine}");
    }

    #[test]
    fn no_install_note_when_the_tests_ran() {
        // Like the glass test: tests ran and mostly passed after a GitHub-only
        // install step, so that step clearly wasn't the cause.
        let d = one_step_project(
            "echo \"E  assert False\"\necho \"1 failed, 1158 passed, 6 skipped in 111.63s\"\nexit 1",
        );
        let log = Logger::new(None);
        let r = run_tests(d.path(), Stack::Python, &env(&log));
        assert_eq!(r.status, TestStatus::Failed);
        assert!(!r.summary.contains(MACHINE_SPECIFIC), "{}", r.summary);
        assert_eq!(r.hint, None, "{:?}", r.hint);
        // Nothing ran: the note still helps.
        let d = one_step_project("echo 'segfault while loading'\nexit 1");
        let r = run_tests(d.path(), Stack::Python, &env(&log));
        assert!(
            r.hint
                .as_deref()
                .unwrap_or("")
                .contains("Install system libraries"),
            "{:?}",
            r.hint
        );
    }

    #[test]
    fn a_missing_lint_tool_doesnt_stop_the_other_steps() {
        // Like the shellcheck report: tests pass, one lint step needs a tool
        // this computer doesn't have, and the steps after it still run.
        let d = tempfile::tempdir().unwrap();
        let wf = d.path().join(".github/workflows");
        std::fs::create_dir_all(&wf).unwrap();
        std::fs::write(
            wf.join("ci.yml"),
            "on: [push]\njobs:\n  test:\n    runs-on: ubuntu-latest\n    steps:\n      - name: Tests\n        run: |\n          echo 'test result: ok. 162 passed; 0 failed; 0 ignored'\n      - name: Check the install and release scripts\n        run: |\n          sh -n install.sh\n          stowlock-shellcheck-not-here install.sh\n      - name: Later step\n        run: touch later-step-ran\n",
        )
        .unwrap();
        std::fs::write(d.path().join("install.sh"), "#!/bin/sh\necho hi\n").unwrap();
        let log = Logger::new(None);
        let r = run_tests(d.path(), Stack::Rust, &env(&log));
        assert_eq!(r.status, TestStatus::Passed, "{}", r.summary);
        assert!(
            r.summary
                .starts_with("Passed: Ran 2 of 3 GitHub steps here"),
            "{}",
            r.summary
        );
        assert!(r.summary.contains("162 passed"), "{}", r.summary);
        assert!(
            r.summary
                .contains("1 step can't run here (needs `stowlock-shellcheck-not-here`"),
            "{}",
            r.summary
        );
        assert!(
            d.path().join("later-step-ran").exists(),
            "stopped at the missing tool"
        );
        assert_eq!(
            r.hint.as_deref(),
            Some("sudo pacman -S stowlock-shellcheck-not-here")
        );
    }

    fn check_script_project(script: &str) -> tempfile::TempDir {
        let d = tempfile::tempdir().unwrap();
        let wf = d.path().join(".github/workflows");
        std::fs::create_dir_all(&wf).unwrap();
        std::fs::write(
            wf.join("ci.yml"),
            "on: [push]\njobs:\n  test:\n    name: Tests (Ubuntu)\n    runs-on: ubuntu-latest\n    steps:\n      - name: Run scripts/check.sh (shell scripts, fmt, clippy, tests)\n        run: sh scripts/check.sh\n",
        )
        .unwrap();
        std::fs::create_dir_all(d.path().join("scripts")).unwrap();
        std::fs::write(d.path().join("scripts/check.sh"), script).unwrap();
        d
    }

    #[test]
    fn a_check_script_that_stops_for_a_missing_tool_cant_run_here() {
        // Like the report: the project's own check.sh stops (exit 1, in red)
        // because shellcheck isn't installed.
        let d = check_script_project(
            "echo 'test result: ok. 162 passed; 0 failed; 0 ignored'\nprintf '\\033[1;31mCheck failed:\\033[0m shellcheck isn'\\''t installed. Install it with: pip install shellcheck-py==0.11.0.1\\n'\nexit 1\n",
        );
        let log = Logger::new(None);
        let r = run_tests(d.path(), Stack::Rust, &env(&log));
        assert_eq!(r.status, TestStatus::Skipped, "{}", r.summary);
        assert!(r.summary.contains("needs `shellcheck`"), "{}", r.summary);
        assert!(r.summary.contains("162 passed"), "{}", r.summary);
        assert_eq!(
            r.hint.as_deref(),
            Some("pip install shellcheck-py==0.11.0.1")
        );
        assert!(
            !r.tail.contains('\u{1b}'),
            "colour codes left in: {:?}",
            r.tail
        );
    }

    #[test]
    fn a_real_failure_with_a_missing_tool_message_still_fails() {
        let d = check_script_project(
            "echo 'test result: FAILED. 160 passed; 2 failed; 0 ignored'\necho 'cargo-audit is not installed'\nexit 1\n",
        );
        let log = Logger::new(None);
        let r = run_tests(d.path(), Stack::Rust, &env(&log));
        assert_eq!(r.status, TestStatus::Failed, "{}", r.summary);
    }

    #[test]
    fn names_tools_from_not_installed_messages() {
        assert_eq!(
            named_missing_tool("Check failed: shellcheck isn't installed. Install it with: pip install shellcheck-py==1.0.").0.as_deref(),
            Some("shellcheck")
        );
        assert_eq!(
            named_missing_tool("error: no such command: `audit`")
                .0
                .as_deref(),
            Some("cargo-audit")
        );
    }

    #[test]
    fn a_formatting_diff_that_mentions_a_missing_tool_still_fails() {
        // The diff's own text said "no such command"; that's code, not a
        // missing tool, and after a GitHub-only install step it must still fail.
        let d = one_step_project(
            "echo 'Diff in /p/src/x.rs at line 3:'\necho '-    named(\"error: no such command: audit\")'\nexit 1",
        );
        let log = Logger::new(None);
        let r = run_tests(d.path(), Stack::Rust, &env(&log));
        assert_eq!(r.status, TestStatus::Failed, "{}", r.summary);
        assert_eq!(r.hint, None, "{:?}", r.hint);
    }

    #[test]
    fn exit_code_3_means_tools_missing() {
        // The House Standard check.sh: tools first, every missing one listed,
        // exit 3 when only tools are missing.
        let d = check_script_project(
            "echo 'Missing tools (nothing was checked):'\necho '  - cargo-audit'\necho '  - rustup'\necho 'Install them with: sudo pacman -S --needed cargo-audit rustup'\nexit 3\n",
        );
        let log = Logger::new(None);
        let r = run_tests(d.path(), Stack::Rust, &env(&log));
        assert_eq!(r.status, TestStatus::Skipped, "{}", r.summary);
        assert!(r.summary.starts_with("Can't run here"), "{}", r.summary);
        assert_eq!(
            r.hint.as_deref(),
            Some("sudo pacman -S --needed cargo-audit rustup")
        );
        // Named from the script's own pacman line, not guessed.
        assert!(
            r.summary.contains("`cargo-audit`, `rustup`"),
            "{}",
            r.summary
        );
        // The list shows in full.
        assert!(
            r.tail.contains("- cargo-audit") && r.tail.contains("- rustup"),
            "{}",
            r.tail
        );
    }

    #[test]
    fn rustup_without_a_chosen_version_is_named() {
        let d = check_script_project(
            "echo 'error: rustup could not choose a version of cargo to run, because one wasn'\\''t specified explicitly, and no default is configured.'\nexit 1\n",
        );
        let log = Logger::new(None);
        let r = run_tests(d.path(), Stack::Rust, &env(&log));
        assert_eq!(r.status, TestStatus::Skipped, "{}", r.summary);
        assert!(
            r.summary.contains("rustup has none chosen"),
            "{}",
            r.summary
        );
        assert_eq!(r.hint.as_deref(), Some("rustup default stable"));
    }

    #[test]
    fn needed_tools_are_named_whatever_the_wording() {
        assert_eq!(
            named_missing_tool("FAILED: shellcheck 0.11.0 is needed (found: none). Install it with: pip install shellcheck-py==0.11.0.1").0.as_deref(),
            Some("shellcheck")
        );
        assert_eq!(
            named_missing_tool("Check failed: cargo-audit isn't installed. Install it with: cargo install cargo-audit --locked").1.as_deref(),
            Some("cargo install cargo-audit --locked")
        );
    }

    #[test]
    fn cargo_tools_you_have_arent_built_again() {
        // `sh` is on every computer, so this "install" must be skipped (and
        // needs no network), then the next step runs.
        let d = tempfile::tempdir().unwrap();
        let wf = d.path().join(".github/workflows");
        std::fs::create_dir_all(&wf).unwrap();
        std::fs::write(
            wf.join("ci.yml"),
            "on: [push]\njobs:\n  t:\n    runs-on: ubuntu-latest\n    steps:\n      - name: Install tools\n        run: |\n          cargo install sh --locked\n          echo \"$HOME/.cargo/bin\" >> \"$GITHUB_PATH\"\n      - name: Tests\n        run: test -n \"$CARGO_INSTALL_ROOT\" || echo ok\n",
        )
        .unwrap();
        let cache = tempfile::tempdir().unwrap();
        let log = Logger::new(None);
        let mut e = env(&log);
        e.build_cache = Some(cache.path().join("proj"));
        let r = run_tests(d.path(), Stack::Rust, &e);
        assert_eq!(r.status, TestStatus::Passed, "{} {}", r.summary, r.tail);
        assert!(
            r.tail
                .contains("Passed: [t] Install tools (here: StowLock's own tools folder)"),
            "{}",
            r.tail
        );
    }

    #[test]
    fn a_check_script_runs_when_the_workflow_cant() {
        // Only a container job (GitHub-only), but the project has check.sh.
        let d = tempfile::tempdir().unwrap();
        let wf = d.path().join(".github/workflows");
        std::fs::create_dir_all(&wf).unwrap();
        std::fs::write(
            wf.join("ci.yml"),
            "on: [push]\njobs:\n  arch:\n    runs-on: ubuntu-latest\n    container: archlinux:latest\n    steps:\n      - run: sh scripts/check.sh\n",
        )
        .unwrap();
        std::fs::create_dir_all(d.path().join("scripts")).unwrap();
        std::fs::write(
            d.path().join("scripts/check.sh"),
            "touch check-ran\necho 'test result: ok. 5 passed; 0 failed'\n",
        )
        .unwrap();
        let log = Logger::new(None);
        let r = run_tests(d.path(), Stack::Other, &env(&log));
        assert_eq!(r.status, TestStatus::Passed, "{} {}", r.summary, r.tail);
        assert!(d.path().join("check-ran").exists());
        assert!(
            r.summary.contains("\"arch\" runs only on GitHub"),
            "{}",
            r.summary
        );
        // Without check.sh: never "no tests", but what GitHub runs.
        std::fs::remove_file(d.path().join("scripts/check.sh")).unwrap();
        let r = run_tests(d.path(), Stack::Other, &env(&log));
        assert_eq!(r.status, TestStatus::Skipped);
        assert!(r.summary.contains("need GitHub"), "{}", r.summary);
    }

    #[test]
    fn a_lint_failure_is_a_real_failure() {
        // Lint errors have no error[E…] code; "could not compile" still makes
        // it a real failure, with no "comes after a GitHub-only step" note.
        let d = one_step_project(
            "echo 'error: this expression creates a reference which is immediately dereferenced'\necho 'error: could not compile `demo` (bin \"demo\" test) due to 1 previous error'\nexit 101",
        );
        let log = Logger::new(None);
        let r = run_tests(d.path(), Stack::Rust, &env(&log));
        assert_eq!(r.status, TestStatus::Failed, "{}", r.summary);
        assert_eq!(r.hint, None, "{:?}", r.hint);
    }

    #[test]
    fn exit_3_names_the_scripts_packages_not_an_earlier_step() {
        // Like the report: an "Install Rust" step only GitHub runs comes
        // first, but the script says the missing tool is shellcheck.
        let d = tempfile::tempdir().unwrap();
        let wf = d.path().join(".github/workflows");
        std::fs::create_dir_all(&wf).unwrap();
        std::fs::write(
            wf.join("ci.yml"),
            "on: [push]\njobs:\n  t:\n    runs-on: ubuntu-latest\n    steps:\n      - name: Install Rust (pinned)\n        run: rustup toolchain install stable\n      - name: Run scripts/check.sh\n        run: sh scripts/check.sh\n",
        )
        .unwrap();
        std::fs::create_dir_all(d.path().join("scripts")).unwrap();
        std::fs::write(
            d.path().join("scripts/check.sh"),
            "echo 'Missing tools:'\necho '    sudo pacman -S --needed shellcheck'\nexit 3\n",
        )
        .unwrap();
        let log = Logger::new(None);
        let r = run_tests(d.path(), Stack::Rust, &env(&log));
        assert!(r.summary.contains("needs `shellcheck`"), "{}", r.summary);
        assert!(!r.summary.contains("Install Rust"), "{}", r.summary);
    }

    #[test]
    fn reads_package_names_from_a_pacman_command() {
        assert_eq!(
            pacman_packages("sudo pacman -S --needed shellcheck cargo-audit"),
            ["shellcheck", "cargo-audit"]
        );
        assert_eq!(pacman_packages("sudo pacman -Syu"), Vec::<String>::new());
    }

    #[test]
    fn each_check_gets_a_fresh_python_environment() {
        if proc::run(
            "python3",
            &["-c", "import venv, ensurepip"],
            &Opts::default_timeout(),
        )
        .code
            != Some(0)
        {
            return;
        }
        let d = tempfile::tempdir().unwrap();
        let wf = d.path().join(".github/workflows");
        std::fs::create_dir_all(&wf).unwrap();
        std::fs::write(
            wf.join("ci.yml"),
            "on: [push]\njobs:\n  t:\n    runs-on: ubuntu-latest\n    steps:\n      - run: pip install -r requirements.txt\n      - name: Nothing left from last time\n        run: test ! -e \"$VIRTUAL_ENV/stale-from-last-check\"\n",
        )
        .unwrap();
        std::fs::write(d.path().join("requirements.txt"), "# none\n").unwrap();
        let cache = tempfile::tempdir().unwrap();
        let log = Logger::new(None);
        let mut e = env(&log);
        e.build_cache = Some(cache.path().to_path_buf());
        e.timeout_secs = 300;
        assert_eq!(
            run_tests(d.path(), Stack::Python, &e).status,
            TestStatus::Passed
        );
        // Something an earlier check left in the environment…
        std::fs::write(
            cache.path().join("stowlock-venv/t/stale-from-last-check"),
            "old build",
        )
        .unwrap();
        // …is gone by the next check.
        let r = run_tests(d.path(), Stack::Python, &e);
        assert_eq!(r.status, TestStatus::Passed, "{} {}", r.summary, r.tail);
    }

    #[test]
    fn installed_programs_come_last_in_checks() {
        let home = crate::util::home_dir();
        let p = proc::checks_path();
        let parts: Vec<&str> = p.split(':').collect();
        let local = home.join(".local/bin").display().to_string();
        let i = parts.iter().position(|s| *s == local).unwrap();
        let usr = parts.iter().position(|s| *s == "/usr/bin").unwrap();
        assert!(usr < i, "{p}");
    }

    #[test]
    fn a_failing_job_doesnt_stop_the_others_and_each_job_starts_clean() {
        // Like Sluice: the "test" job fails, the "rust" job still runs, and
        // the jobs don't share a home folder.
        let d = tempfile::tempdir().unwrap();
        let wf = d.path().join(".github/workflows");
        std::fs::create_dir_all(&wf).unwrap();
        std::fs::write(
            wf.join("ci.yml"),
            "on: [push]\njobs:\n  test:\n    runs-on: ubuntu-latest\n    steps:\n      - name: Leave a mark\n        run: touch \"$HOME/from-test-job\"\n      - name: Tests\n        run: |\n          echo '1 failed, 10 passed in 0.5s'\n          exit 1\n      - name: After the failure\n        run: touch after-failure-ran\n  rust:\n    runs-on: ubuntu-latest\n    steps:\n      - name: Engine tests\n        run: |\n          test ! -e \"$HOME/from-test-job\"\n          touch rust-job-ran\n          echo 'test result: ok. 5 passed; 0 failed'\n",
        )
        .unwrap();
        let cache = tempfile::tempdir().unwrap();
        let log = Logger::new(None);
        let mut e = env(&log);
        e.build_cache = Some(cache.path().to_path_buf());
        let r = run_tests(d.path(), Stack::Rust, &e);
        assert_eq!(r.status, TestStatus::Failed, "{}", r.summary);
        assert!(
            d.path().join("rust-job-ran").exists(),
            "the rust job didn't run: {}",
            r.tail
        );
        assert!(
            !d.path().join("after-failure-ran").exists(),
            "the failed job kept going"
        );
        assert!(
            r.summary.contains("GitHub step \"Tests\" failed"),
            "{}",
            r.summary
        );
        assert!(
            r.summary.contains("jobs: test failed, rust passed"),
            "{}",
            r.summary
        );
        assert!(
            r.tail.contains("Not run: [test] After the failure"),
            "{}",
            r.tail
        );
    }
}
