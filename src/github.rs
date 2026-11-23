// SPDX-License-Identifier: GPL-3.0-or-later
//! All GitHub access goes through the `GitHubApi` trait: the real one drives
//! the `gh` command; the fake one uses local bare git repos so tests and the
//! self-test never touch GitHub.

use crate::git;
use crate::logbook::Logger;
use crate::proc::{self, Opts};
use crate::util::{Ctx, Res};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq)]
pub struct RepoInfo {
    pub name: String,
    pub url: String,
    pub private: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Account {
    pub login: String,
    pub active: bool,
    /// Token kept in the system keyring (true) or a plain file (false).
    pub secure: bool,
}

pub trait GitHubApi: Send + Sync {
    fn login(&self) -> Res<String>;
    fn orgs(&self) -> Vec<String>;
    fn accounts(&self) -> Vec<Account>;
    fn list_repos(&self, owner: &str) -> Res<Vec<RepoInfo>>;
    fn repo_exists(&self, owner: &str, slug: &str) -> Res<bool>;
    fn default_branch(&self, owner: &str, slug: &str) -> Res<String>;
    fn clone_repo(&self, owner: &str, slug: &str, dest: &Path, log: &Logger) -> Res<()>;
    fn create_repo(
        &self,
        dir: &Path,
        owner: &str,
        slug: &str,
        private: bool,
        description: &str,
        log: &Logger,
    ) -> Res<()>;
    fn add_topics(&self, owner: &str, slug: &str, topics: &[String], log: &Logger) -> Res<()>;
    fn create_release(
        &self,
        owner: &str,
        slug: &str,
        tag: &str,
        prerelease: bool,
        log: &Logger,
    ) -> Res<()>;
    fn repo_url(&self, owner: &str, slug: &str) -> String {
        format!("https://github.com/{owner}/{slug}")
    }
}

// ---------------------------------------------------------------- real (gh)

pub struct RealGh;

fn gh(args: &[&str]) -> proc::Output {
    proc::run("gh", args, &Opts::default_timeout())
}

fn gh_logged(args: &[&str], opts: &Opts, log: &Logger, stage: &str) -> Res<proc::Output> {
    let o = proc::run("gh", args, opts);
    log.command(stage, &o);
    if o.ok() {
        Ok(o)
    } else {
        Err(o.combined())
    }
}

/// Parse `gh auth status` text into accounts.
pub fn parse_auth_status(text: &str) -> Vec<Account> {
    let mut accounts: Vec<Account> = Vec::new();
    for line in text.lines() {
        let l = line.trim();
        let rest = if let Some(i) = l.find("Logged in to github.com account ") {
            &l[i + "Logged in to github.com account ".len()..]
        } else if let Some(i) = l.find("Logged in to github.com as ") {
            &l[i + "Logged in to github.com as ".len()..]
        } else {
            if l.contains("Active account: true") {
                if let Some(a) = accounts.last_mut() {
                    a.active = true;
                }
            }
            continue;
        };
        let login = rest.split_whitespace().next().unwrap_or("").to_string();
        if login.is_empty() {
            continue;
        }
        let secure = rest.contains("(keyring)");
        accounts.push(Account {
            login,
            active: false,
            secure,
        });
    }
    if accounts.len() == 1 {
        accounts[0].active = true;
    }
    accounts
}

impl GitHubApi for RealGh {
    fn login(&self) -> Res<String> {
        let o = gh(&["api", "user", "--jq", ".login"]);
        if o.ok() && !o.stdout_trim().is_empty() {
            Ok(o.stdout_trim())
        } else {
            Err(o.combined())
        }
    }

    fn orgs(&self) -> Vec<String> {
        let o = gh(&["api", "user/orgs", "--jq", ".[].login"]);
        o.stdout
            .lines()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect()
    }

    fn accounts(&self) -> Vec<Account> {
        let o = gh(&["auth", "status", "--hostname", "github.com"]);
        parse_auth_status(&o.combined())
    }

