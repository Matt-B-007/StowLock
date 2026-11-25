// SPDX-License-Identifier: GPL-3.0-or-later
//! Local checks that match GitHub's: read the project's own GitHub workflow
//! files and turn them into the commands StowLock runs before pushing, with
//! the same settings. Steps that only GitHub can run (GitHub Actions, system
//! package installs, publishing, secrets, other operating systems, containers)
//! are listed as skipped instead of guessed at.

use std::path::Path;

/// One command to run here, as GitHub would.
#[derive(Debug, Clone, PartialEq)]
pub struct PlanStep {
    pub job: String,
    pub name: String,
    pub run: String,
    pub env: Vec<(String, String)>,
    /// Relative to the project folder.
    pub workdir: Option<String>,
}

/// A step only GitHub can run, and why.
#[derive(Debug, Clone, PartialEq)]
pub struct Skipped {
    pub job: String,
    pub name: String,
    pub why: String,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Plan {
    pub steps: Vec<PlanStep>,
    pub skipped: Vec<Skipped>,
    /// Setup actions GitHub uses that this computer already covers
    /// (checkout, toolchain setup, caches).
    pub provided: usize,
    pub files: Vec<String>,
}

impl Plan {
    /// Steps counted for "Ran 5 of 6 GitHub steps here".
    pub fn total(&self) -> usize {
        self.steps.len() + self.skipped.len()
    }
}

/// GitHub Actions whose job this computer already does.
const PROVIDED: &[&str] = &[
    "actions/checkout",
    "dtolnay/rust-toolchain",
    "actions-rs/toolchain",
    "actions-rust-lang/setup-rust-toolchain",
    "swatinem/rust-cache",
    "actions/cache",
    "actions/setup-python",
    "actions/setup-node",
    "actions/setup-go",
    "actions/setup-java",
];

fn s(v: &serde_yaml::Value) -> Option<&str> {
    v.as_str()
}

/// Does this workflow run as a check on ordinary pushes or pull requests?
/// (A `push` limited to tags is a release workflow, not a check.)
fn triggers_on_push(on: &serde_yaml::Value) -> bool {
    let wanted = |k: &str| k == "push" || k == "pull_request";
    match on {
        serde_yaml::Value::String(t) => wanted(t),
        serde_yaml::Value::Sequence(v) => v.iter().filter_map(s).any(wanted),
        serde_yaml::Value::Mapping(m) => m.iter().any(|(k, v)| {
            let Some(k) = s(k) else { return false };
            if k == "pull_request" {
                return true;
            }
            if k != "push" {
                return false;
            }
            match v.as_mapping() {
                Some(filters) => {
                    let keys: Vec<&str> = filters.keys().filter_map(s).collect();
                    !(keys.iter().any(|k| k.starts_with("tags"))
                        && !keys
                            .iter()
                            .any(|k| k.starts_with("branches") || k.starts_with("paths")))
                }
                None => true,
            }
        }),
        _ => false,
    }
}

fn env_of(v: &serde_yaml::Value) -> Vec<(String, String)> {
    let mut out = Vec::new();
    if let Some(m) = v.get("env").and_then(|e| e.as_mapping()) {
        for (k, val) in m {
            let (Some(k), Some(val)) = (
                s(k),
                match val {
                    serde_yaml::Value::String(t) => Some(t.clone()),
                    serde_yaml::Value::Number(n) => Some(n.to_string()),
                    serde_yaml::Value::Bool(b) => Some(b.to_string()),
                    _ => None,
                },
            ) else {
                continue;
            };
            // Values from GitHub (secrets, tokens, contexts) can't exist here.
            if !val.contains("${{") {
                out.push((k.to_string(), val));
            }
        }
    }
    out
}

/// Why a `run:` command only works on GitHub, if it doesn't work here.
fn github_only(run: &str) -> Option<&'static str> {
    let r = run.to_ascii_lowercase();
    if r.contains("${{") {
        return Some("uses GitHub's own variables");
    }
    let installs = [
        "sudo ",
        "apt-get",
        "apt install",
        "pacman ",
        "dnf ",
        "yum ",
        "brew ",
        "choco ",
        "snap install",
        "pip install",
        "pip3 install",
        "npm install -g",
        "npm i -g",
        "cargo install",
        "go install",
        "gem install",
        "rustup ",
    ];
    if installs.iter().any(|w| r.contains(w)) {
        return Some("installs system packages (GitHub's machine only)");
    }
    let publishes = [
        "gh release",
        "gh api",
        "git push",
        "ssh ",
        "scp ",
        "npm publish",
        "cargo publish",
        "twine upload",
        "docker push",
        "docker login",
        "aws ",
        "gcloud ",
    ];
    if publishes.iter().any(|w| r.contains(w)) {
        return Some("publishes or uploads (GitHub only)");
    }
    None
}

