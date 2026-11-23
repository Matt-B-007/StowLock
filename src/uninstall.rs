// SPDX-License-Identifier: GPL-3.0-or-later
//! Three uninstall levels. Levels 1–2 never touch the workspace. Level 3
//! (Everything) checks for unpushed work, needs typed confirmation, moves the
//! workspace to the Trash, and refuses dangerous workspace locations.

use crate::config::{AppPaths, Config};
use crate::git;
use crate::install::InstallPaths;
use crate::proc::{self, Opts};
use crate::state::{ProjectsState, Status};
use crate::util::{self, Res};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    AppOnly,
    AppAndData,
    Everything,
}

impl Level {
    pub fn label(&self) -> &'static str {
        match self {
            Level::AppOnly => "App only",
            Level::AppAndData => "App and data",
            Level::Everything => "Everything",
        }
    }
}

#[derive(Debug, Clone)]
pub struct PlanItem {
    pub label: String,
    pub path: PathBuf,
    pub files: u64,
    pub bytes: u64,
    pub to_trash: bool,
}

#[derive(Debug, Clone)]
pub struct Plan {
    pub level: Level,
    pub items: Vec<PlanItem>,
    /// Binary installed by pacman: remove it with this command instead.
    pub pacman_owned: Option<String>,
    /// Projects with work that isn't on GitHub (Everything only).
    pub unpushed: Vec<String>,
    /// Reasons Everything is refused outright.
    pub refused: Vec<String>,
    pub workspace: PathBuf,
}

impl Plan {
    pub fn describe(&self) -> String {
        let mut s = format!("StowLock uninstall: {}\n\n", self.level.label());
        if let Some(cmd) = &self.pacman_owned {
            s.push_str(&format!(
                "The program was installed by pacman. Remove it with: {cmd}\n\n"
            ));
        }
        s.push_str("Will remove:\n");
        for i in &self.items {
            let how = if i.to_trash { " → Trash" } else { "" };
            s.push_str(&format!(
                "  {}  {} ({} files, {}){how}\n",
                i.label,
                util::tilde(&i.path),
                i.files,
                util::human_size(i.bytes)
            ));
        }
        if self.level != Level::Everything {
            s.push_str(&format!(
                "\nYour projects and archived zips in {} are not touched.\n",
                util::tilde(&self.workspace)
            ));
        }
        s.push_str("Never touched: your repos on GitHub, git and gh, your GitHub sign-in, your git settings.\n");
        if !self.unpushed.is_empty() {
            s.push_str(&format!(
                "\nThese {} project(s) have work that isn't on GitHub and would be lost:\n",
                self.unpushed.len()
            ));
            for p in &self.unpushed {
                s.push_str(&format!("  - {p}\n"));
            }
        }
        for r in &self.refused {
            s.push_str(&format!("\nREFUSED: {r}\n"));
        }
        s
    }
}

fn item(label: &str, path: &Path, to_trash: bool) -> Option<PlanItem> {
    if !path.exists() {
        return None;
    }
    let (bytes, files) = if path.is_dir() {
        util::dir_stats(path)
    } else {
        (std::fs::metadata(path).map(|m| m.len()).unwrap_or(0), 1)
    };
    Some(PlanItem {
        label: label.into(),
        path: path.to_path_buf(),
        files,
        bytes,
        to_trash,
    })
}

/// Is this workspace location too dangerous to delete?
pub fn workspace_refusals(ws: &Path, home: &Path) -> Vec<String> {
    let mut v = Vec::new();
    let canon = std::fs::canonicalize(ws).unwrap_or_else(|_| ws.to_path_buf());
    let home_c = std::fs::canonicalize(home).unwrap_or_else(|_| home.to_path_buf());
    if canon == Path::new("/") {
        v.push("The workspace is set to / (the whole system).".into());
    } else if canon == home_c {
        v.push("The workspace is set to your home folder.".into());
    } else if home_c.starts_with(&canon) {
        v.push(format!(
            "The workspace ({}) contains your home folder.",
            canon.display()
        ));
    }
    if v.is_empty() && canon.is_dir() {
        let odd: Vec<String> = std::fs::read_dir(&canon)
            .map(|rd| {
                rd.flatten()
                    .filter(|e| {
                        let n = e.file_name().to_string_lossy().to_string();
                        let is_dir = e.path().is_dir() && !e.path().is_symlink();
                        !(is_dir || n.starts_with(".stowlock-"))
                    })
                    .map(|e| e.file_name().to_string_lossy().to_string())
                    .collect()
            })
            .unwrap_or_default();
        if !odd.is_empty() {
            v.push(format!(
                "The workspace holds things that aren't projects: {}. Move them out first.",
                odd.join(", ")
            ));
        }
    }
    v
}

