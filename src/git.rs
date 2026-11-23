// SPDX-License-Identifier: GPL-3.0-or-later
//! Thin, logged wrappers around the `git` command.

use crate::logbook::Logger;
use crate::proc::{self, Opts, Output};
use crate::util::Res;
use std::path::{Path, PathBuf};

pub fn git(dir: &Path, args: &[&str]) -> Output {
    proc::run("git", args, &Opts::in_dir(dir))
}

/// Run git against a bare repository (the self-test's stand-in for GitHub).
/// The repo is named with --git-dir instead of running inside it, because
/// newer git can refuse to work inside a bare repo
/// (safe.bareRepository = explicit).
pub fn git_bare(bare: &Path, args: &[&str]) -> Output {
    let gd = format!("--git-dir={}", bare.display());
    let mut full: Vec<&str> = vec![&gd];
    full.extend_from_slice(args);
    let cwd = bare.parent().unwrap_or(Path::new("/"));
    proc::run("git", &full, &Opts::in_dir(cwd))
}

/// Run git, log it, and turn failure into an error with git's message.
pub fn git_ok(dir: &Path, args: &[&str], log: &Logger, stage: &str) -> Res<Output> {
    let o = git(dir, args);
    log.command(stage, &o);
    if o.ok() {
        Ok(o)
    } else {
        Err(o.combined())
    }
}

pub fn is_repo(dir: &Path) -> bool {
    dir.join(".git").exists()
}

/// `git status --porcelain` lines (all untracked files listed).
pub fn status_lines(dir: &Path) -> Vec<String> {
    let o = git(dir, &["status", "--porcelain", "--untracked-files=all"]);
    o.stdout
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.to_string())
        .collect()
}

#[derive(Debug, Default, Clone, PartialEq)]
pub struct Changes {
    pub added: Vec<String>,
    pub changed: Vec<String>,
    pub removed: Vec<String>,
}

impl Changes {
    pub fn total(&self) -> usize {
        self.added.len() + self.changed.len() + self.removed.len()
    }
    pub fn summary(&self) -> String {
        format!(
            "+{} added, ~{} changed, −{} removed",
            self.added.len(),
            self.changed.len(),
            self.removed.len()
        )
    }
    pub fn short(&self) -> String {
        format!(
            "+{} ~{} −{}",
            self.added.len(),
            self.changed.len(),
            self.removed.len()
        )
    }
}

pub fn parse_changes(lines: &[String]) -> Changes {
    let mut c = Changes::default();
    for l in lines {
        if l.len() < 4 {
            continue;
        }
        let code = &l[..2];
        let path = l[3..].trim().trim_matches('"').to_string();
        let path = path.split(" -> ").last().unwrap_or(&path).to_string();
        if code == "??" || code.contains('A') {
            c.added.push(path);
        } else if code.contains('D') {
            c.removed.push(path);
        } else {
            c.changed.push(path);
        }
    }
    c
}

pub fn changes(dir: &Path) -> Changes {
    parse_changes(&status_lines(dir))
}

/// Files git would commit: tracked + untracked, minus ignored.
pub fn files_to_commit(dir: &Path) -> Vec<PathBuf> {
    let o = git(
        dir,
        &[
            "ls-files",
            "--cached",
            "--others",
            "--exclude-standard",
            "-z",
        ],
    );
    let mut v: Vec<PathBuf> = o
        .stdout
        .split('\0')
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .collect();
    v.retain(|p| dir.join(p).is_file());
    v.sort();
    v.dedup();
    v
}

pub fn remote_url(dir: &Path) -> Option<String> {
    let o = git(dir, &["remote", "get-url", "origin"]);
    if o.ok() {
        Some(o.stdout_trim())
    } else {
        None
    }
}

/// Does a remote URL point to owner/slug? Accepts https, ssh and file forms.
pub fn origin_matches(url: &str, owner: &str, slug: &str) -> bool {
    let u = url
        .trim()
        .trim_end_matches('/')
        .trim_end_matches(".git")
        .to_ascii_lowercase();
    let want = format!(
        "{}/{}",
        owner.to_ascii_lowercase(),
        slug.to_ascii_lowercase()
    );
    u.ends_with(&format!("/{want}")) || u.ends_with(&format!(":{want}"))
}

pub fn current_branch(dir: &Path) -> Option<String> {
    let o = git(dir, &["rev-parse", "--abbrev-ref", "HEAD"]);
    if o.ok() {
        Some(o.stdout_trim())
    } else {
        None
    }
}

pub fn has_commits(dir: &Path) -> bool {
    git(dir, &["rev-parse", "--verify", "HEAD"]).ok()
}

