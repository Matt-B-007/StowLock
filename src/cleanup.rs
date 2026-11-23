// SPDX-License-Identifier: GPL-3.0-or-later
//! Clean-up after staging: OS junk, dependency/build folders, nested .git,
//! `.gitignore` rules (never deleting `.env`), and file size limits.

use crate::detect::{self, Stack};
use crate::util::{Ctx, Res};
use std::fs;
use std::path::{Path, PathBuf};

pub const JUNK_FILES: [&str; 2] = [".DS_Store", "Thumbs.db"];
pub const JUNK_DIRS: [&str; 6] = [
    "__MACOSX",
    "node_modules",
    "target",
    "venv",
    ".venv",
    "__pycache__",
];
pub const WARN_BYTES: u64 = 50 * 1024 * 1024;
pub const BLOCK_BYTES: u64 = 100 * 1024 * 1024;

#[derive(Debug, Default, Clone)]
pub struct CleanupReport {
    pub junk_files: usize,
    pub removed_dirs: Vec<String>,
    pub nested_git: usize,
    pub gitignore: Option<String>,
}

impl CleanupReport {
    pub fn lines(&self) -> Vec<String> {
        let mut v = Vec::new();
        if self.junk_files > 0 {
            v.push(format!(
                "Removed {} junk file{}",
                self.junk_files,
                if self.junk_files == 1 { "" } else { "s" }
            ));
        }
        if !self.removed_dirs.is_empty() {
            let mut names: Vec<&str> = self
                .removed_dirs
                .iter()
                .map(|s| s.rsplit('/').next().unwrap_or(s))
                .collect();
            names.sort();
            names.dedup();
            v.push(format!(
                "Removed {} ({} folder{})",
                names.join(", "),
                self.removed_dirs.len(),
                if self.removed_dirs.len() == 1 {
                    ""
                } else {
                    "s"
                }
            ));
        }
        if self.nested_git > 0 {
            v.push(format!(
                "Removed {} nested .git folder{}",
                self.nested_git,
                if self.nested_git == 1 { "" } else { "s" }
            ));
        }
        if let Some(g) = &self.gitignore {
            v.push(g.clone());
        }
        v
    }
}

/// Remove junk and make sure `.gitignore` protects secrets.
pub fn cleanup(dir: &Path, stack: Stack) -> Res<CleanupReport> {
    let mut report = CleanupReport::default();
    let mut to_remove_dirs: Vec<PathBuf> = Vec::new();
    let mut to_remove_files: Vec<PathBuf> = Vec::new();
    let mut it = walkdir::WalkDir::new(dir)
        .follow_links(false)
        .min_depth(1)
        .into_iter();
    while let Some(Ok(e)) = it.next() {
        let name = e.file_name().to_string_lossy().to_string();
        let depth = e.depth();
        if e.file_type().is_dir() {
            if name == ".git" {
                if depth > 1 {
                    to_remove_dirs.push(e.path().to_path_buf());
                    report.nested_git += 1;
                }
                it.skip_current_dir();
                continue;
            }
            if JUNK_DIRS.contains(&name.as_str()) {
                report.removed_dirs.push(
                    e.path()
                        .strip_prefix(dir)
                        .unwrap_or(e.path())
                        .display()
                        .to_string(),
                );
                to_remove_dirs.push(e.path().to_path_buf());
                it.skip_current_dir();
            }
        } else if JUNK_FILES.contains(&name.as_str()) {
            to_remove_files.push(e.path().to_path_buf());
            report.junk_files += 1;
        }
    }
    for d in to_remove_dirs {
        fs::remove_dir_all(&d).ctx(&format!("removing {}", d.display()))?;
    }
    for f in to_remove_files {
        let _ = fs::remove_file(&f);
    }
    let gi = dir.join(".gitignore");
    if gi.is_file() {
        let text = fs::read_to_string(&gi).unwrap_or_default();
        let (new, changed) = detect::add_env_rules(&text);
        if changed {
            fs::write(&gi, new).ctx("updating .gitignore")?;
            report.gitignore = Some("Added .env rules to .gitignore".into());
        }
    } else {
        fs::write(&gi, detect::gitignore_template(stack)).ctx("writing .gitignore")?;
        report.gitignore = Some(format!(
            "Wrote {} .gitignore",
            if stack == Stack::Other {
                "a generic".to_string()
            } else {
                stack.name().to_string()
            }
        ));
    }
    Ok(report)
}

/// (file name, size in bytes)
pub type SizedFiles = Vec<(String, u64)>;

/// Files over 50 MB (warn) and over 100 MB (block), among the given files.
pub fn size_check(root: &Path, files: &[PathBuf]) -> (SizedFiles, SizedFiles) {
    let mut warn = Vec::new();
    let mut block = Vec::new();
    for f in files {
        if let Ok(m) = fs::metadata(root.join(f)) {
            let len = m.len();
            let name = f.display().to_string();
            if len > BLOCK_BYTES {
                block.push((name, len));
            } else if len > WARN_BYTES {
                warn.push((name, len));
            }
        }
    }
    (warn, block)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn removes_junk_keeps_env() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path();
        fs::create_dir_all(p.join("node_modules/x")).unwrap();
        fs::create_dir_all(p.join("sub/.git")).unwrap();
        fs::create_dir_all(p.join(".git")).unwrap();
        fs::create_dir_all(p.join("src/__pycache__")).unwrap();
        fs::write(p.join(".DS_Store"), "x").unwrap();
        fs::write(p.join(".env"), "SECRET=1").unwrap();
        fs::write(p.join("package.json"), "{}").unwrap();
        let r = cleanup(p, Stack::Node).unwrap();
        assert!(!p.join("node_modules").exists());
        assert!(!p.join("sub/.git").exists());
        assert!(p.join(".git").exists(), "project's own .git must stay");
        assert!(!p.join("src/__pycache__").exists());
        assert!(!p.join(".DS_Store").exists());
        assert!(p.join(".env").exists(), ".env must never be deleted");
        let gi = fs::read_to_string(p.join(".gitignore")).unwrap();
        assert!(gi.contains(".env\n") && gi.contains("node_modules/"));
        assert_eq!(r.nested_git, 1);
        assert!(!r.lines().is_empty());
    }

    #[test]
    fn size_limits() {
        let d = tempfile::tempdir().unwrap();
        let f = fs::File::create(d.path().join("big.bin")).unwrap();
        f.set_len(BLOCK_BYTES + 1).unwrap(); // sparse: no real disk use
        let f2 = fs::File::create(d.path().join("mid.bin")).unwrap();
        f2.set_len(WARN_BYTES + 1).unwrap();
        let (warn, block) = size_check(d.path(), &["big.bin".into(), "mid.bin".into()]);
        assert_eq!(block.len(), 1);
        assert_eq!(warn.len(), 1);
    }
}