/// Projects whose work isn't fully on GitHub.
pub fn unpushed_projects(paths: &AppPaths, ws: &Path) -> Vec<String> {
    let state = ProjectsState::load(paths);
    let mut v = Vec::new();
    let Ok(rd) = std::fs::read_dir(ws) else {
        return v;
    };
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        if name.starts_with('.') || name == "_archive" || !e.path().is_dir() {
            continue;
        }
        let dir = e.path();
        let status_ok = git::git(&dir, &["status", "--porcelain"]).ok();
        let reason = if !git::is_repo(&dir) {
            Some("not a git repo")
        } else if !status_ok {
            Some("couldn't read its git status")
        } else if !git::status_lines(&dir).is_empty() {
            Some("uncommitted changes")
        } else if !git::has_commits(&dir) {
            Some("no commits yet")
        } else if git::unpushed_commits(&dir) != Some(0) {
            Some("commits not on GitHub")
        } else if state.get(&name).status == Status::Staged {
            Some("staged, not pushed")
        } else {
            None
        };
        if let Some(r) = reason {
            v.push(format!("{name} ({r})"));
        }
    }
    v.sort();
    v
}

pub fn pacman_owner(exe: &Path) -> Option<String> {
    proc::which("pacman")?;
    let o = proc::run(
        "pacman",
        &["-Qqo", &exe.display().to_string()],
        &Opts::default_timeout().with_timeout(15),
    );
    if o.ok() {
        Some(o.stdout_trim())
    } else {
        None
    }
}

pub fn plan(
    level: Level,
    paths: &AppPaths,
    config: &Config,
    inst: &InstallPaths,
    home: &Path,
) -> Plan {
    let mut items = Vec::new();
    let exe = std::env::current_exe().ok();
    let pacman_owned = exe
        .as_deref()
        .and_then(pacman_owner)
        .map(|pkg| format!("sudo pacman -R {pkg}"));
    items.extend(item("Launcher entry", &inst.desktop, false));
    items.extend(item("Icon", &inst.icon, false));
    items.extend(item("Cache", &paths.cache_dir, false));
    if level != Level::AppOnly {
        items.extend(item("Settings", &paths.config_dir, false));
        items.extend(item("History, logs and reports", &paths.state_dir, false));
    }
    let ws = config.workspace();
    let mut unpushed = Vec::new();
    let mut refused = Vec::new();
    if level == Level::Everything {
        refused = workspace_refusals(&ws, home);
        if refused.is_empty() {
            unpushed = unpushed_projects(paths, &ws);
            items.extend(item("Projects and archived zips", &ws, true));
        }
    }
    if pacman_owned.is_none() {
        items.extend(item("Program", &inst.bin, false));
    }
    Plan {
        level,
        items,
        pacman_owned,
        unpushed,
        refused,
        workspace: ws,
    }
}

#[derive(Debug, Default)]
pub struct Outcome {
    pub removed: Vec<String>,
    pub trashed: Vec<String>,
    pub problems: Vec<String>,
}

