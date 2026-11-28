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
    /// Installs Python packages with pip: run here in StowLock's own
    /// throwaway environment, never system-wide.
    pub python_deps: bool,
    /// Installs Cargo tools (cargo-audit, say): run here into StowLock's own
    /// tools folder, skipping any tool this computer already has.
    pub cargo_tools: bool,
}

/// A step only GitHub can run, and why.
#[derive(Debug, Clone, PartialEq)]
pub struct Skipped {
    pub job: String,
    pub name: String,
    pub why: String,
    /// The command (for "install it yourself" hints), when it's a run step.
    pub run: Option<String>,
    /// How many steps to run here came before it (so a later failure can
    /// tell it may depend on this skipped step).
    pub before: usize,
    /// Part of a job that runs only on GitHub as a whole (another system or
    /// a container): listed, but not counted as a step that could run here.
    pub whole_job: bool,
}

impl Skipped {
    /// A skipped step that installs things the later steps may need.
    pub fn is_install(&self) -> bool {
        self.why.starts_with("installs")
    }
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
    /// Steps in the jobs that could run here (jobs that run only on GitHub,
    /// like an Arch container, are named separately by `github_only_jobs`).
    pub fn total(&self) -> usize {
        self.steps.len() + self.skipped.iter().filter(|s| !s.whole_job).count()
    }

