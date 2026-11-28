// SPDX-License-Identifier: GPL-3.0-or-later
//! Health view: per feature area, is it working? Fed by the self-test and by
//! the real outcome of each step. Persisted in `health.json`.

use crate::config::AppPaths;
use crate::util::{self, Res};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const AREAS: [&str; 16] = [
    "Tools & sign-in",
    "Zip intake",
    "Extraction",
    "Clean-up",
    "Stack detection",
    "Tests",
    "Secret scan",
    "New repo push",
    "Update sync",
    "Tags & releases",
    "Archive",
    "History",
    "Run/Editor",
    "Self-update",
    "Uninstall",
    "Settings & state",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Health {
    Working,
    NeedsAttention,
    Broken,
    #[default]
    NotUsed,
}

impl Health {
    pub fn label(&self) -> &'static str {
        match self {
            Health::Working => "Working",
            Health::NeedsAttention => "Needs attention",
            Health::Broken => "Broken",
            Health::NotUsed => "Not used yet",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct AreaStatus {
    pub status: Health,
    pub checked_at: String,
    /// "self-test" or "real use"
    pub source: String,
    pub hint: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct HealthState {
    pub areas: BTreeMap<String, AreaStatus>,
}

impl HealthState {
    pub fn load(paths: &AppPaths) -> Self {
        util::read_json_or_default(&paths.health_file())
    }
    pub fn save(&self, paths: &AppPaths) -> Res<()> {
        util::write_json(&paths.health_file(), self)
    }
    pub fn get(&self, area: &str) -> AreaStatus {
        self.areas.get(area).cloned().unwrap_or_default()
    }
    pub fn set(&mut self, area: &str, status: Health, source: &str, hint: &str) {
        self.areas.insert(
            area.to_string(),
            AreaStatus {
                status,
                checked_at: chrono::Local::now().format("%Y-%m-%d %H:%M").to_string(),
                source: source.into(),
                hint: hint.into(),
            },
        );
    }
    /// Record that something needs the user's attention (not broken).
    pub fn attention(paths: &AppPaths, area: &str, hint: &str) {
        let mut h = Self::load(paths);
        h.set(area, Health::NeedsAttention, "real use", hint);
        let _ = h.save(paths);
    }
    /// Record a real-use outcome and save.
    pub fn record(paths: &AppPaths, area: &str, ok: bool, hint: &str) {
        let mut h = Self::load(paths);
        h.set(
            area,
            if ok { Health::Working } else { Health::Broken },
            "real use",
            hint,
        );
        let _ = h.save(paths);
    }
    pub fn counts(&self) -> (usize, usize) {
        let broken = self
            .areas
            .values()
            .filter(|a| a.status == Health::Broken)
            .count();
        let attention = self
            .areas
            .values()
            .filter(|a| a.status == Health::NeedsAttention)
            .count();
        (broken, attention)
    }
    pub fn summary(&self) -> String {
        match self.counts() {
            (0, 0) => "All working".into(),
            (0, 1) => "1 area needs attention".into(),
            (0, a) => format!("{a} areas need attention"),
            (1, _) => "1 area is broken".into(),
            (b, _) => format!("{b} areas are broken"),
        }
    }
    pub fn lines(&self) -> Vec<String> {
        AREAS
            .iter()
            .map(|a| {
                let s = self.get(a);
                if s.status == Health::NotUsed {
                    format!("{a}: Not used yet")
                } else {
                    format!("{a}: {} ({}, {})", s.status.label(), s.source, s.checked_at)
                }
            })
            .collect()
    }
}

/// Does a failed step mean StowLock itself is broken? A project's failing
/// tests, a safety refusal ("unsaved changes", "tag already exists") or a
/// possible secret is StowLock doing its job, so the area stays Working.
pub fn counts_against_stowlock(step: &str, detail: &str) -> bool {
    let d = detail.to_ascii_lowercase();
    const REFUSALS: &[&str] = &[
        "unsaved changes",
        "already exists",
        "pick a new version",
        "pick another name",
        "isn't on github",
        "not a fast-forward",
        "diverged",
        "aren't on github",
        "too large",
        "too many files",
        "isn't a zip",
        "not a valid zip",
        "possible secret",
        "replace the existing folder",
        "not signed in",
        "sign in",
        "could not resolve host",
        "network",
    ];
    if REFUSALS.iter().any(|r| d.contains(r)) {
        return false;
    }
    if step == "Pre-flight tests" {
        // Only a test tool that couldn't start at all is StowLock's problem.
        return d.contains("command not found") || d.contains("couldn't start");
    }
    true
}

/// Every health area a pipeline step covers.
pub fn areas_for_step(step: &str, update_mode: bool) -> Vec<&'static str> {
    match step {
        "Extract & clean" => {
            let mut v = vec!["Zip intake", "Extraction", "Clean-up", "Stack detection"];
            if update_mode {
                v.push("Update sync");
            }
            v
        }
        "Pre-flight tests" => vec!["Tests"],
        "Secret scan" => vec!["Secret scan"],
        "Commit & push" => {
            if update_mode {
                vec!["Update sync"]
            } else {
                vec!["New repo push"]
            }
        }
        "Tag & release" => vec!["Tags & releases"],
        "Archive zip" => vec!["Zip intake", "Archive"],
        _ => vec![],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_problems_dont_break_stowlock() {
        assert!(!counts_against_stowlock(
            "Pre-flight tests",
            "cargo test --no-fail-fast failed · 65 passed, 2 failed"
        ));
        assert!(counts_against_stowlock(
            "Pre-flight tests",
            "cargo: command not found"
        ));
        assert!(!counts_against_stowlock(
            "Extract & clean",
            "This project has unsaved changes that aren't on GitHub."
        ));
        assert!(!counts_against_stowlock(
            "Commit & push",
            "The tag v1.0.0 already exists on GitHub."
        ));
        assert!(counts_against_stowlock(
            "Extract & clean",
            "Couldn't write the folder: permission denied"
        ));
    }

    #[test]
    fn records_and_summarises() {
        let d = tempfile::tempdir().unwrap();
        let paths = AppPaths::under(d.path());
        paths.ensure().unwrap();
        HealthState::record(&paths, "Archive", true, "");
        HealthState::record(&paths, "Tests", false, "cargo missing");
        let h = HealthState::load(&paths);
        assert_eq!(h.summary(), "1 area is broken");
        assert_eq!(h.get("Archive").status, Health::Working);
        assert_eq!(areas_for_step("Commit & push", true), vec!["Update sync"]);
        let ex = areas_for_step("Extract & clean", true);
        assert!(
            ex.contains(&"Extraction")
                && ex.contains(&"Stack detection")
                && ex.contains(&"Update sync")
        );
    }
}
