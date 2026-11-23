// SPDX-License-Identifier: GPL-3.0-or-later
//! Per-project app state (`projects.json`). Never written into a project.

use crate::config::AppPaths;
use crate::util::{self, Res};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Status {
    #[default]
    Unknown,
    Staged,
    Pushed,
    Archived,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct ProjectState {
    pub status: Status,
    pub run_command: Option<String>,
    pub source_zip: Option<String>,
    pub last_tag: Option<String>,
    pub last_channel: Option<String>,
    pub owner: Option<String>,
    /// Secret-scan hits the user marked "Not a secret" (hit ids).
    pub not_secret: Vec<String>,
    /// Pass the full environment to Run and tests (off by default).
    pub pass_env: bool,
    pub updated_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct ProjectsState {
    pub projects: BTreeMap<String, ProjectState>,
}

impl ProjectsState {
    pub fn load(paths: &AppPaths) -> Self {
        util::read_json_or_default(&paths.projects_file())
    }

    pub fn save(&self, paths: &AppPaths) -> Res<()> {
        util::ensure_private_dir(&paths.state_dir)?;
        util::write_json(&paths.projects_file(), self)
    }

    pub fn get(&self, slug: &str) -> ProjectState {
        self.projects.get(slug).cloned().unwrap_or_default()
    }

    /// True when StowLock staged this project and it hasn't been pushed since,
    /// so any loose files in its folder are StowLock's own work.
    pub fn staged_not_pushed(&self, slug: &str) -> bool {
        self.projects
            .iter()
            .find(|(k, _)| k.as_str() == slug)
            .or_else(|| {
                self.projects
                    .iter()
                    .find(|(k, _)| k.eq_ignore_ascii_case(slug))
            })
            .map(|(_, p)| p.status == Status::Staged)
            .unwrap_or(false)
    }

    /// Load, change one project, save. Safe to call from worker threads.
    pub fn update(paths: &AppPaths, slug: &str, f: impl FnOnce(&mut ProjectState)) -> Res<()> {
        let mut s = Self::load(paths);
        let p = s.projects.entry(slug.to_string()).or_default();
        f(p);
        p.updated_at = Some(util::now_iso());
        s.save(paths)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn update_persists() {
        let d = tempfile::tempdir().unwrap();
        let paths = AppPaths::under(d.path());
        ProjectsState::update(&paths, "sluice", |p| {
            p.status = Status::Staged;
            p.run_command = Some("cargo run".into());
        })
        .unwrap();
        let s = ProjectsState::load(&paths);
        assert_eq!(s.get("sluice").status, Status::Staged);
        assert_eq!(s.get("sluice").run_command.as_deref(), Some("cargo run"));
    }
}