/// Carry out a plan. `confirmed_delete` must be true for Everything;
/// `permanent` deletes the workspace instead of trashing it.
pub fn execute(plan: &Plan, confirmed_delete: bool, permanent: bool) -> Res<Outcome> {
    if plan.level == Level::Everything {
        if !plan.refused.is_empty() {
            return Err(plan.refused.join(" "));
        }
        if !confirmed_delete {
            return Err("Removing everything needs you to type DELETE to confirm.".into());
        }
    }
    let mut out = Outcome::default();
    // Program last.
    let mut ordered: Vec<&PlanItem> = plan.items.iter().filter(|i| i.label != "Program").collect();
    ordered.extend(plan.items.iter().filter(|i| i.label == "Program"));
    for i in ordered {
        if i.to_trash && !permanent {
            match util::move_to_trash(&i.path) {
                Ok(_) => out.trashed.push(util::tilde(&i.path)),
                Err(e) => out.problems.push(format!("{e} Nothing in it was deleted. Run again with permanent delete if you're sure.")),
            }
            continue;
        }
        let r = if i.path.is_dir() {
            std::fs::remove_dir_all(&i.path)
        } else {
            std::fs::remove_file(&i.path)
        };
        match r {
            Ok(_) => out.removed.push(util::tilde(&i.path)),
            Err(e) => out.problems.push(format!("{}: {e}", util::tilde(&i.path))),
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fx {
        _d: tempfile::TempDir,
        paths: AppPaths,
        config: Config,
        inst: InstallPaths,
        home: PathBuf,
    }

    fn fx() -> Fx {
        let d = tempfile::tempdir().unwrap();
        let home = d.path().join("home");
        let paths = AppPaths::under(&home.join("app"));
        paths.ensure().unwrap();
        std::fs::write(paths.config_file(), "x").unwrap();
        let ws = home.join("GitHub_Projects");
        std::fs::create_dir_all(ws.join("proj")).unwrap();
        std::fs::create_dir_all(ws.join("_archive/proj")).unwrap();
        let inst = InstallPaths::under(&home);
        std::fs::create_dir_all(inst.bin.parent().unwrap()).unwrap();
        std::fs::write(&inst.bin, "bin").unwrap();
        let config = Config {
            workspace_dir: ws.display().to_string(),
            ..Config::default()
        };
        Fx {
            _d: d,
            paths,
            config,
            inst,
            home,
        }
    }

    #[test]
    fn levels_one_and_two_never_touch_workspace() {
        let f = fx();
        for level in [Level::AppOnly, Level::AppAndData] {
            let p = plan(level, &f.paths, &f.config, &f.inst, &f.home);
            assert!(
                p.items
                    .iter()
                    .all(|i| !i.path.starts_with(f.config.workspace())),
                "{level:?}"
            );
            assert!(p.describe().contains("not touched"));
        }
        let p2 = plan(Level::AppAndData, &f.paths, &f.config, &f.inst, &f.home);
        assert!(p2.items.iter().any(|i| i.path == f.paths.config_dir));
        let out = execute(&p2, false, false).unwrap();
        assert!(out.problems.is_empty(), "{:?}", out.problems);
        assert!(!f.paths.config_dir.exists() && !f.inst.bin.exists());
        assert!(f.config.workspace().join("proj").exists());
    }

    #[test]
    fn everything_needs_confirmation_and_flags_unpushed() {
        let f = fx();
        let p = plan(Level::Everything, &f.paths, &f.config, &f.inst, &f.home);
        assert!(p.refused.is_empty(), "{:?}", p.refused);
        assert!(p.unpushed.iter().any(|u| u.starts_with("proj")));
        // A broken or empty git folder is flagged too.
        std::fs::create_dir_all(f.config.workspace().join("broken/.git")).unwrap();
        let p2 = plan(Level::Everything, &f.paths, &f.config, &f.inst, &f.home);
        assert!(
            p2.unpushed.iter().any(|u| u.starts_with("broken")),
            "{:?}",
            p2.unpushed
        );
        assert!(p.items.iter().any(|i| i.to_trash));
        assert!(execute(&p, false, false).is_err());
        assert!(f.config.workspace().exists());
    }

    #[test]
    fn refuses_dangerous_workspaces() {
        let f = fx();
        assert!(!workspace_refusals(Path::new("/"), &f.home).is_empty());
        assert!(!workspace_refusals(&f.home, &f.home).is_empty());
        let ws = f.config.workspace();
        std::fs::write(ws.join("notes.txt"), "mine").unwrap();
        assert!(workspace_refusals(&ws, &f.home)[0].contains("notes.txt"));
    }
}