/// Read the project's workflows. None when it has no workflow files.
pub fn plan(dir: &Path) -> Option<Plan> {
    let wf_dir = dir.join(".github").join("workflows");
    let mut files: Vec<_> = std::fs::read_dir(&wf_dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            matches!(
                p.extension().and_then(|x| x.to_str()),
                Some("yml") | Some("yaml")
            ) && p.metadata().map(|m| m.len() < 1_000_000).unwrap_or(false)
        })
        .collect();
    files.sort();
    files.truncate(20);
    if files.is_empty() {
        return None;
    }
    let mut plan = Plan::default();
    for f in files {
        let name = f
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        let Ok(text) = std::fs::read_to_string(&f) else {
            continue;
        };
        let Ok(doc) = serde_yaml::from_str::<serde_yaml::Value>(&text) else {
            plan.skipped.push(Skipped {
                job: name.clone(),
                name: "whole file".into(),
                why: "couldn't be read".into(),
            });
            continue;
        };
        // YAML reads a bare `on:` key as true.
        let on = doc
            .get("on")
            .or_else(|| doc.get(serde_yaml::Value::Bool(true)));
        if !on.map(triggers_on_push).unwrap_or(false) {
            continue; // releases, schedules and manual runs aren't checks
        }
        plan.files.push(name.clone());
        let wf_env = env_of(&doc);
        let Some(jobs) = doc.get("jobs").and_then(|j| j.as_mapping()) else {
            continue;
        };
        for (job_id, job) in jobs {
            let job_name = job
                .get("name")
                .and_then(s)
                .or_else(|| s(job_id))
                .unwrap_or("job")
                .to_string();
            let runs_on = format!("{:?}", job.get("runs-on").cloned().unwrap_or_default())
                .to_ascii_lowercase();
            let steps = job
                .get("steps")
                .and_then(|st| st.as_sequence())
                .cloned()
                .unwrap_or_default();
            let skip_job = |why: &str, plan: &mut Plan| {
                for st in &steps {
                    if st.get("run").is_some() {
                        let n = st
                            .get("name")
                            .and_then(s)
                            .unwrap_or_else(|| st.get("run").and_then(s).unwrap_or("step"));
                        plan.skipped.push(Skipped {
                            job: job_name.clone(),
                            name: first_line(n),
                            why: why.to_string(),
                        });
                    }
                }
            };
            if runs_on.contains("windows") || runs_on.contains("macos") {
                skip_job("runs on Windows or macOS on GitHub", &mut plan);
                continue;
            }
            if job.get("container").is_some() {
                skip_job(
                    "runs in a container on GitHub (exact mode can run it)",
                    &mut plan,
                );
                continue;
            }
            let job_env = env_of(job);
            let job_wd = job
                .get("defaults")
                .and_then(|d| d.get("run"))
                .and_then(|r| r.get("working-directory"))
                .and_then(s)
                .map(|t| t.to_string());
            for st in &steps {
                if let Some(uses) = st.get("uses").and_then(s) {
                    let base = uses.split('@').next().unwrap_or("").to_ascii_lowercase();
                    if PROVIDED.iter().any(|p| base == *p) {
                        plan.provided += 1;
                    } else {
                        let n = st.get("name").and_then(s).unwrap_or(uses);
                        plan.skipped.push(Skipped {
                            job: job_name.clone(),
                            name: n.to_string(),
                            why: "a GitHub Action (runs only on GitHub)".into(),
                        });
                    }
                    continue;
                }
                let Some(run) = st.get("run").and_then(s) else {
                    continue;
                };
                let step_name = st
                    .get("name")
                    .and_then(s)
                    .map(|t| t.to_string())
                    .unwrap_or_else(|| first_line(run));
                let cond = st.get("if").map(|c| format!("{c:?}")).unwrap_or_default();
                let why = if cond.contains("github.") || cond.contains("secrets") {
                    Some("only runs on GitHub in certain cases")
                } else if st
                    .get("shell")
                    .and_then(s)
                    .is_some_and(|sh| sh.contains("pwsh") || sh.contains("powershell"))
                {
                    Some("uses PowerShell")
                } else {
                    github_only(run)
                };
                if let Some(why) = why {
                    plan.skipped.push(Skipped {
                        job: job_name.clone(),
                        name: step_name,
                        why: why.into(),
                    });
                    continue;
                }
                // The same command in two jobs only needs to run once here.
                if plan.steps.iter().any(|p| p.run.trim() == run.trim()) {
                    continue;
                }
                let mut env = wf_env.clone();
                env.extend(job_env.clone());
                env.extend(env_of(st));
                let workdir = st
                    .get("working-directory")
                    .and_then(s)
                    .map(|t| t.to_string())
                    .or_else(|| job_wd.clone());
                plan.steps.push(PlanStep {
                    job: job_name.clone(),
                    name: step_name,
                    run: run.to_string(),
                    env,
                    workdir,
                });
            }
        }
    }
    if plan.files.is_empty() {
        return None;
    }
    Some(plan)
}

