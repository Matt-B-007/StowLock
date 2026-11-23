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

#[derive(Default)]
struct Inner {
    entries: VecDeque<LogEntry>,
    file: Option<PathBuf>,
    steps: Vec<Step>,
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
            let lines: Vec<&str> = text.lines().collect();
            let start = lines.len().saturating_sub(200);
            for l in &lines[start..] {
                self.log(lvl, stage, &format!("  {l}"));
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

    pub fn begin_run(&self) {
        let mut g = self.inner.lock().unwrap();
        g.steps.clear();
        g.failing_command = None;
    }

    pub fn step(&self, name: &str, status: StepStatus, millis: u128, detail: &str) {
        let mut g = self.inner.lock().unwrap();
        if let Some(s) = g.steps.iter_mut().find(|s| s.name == name) {
            s.status = status;
            s.millis = millis;
            s.detail = redact_secrets(detail);
        } else {
            g.steps.push(Step {
                name: name.into(),
                status,
                millis,
                detail: redact_secrets(detail),
            });
        }
    }

    pub fn steps(&self) -> Vec<Step> {
        self.inner.lock().unwrap().steps.clone()
    }

    pub fn failing_command(&self) -> Option<(String, Option<i32>)> {
        self.inner.lock().unwrap().failing_command.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        l.begin_run();
        l.step("Stage", StepStatus::Running, 0, "");
        l.step("Stage", StepStatus::Passed, 12, "ok");
        assert_eq!(l.steps().len(), 1);
        assert_eq!(l.steps()[0].status, StepStatus::Passed);
    }
}
