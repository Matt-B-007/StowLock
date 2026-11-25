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

/// How GitHub's build (Actions) for a pushed commit is going.
#[derive(Debug, Clone, PartialEq)]
pub enum Build {
    /// The project has no GitHub build for this commit.
    None,
    Running {
        url: String,
    },
    Passed {
        url: String,
    },
    Failed {
        url: String,
        /// "job (step)": where it failed.
        step: String,
        /// The end of the failed step's log.
        log: String,
    },
}

#[derive(Debug, PartialEq)]
pub enum RunsState {
    None,
    Running(String),
    Passed(String),
    Failed(String),
}

/// Overall state of a commit's GitHub runs, and the id of a failed run.
pub fn summarize_runs(runs: &[serde_json::Value]) -> (RunsState, Option<u64>) {
    if runs.is_empty() {
        return (RunsState::None, None);
    }
    let url = |r: &serde_json::Value| r["url"].as_str().unwrap_or("").to_string();
    if let Some(r) = runs.iter().find(|r| {
        matches!(
            r["conclusion"].as_str(),
            Some("failure") | Some("timed_out") | Some("startup_failure")
        )
    }) {
        return (RunsState::Failed(url(r)), r["databaseId"].as_u64());
    }
    if let Some(r) = runs
        .iter()
        .find(|r| r["status"].as_str() != Some("completed"))
    {
        return (RunsState::Running(url(r)), None);
    }
    (RunsState::Passed(url(&runs[0])), None)
}