fn first_line(t: &str) -> String {
    let l = t.lines().next().unwrap_or("").trim();
    if l.chars().count() > 60 {
        format!("{}…", l.chars().take(60).collect::<String>())
    } else {
        l.to_string()
    }
}

/// Point `./target/…` paths at StowLock's build cache, where cargo builds
/// when re-checks are kept fast.
pub fn use_build_cache(run: &str, cache: &Path) -> String {
    let c = cache.display().to_string();
    let mut out = String::new();
    for (i, part) in run.split("./target/").enumerate() {
        if i > 0 {
            out.push_str(&c);
            out.push('/');
        }
        out.push_str(part);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, name: &str, text: &str) {
        let wf = dir.join(".github/workflows");
        std::fs::create_dir_all(&wf).unwrap();
        std::fs::write(wf.join(name), text).unwrap();
    }

    #[test]
    fn reads_stowlocks_own_ci() {
        let d = tempfile::tempdir().unwrap();
        write(
            d.path(),
            "ci.yml",
            include_str!("../.github/workflows/ci.yml"),
        );
        write(
            d.path(),
            "release.yml",
            include_str!("../.github/workflows/release.yml"),
        );
        let p = plan(d.path()).unwrap();
        let runs: Vec<&str> = p.steps.iter().map(|s| s.run.as_str()).collect();
        assert!(runs.contains(&"cargo test --locked"), "{runs:?}");
        assert!(runs.contains(&"cargo build --release --locked"));
        assert!(runs.contains(&"./target/release/stowlock --self-test"));
        assert_eq!(
            runs.iter().filter(|r| **r == "cargo test --locked").count(),
            1,
            "run once"
        );
        // The Arch job runs in a container: listed, not run.
        assert!(p.skipped.iter().any(|s| s.why.contains("container")));
        // The release workflow only runs for tags: not a check.
        assert_eq!(p.files, ["ci.yml"]);
        assert!(
            p.provided >= 2,
            "checkout and the toolchain are provided here"
        );
    }

    #[test]
    fn github_only_steps_are_listed_with_reasons() {
        let d = tempfile::tempdir().unwrap();
        write(
            d.path(),
            "ci.yml",
            r#"
name: CI
on: [push]
env:
  RUSTFLAGS: "-D warnings"
  TOKEN: ${{ secrets.TOKEN }}
jobs:
  test:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: some/deploy-action@v1
      - run: sudo apt-get install -y libfoo
      - name: Tests
        run: cargo test
        env:
          RUST_LOG: debug
      - run: echo ${{ github.sha }}
      - run: gh release upload v1 x
      - name: Only on main
        if: github.ref == 'refs/heads/main'
        run: make deploy
  mac:
    runs-on: macos-latest
    steps:
      - run: cargo test --features mac
"#,
        );
        let p = plan(d.path()).unwrap();
        assert_eq!(p.steps.len(), 1);
        let t = &p.steps[0];
        assert_eq!(t.name, "Tests");
        assert!(t.env.contains(&("RUSTFLAGS".into(), "-D warnings".into())));
        assert!(t.env.contains(&("RUST_LOG".into(), "debug".into())));
        assert!(
            !t.env.iter().any(|(k, _)| k == "TOKEN"),
            "secrets never come along"
        );
        let why: Vec<&str> = p.skipped.iter().map(|s| s.why.as_str()).collect();
        for w in [
            "a GitHub Action",
            "installs system packages",
            "GitHub's own variables",
            "publishes",
            "certain cases",
            "Windows or macOS",
        ] {
            assert!(why.iter().any(|x| x.contains(w)), "missing: {w} in {why:?}");
        }
        assert_eq!(p.total(), 7);
    }

    #[test]
    fn no_workflows_no_plan() {
        let d = tempfile::tempdir().unwrap();
        assert_eq!(plan(d.path()), None);
        write(
            d.path(),
            "nightly.yml",
            "on:\n  schedule:\n    - cron: '0 0 * * *'\njobs: {}\n",
        );
        assert_eq!(
            plan(d.path()),
            None,
            "only push/pull_request workflows are checks"
        );
    }

    #[test]
    fn target_paths_follow_the_build_cache() {
        let c = Path::new("/cache/builds/demo");
        assert_eq!(
            use_build_cache("./target/release/app --self-test", c),
            "/cache/builds/demo/release/app --self-test"
        );
        assert_eq!(use_build_cache("cargo test", c), "cargo test");
    }
}
