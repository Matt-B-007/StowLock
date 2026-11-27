// SPDX-License-Identifier: GPL-3.0-or-later
//! The log: structured entries in memory, mirrored to `stowlock.log`
//! (1 MB cap, one old file kept). Secrets are redacted at write time.
//! Also records the step timeline of the latest run for bug reports.

use crate::secrets::redact_secrets;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Level {
    Info,
    Warn,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogEntry {
    pub time: String,
    pub level: Level,
    pub stage: String,
    pub message: String,
}

impl LogEntry {
    pub fn line(&self) -> String {
        let lvl = match self.level {
            Level::Info => "INFO",
            Level::Warn => "WARN",
            Level::Error => "ERROR",
        };
        format!("{} {lvl:<5} [{}] {}", self.time, self.stage, self.message)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StepStatus {
    Running,
    Passed,
    Failed,
    Skipped,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Step {
    pub name: String,
    pub status: StepStatus,
    pub millis: u128,
    pub detail: String,
}

/// One run of the pipeline (stage + push, or an Archive Only), for reports.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Run {
    pub label: String,
    pub started: String,
    pub steps: Vec<Step>,
}

/// Progress lines that only add noise to the log (build tools).
fn is_progress(line: &str) -> bool {
    let t = line.trim_start();
    const WORDS: [&str; 12] = [
        "Compiling ",
        "Downloading ",
        "Downloaded ",
        "Checking ",
        "Fresh ",
        "Installing ",
        "Updating ",
        "Locking ",
        "Adding ",
        "Blocking ",
        "Building ",
        "Documenting ",
    ];
    WORDS.iter().any(|w| t.starts_with(w))
}

/// Lines worth keeping however long the output is (results, failures).
pub(crate) fn is_important(line: &str) -> bool {
    let l = line.trim().to_ascii_lowercase();
    // A passing test is never important, even if its name says "failed".
    if l.ends_with("... ok") || l.ends_with(" ok") && l.starts_with("test ") {
        return false;
    }
    l.contains("test result")
        || l.contains("error")
        || l.contains("warning:")
        || l.contains("failed")
        || l.contains("panicked")
        || l.starts_with("---- ")
        || l.starts_with("failures:")
        || l.contains(" passed")
        || l.starts_with("tests:")
        || l.starts_with("ok ")
        || l.starts_with("fail")
        || is_failure_message(&l)
}

/// Something the project needs isn't on this computer. These lines name a
/// tool, module or library, never the project's own code, so reports keep them.
pub(crate) fn is_missing_line(l: &str) -> bool {
    let l = l.to_ascii_lowercase();
    l.contains("command not found")
        || l.contains("no module named")
        || l.contains("modulenotfounderror")
        || l.contains("cannot open shared object file")
        || l.contains("missing librar")
        || (l.contains("not found") && l.contains("no such file"))
}

/// The lines that say *what* failed: assertion messages and their values.
fn is_failure_message(l: &str) -> bool {
    is_missing_line(l)
        || l.starts_with("assertion")
        || l.starts_with("left:")
        || l.starts_with("right:")
        || l.starts_with("e  ")
        || l.starts_with("expected")
        || l.starts_with("received")
        || l.contains("_test.go:")
        || l.starts_with("--- fail")
}

/// A line that means something went wrong (logged as a warning). Passing
/// results such as "test result: ok" are kept, but as normal lines.
pub(crate) fn is_failure_line(line: &str) -> bool {
    let l = line.trim().to_ascii_lowercase();
    if l.ends_with("... ok") || l.starts_with("test result: ok") {
        return false;
    }
    (l.contains("failed") && !l.contains(" 0 failed"))
        || l.contains("panicked")
        || l.contains("error")
        || l.starts_with("---- ")
        || l.starts_with("failures:")
        || is_failure_message(&l)
}

/// Which lines to keep from test output: the important ones, plus up to four
/// lines after each "panicked at" (the message saying what went wrong).
pub(crate) fn mark_important(lines: &[&str]) -> Vec<bool> {
    let mut keep = vec![false; lines.len()];
    let mut after_panic = 0usize;
    for (i, l) in lines.iter().enumerate() {
        let t = l.trim();
        if t.is_empty() {
            after_panic = 0;
        }
        if is_important(t) || (after_panic > 0 && !t.is_empty()) {
            keep[i] = true;
        }
        if t.contains("panicked at") {
            after_panic = 4;
        } else {
            after_panic = after_panic.saturating_sub(1);
        }
    }
    keep
}

/// Shorten long command output for the log: build progress lines collapse
/// into one summary line, then the first 10 and last 30 lines are kept,
/// plus any result or error lines in between.
pub fn shorten(text: &str) -> Vec<String> {
    let mut progress = 0usize;
    let mut rest: Vec<&str> = Vec::new();
    for l in text.lines() {
        if is_progress(l) {
            progress += 1;
        } else {
            rest.push(l);
        }
    }
    let mut out = Vec::new();
    if progress > 0 {
        out.push(format!(
            "[{progress} build progress line{} hidden]",
            if progress == 1 { "" } else { "s" }
        ));
    }
    if rest.len() <= 40 {
        out.extend(rest.iter().map(|s| s.to_string()));
        return out;
    }
    out.extend(rest[..10].iter().map(|s| s.to_string()));
    let middle = &rest[10..rest.len() - 30];
    let marks = mark_important(middle);
    let kept: Vec<&&str> = middle
        .iter()
        .zip(marks)
        .filter(|(_, k)| *k)
        .map(|(l, _)| l)
        .take(40)
        .collect();
    out.push(format!(
        "[{} line{} hidden]",
        middle.len() - kept.len(),
        if middle.len() - kept.len() == 1 {
            ""
        } else {
            "s"
        }
    ));
    out.extend(kept.into_iter().map(|s| s.to_string()));
    out.extend(rest[rest.len() - 30..].iter().map(|s| s.to_string()));
    out
}

#[derive(Default)]
struct Inner {
    entries: VecDeque<LogEntry>,
    file: Option<PathBuf>,
    runs: VecDeque<Run>,
    failing_command: Option<(String, Option<i32>)>,
}

#[derive(Clone, Default)]
pub struct Logger {
    inner: Arc<Mutex<Inner>>,
}

const MAX_ENTRIES: usize = 3000;
const MAX_FILE: u64 = 1024 * 1024;

impl Logger {
    pub fn new(file: Option<PathBuf>) -> Self {
        let l = Logger::default();
        l.inner.lock().unwrap().file = file;
        l
    }

    pub fn log(&self, level: Level, stage: &str, message: &str) {
        let entry = LogEntry {
            time: chrono::Local::now().format("%H:%M:%S").to_string(),
            level,
            stage: stage.to_string(),
            message: redact_secrets(message),
        };
        let mut g = self.inner.lock().unwrap();
        if let Some(path) = g.file.clone() {
            if std::fs::metadata(&path)
                .map(|m| m.len() > MAX_FILE)
                .unwrap_or(false)
            {
                let _ = std::fs::rename(&path, path.with_extension("log.old"));
            }
            if let Ok(mut f) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = f.set_permissions(std::fs::Permissions::from_mode(0o600));
                let date = chrono::Local::now().format("%Y-%m-%d");
                let _ = writeln!(f, "{date} {}", entry.line());
            }
        }
        g.entries.push_back(entry);
        while g.entries.len() > MAX_ENTRIES {
            g.entries.pop_front();
        }
    }

    pub fn info(&self, stage: &str, m: &str) {
        self.log(Level::Info, stage, m)
    }
    pub fn warn(&self, stage: &str, m: &str) {
        self.log(Level::Warn, stage, m)
    }
    pub fn error(&self, stage: &str, m: &str) {
        self.log(Level::Error, stage, m)
    }

    /// Log a finished command: the command line, then its output (trimmed).
    /// Log a command under a readable name (a GitHub step's name instead of
    /// its whole script).
    pub fn command_as(&self, stage: &str, label: &str, out: &crate::proc::Output) {
        let mut o = out.clone();
        o.program = label.to_string();
        o.args.clear();
        self.command(stage, &o);
    }

    pub fn command(&self, stage: &str, out: &crate::proc::Output) {
        let lvl = if out.ok() { Level::Info } else { Level::Warn };
        self.log(
            lvl,
            stage,
            &format!(
                "$ {}  (exit {:?}, {} ms)",
                out.command_line(),
                out.code,
                out.duration.as_millis()
            ),
        );
        let text = out.combined();
        if !text.is_empty() {
            for l in shorten(&text) {
                // Only failure lines are warnings; the rest is normal output.
                let line_lvl = if !out.ok() && is_failure_line(&l) {
                    Level::Warn
                } else {
                    Level::Info
                };
                self.log(line_lvl, stage, &format!("  {l}"));
            }
        }
        if !out.ok() {
            self.inner.lock().unwrap().failing_command = Some((out.command_line(), out.code));
        }
    }

    pub fn entries(&self) -> Vec<LogEntry> {
        self.inner.lock().unwrap().entries.iter().cloned().collect()
    }

    pub fn last_lines(&self, n: usize) -> Vec<String> {
        let g = self.inner.lock().unwrap();
        let start = g.entries.len().saturating_sub(n);
        g.entries.iter().skip(start).map(|e| e.line()).collect()
    }

    pub fn text(&self) -> String {
        self.entries()
            .iter()
            .map(|e| e.line())
            .collect::<Vec<_>>()
            .join("\n")
    }

    pub fn clear(&self) {
        self.inner.lock().unwrap().entries.clear();
    }

    // ----- step timeline of the latest run -----

    /// Start a new run (the last 3 are kept for bug reports).
    pub fn begin_run(&self, label: &str) {
        let mut g = self.inner.lock().unwrap();
        // A run that never got a step (e.g. a stage that was replaced by a
        // push straight away) is reused instead of leaving an empty entry.
        if let Some(last) = g.runs.back_mut() {
            if last.steps.is_empty() {
                last.label = label.to_string();
                last.started = chrono::Local::now().format("%H:%M").to_string();
                g.failing_command = None;
                return;
            }
        }
        g.runs.push_back(Run {
            label: label.to_string(),
            started: chrono::Local::now().format("%H:%M").to_string(),
            steps: Vec::new(),
        });
        while g.runs.len() > 3 {
            g.runs.pop_front();
        }
        g.failing_command = None;
    }

    /// Rename the current run (e.g. once a push adds a version).
    pub fn set_run_label(&self, label: &str) {
        if let Some(r) = self.inner.lock().unwrap().runs.back_mut() {
            r.label = label.to_string();
        }
    }

    pub fn step(&self, name: &str, status: StepStatus, millis: u128, detail: &str) {
        let mut g = self.inner.lock().unwrap();
        if g.runs.is_empty() {
            g.runs.push_back(Run {
                label: "Run".into(),
                started: chrono::Local::now().format("%H:%M").to_string(),
                steps: Vec::new(),
            });
        }
        let run = g.runs.back_mut().unwrap();
        if let Some(s) = run.steps.iter_mut().find(|s| s.name == name) {
            s.status = status;
            s.millis = millis;
            s.detail = redact_secrets(detail);
        } else {
            run.steps.push(Step {
                name: name.into(),
                status,
                millis,
                detail: redact_secrets(detail),
            });
        }
    }

    /// Steps of the current run.
    pub fn steps(&self) -> Vec<Step> {
        self.inner
            .lock()
            .unwrap()
            .runs
            .back()
            .map(|r| r.steps.clone())
            .unwrap_or_default()
    }

    /// The last (up to 3) runs, oldest first.
    pub fn runs(&self) -> Vec<Run> {
        self.inner.lock().unwrap().runs.iter().cloned().collect()
    }

    /// Warnings and errors among the last `n` entries, and the latest lines.
    pub fn report_lines(&self, problems: usize, latest: usize) -> (Vec<String>, Vec<String>) {
        let g = self.inner.lock().unwrap();
        let p: Vec<String> = g
            .entries
            .iter()
            .filter(|e| e.level != Level::Info)
            .map(|e| e.line())
            .collect();
        let p = p[p.len().saturating_sub(problems)..].to_vec();
        let start = g.entries.len().saturating_sub(latest);
        (p, g.entries.iter().skip(start).map(|e| e.line()).collect())
    }

    pub fn failing_command(&self) -> Option<(String, Option<i32>)> {
        self.inner.lock().unwrap().failing_command.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failing_test_survives_long_build_output() {
        // Like `cargo test`: test results on stdout, then 60 build lines on stderr.
        let mut t = String::from("running 3 tests\n");
        for i in 0..50 {
            t.push_str(&format!("test filler_{i} ... ok\n"));
        }
        t.push_str("test pipeline::tests::demo ... FAILED\n\nfailures:\n\n");
        t.push_str("---- pipeline::tests::demo stdout ----\n");
        t.push_str("thread 'pipeline::tests::demo' panicked at src/pipeline.rs:9:5:\n");
        t.push_str("test result: FAILED. 2 passed; 1 failed\n");
        for i in 0..60 {
            t.push_str(&format!("   Compiling crate{i} v1.0.0\n"));
        }
        t.push_str("error: test failed, to rerun pass `--bin stowlock`\n");
        let out = shorten(&t).join("\n");
        assert!(out.contains("---- pipeline::tests::demo stdout ----"));
        assert!(out.contains("panicked at src/pipeline.rs:9:5"));
        assert!(out.contains("test result: FAILED"));
        assert!(out.contains("60 build progress lines hidden"));
        assert!(!out.contains("Compiling crate"));
    }

    #[test]
    fn redacts_at_write_time() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("stowlock.log");
        let l = Logger::new(Some(f.clone()));
        let tok = format!("ghp_{}", "a".repeat(36));
        l.info("push", &format!("token is {tok}"));
        let file = std::fs::read_to_string(&f).unwrap();
        assert!(!file.contains(&tok));
        assert!(file.contains("[REDACTED]"));
        assert!(!l.text().contains(&tok));
    }

    #[test]
    fn records_steps() {
        let l = Logger::new(None);
        l.begin_run("Stage demo");
        l.step("Stage", StepStatus::Running, 0, "");
        l.step("Stage", StepStatus::Passed, 12, "ok");
        assert_eq!(l.steps().len(), 1);
        assert_eq!(l.steps()[0].status, StepStatus::Passed);
        for i in 0..4 {
            l.begin_run(&format!("run {i}"));
            l.step("Stage", StepStatus::Passed, 1, "");
        }
        let runs = l.runs();
        assert_eq!(runs.len(), 3, "last 3 runs kept");
        assert_eq!(runs[2].label, "run 3");
        // A run that never got a step is reused, not left empty.
        l.begin_run("Stage x");
        l.begin_run("Push x");
        let runs = l.runs();
        assert_eq!(runs.len(), 3);
        assert_eq!(runs[2].label, "Push x");
        assert_eq!(runs[1].label, "run 3");
    }

    #[test]
    fn long_build_output_is_shortened() {
        let mut text = String::new();
        for i in 0..400 {
            text.push_str(&format!("   Compiling crate{i} v1.0.0\n"));
        }
        for i in 0..100 {
            text.push_str(&format!("line {i}\n"));
            if i == 50 {
                text.push_str("test result: ok. 54 passed; 0 failed\n");
            }
        }
        let s = shorten(&text);
        assert!(s.len() < 70, "{}", s.len());
        assert!(s[0].contains("400 build progress lines hidden"));
        assert!(
            s.iter().any(|l| l.contains("test result: ok. 54 passed")),
            "results kept"
        );
        assert_eq!(shorten("a\nb").len(), 2);
    }
}