    fn list_repos(&self, owner: &str) -> Res<Vec<RepoInfo>> {
        let o = gh(&[
            "repo",
            "list",
            owner,
            "--limit",
            "1000",
            "--json",
            "name,url,isPrivate",
        ]);
        if !o.ok() {
            return Err(o.combined());
        }
        let v: serde_json::Value = serde_json::from_str(&o.stdout).ctx("reading repo list")?;
        Ok(v.as_array()
            .map(|a| {
                a.iter()
                    .map(|r| RepoInfo {
                        name: r["name"].as_str().unwrap_or("").to_string(),
                        url: r["url"].as_str().unwrap_or("").to_string(),
                        private: r["isPrivate"].as_bool().unwrap_or(true),
                    })
                    .collect()
            })
            .unwrap_or_default())
    }

    fn repo_exists(&self, owner: &str, slug: &str) -> Res<bool> {
        let o = gh(&["repo", "view", &format!("{owner}/{slug}"), "--json", "name"]);
        if o.ok() {
            return Ok(true);
        }
        let t = o.combined();
        if t.contains("Could not resolve to a Repository")
            || t.contains("Not Found")
            || t.contains("404")
        {
            Ok(false)
        } else {
            Err(t)
        }
    }

    fn default_branch(&self, owner: &str, slug: &str) -> Res<String> {
        let o = gh(&[
            "repo",
            "view",
            &format!("{owner}/{slug}"),
            "--json",
            "defaultBranchRef",
            "--jq",
            ".defaultBranchRef.name",
        ]);
        if o.ok() && !o.stdout_trim().is_empty() {
            Ok(o.stdout_trim())
        } else {
            Err(o.combined())
        }
    }

    fn clone_repo(&self, owner: &str, slug: &str, dest: &Path, log: &Logger) -> Res<()> {
        let d = dest.display().to_string();
        gh_logged(
            &["repo", "clone", &format!("{owner}/{slug}"), &d],
            &Opts::default_timeout().with_timeout(600),
            log,
            "clone",
        )
        .map(|_| ())
    }

    fn create_repo(
        &self,
        dir: &Path,
        owner: &str,
        slug: &str,
        private: bool,
        description: &str,
        log: &Logger,
    ) -> Res<()> {
        let vis = if private { "--private" } else { "--public" };
        let name = format!("{owner}/{slug}");
        let args = [
            "repo",
            "create",
            name.as_str(),
            vis,
            "--description",
            description,
            "--source=.",
            "--remote=origin",
            "--push",
        ];
        gh_logged(&args, &Opts::in_dir(dir).with_timeout(600), log, "push").map(|_| ())
    }

    fn add_topics(&self, owner: &str, slug: &str, topics: &[String], log: &Logger) -> Res<()> {
        if topics.is_empty() {
            return Ok(());
        }
        let joined = topics.join(",");
        gh_logged(
            &[
                "repo",
                "edit",
                &format!("{owner}/{slug}"),
                "--add-topic",
                &joined,
            ],
            &Opts::default_timeout(),
            log,
            "topics",
        )
        .map(|_| ())
    }

