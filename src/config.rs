// SPDX-License-Identifier: GPL-3.0-or-later
//! Settings (`~/.config/stowlock/config.toml`) and the app's own folders.

use crate::util::{self, Ctx, Res};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Where StowLock's own releases live. Set once by the author at build time.
pub const UPDATE_REPO: &str = "Matt-B-007/StowLock";
pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

/// All of StowLock's own folders. Never inside the workspace.
#[derive(Debug, Clone)]
pub struct AppPaths {
    pub config_dir: PathBuf,
    pub state_dir: PathBuf,
    pub cache_dir: PathBuf,
}

impl AppPaths {
    /// The real locations, unless `STOWLOCK_ROOT` points somewhere else
    /// (used by the self-test sandbox and automated tests).
    pub fn detect() -> Self {
        if let Ok(root) = std::env::var("STOWLOCK_ROOT") {
            return Self::under(Path::new(&root));
        }
        let home = util::home_dir();
        AppPaths {
            config_dir: dirs::config_dir()
                .unwrap_or_else(|| home.join(".config"))
                .join("stowlock"),
            state_dir: dirs::state_dir()
                .unwrap_or_else(|| home.join(".local/state"))
                .join("stowlock"),
            cache_dir: dirs::cache_dir()
                .unwrap_or_else(|| home.join(".cache"))
                .join("stowlock"),
        }
    }

    /// Everything under one root folder (sandbox / tests).
    pub fn under(root: &Path) -> Self {
        AppPaths {
            config_dir: root.join("config"),
            state_dir: root.join("state"),
            cache_dir: root.join("cache"),
        }
    }

    pub fn config_file(&self) -> PathBuf {
        self.config_dir.join("config.toml")
    }
    pub fn projects_file(&self) -> PathBuf {
        self.state_dir.join("projects.json")
    }
    pub fn history_file(&self) -> PathBuf {
        self.state_dir.join("history.json")
    }
    pub fn health_file(&self) -> PathBuf {
        self.state_dir.join("health.json")
    }
    pub fn reports_dir(&self) -> PathBuf {
        self.state_dir.join("reports")
    }
    pub fn log_file(&self) -> PathBuf {
        self.state_dir.join("stowlock.log")
    }
    pub fn pending_dir(&self) -> PathBuf {
        self.state_dir.join("pending")
    }
    pub fn lock_file(&self) -> PathBuf {
        self.state_dir.join("instance.lock")
    }
    pub fn crash_marker(&self) -> PathBuf {
        self.state_dir.join("last-crash")
    }
    pub fn update_dir(&self) -> PathBuf {
        self.cache_dir.join("update")
    }

    /// Create all folders with private permissions (0700).
    pub fn ensure(&self) -> Res<()> {
        for d in [&self.config_dir, &self.state_dir, &self.cache_dir] {
            util::ensure_private_dir(d)?;
        }
        util::ensure_private_dir(&self.reports_dir())?;
        util::ensure_private_dir(&self.pending_dir())?;
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Config {
    pub workspace_dir: String,
    pub default_private: bool,
    pub archive_after_push: bool,
    /// "move" or "copy"
    pub archive_mode: String,
    pub run_tests_before_push: bool,
    pub test_timeout_secs: u64,
    pub block_on_secrets: bool,
    /// "none", "MIT" or "GPL-3.0"
    pub default_license: String,
    pub license_holder: String,
    pub fallback_git_name: String,
    pub fallback_git_email: String,
    pub tag_prefix: String,
    pub check_for_updates: bool,
    /// Check once a day whether a newer stable Rust is out (needs rustup).
    pub rust_update_check: bool,
    /// "stable", "beta", or "" = Beta when this build is itself a beta.
    pub update_channel: String,
    /// The user's own words to hide in bug reports.
    pub report_hide_words: Vec<String>,
    /// "light", "dark", or "" = follow the desktop.
    pub theme: String,
    /// Run the project's own GitHub workflow steps in Stage & check.
    pub match_github_checks: bool,
    /// Run the GitHub workflows exactly with act (needs act and Docker).
    pub exact_github_checks: bool,
    /// Run GitHub steps in a fresh home folder without the desktop session.
    pub clean_test_home: bool,
    pub report_retention_days: u32,
    /// Advanced: where StowLock checks for its own updates.
    pub update_repo: String,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            workspace_dir: "~/GitHub_Projects".into(),
            default_private: true,
            archive_after_push: true,
            archive_mode: "move".into(),
            run_tests_before_push: true,
            test_timeout_secs: 300,
            block_on_secrets: true,
            default_license: "none".into(),
            license_holder: String::new(),
            fallback_git_name: String::new(),
            fallback_git_email: String::new(),
            tag_prefix: "v".into(),
            check_for_updates: true,
            rust_update_check: true,
            update_channel: String::new(),
            report_hide_words: Vec::new(),
            theme: String::new(),
            match_github_checks: true,
            exact_github_checks: false,
            clean_test_home: true,
            report_retention_days: 30,
            update_repo: UPDATE_REPO.into(),
        }
    }
}

impl Config {
    pub fn load(paths: &AppPaths) -> Config {
        match std::fs::read_to_string(paths.config_file()) {
            Ok(text) => toml::from_str(&text).unwrap_or_else(|_| {
                let aside = paths
                    .config_file()
                    .with_extension(format!("corrupt-{}", util::now_stamp()));
                let _ = std::fs::rename(paths.config_file(), aside);
                Config::default()
            }),
            Err(_) => Config::default(),
        }
    }

    pub fn save(&self, paths: &AppPaths) -> Res<()> {
        let text = toml::to_string_pretty(self).ctx("encoding settings")?;
        util::ensure_private_dir(&paths.config_dir)?;
        util::atomic_write(&paths.config_file(), text.as_bytes())
    }

    pub fn workspace(&self) -> PathBuf {
        util::expand_tilde(&self.workspace_dir)
    }

    pub fn archive_root(&self) -> PathBuf {
        self.workspace().join("_archive")
    }

    /// Whether updates include betas: the chosen channel, or Beta when this
    /// build is itself a beta and no channel was chosen.
    pub fn beta_updates(&self) -> bool {
        match self.update_channel.as_str() {
            "beta" => true,
            "stable" => false,
            _ => APP_VERSION.contains('-'),
        }
    }

    pub fn move_archives(&self) -> bool {
        self.archive_mode != "copy"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_spec() {
        let c = Config::default();
        assert!(c.default_private);
        assert_eq!(c.archive_mode, "move");
        assert_eq!(c.test_timeout_secs, 300);
        assert_eq!(c.update_repo, "Matt-B-007/StowLock");
    }

    #[test]
    fn round_trips_and_keeps_unknown_defaults() {
        let d = tempfile::tempdir().unwrap();
        let paths = AppPaths::under(d.path());
        let c = Config {
            workspace_dir: "/tmp/ws".into(),
            ..Config::default()
        };
        c.save(&paths).unwrap();
        let back = Config::load(&paths);
        assert_eq!(back, c);
        // A partial file fills in defaults.
        std::fs::write(paths.config_file(), "default_private = false\n").unwrap();
        let partial = Config::load(&paths);
        assert!(!partial.default_private);
        assert_eq!(partial.archive_mode, "move");
    }
}
