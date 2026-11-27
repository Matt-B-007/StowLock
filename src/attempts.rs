// SPDX-License-Identifier: GPL-3.0-or-later
//! Earlier attempts StowLock set aside instead of throwing away: a git stash
//! (for an update that was never pushed) or a `.stowlock-old-…` folder (for a
//! new repo that was never pushed). Listed in Projects with Restore and Delete.

use crate::git;
use crate::util::{self, Ctx, Res};
use std::path::{Path, PathBuf};

/// The message every stash StowLock makes starts with.
pub const STASH_PREFIX: &str = "StowLock: unpushed staging saved";
/// Folders set aside in the workspace start with this.
pub const FOLDER_PREFIX: &str = ".stowlock-old-";

#[derive(Debug, Clone, PartialEq)]
pub enum Kept {
    /// A git stash in the project folder, found again by its message.
    Stash { dir: PathBuf, message: String },
    /// A whole folder set aside in the workspace.
    Folder { path: PathBuf },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Attempt {
    /// The project it belongs to.
    pub project: String,
    /// When it was set aside, as StowLock wrote it (YYYY-MM-DD-HHMMSS).
    pub when: String,
    pub kept: Kept,
}

impl Attempt {
    pub fn where_kept(&self) -> &'static str {
        match self.kept {
            Kept::Stash { .. } => "kept in git stash",
            Kept::Folder { .. } => "kept as a folder in your workspace",
        }
    }
}

/// Every attempt set aside in `workspace`, newest first.
pub fn list(workspace: &Path) -> Vec<Attempt> {
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(workspace) else {
        return out;
    };
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        let path = e.path();
        if !path.is_dir() {
            continue;
        }
        if let Some(rest) = name.strip_prefix(FOLDER_PREFIX) {
            // .stowlock-old-<project>-YYYY-MM-DD-HHMMSS
            let (project, when) = split_stamp(rest);
            out.push(Attempt {
                project,
                when,
                kept: Kept::Folder { path },
            });
            continue;
        }
        if name.starts_with('.') || name == "_archive" || !git::is_repo(&path) {
            continue;
        }
        for message in stash_messages(&path) {
            let when = message
                .split(STASH_PREFIX)
                .nth(1)
                .unwrap_or("")
                .trim()
                .to_string();
            out.push(Attempt {
                project: name.clone(),
                when,
                kept: Kept::Stash {
                    dir: path.clone(),
                    message,
                },
            });
        }
    }
    out.sort_by(|a, b| b.when.cmp(&a.when));
    out
}

/// "proj-name-2026-11-23-111822" -> ("proj-name", "2026-11-23-111822").
fn split_stamp(rest: &str) -> (String, String) {
    let parts: Vec<&str> = rest.rsplitn(5, '-').collect();
    if parts.len() == 5
        && parts[..4]
            .iter()
            .all(|p| p.chars().all(|c| c.is_ascii_digit()))
    {
        let when = format!("{}-{}-{}-{}", parts[3], parts[2], parts[1], parts[0]);
        return (parts[4].to_string(), when);
    }
    (rest.to_string(), String::new())
}

/// StowLock's own stash messages in a repo (other stashes are left alone).
fn stash_messages(dir: &Path) -> Vec<String> {
    git::git(dir, &["stash", "list", "--format=%gs"])
        .stdout
        .lines()
        .filter_map(|l| {
            // "On main: StowLock: unpushed staging saved …"
            l.find(STASH_PREFIX).map(|i| l[i..].trim().to_string())
        })
        .collect()
}

/// The current `stash@{n}` for a message (numbers shift as stashes change).
fn stash_ref(dir: &Path, message: &str) -> Option<String> {
    git::git(dir, &["stash", "list", "--format=%gd%x09%gs"])
        .stdout
        .lines()
        .find_map(|l| {
            let (r, subject) = l.split_once('\t')?;
            subject.ends_with(message).then(|| r.to_string())
        })
}

/// Put an attempt back. A stash goes back into its project folder (which must
/// have no unsaved changes); a folder goes back under its project's name (which
/// must be free). On success the attempt is no longer listed.
pub fn restore(a: &Attempt, workspace: &Path) -> Res<String> {
    match &a.kept {
        Kept::Stash { dir, message } => {
            if !git::status_lines(dir).is_empty() {
                return Err(format!(
                    "{} has unsaved changes. Push them or set them aside first, then restore.",
                    a.project
                ));
            }
            let r = stash_ref(dir, message).ok_or("That saved attempt is gone.")?;
            let o = git::git(dir, &["stash", "pop", &r]);
            if !o.ok() {
                return Err(format!("Couldn't restore it: {}", o.combined().trim()));
            }
            Ok(format!("Restored the earlier attempt into {}", a.project))
        }
        Kept::Folder { path } => {
            let dest = workspace.join(&a.project);
            if dest.exists() {
                return Err(format!(
                    "A folder named {} is already in your workspace. Move or delete it first, then restore.",
                    a.project
                ));
            }
            std::fs::rename(path, &dest).ctx("restoring the folder")?;
            Ok(format!("Restored {}", util::tilde(&dest)))
        }
    }
}