/// "job (step)" for the first failed step in `gh run view --json jobs`.
pub fn failed_step(jobs_json: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(jobs_json).ok()?;
    for job in v["jobs"].as_array()? {
        if job["conclusion"].as_str() != Some("failure") {
            continue;
        }
        let name = job["name"].as_str().unwrap_or("job");
        let step = job["steps"]
            .as_array()
            .and_then(|st| {
                st.iter()
                    .find(|x| x["conclusion"].as_str() == Some("failure"))
            })
            .and_then(|x| x["name"].as_str());
        return Some(match step {
            Some(st) => format!("{name} ({st})"),
            None => name.to_string(),
        });
    }
    None
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
        notes: Option<&str>,
        log: &Logger,
    ) -> Res<()>;
    /// Delete a release and its tag on GitHub (the code stays).
    fn delete_release(&self, owner: &str, slug: &str, tag: &str, log: &Logger) -> Res<()>;
    /// GitHub's build status for a pushed commit.
    fn build_status(&self, owner: &str, slug: &str, commit: &str) -> Res<Build>;

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
        notes: Option<&str>,
        log: &Logger,
    ) -> Res<()> {
        let repo = format!("{owner}/{slug}");
        // The project's own CHANGELOG notes when there are some, otherwise
        // GitHub's generated list of changes.
        let notes_file = match notes {
            Some(n) => {
                let f = tempfile::NamedTempFile::new().ctx("writing the release notes")?;
                std::fs::write(f.path(), n).ctx("writing the release notes")?;
                Some(f)
            }
            None => None,
        };
        let notes_path = notes_file.as_ref().map(|f| f.path().display().to_string());
        let mut args = vec![
            "release",
            "create",
            tag,
            "-R",
            repo.as_str(),
            "--verify-tag",
        ];
        match &notes_path {
            Some(p) => {
                args.push("--notes-file");
                args.push(p.as_str());
            }
            None => args.push("--generate-notes"),
        }
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

    fn delete_release(&self, owner: &str, slug: &str, tag: &str, log: &Logger) -> Res<()> {
        let repo = format!("{owner}/{slug}");
        match gh_logged(
            &[
                "release",
                "delete",
                tag,
                "-R",
                &repo,
                "--cleanup-tag",
                "--yes",
            ],
            &Opts::default_timeout().with_timeout(60),
            log,
            "release",
        ) {
            Ok(_) => Ok(()),
            // No release (only a tag): delete the tag itself.
            Err(e) if e.to_ascii_lowercase().contains("not found") => {
                let o = gh(&[
                    "api",
                    "-X",
                    "DELETE",
                    &format!("repos/{repo}/git/refs/tags/{tag}"),
                ]);
                if o.ok() || o.combined().contains("Reference does not exist") {
                    Ok(())
                } else {
                    Err(o.combined())
                }
            }
            Err(e) => Err(e),
        }
    }

    fn build_status(&self, owner: &str, slug: &str, commit: &str) -> Res<Build> {
        let repo = format!("{owner}/{slug}");
        let o = gh(&[
            "run",
            "list",
            "-R",
            &repo,
            "--commit",
            commit,
            "--limit",
            "20",
            "--json",
            "databaseId,status,conclusion,url,workflowName",
        ]);
        if !o.ok() {
            return Err(o.combined());
        }
        let runs: Vec<serde_json::Value> = serde_json::from_str(&o.stdout).unwrap_or_default();
        Ok(match summarize_runs(&runs) {
            (RunsState::None, _) => Build::None,
            (RunsState::Running(url), _) => Build::Running { url },
            (RunsState::Passed(url), _) => Build::Passed { url },
            (RunsState::Failed(url), id) => {
                let (step, log) = match id {
                    Some(id) => {
                        let id = id.to_string();
                        let jobs = gh(&["run", "view", &id, "-R", &repo, "--json", "jobs"]);
                        let log = gh(&["run", "view", &id, "-R", &repo, "--log-failed"]);
                        let mut tail: Vec<&str> = log.stdout.lines().rev().take(60).collect();
                        tail.reverse();
                        (
                            failed_step(&jobs.stdout).unwrap_or_else(|| "a step".into()),
                            tail.join("\n"),
                        )
                    }
                    None => ("a step".into(), String::new()),
                };
                Build::Failed { url, step, log }
            }
        })
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
        notes: Option<&str>,
        _log: &Logger,
    ) -> Res<()> {
        // Tests make GitHub fail release creation by leaving this file.
        if let Ok(why) = std::fs::read_to_string(self.root.join("fail-release")) {
            return Err(why);
        }
        if let Some(n) = notes {
            std::fs::write(
                self.bare(owner, slug)
                    .join(format!("stowlock-notes-{tag}.md")),
                n,
            )
            .ctx("release notes")?;
        }
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

    fn delete_release(&self, owner: &str, slug: &str, tag: &str, _log: &Logger) -> Res<()> {
        let mut v = self.releases(owner, slug);
        v.retain(|(t, _)| t != tag);
        std::fs::write(
            self.bare(owner, slug).join("stowlock-releases.json"),
            serde_json::to_string(&v).unwrap_or_default(),
        )
        .ctx("releases")?;
        let _ = git::git_bare(&self.bare(owner, slug), &["tag", "-d", tag]);
        Ok(())
    }

    /// Tests set the build result in `<root>/build.json` as
    /// {"state":"passed"|"running"|"failed","step":"…","log":"…"}.
    fn build_status(&self, _owner: &str, _slug: &str, _commit: &str) -> Res<Build> {
        let Ok(t) = std::fs::read_to_string(self.root.join("build.json")) else {
            return Ok(Build::None);
        };
        let v: serde_json::Value = serde_json::from_str(&t).map_err(|e| e.to_string())?;
        let url = "https://github.com/test/build/1".to_string();
        Ok(match v["state"].as_str() {
            Some("passed") => Build::Passed { url },
            Some("running") => Build::Running { url },
            Some("failed") => Build::Failed {
                url,
                step: v["step"].as_str().unwrap_or("a step").into(),
                log: v["log"].as_str().unwrap_or("").into(),
            },
            _ => Build::None,
        })
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

#[cfg(test)]
mod build_tests {
    use super::*;

    #[test]
    fn summarises_a_commits_runs() {
        let j = |s: &str| -> Vec<serde_json::Value> { serde_json::from_str(s).unwrap() };
        assert_eq!(summarize_runs(&[]).0, RunsState::None);
        let running = j(
            r#"[{"status":"in_progress","conclusion":"","url":"u1"},{"status":"completed","conclusion":"success","url":"u2"}]"#,
        );
        assert_eq!(summarize_runs(&running).0, RunsState::Running("u1".into()));
        let failed = j(
            r#"[{"status":"in_progress","conclusion":"","url":"u1"},{"status":"completed","conclusion":"failure","url":"u3","databaseId":42}]"#,
        );
        assert_eq!(
            summarize_runs(&failed),
            (RunsState::Failed("u3".into()), Some(42))
        );
        let ok = j(
            r#"[{"status":"completed","conclusion":"success","url":"u2"},{"status":"completed","conclusion":"skipped","url":"u4"}]"#,
        );
        assert_eq!(summarize_runs(&ok).0, RunsState::Passed("u2".into()));
    }

    #[test]
    fn names_the_failed_step() {
        let jobs = r#"{"jobs":[{"name":"test","conclusion":"success","steps":[]},
            {"name":"arch","conclusion":"failure","steps":[{"name":"Install tools","conclusion":"success"},{"name":"cargo test --locked","conclusion":"failure"}]}]}"#;
        assert_eq!(
            failed_step(jobs).as_deref(),
            Some("arch (cargo test --locked)")
        );
        assert_eq!(failed_step("{}"), None);
    }
}