    fn create_release(
        &self,
        owner: &str,
        slug: &str,
        tag: &str,
        prerelease: bool,
        log: &Logger,
    ) -> Res<()> {
        let repo = format!("{owner}/{slug}");
        let mut args = vec![
            "release",
            "create",
            tag,
            "-R",
            repo.as_str(),
            "--generate-notes",
            "--verify-tag",
        ];
        if prerelease {
            args.push("--prerelease");
        }
        gh_logged(
            &args,
            &Opts::default_timeout().with_timeout(300),
            log,
            "release",
        )
        .map(|_| ())
    }
}

// ---------------------------------------------------------------- fake

/// Bare git repos under a folder stand in for GitHub.
pub struct FakeGh {
    pub root: PathBuf,
    pub user: String,
}

impl FakeGh {
    pub fn new(root: &Path) -> Self {
        FakeGh {
            root: root.to_path_buf(),
            user: "selftest-user".into(),
        }
    }
    pub fn bare(&self, owner: &str, slug: &str) -> PathBuf {
        self.root.join(owner).join(format!("{slug}.git"))
    }
    pub fn releases(&self, owner: &str, slug: &str) -> Vec<(String, bool)> {
        let p = self.bare(owner, slug).join("stowlock-releases.json");
        let v: Vec<(String, bool)> = std::fs::read_to_string(p)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default();
        v
    }
}

impl GitHubApi for FakeGh {
    fn login(&self) -> Res<String> {
        Ok(self.user.clone())
    }
    fn orgs(&self) -> Vec<String> {
        vec!["selftest-org".into()]
    }
    fn accounts(&self) -> Vec<Account> {
        vec![Account {
            login: self.user.clone(),
            active: true,
            secure: true,
        }]
    }
    fn list_repos(&self, owner: &str) -> Res<Vec<RepoInfo>> {
        let dir = self.root.join(owner);
        let mut v = Vec::new();
        if let Ok(rd) = std::fs::read_dir(&dir) {
            for e in rd.flatten() {
                let n = e.file_name().to_string_lossy().to_string();
                if let Some(name) = n.strip_suffix(".git") {
                    v.push(RepoInfo {
                        name: name.into(),
                        url: e.path().display().to_string(),
                        private: true,
                    });
                }
            }
        }
        Ok(v)
    }
    fn repo_exists(&self, owner: &str, slug: &str) -> Res<bool> {
        Ok(self.bare(owner, slug).exists())
    }
    fn default_branch(&self, owner: &str, slug: &str) -> Res<String> {
        let o = git::git_bare(
            &self.bare(owner, slug),
            &["symbolic-ref", "--short", "HEAD"],
        );
        if o.ok() {
            Ok(o.stdout_trim())
        } else {
            Err(o.combined())
        }
    }
    fn clone_repo(&self, owner: &str, slug: &str, dest: &Path, log: &Logger) -> Res<()> {
        let src = self.bare(owner, slug).display().to_string();
        let parent = dest.parent().unwrap_or(Path::new("/"));
        git::git_ok(
            parent,
            &["clone", &src, &dest.display().to_string()],
            log,
            "clone",
        )
        .map(|_| ())
    }
    fn create_repo(
        &self,
        dir: &Path,
        owner: &str,
        slug: &str,
        _private: bool,
        _description: &str,
        log: &Logger,
    ) -> Res<()> {
        let bare = self.bare(owner, slug);
        std::fs::create_dir_all(&bare).ctx("creating fake remote")?;
        git::git_ok(&bare, &["init", "--bare", "-b", "main"], log, "push")?;
        let url = bare.display().to_string();
        git::git_ok(dir, &["remote", "add", "origin", &url], log, "push")?;
        let branch = git::current_branch(dir).unwrap_or_else(|| "main".into());
        git::git_ok(dir, &["push", "-u", "origin", &branch], log, "push")?;
        Ok(())
    }
    fn add_topics(&self, owner: &str, slug: &str, topics: &[String], _log: &Logger) -> Res<()> {
        std::fs::write(
            self.bare(owner, slug).join("stowlock-topics.txt"),
            topics.join(","),
        )
        .ctx("topics")
    }
    fn create_release(
        &self,
        owner: &str,
        slug: &str,
        tag: &str,
        prerelease: bool,
        _log: &Logger,
    ) -> Res<()> {
        let mut v = self.releases(owner, slug);
        v.push((tag.to_string(), prerelease));
        std::fs::write(
            self.bare(owner, slug).join("stowlock-releases.json"),
            serde_json::to_string(&v).unwrap(),
        )
        .ctx("release")
    }
    fn repo_url(&self, owner: &str, slug: &str) -> String {
        self.bare(owner, slug).display().to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_new_and_old_auth_status() {
        let new = "github.com\n  ✓ Logged in to github.com account Matt-B-007 (keyring)\n  - Active account: true\n  - Git operations protocol: https\n  ✓ Logged in to github.com account work-acct (/home/u/.config/gh/hosts.yml)\n  - Active account: false\n";
        let a = parse_auth_status(new);
        assert_eq!(a.len(), 2);
        assert_eq!(
            a[0],
            Account {
                login: "Matt-B-007".into(),
                active: true,
                secure: true
            }
        );
        assert!(!a[1].secure && !a[1].active);
        let old =
            "github.com\n  ✓ Logged in to github.com as octo (/home/u/.config/gh/hosts.yml)\n";
        let b = parse_auth_status(old);
        assert_eq!(b[0].login, "octo");
        assert!(b[0].active && !b[0].secure);
        assert!(parse_auth_status("You are not logged into any GitHub hosts.").is_empty());
    }
}
