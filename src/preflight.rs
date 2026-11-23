// SPDX-License-Identifier: GPL-3.0-or-later
//! Pre-flight tests: build/test commands per stack, with a timeout, cancel,
//! graceful handling of missing toolchains and dependencies, and a cleaned
//! environment (token-like variables removed) unless the user allows it.

use crate::detect::Stack;
use crate::logbook::Logger;
use crate::proc::{self, Opts, Output};
use std::path::Path;
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
    /// Last 60 lines of output.
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
}

fn tail(text: &str, n: usize) -> String {
    let lines: Vec<&str> = text.lines().collect();
    lines[lines.len().saturating_sub(n)..].join("\n")
}

fn run_step(dir: &Path, program: &str, args: &[&str], env: &TestEnv) -> Output {
    let opts = Opts {
        cwd: Some(dir.to_path_buf()),
        timeout: Some(Duration::from_secs(env.timeout_secs)),
        cancel: Some(env.cancel.clone()),
        scrub_env: !env.pass_env,
        env: vec![("CI".into(), "true".into())],
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
        tail: tail(transcript, 60),
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
            return finish(
                TestStatus::Failed,
                format!("{} {what}", o.command_line()),
                &transcript,
            );
        }
    }
    finish(
        TestStatus::Passed,
        format!("Passed: {}", ran.join(", ")),
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
    fn other_is_skipped_neutrally() {
        let log = Logger::new(None);
        let d = tempfile::tempdir().unwrap();
        let r = run_tests(d.path(), Stack::Other, &env(&log));
        assert_eq!(r.status, TestStatus::Skipped);
        assert!(r.passed_or_skipped());
    }
}