/// Tags, newest version first.
pub fn tags(dir: &Path) -> Vec<String> {
    let o = git(dir, &["tag", "--sort=-v:refname"]);
    o.stdout
        .lines()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

pub fn latest_tag(dir: &Path) -> Option<String> {
    tags(dir).into_iter().next()
}

/// Commits on HEAD that aren't on its upstream (None = no upstream).
pub fn unpushed_commits(dir: &Path) -> Option<usize> {
    let o = git(dir, &["rev-list", "--count", "@{u}..HEAD"]);
    if o.ok() {
        o.stdout_trim().parse().ok()
    } else {
        None
    }
}

/// The global git identity (name, email), if set.
pub fn global_identity() -> (Option<String>, Option<String>) {
    let get = |k: &str| {
        let o = proc::run("git", &["config", "--global", k], &Opts::default_timeout());
        let v = o.stdout_trim();
        if o.ok() && !v.is_empty() {
            Some(v)
        } else {
            None
        }
    };
    (get("user.name"), get("user.email"))
}

/// Identity effective inside a repo (local or global).
pub fn effective_identity(dir: &Path) -> (Option<String>, Option<String>) {
    let get = |k: &str| {
        let o = git(dir, &["config", k]);
        let v = o.stdout_trim();
        if o.ok() && !v.is_empty() {
            Some(v)
        } else {
            None
        }
    };
    (get("user.name"), get("user.email"))
}

pub fn set_global_identity(name: &str, email: &str) -> Res<()> {
    for (k, v) in [("user.name", name), ("user.email", email)] {
        let o = proc::run(
            "git",
            &["config", "--global", k, v],
            &Opts::default_timeout(),
        );
        if !o.ok() {
            return Err(o.combined());
        }
    }
    Ok(())
}

/// `-c user.name=… -c user.email=…` when the repo has no identity, using the
/// fallback from settings (the user's global config is never changed).
pub fn identity_args(dir: &Path, fallback: (&str, &str)) -> Res<Vec<String>> {
    let (name, email) = effective_identity(dir);
    if name.is_some() && email.is_some() {
        return Ok(Vec::new());
    }
    if fallback.0.is_empty() || fallback.1.is_empty() {
        return Err("Author identity unknown: git doesn't know your name and email yet.".into());
    }
    Ok(vec![
        "-c".into(),
        format!("user.name={}", fallback.0),
        "-c".into(),
        format!("user.email={}", fallback.1),
    ])
}

/// Run a git command that records an author (commit, annotated tag).
pub fn with_identity(
    dir: &Path,
    args: &[&str],
    fallback: (&str, &str),
    log: &Logger,
    stage: &str,
) -> Res<Output> {
    let mut full = identity_args(dir, fallback)?;
    full.extend(args.iter().map(|s| s.to_string()));
    let refs: Vec<&str> = full.iter().map(|s| s.as_str()).collect();
    git_ok(dir, &refs, log, stage)
}

/// Commit everything staged.
pub fn commit(
    dir: &Path,
    message: &str,
    fallback: (&str, &str),
    log: &Logger,
    stage: &str,
) -> Res<Output> {
    with_identity(dir, &["commit", "-m", message], fallback, log, stage)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bare_repos_work_with_strict_git() {
        // Newer git can refuse to run inside a bare repo. git_bare must
        // still work with that rule switched on.
        let d = tempfile::tempdir().unwrap();
        let bare = d.path().join("demo.git");
        std::fs::create_dir_all(&bare).unwrap();
        assert!(git(&bare, &["init", "--bare", "-b", "main"]).ok());
        let strict = ["-c", "safe.bareRepository=explicit"];
        let inside = git(&bare, &[strict[0], strict[1], "symbolic-ref", "HEAD"]);
        assert!(!inside.ok(), "strict git refuses to run inside a bare repo");
        let named = git_bare(&bare, &[strict[0], strict[1], "symbolic-ref", "HEAD"]);
        assert!(named.ok(), "{}", named.combined());
        assert_eq!(named.stdout_trim(), "refs/heads/main");
    }

    #[test]
    fn origin_matching() {
        assert!(origin_matches(
            "https://github.com/Matt-B-007/sluice.git",
            "matt-b-007",
            "sluice"
        ));
        assert!(origin_matches(
            "git@github.com:Matt-B-007/sluice.git",
            "Matt-B-007",
            "sluice"
        ));
        assert!(origin_matches(
            "/tmp/fake/Matt-B-007/sluice.git",
            "Matt-B-007",
            "sluice"
        ));
        assert!(!origin_matches(
            "https://github.com/Matt-B-007/sluice-v2.git",
            "Matt-B-007",
            "sluice"
        ));
        assert!(!origin_matches(
            "https://github.com/other/sluice.git",
            "Matt-B-007",
            "sluice"
        ));
    }

    #[test]
    fn fallback_identity_used_for_commit_and_tag() {
        let d = tempfile::tempdir().unwrap();
        let log = Logger::new(None);
        // An isolated HOME so no global identity exists.
        let home = d.path().join("home");
        std::fs::create_dir_all(&home).unwrap();
        let dir = d.path().join("repo");
        std::fs::create_dir_all(&dir).unwrap();
        let env = |args: &[&str]| {
            let mut o = crate::proc::Opts::in_dir(&dir);
            o.env = vec![
                ("HOME".into(), home.display().to_string()),
                ("GIT_CONFIG_NOSYSTEM".into(), "1".into()),
                ("XDG_CONFIG_HOME".into(), home.display().to_string()),
            ];
            crate::proc::run("git", args, &o)
        };
        assert!(env(&["init", "-b", "main"]).ok());
        std::fs::write(dir.join("a.txt"), "a").unwrap();
        assert!(env(&["add", "-A"]).ok());
        let fb = [
            "-c",
            "user.name=Fallback",
            "-c",
            "user.email=f@example.invalid",
        ];
        let mut c: Vec<&str> = fb.to_vec();
        c.extend(["commit", "-m", "x"]);
        assert!(env(&c).ok(), "commit with fallback identity");
        let mut t: Vec<&str> = fb.to_vec();
        t.extend(["tag", "-a", "v0.1.0", "-m", "Release v0.1.0"]);
        assert!(env(&t).ok(), "annotated tag with fallback identity");
        let _ = log;
    }

    #[test]
    fn parses_porcelain() {
        let lines: Vec<String> = [
            "?? new.txt",
            " M src/a.rs",
            " D old.txt",
            "A  staged.rs",
            "R  a -> b",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let c = parse_changes(&lines);
        assert_eq!(c.added, vec!["new.txt", "staged.rs"]);
        assert_eq!(c.removed, vec!["old.txt"]);
        assert_eq!(c.changed, vec!["src/a.rs", "b"]);
        assert_eq!(c.short(), "+2 ~2 −1");
    }
}
