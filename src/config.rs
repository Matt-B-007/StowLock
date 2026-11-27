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
    /// Which settings layout this file uses (migrated on load when older).
    pub settings_version: u32,
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
    /// Tell me when a push or GitHub's build finishes (when StowLock isn't in front).
    pub desktop_notifications: bool,
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
    /// Run GitHub steps with only the fonts GitHub's machine has (DejaVu).
    pub github_fonts: bool,
    pub report_retention_days: u32,
    /// Advanced: where StowLock checks for its own updates.
    pub update_repo: String,
    /// The tour has been shown (it starts by itself only on a new install).
    pub tour_seen: bool,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            settings_version: SETTINGS_VERSION,
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
            desktop_notifications: true,
            update_channel: String::new(),
            report_hide_words: Vec::new(),
            theme: String::new(),
            match_github_checks: true,
            exact_github_checks: false,
            clean_test_home: true,
            github_fonts: true,
            report_retention_days: 30,
            update_repo: UPDATE_REPO.into(),
            tour_seen: false,
        }
    }
}

/// The settings layout this build writes. Raise it (and add a step to
/// `migrate`) whenever a setting is renamed or its meaning changes.
pub const SETTINGS_VERSION: u32 = 2;
/// Keep a backup of the settings at most this often, and this many of them.
const BACKUP_EVERY_SECS: u64 = 15 * 60;
const BACKUPS_KEPT: usize = 5;

/// Bring an older settings file up to date, one version at a time.
fn migrate(mut t: toml::Table) -> toml::Table {
    let mut v = t
        .get("settings_version")
        .and_then(|v| v.as_integer())
        .unwrap_or(0) as u32;
    while v < SETTINGS_VERSION {
        match v {
            // 0 → 1: files from before versioning; every setting kept as is.
            0 => {}
            // 1 → 2: the tour. Someone upgrading already knows the app, so
            // it doesn't start by itself for them.
            1 => {
                t.insert("tour_seen".into(), toml::Value::Boolean(true));
            }
            _ => {}
        }
        v += 1;
    }
    t.insert(
        "settings_version".into(),
        toml::Value::Integer(SETTINGS_VERSION as i64),
    );
    t
}

fn parse(text: &str) -> Option<Config> {
    let table: toml::Table = toml::from_str(text).ok()?;
    migrate(table).try_into().ok()
}

impl Config {
    fn backup_dir(paths: &AppPaths) -> PathBuf {
        paths.config_dir.join("backups")
    }

    /// The settings backups, newest first.
    pub fn backups(paths: &AppPaths) -> Vec<PathBuf> {
        let mut v: Vec<PathBuf> = std::fs::read_dir(Self::backup_dir(paths))
            .map(|rd| rd.flatten().map(|e| e.path()).collect())
            .unwrap_or_default();
        v.retain(|p| p.extension().is_some_and(|e| e == "toml"));
        v.sort();
        v.reverse();
        v
    }

    pub fn load(paths: &AppPaths) -> Config {
        let Ok(text) = std::fs::read_to_string(paths.config_file()) else {
            return Config::default();
        };
        if let Some(c) = parse(&text) {
            return c;
        }
        // Damaged: set it aside, and use the newest backup that still reads.
        let aside = paths
            .config_file()
            .with_extension(format!("corrupt-{}", util::now_stamp()));
        let _ = std::fs::rename(paths.config_file(), aside);
        Self::backups(paths)
            .iter()
            .find_map(|b| parse(&std::fs::read_to_string(b).ok()?))
            .unwrap_or_default()
    }

    pub fn save(&self, paths: &AppPaths) -> Res<()> {
        let text = toml::to_string_pretty(self).ctx("encoding settings")?;
        util::ensure_private_dir(&paths.config_dir)?;
        self.backup_if_due(paths);
        // Written to a temporary file, then renamed over the old one.
        util::atomic_write(&paths.config_file(), text.as_bytes())
    }

    /// Copy the current settings file to backups/ (at most every 15 minutes,
    /// keeping the newest 5).
    fn backup_if_due(&self, paths: &AppPaths) {
        let current = paths.config_file();
        if !current.exists() {
            return;
        }
        let dir = Self::backup_dir(paths);
        let backups = Self::backups(paths);
        let recent = backups
            .first()
            .and_then(|b| b.metadata().ok()?.modified().ok());
        let due = recent
            .and_then(|t| t.elapsed().ok())
            .is_none_or(|age| age.as_secs() >= BACKUP_EVERY_SECS);
        if !due || util::ensure_private_dir(&dir).is_err() {
            return;
        }
        let name = format!("config-{}.toml", util::now_stamp_secs());
        if std::fs::copy(&current, dir.join(name)).is_ok() {
            for old in Self::backups(paths).iter().skip(BACKUPS_KEPT) {
                let _ = std::fs::remove_file(old);
            }
        }
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

    #[test]
    fn old_settings_are_migrated_and_backups_rotate() {
        let d = tempfile::tempdir().unwrap();
        let paths = AppPaths::under(d.path());
        std::fs::create_dir_all(&paths.config_dir).unwrap();
        // A file from before versioning: settings kept, version stamped.
        std::fs::write(paths.config_file(), "workspace_dir = \"~/Projects\"\n").unwrap();
        let c = Config::load(&paths);
        assert_eq!(c.workspace_dir, "~/Projects");
        assert_eq!(c.settings_version, SETTINGS_VERSION);
        // Upgraders don't get the tour by themselves; a new install does.
        assert!(c.tour_seen);
        assert!(!Config::default().tour_seen);
        // Saving keeps one backup, not one per save.
        c.save(&paths).unwrap();
        c.save(&paths).unwrap();
        assert_eq!(Config::backups(&paths).len(), 1);
        // A damaged file falls back to the newest backup.
        std::fs::write(paths.config_file(), "this is = = not toml").unwrap();
        assert_eq!(Config::load(&paths).workspace_dir, "~/Projects");
    }
}