/// Throw an attempt away: a stash is dropped, a folder goes to the Trash
/// (via `trash`, so tests use a pretend one).
pub fn delete(a: &Attempt, trash: impl Fn(&Path) -> Res<String>) -> Res<String> {
    match &a.kept {
        Kept::Stash { dir, message } => {
            let r = stash_ref(dir, message).ok_or("That saved attempt is gone.")?;
            let o = git::git(dir, &["stash", "drop", &r]);
            if !o.ok() {
                return Err(format!("Couldn't delete it: {}", o.combined().trim()));
            }
            Ok("Deleted the saved attempt".into())
        }
        Kept::Folder { path } => {
            trash(path)?;
            Ok("Moved the saved attempt to the Trash".into())
        }
    }
}

/// The newest saved attempt is identical to the one before it (the same zip
/// staged again): drop the new copy, so retries don't pile up. True if dropped.
pub fn drop_if_duplicate(dir: &Path) -> bool {
    let out = |args: &[&str]| crate::git::git(dir, args).stdout.trim().to_string();
    let prev_msg = out(&["log", "-1", "--format=%s", "stash@{1}"]);
    if !prev_msg.contains(STASH_PREFIX) {
        return false;
    }
    let tree = |r: &str| out(&["rev-parse", "--verify", "-q", &format!("{r}^{{tree}}")]);
    let same_tracked = {
        let (a, b) = (tree("stash@{0}"), tree("stash@{1}"));
        !a.is_empty() && a == b
    };
    // Untracked files live in the stash's third parent (when there are any).
    let same_untracked = tree("stash@{0}^3") == tree("stash@{1}^3");
    if same_tracked && same_untracked {
        return crate::git::git(dir, &["stash", "drop", "-q", "stash@{0}"]).ok();
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo(dir: &Path) {
        std::fs::create_dir_all(dir).unwrap();
        let g = |args: &[&str]| assert!(git::git(dir, args).ok(), "{args:?}");
        g(&["init", "-b", "main"]);
        std::fs::write(dir.join("a.txt"), "one\n").unwrap();
        g(&["add", "-A"]);
        g(&[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "commit",
            "-m",
            "first",
        ]);
    }

    fn stash(dir: &Path, text: &str, stamp: &str) {
        std::fs::write(dir.join("a.txt"), text).unwrap();
        let msg = format!("{STASH_PREFIX} {stamp}");
        assert!(git::git(
            dir,
            &[
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "stash",
                "push",
                "-u",
                "-m",
                &msg
            ]
        )
        .ok());
    }

    #[test]
    fn lists_stashes_and_folders_newest_first() {
        let d = tempfile::tempdir().unwrap();
        let ws = d.path();
        repo(&ws.join("sluice"));
        stash(&ws.join("sluice"), "two\n", "2026-11-23-111459");
        // Someone's own stash is never listed.
        std::fs::write(ws.join("sluice/a.txt"), "mine\n").unwrap();
        git::git(
            &ws.join("sluice"),
            &[
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "stash",
                "push",
                "-m",
                "my own work",
            ],
        );
        std::fs::create_dir_all(ws.join(".stowlock-old-demo-app-2026-11-23-120000")).unwrap();
        let all = list(ws);
        assert_eq!(all.len(), 2, "{all:?}");
        assert_eq!(all[0].project, "demo-app");
        assert_eq!(all[0].when, "2026-11-23-120000");
        assert_eq!(all[1].project, "sluice");
        assert_eq!(all[1].where_kept(), "kept in git stash");
    }

    #[test]
    fn restore_and_delete_a_stash() {
        let d = tempfile::tempdir().unwrap();
        let ws = d.path();
        let p = ws.join("sluice");
        repo(&p);
        stash(&p, "attempt one\n", "2026-11-23-100000");
        stash(&p, "attempt two\n", "2026-11-23-110000");
        let all = list(ws);
        // Restore the older one even though its stash number changed.
        let older = all.iter().find(|a| a.when.ends_with("100000")).unwrap();
        restore(older, ws).unwrap();
        assert_eq!(
            std::fs::read_to_string(p.join("a.txt")).unwrap(),
            "attempt one\n"
        );
        // Restoring into a folder with unsaved changes is refused.
        let newer = list(ws).into_iter().next().unwrap();
        assert!(restore(&newer, ws).unwrap_err().contains("unsaved changes"));
        delete(&newer, |_| unreachable!()).unwrap();
        assert!(list(ws).is_empty());
    }

    #[test]
    fn restore_and_delete_a_folder() {
        let d = tempfile::tempdir().unwrap();
        let ws = d.path();
        let kept = ws.join(".stowlock-old-demo-2026-11-23-120000");
        std::fs::create_dir_all(&kept).unwrap();
        let a = list(ws).remove(0);
        std::fs::create_dir_all(ws.join("demo")).unwrap();
        assert!(restore(&a, ws)
            .unwrap_err()
            .contains("already in your workspace"));
        std::fs::remove_dir(ws.join("demo")).unwrap();
        restore(&a, ws).unwrap();
        assert!(ws.join("demo").is_dir() && !kept.exists());
        // Delete sends a folder to the (pretend) Trash.
        std::fs::create_dir_all(&kept).unwrap();
        let a = list(ws).remove(0);
        let bin = ws.join("trash-bin");
        delete(&a, |p| {
            std::fs::rename(p, &bin).unwrap();
            Ok("Trash".into())
        })
        .unwrap();
        assert!(bin.exists() && list(ws).is_empty());
    }
}
