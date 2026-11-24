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
}

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
    let opts = Opts {
        cwd: Some(dir.to_path_buf()),
        timeout: Some(Duration::from_secs(env.timeout_secs)),
        cancel: Some(env.cancel.clone()),
        scrub_env: !env.pass_env,
        env: {
            let mut e = vec![("CI".into(), "true".into())];
            if let Some(c) = &env.build_cache {
                e.push(("CARGO_TARGET_DIR".into(), c.display().to_string()));
            }
            e
        },
        stdin: None,
    };
    let o = proc::run(program, args, &opts);
    env.log.command("tests", &o);
    o
}

fn missing(tool: &str, pkg: &str) -> TestResult {
    TestResult::skipped(
        &format!("Skipped: {tool} not installed"),
        Some(&format!("sudo pacman -S {pkg}")),
    )
}

/// Run the checks for a stack. Steps stop at the first failure.
pub fn run_tests(dir: &Path, stack: Stack, env: &TestEnv) -> TestResult {
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
}