    /// Jobs that run only on GitHub as a whole, by name.
    pub fn github_only_jobs(&self) -> Vec<String> {
        let mut jobs: Vec<String> = Vec::new();
        for sk in self.skipped.iter().filter(|s| s.whole_job) {
            if !jobs.contains(&sk.job) {
                jobs.push(sk.job.clone());
            }
        }
        jobs
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

/// pip installs StowLock can run here in its own environment.
fn pip_words() -> [&'static str; 4] {
    [
        "pip install",
        "pip3 install",
        "-m pip install",
        "pipx install",
    ]
}

/// A step that only installs Python packages with pip (nothing system-wide).
pub fn is_python_install(run: &str) -> bool {
    let r = run.to_ascii_lowercase();
    let mut rest = r.clone();
    for w in pip_words() {
        rest = rest.replace(w, " ");
    }
    pip_words().iter().any(|w| r.contains(w))
        && !r.contains("pipx install")
        && github_only_inner(&rest).is_none()
        && !r.contains("${{")
}

/// Why a `run:` command only works on GitHub, if it doesn't work here.
fn github_only(run: &str) -> Option<&'static str> {
    if is_python_install(run) {
        return None;
    }
    github_only_inner(&run.to_ascii_lowercase())
}

fn github_only_inner(r: &str) -> Option<&'static str> {
    let r = r.to_string();
    // GitHub's own files for passing values between steps don't exist here.
    const GITHUB_FILES: [&str; 5] = [
        "github_path",
        "github_env",
        "github_output",
        "github_step_summary",
        "github_state",
    ];
    if GITHUB_FILES.iter().any(|f| r.contains(f)) {
        return Some("uses GitHub's own variables");
    }
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

/// The project has a workflow that runs on version tags and publishes the
/// GitHub release itself (so StowLock should push the tag and let it).
/// Does the project have any GitHub workflow (so a push starts a build)?
pub fn has_workflows(dir: &Path) -> bool {
    std::fs::read_dir(dir.join(".github").join("workflows"))
        .map(|d| {
            d.flatten().any(|e| {
                let n = e.file_name().to_string_lossy().to_ascii_lowercase();
                n.ends_with(".yml") || n.ends_with(".yaml")
            })
        })
        .unwrap_or(false)
}

pub fn publishes_releases(dir: &Path) -> bool {
    const PUBLISHERS: [&str; 6] = [
        "gh release create",
        "softprops/action-gh-release",
        "ncipollo/release-action",
        "actions/create-release",
        "goreleaser",
        "taiki-e/upload-rust-binary-action",
    ];
    let Ok(rd) = std::fs::read_dir(dir.join(".github").join("workflows")) else {
        return false;
    };
    rd.flatten().any(|e| {
        let p = e.path();
        if !matches!(
            p.extension().and_then(|x| x.to_str()),
            Some("yml") | Some("yaml")
        ) {
            return false;
        }
        let Ok(text) = std::fs::read_to_string(&p) else {
            return false;
        };
        let Ok(doc) = serde_yaml::from_str::<serde_yaml::Value>(&text) else {
            return false;
        };
        let on = doc
            .get("on")
            .or_else(|| doc.get(serde_yaml::Value::Bool(true)));
        let on_tags = on
            .and_then(|o| o.get("push"))
            .and_then(|p| p.as_mapping())
            .is_some_and(|m| m.keys().filter_map(s).any(|k| k.starts_with("tags")));
        on_tags && PUBLISHERS.iter().any(|w| text.contains(w))
    })
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
                run: None,
                before: plan.steps.len(),
                whole_job: false,
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
                        let before = plan.steps.len();
                        plan.skipped.push(Skipped {
                            job: job_name.clone(),
                            name: first_line(n),
                            why: why.to_string(),
                            run: None,
                            before,
                            whole_job: true,
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
                            run: None,
                            before: plan.steps.len(),
                            whole_job: false,
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
                // A GitHub-only step that installs tools with pip (shellcheck-py,
                // say): install the same packages here, in StowLock's own Python
                // environment, so the checks that need them can run too.
                let mut step_env = wf_env.clone();
                step_env.extend(job_env.clone());
                step_env.extend(env_of(st));
                let convertible =
                    why.filter(|w| !w.starts_with("only runs") && *w != "uses PowerShell");
                if let Some(cargo) = convertible.and_then(|_| cargo_lines(run, &step_env)) {
                    if !plan.steps.iter().any(|p| p.run == cargo) {
                        plan.steps.push(PlanStep {
                            job: job_name.clone(),
                            name: format!("{step_name} (here: StowLock's own tools folder)"),
                            run: cargo,
                            python_deps: false,
                            cargo_tools: true,
                            env: step_env.clone(),
                            workdir: None,
                        });
                    }
                    if pip_lines(run, &step_env).is_none() {
                        continue;
                    }
                }
                if let Some(pip) = convertible.and_then(|_| pip_lines(run, &step_env)) {
                    let env = step_env.clone();
                    // Anything else the step does (apt-get, say) still only runs on GitHub.
                    let other = run.lines().map(str::trim).any(|l| {
                        !(l.is_empty()
                            || l.starts_with('#')
                            || l.contains("pip install")
                            || l.contains("pip3 install")
                            || l.contains(" -m venv ")
                            || l.contains("cargo install ")
                            || l.to_ascii_lowercase().contains("github_path")
                            || l.starts_with("echo "))
                    });
                    if other {
                        plan.skipped.push(Skipped {
                            job: job_name.clone(),
                            name: step_name.clone(),
                            why: why.unwrap_or_default().into(),
                            run: Some(run.to_string()),
                            before: plan.steps.len(),
                            whole_job: false,
                        });
                    }
                    if !plan.steps.iter().any(|p| p.run == pip) {
                        plan.steps.push(PlanStep {
                            job: job_name.clone(),
                            name: format!("{step_name} (here: StowLock's own Python environment)"),
                            run: pip,
                            python_deps: true,
                            cargo_tools: false,
                            env,
                            workdir: None,
                        });
                    }
                    continue;
                }
                if let Some(why) = why {
                    plan.skipped.push(Skipped {
                        job: job_name.clone(),
                        name: step_name,
                        why: why.into(),
                        run: Some(run.to_string()),
                        before: plan.steps.len(),
                        whole_job: false,
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
                    python_deps: is_python_install(run),
                    cargo_tools: false,
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

/// The pip installs in a step, rewritten to run in StowLock's own Python
/// environment: "/tmp/tools/bin/pip install shellcheck-py==0.11.0.1" becomes
/// "pip install shellcheck-py==0.11.0.1". None when there are none, or when
/// they depend on GitHub's own values.
pub fn pip_lines(run: &str, env: &[(String, String)]) -> Option<String> {
    let mut out = Vec::new();
    for line in run.lines() {
        let filled = fill_env(line.trim(), env);
        let l = filled.as_str();
        if l.starts_with('#') || l.contains("${{") {
            continue;
        }
        let at = ["pip3 install ", "pip install "]
            .iter()
            .filter_map(|w| l.find(w).map(|i| (i, w.len())))
            .min_by_key(|(i, _)| *i);
        let Some((i, n)) = at else { continue };
        let args: Vec<&str> = l[i + n..]
            .split_whitespace()
            .take_while(|a| !matches!(*a, "&&" | "||" | ";" | "|"))
            .filter(|a| !matches!(*a, "--user" | "--break-system-packages"))
            .collect();
        if !args.is_empty() {
            out.push(format!("pip install {}", args.join(" ")));
        }
    }
    (!out.is_empty()).then(|| out.join("\n"))
}

/// Fill in `${{ env.NAME }}` from the workflow's own values. Anything else in
/// `${{ }}` (secrets, matrix, github.*) is left as it is, so the line is skipped.
pub fn fill_env(line: &str, env: &[(String, String)]) -> String {
    let re = regex::Regex::new(r"\$\{\{\s*env\.([A-Za-z_][A-Za-z0-9_]*)\s*\}\}").unwrap();
    re.replace_all(line, |c: &regex::Captures| {
        env.iter()
            .find(|(k, _)| k == &c[1])
            .map(|(_, v)| v.clone())
            .unwrap_or_else(|| c[0].to_string())
    })
    .into_owned()
}

/// The `cargo install` lines in a step, rewritten so a tool this computer
/// already has (from pacman, say) isn't built again:
/// "cargo install cargo-audit --locked" becomes
/// "command -v cargo-audit >/dev/null 2>&1 || cargo install cargo-audit --locked".
pub fn cargo_lines(run: &str, env: &[(String, String)]) -> Option<String> {
    let mut out = Vec::new();
    for line in run.lines() {
        let filled = fill_env(line.trim(), env);
        let l = filled.as_str();
        if l.starts_with('#') || l.contains("${{") {
            continue;
        }
        let Some(i) = l.find("cargo install ") else {
            continue;
        };
        let args: Vec<&str> = l[i + 14..]
            .split_whitespace()
            .take_while(|a| !matches!(*a, "&&" | "||" | ";" | "|"))
            .collect();
        // The first argument that isn't an option (or an option's value).
        let mut tool = None;
        let mut skip_next = false;
        for a in &args {
            if skip_next {
                skip_next = false;
                continue;
            }
            if a.starts_with('-') {
                skip_next = matches!(*a, "--version" | "--git" | "--root" | "--features" | "-F");
                continue;
            }
            tool = Some(a.split('@').next().unwrap_or(a));
            break;
        }
        if let Some(t) = tool {
            out.push(format!(
                "command -v {t} >/dev/null 2>&1 || cargo install {}",
                args.join(" ")
            ));
        }
    }
    (!out.is_empty()).then(|| out.join("\n"))
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
        assert!(runs.contains(&"sh scripts/check.sh test"), "{runs:?}");
        assert!(runs.contains(&"sh scripts/check.sh build"));
        assert!(runs.contains(&"sh scripts/check.sh selftest"));
        assert_eq!(
            runs.iter()
                .filter(|r| **r == "sh scripts/check.sh test")
                .count(),
            1,
            "run once"
        );
        // The Arch job runs in a container: listed, not run.
        assert!(p.skipped.iter().any(|s| s.why.contains("container")));
        // The release workflow only runs for tags: not a check.
        assert_eq!(p.files, ["ci.yml"]);
        assert!(p.provided >= 1, "checkout is provided here");
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
        // The macOS job runs only on GitHub: named, not counted as a step.
        assert_eq!(p.total(), 6);
        assert_eq!(p.github_only_jobs(), ["mac"]);
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

    #[test]
    fn knows_when_a_project_publishes_its_own_releases() {
        assert!(publishes_releases(std::path::Path::new(env!(
            "CARGO_MANIFEST_DIR"
        ))));
        let d = tempfile::tempdir().unwrap();
        let wf = d.path().join(".github/workflows");
        std::fs::create_dir_all(&wf).unwrap();
        std::fs::write(wf.join("ci.yml"), "on: [push]\njobs:\n  t:\n    runs-on: ubuntu-latest\n    steps:\n      - run: gh release create x\n").unwrap();
        assert!(!publishes_releases(d.path()), "not on tags");
        std::fs::write(wf.join("rel.yml"), "on:\n  push:\n    tags: ['v*']\njobs:\n  t:\n    runs-on: ubuntu-latest\n    steps:\n      - uses: softprops/action-gh-release@v2\n").unwrap();
        assert!(publishes_releases(d.path()));
    }

    #[test]
    fn stowlocks_own_ci_runs_the_check_script_here() {
        let plan = plan(std::path::Path::new(env!("CARGO_MANIFEST_DIR"))).unwrap();
        let runs: Vec<&str> = plan.steps.iter().map(|s| s.run.trim()).collect();
        assert_eq!(
            runs,
            [
                "pip install --quiet \"shellcheck-py==$SHELLCHECK_VERSION\"",
                "sh scripts/check.sh fmt",
                "sh scripts/check.sh clippy",
                "sh scripts/check.sh test",
                "sh scripts/check.sh build",
                "sh scripts/check.sh selftest",
                "sh scripts/check.sh shell",
                "sh scripts/check.sh sign",
            ],
            "{plan:#?}"
        );
        // The pinned shellcheck is installed here too, in StowLock's own
        // Python environment, so the shell-script section runs locally.
        assert!(plan.steps[0].python_deps);
        assert!(!plan
            .skipped
            .iter()
            .any(|s| s.job == "Checks (Ubuntu)" && s.name == "Install the pinned shellcheck"));
    }

    #[test]
    fn pip_tool_installs_are_rewritten_for_stowlocks_environment() {
        assert_eq!(
            pip_lines("python3 -m venv /tmp/tools\n/tmp/tools/bin/pip install --quiet \"shellcheck-py==$V\"\necho /tmp/tools/bin >> \"$GITHUB_PATH\"", &[]).as_deref(),
            Some("pip install --quiet \"shellcheck-py==$V\"")
        );
        assert_eq!(pip_lines("sudo apt-get install -y shellcheck", &[]), None);
        assert_eq!(
            pip_lines("pip install --user ruff && ruff --version", &[]).as_deref(),
            Some("pip install ruff")
        );
    }

    #[test]
    fn workflow_env_values_are_filled_in() {
        let env = vec![("SHELLCHECK_VERSION".to_string(), "0.11.0.1".to_string())];
        assert_eq!(
            pip_lines(
                "pip install \"shellcheck-py==${{ env.SHELLCHECK_VERSION }}\"",
                &env
            )
            .as_deref(),
            Some("pip install \"shellcheck-py==0.11.0.1\"")
        );
        // Values from GitHub itself can't be known here: the line is left out.
        assert_eq!(pip_lines("pip install x==${{ matrix.v }}", &env), None);
    }

    #[test]
    fn cargo_tool_installs_skip_tools_you_already_have() {
        assert_eq!(
            cargo_lines("cargo install cargo-audit --locked", &[]).as_deref(),
            Some("command -v cargo-audit >/dev/null 2>&1 || cargo install cargo-audit --locked")
        );
        assert_eq!(
            cargo_lines("cargo install --locked --version 0.18.3 cargo-deny", &[]).as_deref(),
            Some("command -v cargo-deny >/dev/null 2>&1 || cargo install --locked --version 0.18.3 cargo-deny")
        );
        assert_eq!(cargo_lines("cargo build --release", &[]), None);
    }

    #[test]
    fn jobs_that_only_run_on_github_arent_counted_as_steps() {
        let p = plan(std::path::Path::new(env!("CARGO_MANIFEST_DIR"))).unwrap();
        assert!(
            p.github_only_jobs().iter().any(|j| j.contains("Arch")),
            "{:?}",
            p.github_only_jobs()
        );
        let arch_steps = p.skipped.iter().filter(|s| s.whole_job).count();
        assert!(arch_steps > 5);
        assert_eq!(p.total(), p.steps.len() + p.skipped.len() - arch_steps);
    }
}
