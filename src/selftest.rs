// SPDX-License-Identifier: GPL-3.0-or-later
//! The self-test: proves each feature works, in a sandbox under the cache
//! folder, against a fake GitHub. Never touches the real workspace and never
//! writes to GitHub. Produces a report with an AI fix prompt per failure.

use crate::cleanup;
use crate::config::{AppPaths, Config, APP_VERSION};
use crate::detect::{self, Stack};
use crate::extract::{self, Limits};
use crate::fixtures::{fake_token, Sandbox};
use crate::git;
use crate::health::{Health, HealthState};
use crate::history::History;
use crate::install::InstallPaths;
use crate::instance;
use crate::logbook::Logger;
use crate::pipeline::{self, Mode, PushRequest, StageRequest};
use crate::proc::{self, Opts};
use crate::report::{self, RedactContext};
use crate::secrets;
use crate::uninstall;
use crate::updater;
use crate::util;
use crate::version::{self, Channel};
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::time::Instant;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Pass,
    Fail,
    Skip,
}

#[derive(Debug, Clone)]
pub struct CheckResult {
    pub id: &'static str,
    pub area: &'static str,
    pub name: &'static str,
    pub module: &'static str,
    pub outcome: Outcome,
    pub millis: u128,
    /// Skip reason, or failure "expected … / actual …".
    pub detail: String,
    pub expected: String,
    pub actual: String,
    pub next_step: String,
}

#[derive(Debug, Clone, Default)]
pub struct SelfTestReport {
    pub checks: Vec<CheckResult>,
    pub env: Vec<(String, String)>,
    pub kernel: String,
}

/// A failed expectation.
pub struct Fail {
    expected: String,
    actual: String,
}

type CheckFn<'a> = Box<dyn FnOnce() -> Result<Option<String>, Fail> + 'a>;

fn fail(expected: impl Into<String>, actual: impl Into<String>) -> Fail {
    Fail {
        expected: expected.into(),
        actual: actual.into(),
    }
}

macro_rules! ensure {
    ($cond:expr, $exp:expr, $act:expr) => {
        if !$cond {
            return Err(fail($exp, $act));
        }
    };
}

impl SelfTestReport {
    pub fn counts(&self) -> (usize, usize, usize) {
        let c = |o| self.checks.iter().filter(|r| r.outcome == o).count();
        (c(Outcome::Pass), c(Outcome::Fail), c(Outcome::Skip))
    }

    pub fn summary_line(&self) -> String {
        let (p, f, s) = self.counts();
        format!("{p} working · {f} need fixing · {s} skipped")
    }

    pub fn ai_prompts(&self) -> String {
        let mut s = String::new();
        for c in self.checks.iter().filter(|c| c.outcome == Outcome::Fail) {
            s.push_str(&format!(
                "StowLock {APP_VERSION} self-test {} ({}) failed. Expected: {} Actual: {} Module: {}. Fix the cause without changing behaviour elsewhere, and add a regression test.\n",
                c.id, c.area, c.expected, c.actual, c.module
            ));
        }
        s
    }

    pub fn to_markdown(&self) -> String {
        let mut s = format!(
            "# StowLock self-test\n\n**{}**  \nStowLock {APP_VERSION} · {}\n\n",
            self.summary_line(),
            util::now_iso()
        );
        s.push_str("## Summary\n\n| ID | Area | Check | Result | Time |\n| --- | --- | --- | --- | --- |\n");
        for c in &self.checks {
            let r = match c.outcome {
                Outcome::Pass => "PASS",
                Outcome::Fail => "FAIL",
                Outcome::Skip => "SKIP",
            };
            s.push_str(&format!(
                "| {} | {} | {} | {r} | {} ms |\n",
                c.id, c.area, c.name, c.millis
            ));
        }
        let fails: Vec<&CheckResult> = self
            .checks
            .iter()
            .filter(|c| c.outcome == Outcome::Fail)
            .collect();
        if !fails.is_empty() {
            s.push_str("\n## Needs fixing\n\n");
            for c in &fails {
                s.push_str(&format!(
                    "### {} {}: {}\n\n- **Expected:** {}\n- **Actual:** {}\n- **Module:** `{}`\n- **Next step:** {}\n\n",
                    c.id, c.area, c.name, c.expected, c.actual, c.module, c.next_step
                ));
            }
        }
        let skips: Vec<&CheckResult> = self
            .checks
            .iter()
            .filter(|c| c.outcome == Outcome::Skip)
            .collect();
        if !skips.is_empty() {
            s.push_str("\n## Skipped\n\n");
            for c in skips {
                s.push_str(&format!("- {} {}: {}\n", c.id, c.name, c.detail));
            }
        }
        s.push_str(&format!(
            "\n## Environment\n\n- **StowLock:** {APP_VERSION}\n- **Kernel:** {}\n",
            self.kernel
        ));
        for (t, v) in &self.env {
            s.push_str(&format!("- **{t}:** {v}\n"));
        }
        if !fails.is_empty() {
            s.push_str(&format!(
                "\n## AI fix prompt\n\n```text\n{}```\n",
                self.ai_prompts()
            ));
        }
        s
    }

    /// Feed results into the Health view.
    pub fn apply_to_health(&self, paths: &AppPaths) {
        let mut h = HealthState::load(paths);
        let mut by_area: std::collections::BTreeMap<&str, Vec<&CheckResult>> = Default::default();
        for c in &self.checks {
            by_area.entry(c.area).or_default().push(c);
        }
        for (area, cs) in by_area {
            let status = if cs.iter().any(|c| c.outcome == Outcome::Fail) {
                Health::Broken
            } else if cs.iter().all(|c| c.outcome == Outcome::Skip) {
                Health::NeedsAttention
            } else {
                Health::Working
            };
            let hint = cs
                .iter()
                .find(|c| c.outcome != Outcome::Pass)
                .map(|c| {
                    if c.outcome == Outcome::Fail {
                        c.next_step.clone()
                    } else {
                        c.detail.clone()
                    }
                })
                .unwrap_or_default();
            h.set(area, status, "self-test", &hint);
        }
        let _ = h.save(paths);
    }
}

struct Runner {
    results: Vec<CheckResult>,
}

impl Runner {
    fn check(
        &mut self,
        id: &'static str,
        area: &'static str,
        name: &'static str,
        module: &'static str,
        next: &str,
        f: CheckFn,
    ) {
        let t = Instant::now();
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)).unwrap_or_else(|p| {
            let msg = p
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| p.downcast_ref::<&str>().map(|s| s.to_string()))
                .unwrap_or_else(|| "panic".into());
            Err(fail("no crash", format!("crashed: {msg}")))
        });
        let (outcome, detail, expected, actual) = match r {
            Ok(None) => (Outcome::Pass, String::new(), String::new(), String::new()),
            Ok(Some(reason)) => (Outcome::Skip, reason, String::new(), String::new()),
            Err(f) => (
                Outcome::Fail,
                format!("expected {} / actual {}", f.expected, f.actual),
                f.expected,
                f.actual,
            ),
        };
        self.results.push(CheckResult {
            id,
            area,
            name,
            module,
            outcome,
            millis: t.elapsed().as_millis(),
            detail,
            expected,
            actual,
            next_step: next.to_string(),
        });
    }
}

fn mode_of(p: &Path) -> u32 {
    std::fs::metadata(p)
        .map(|m| m.permissions().mode() & 0o777)
        .unwrap_or(0)
}

/// Run every check. `online` adds read-only GitHub checks.
pub fn run(real: &AppPaths, online: bool) -> SelfTestReport {
    let root = real
        .cache_dir
        .join(format!("selftest-{}", util::now_stamp_secs()));
    let _ = util::ensure_private_dir(&real.cache_dir);
    let mut rn = Runner {
        results: Vec::new(),
    };
    let sb = match Sandbox::new(&root) {
        Ok(s) => s,
        Err(e) => {
            rn.results.push(CheckResult {
                id: "T00",
                area: "Settings & state",
                name: "Create sandbox",
                module: "selftest.rs",
                outcome: Outcome::Fail,
                millis: 0,
                detail: e.clone(),
                expected: "a sandbox folder".into(),
                actual: e,
                next_step: "Check that the cache folder is writable.".into(),
            });
            return SelfTestReport {
                checks: rn.results,
                ..Default::default()
            };
        }
    };
    let tmp = root.join("scratch");
    let _ = std::fs::create_dir_all(&tmp);

    rn.check(
        "T01",
        "Tools & sign-in",
        "git and gh present; signed in; credential helper set",
        "auth.rs",
        "Install git and gh from Settings → GitHub account, then sign in.",
        Box::new(|| {
            ensure!(
                proc::which("git").is_some(),
                "git installed",
                "git not found"
            );
            ensure!(proc::which("gh").is_some(), "gh installed", "gh not found");
            if !online {
                return Ok(Some(
                    "tools found; sign-in not checked (run with online checks)".into(),
                ));
            }
            let st = proc::run(
                "gh",
                &["auth", "status", "--hostname", "github.com"],
                &Opts::default_timeout(),
            );
            ensure!(st.ok(), "signed in to GitHub", "gh auth status failed");
            let helper = proc::run(
                "git",
                &["config", "--global", "--get-regexp", "credential.*helper"],
                &Opts::default_timeout(),
            );
            ensure!(
                helper.stdout.contains("gh"),
                "git credential helper uses gh",
                "no gh credential helper (run gh auth setup-git)"
            );
            let one = proc::run(
                "gh",
                &["repo", "list", "--limit", "1"],
                &Opts::default_timeout(),
            );
            ensure!(one.ok(), "repo list readable", "gh repo list failed");
            Ok(None)
        }),
    );

    rn.check(
        "T02",
        "Zip intake",
        "Slug rules, name formatting and version detection",
        "detect.rs",
        "Fix slug_from_filename suffix patterns.",
        Box::new(|| {
            for (input, want) in [
                ("sluice-v2.zip", "sluice"),
                ("My App (1).zip", "my-app"),
                ("tool-2026-11-22.zip", "tool"),
                ("sluice-main.zip", "sluice"),
                ("x-v0.2.0-beta.1.zip", "x"),
            ] {
                let got = detect::slug_from_filename(input);
                ensure!(
                    got == want,
                    format!("{input} → {want}"),
                    format!("{input} → {got}")
                );
            }
            let typed = detect::format_typing("My Cool App");
            ensure!(
                typed == "my-cool-app",
                "\"My Cool App\" → my-cool-app",
                typed
            );
            ensure!(
                detect::valid_slug("Sluice"),
                "existing name \"Sluice\" accepted",
                "rejected"
            );
            let zp = tmp.join("sluice-v0.11.7.zip");
            extract::write_zip(&zp, &[("a.txt", b"a", 0o644)]).map_err(|e| fail("zip", e))?;
            let v = detect::detect_version(&zp, "v")
                .map(|v| v.tag)
                .unwrap_or_default();
            ensure!(v == "v0.11.7", "version v0.11.7 read from the zip name", v);
            Ok(None)
        }),
    );

    rn.check(
        "T03",
        "Extraction",
        "Rejects ../ paths, absolute paths and symlinks",
        "extract.rs",
        "Make sure every entry uses enclosed_name() and symlinks are skipped.",
        Box::new(|| {
            use std::io::Write;
            let zp = tmp.join("evil.zip");
            let f = std::fs::File::create(&zp).map_err(|e| fail("zip created", e.to_string()))?;
            let mut z = zip::ZipWriter::new(f);
            let o = zip::write::SimpleFileOptions::default();
            for n in ["../evil.txt", "/abs-evil.txt"] {
                z.start_file(n, o)
                    .map_err(|e| fail("zip entry", e.to_string()))?;
                z.write_all(b"x").ok();
            }
            z.add_symlink("link", "/etc/passwd", o)
                .map_err(|e| fail("zip symlink", e.to_string()))?;
            z.start_file("ok.txt", o).ok();
            z.write_all(b"ok").ok();
            z.finish().map_err(|e| fail("zip written", e.to_string()))?;
            let out = tmp.join("evil-out");
            let r = extract::extract_zip(&zp, &out).map_err(|e| fail("extraction runs", e))?;
            ensure!(
                !tmp.join("evil.txt").exists(),
                "nothing written outside",
                "../evil.txt escaped"
            );
            ensure!(
                !out.join("link").exists(),
                "symlink skipped",
                "symlink extracted"
            );
            ensure!(
                out.join("ok.txt").exists(),
                "safe file extracted",
                "ok.txt missing"
            );
            ensure!(
                !r.skipped_symlinks.is_empty(),
                "symlink reported",
                "not reported"
            );
            Ok(None)
        }),
    );

    rn.check(
        "T04",
        "Extraction",
        "Zip bomb guard",
        "extract.rs",
        "Enforce the byte and entry limits while writing.",
        Box::new(|| {
            let zp = tmp.join("bomb.zip");
            extract::write_zip(&zp, &[("big.txt", &vec![b'a'; 300_000], 0o644)])
                .map_err(|e| fail("zip", e))?;
            let r = extract::extract_zip_with(
                &zp,
                &tmp.join("bomb-out"),
                &Limits {
                    max_bytes: 100_000,
                    max_entries: 100,
                },
            );
            ensure!(r.is_err(), "extraction refused", "extracted anyway");
            Ok(None)
        }),
    );

    rn.check(
        "T05",
        "Extraction",
        "Unwraps one top folder only; keeps executable bits",
        "extract.rs",
        "Check unwrap_single_folder and the permission logic.",
        Box::new(|| {
            let zp = tmp.join("wrap.zip");
            extract::write_zip(
                &zp,
                &[
                    ("app-main/a.txt", b"a", 0o644),
                    ("app-main/tool", b"bin", 0o755),
                    ("app-main/s.sh", b"#!/bin/sh\n", 0o644),
                ],
            )
            .map_err(|e| fail("zip", e))?;
            let out = tmp.join("wrap-out");
            let r = extract::extract_zip(&zp, &out).map_err(|e| fail("extract", e))?;
            ensure!(
                r.unwrapped.as_deref() == Some("app-main"),
                "unwrapped app-main",
                format!("{:?}", r.unwrapped)
            );
            ensure!(
                mode_of(&out.join("tool")) & 0o111 != 0,
                "tool executable",
                "not executable"
            );
            ensure!(
                mode_of(&out.join("s.sh")) & 0o111 != 0,
                "shebang file executable",
                "not executable"
            );
            Ok(None)
        }),
    );

    rn.check(
        "T06",
        "Clean-up",
        "Removes junk and node_modules; keeps .env; .gitignore gets .env once",
        "cleanup.rs",
        "Check JUNK_DIRS and add_env_rules.",
        Box::new(|| {
            let d = tmp.join("clean");
            std::fs::create_dir_all(d.join("node_modules/x")).ok();
            std::fs::write(d.join(".DS_Store"), "x").ok();
            std::fs::write(d.join(".env"), "S=1").ok();
            std::fs::write(d.join(".gitignore"), "dist/\n").ok();
            cleanup::cleanup(&d, Stack::Node).map_err(|e| fail("clean-up runs", e))?;
            cleanup::cleanup(&d, Stack::Node).map_err(|e| fail("clean-up runs twice", e))?;
            let gi = std::fs::read_to_string(d.join(".gitignore")).unwrap_or_default();
            ensure!(
                !d.join("node_modules").exists() && !d.join(".DS_Store").exists(),
                "junk removed",
                "junk still there"
            );
            ensure!(d.join(".env").exists(), ".env kept", ".env deleted");
            ensure!(
                gi.matches("\n.env\n").count() == 1,
                ".env rule once",
                format!("{} times", gi.matches("\n.env\n").count())
            );
            Ok(None)
        }),
    );

    rn.check(
        "T07",
        "Stack detection",
        "Rust, Node, Python, Go and generic",
        "detect.rs",
        "Check detect_stack marker files.",
        Box::new(|| {
            for (file, want) in [
                ("Cargo.toml", Stack::Rust),
                ("package.json", Stack::Node),
                ("main.py", Stack::Python),
                ("go.mod", Stack::Go),
                ("notes.md", Stack::Other),
            ] {
                let d = tmp.join(format!("stack-{file}"));
                std::fs::create_dir_all(&d).ok();
                std::fs::write(d.join(file), "x").ok();
                let got = detect::detect_stack(&d);
                ensure!(got == want, format!("{file} → {}", want.name()), got.name());
            }
            Ok(None)
        }),
    );

    rn.check(
        "T08",
        "Clean-up",
        "File over 100 MB blocks push",
        "cleanup.rs",
        "Check size_check thresholds.",
        Box::new(|| {
            let d = tmp.join("big");
            std::fs::create_dir_all(&d).ok();
            let f = std::fs::File::create(d.join("huge.bin"))
                .map_err(|e| fail("file", e.to_string()))?;
            f.set_len(cleanup::BLOCK_BYTES + 1).ok();
            let (_, block) = cleanup::size_check(&d, &["huge.bin".into()]);
            ensure!(block.len() == 1, "blocked", "not blocked");
            Ok(None)
        }),
    );

    rn.check(
        "T09",
        "Secret scan",
        "Finds a planted fake key, masks it, ignores .env.example",
        "secrets.rs",
        "Check PATTERNS and masking.",
        Box::new(|| {
            let d = tmp.join("sec");
            std::fs::create_dir_all(&d).ok();
            let tok = fake_token();
            std::fs::write(d.join("cfg.js"), format!("k='{tok}'")).ok();
            std::fs::write(d.join(".env.example"), format!("K={tok}")).ok();
            let hits = secrets::scan_files(&d, &["cfg.js".into(), ".env.example".into()]);
            ensure!(hits.len() == 1, "1 hit", format!("{} hits", hits.len()));
            ensure!(
                !hits[0].masked.contains(&tok[4..]),
                "masked value",
                "full value shown"
            );
            Ok(None)
        }),
    );

    let log = sb.ctx.log.clone();
    rn.check(
        "T10",
        "Tests",
        "Passing project passes, failing project fails",
        "preflight.rs",
        "Check run_tests for Python.",
        Box::new(|| {
            if proc::which("python3").is_none() {
                return Ok(Some("python3 not installed".into()));
            }
            let env = crate::preflight::TestEnv {
                build_cache: None,
                match_github: false,
                exact: false,
                clean_home: false,
                timeout_secs: 60,
                cancel: Default::default(),
                pass_env: false,
                log: &log,
            };
            let d = tmp.join("py");
            std::fs::create_dir_all(&d).ok();
            std::fs::write(d.join("main.py"), "print(1)\n").ok();
            let ok = crate::preflight::run_tests(&d, Stack::Python, &env);
            ensure!(
                ok.status == crate::preflight::TestStatus::Passed,
                "passed",
                ok.summary
            );
            std::fs::write(d.join("bad.py"), "def x(:\n").ok();
            let bad = crate::preflight::run_tests(&d, Stack::Python, &env);
            ensure!(
                bad.status == crate::preflight::TestStatus::Failed,
                "failed",
                bad.summary
            );
            Ok(None)
        }),
    );

    // T11–T16 share one fake repo, in order.
    let ws = sb.ctx.workspace();
    let owner = sb.owner.clone();
    let req = |zip: &Path, mode| StageRequest {
        zip: zip.to_path_buf(),
        slug: "selftest-app".into(),
        owner: owner.clone(),
        mode,
        replace_local: false,
        description: "Self-test".into(),
        license: "MIT".into(),
        license_holder: "Self-Test".into(),
    };
    let preq = |mode, tag: &str, channel, zip: Option<std::path::PathBuf>| PushRequest {
        slug: "selftest-app".into(),
        owner: owner.clone(),
        mode,
        private: true,
        description: "Self-test".into(),
        topics: vec![],
        tag: tag.into(),
        channel,
        commit_message: version::default_commit_message(mode == Mode::New, tag),
        archive_zip: zip,
        archive_move: true,
    };
    let zip1 = sb.python_zip(
        "selftest-app-main.zip",
        "selftest-app-main",
        &[("old.py", b"x = 1\n")],
    );
    rn.check(
        "T11",
        "New repo push",
        "Init, commit and push to the fake remote; one commit on main",
        "pipeline.rs",
        "Check stage_new and push (Mode::New).",
        Box::new(|| {
            pipeline::stage(&sb.ctx, &req(&zip1, Mode::New)).map_err(|e| fail("staged", e))?;
            pipeline::push(
                &sb.ctx,
                &preq(Mode::New, "v0.1.0", Channel::Stable, Some(zip1.clone())),
            )
            .map_err(|e| fail("pushed", e))?;
            let bare = sb.fake.bare(&owner, "selftest-app");
            let n = git::git_bare(&bare, &["rev-list", "--count", "main"]).stdout_trim();
            ensure!(n == "1", "1 commit", format!("{n} commits"));
            Ok(None)
        }),
    );
    rn.check(
        "T12",
        "Update sync",
        "Removed file disappears; .git kept; changes summary correct",
        "pipeline.rs",
        "Check stage_update's sync step.",
        Box::new(|| {
            let z = sb.zip(
                "selftest-app-v2.zip",
                &[
                    ("main.py", b"print(2)\n", 0o644),
                    ("util.py", b"def add(a, b):\n    return a + b\n", 0o644),
                    ("run.sh", b"#!/bin/sh\npython3 main.py\n", 0o755),
                ],
            );
            let st = pipeline::stage(&sb.ctx, &req(&z, Mode::Update))
                .map_err(|e| fail("staged update", e))?;
            let ch = st.changes.unwrap_or_default();
            ensure!(
                ch.removed.iter().any(|f| f == "old.py"),
                "old.py removed",
                ch.summary()
            );
            ensure!(
                ws.join("selftest-app/.git").exists(),
                ".git kept",
                ".git gone"
            );
            pipeline::push(&sb.ctx, &preq(Mode::Update, "", Channel::Stable, None))
                .map_err(|e| fail("pushed update", e))?;
            Ok(None)
        }),
    );
    rn.check(
        "T13",
        "Update sync",
        "Refuses a dirty tree and diverged history; never force-pushes",
        "pipeline.rs",
        "Check the status and pull --ff-only guards.",
        Box::new(|| {
            let dir = ws.join("selftest-app");
            let z = sb.zip("selftest-app-v3.zip", &[("main.py", b"print(3)\n", 0o644)]);
            std::fs::write(dir.join("wip.txt"), "x").ok();
            let e = pipeline::stage(&sb.ctx, &req(&z, Mode::Update));
            ensure!(
                e.as_ref()
                    .err()
                    .map(|s| s.contains("unsaved"))
                    .unwrap_or(false),
                "dirty tree refused",
                format!("{e:?}")
            );
            std::fs::remove_file(dir.join("wip.txt")).ok();
            let bare = sb.fake.bare(&owner, "selftest-app");
            let other = tmp.join("other");
            git::git(
                &tmp,
                &[
                    "clone",
                    &bare.display().to_string(),
                    &other.display().to_string(),
                ],
            );
            std::fs::write(other.join("r.txt"), "r").ok();
            git::git(&other, &["add", "-A"]);
            git::git(
                &other,
                &[
                    "-c",
                    "user.name=s",
                    "-c",
                    "user.email=s@x",
                    "commit",
                    "-m",
                    "remote",
                ],
            );
            git::git(&other, &["push", "origin", "main"]);
            std::fs::write(dir.join("l.txt"), "l").ok();
            git::git(&dir, &["add", "-A"]);
            git::git(
                &dir,
                &[
                    "-c",
                    "user.name=s",
                    "-c",
                    "user.email=s@x",
                    "commit",
                    "-m",
                    "local",
                ],
            );
            let e2 = pipeline::stage(&sb.ctx, &req(&z, Mode::Update));
            ensure!(e2.is_err(), "diverged history refused", "staged anyway");
            let head = git::git_bare(&bare, &["log", "-1", "--format=%s", "main"]).stdout_trim();
            ensure!(
                head == "remote",
                "remote untouched",
                format!("remote head is {head}")
            );
            git::git(&dir, &["reset", "--hard", "origin/main"]);
            git::git(&dir, &["pull", "--ff-only", "origin", "main"]);
            Ok(None)
        }),
    );
    rn.check(
        "T14",
        "Tags & releases",
        "Stable and beta suggestions; beta marked pre-release",
        "version.rs",
        "Check suggest() and create_release prerelease flag.",
        Box::new(|| {
            let t = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
            ensure!(
                version::suggest(&t(&["v0.1.0"]), Channel::Stable, "v") == "v0.2.0",
                "v0.2.0",
                version::suggest(&t(&["v0.1.0"]), Channel::Stable, "v")
            );
            ensure!(
                version::suggest(&t(&["v0.1.0", "v0.2.0-beta.1"]), Channel::Beta, "v")
                    == "v0.2.0-beta.2",
                "v0.2.0-beta.2",
                "other"
            );
            let z = sb.zip("selftest-app-v4.zip", &[("main.py", b"print(4)\n", 0o644)]);
            pipeline::stage(&sb.ctx, &req(&z, Mode::Update)).map_err(|e| fail("staged", e))?;
            pipeline::push(
                &sb.ctx,
                &preq(Mode::Update, "v0.2.0-beta.1", Channel::Beta, None),
            )
            .map_err(|e| fail("pushed beta", e))?;
            let rel = sb.fake.releases(&owner, "selftest-app");
            ensure!(
                rel.contains(&("v0.2.0-beta.1".to_string(), true)),
                "beta release marked pre-release",
                format!("{rel:?}")
            );
            Ok(None)
        }),
    );
    rn.check(
        "T15",
        "Archive",
        "Correct name; never overwrites; Archive Only pushes nothing",
        "archive.rs",
        "Check archive_name and archive_only.",
        Box::new(|| {
            let archived = sb
                .ctx
                .config
                .archive_root()
                .join("selftest-app/selftest-app-v0.1.0.zip");
            ensure!(
                archived.exists(),
                "zip archived as selftest-app-v0.1.0.zip",
                "not found"
            );
            let bare = sb.fake.bare(&owner, "selftest-app");
            let before = git::git_bare(&bare, &["rev-list", "--count", "main"]).stdout_trim();
            for _ in 0..2 {
                let z = sb.zip("old.zip", &[("a.txt", b"a", 0o644)]);
                pipeline::archive_only(&sb.ctx, &z, "selftest-app", Some("v0.0.1"), true)
                    .map_err(|e| fail("archived", e))?;
            }
            let dir = sb.ctx.config.archive_root().join("selftest-app");
            ensure!(
                dir.join("selftest-app-v0.0.1.zip").exists()
                    && dir.join("selftest-app-v0.0.1-2.zip").exists(),
                "no overwrite",
                "file overwritten"
            );
            let after = git::git_bare(&bare, &["rev-list", "--count", "main"]).stdout_trim();
            ensure!(before == after, "nothing pushed", "a commit appeared");
            Ok(None)
        }),
    );
    rn.check(
        "T16",
        "History",
        "Pushes recorded; Pushed today count correct",
        "history.rs",
        "Check History::append in push().",
        Box::new(|| {
            let n = History::load(&sb.ctx.paths).today().len();
            ensure!(n == 3, "3 pushes today", format!("{n}"));
            Ok(None)
        }),
    );

    rn.check(
        "T17",
        "Settings & state",
        "Corrupt JSON recovered; atomic private writes",
        "util.rs",
        "Check read_json_or_default and atomic_write.",
        Box::new(|| {
            let p = tmp.join("state.json");
            std::fs::write(&p, "{broken").ok();
            let v: std::collections::BTreeMap<String, String> = util::read_json_or_default(&p);
            ensure!(
                v.is_empty() && !p.exists(),
                "recovered with a backup",
                "corrupt file still in place"
            );
            util::atomic_write(&p, b"{}").map_err(|e| fail("written", e))?;
            ensure!(mode_of(&p) == 0o600, "0600", format!("{:o}", mode_of(&p)));
            Ok(None)
        }),
    );

    rn.check(
        "T18",
        "Bug report",
        "Redaction removes tokens, secrets and home path",
        "report.rs",
        "Check report::redact rules.",
        Box::new(|| {
            let rc = RedactContext {
                home: "/home/selftest".into(),
                username: "selftest".into(),
                hostname: "st-host".into(),
                logins: vec!["st-user".into()],
                repos: vec![("st-repo".into(), true)],
                ..Default::default()
            };
            let tok = fake_token();
            let out = report::redact(
                &format!("/home/selftest st-host st-user st-repo {tok} a@b.io 10.0.0.1"),
                &rc,
            );
            for bad in [
                "/home/", "st-host", "st-user", "st-repo", &tok, "a@b.io", "10.0.0.1",
            ] {
                ensure!(!out.contains(bad), format!("{bad} removed"), out.clone());
            }
            Ok(None)
        }),
    );

    rn.check(
        "T19",
        "Run/Editor",
        "A terminal launcher is found",
        "pipeline.rs",
        "Install konsole or another terminal.",
        Box::new(|| match pipeline::terminal_available() {
            Some(_) => Ok(None),
            None => Ok(Some(
                "no terminal app found (Run needs konsole or similar)".into(),
            )),
        }),
    );

    rn.check(
        "T20",
        "Self-update",
        "Update channels; version comparison; checksum mismatch and bad signatures rejected",
        "updater.rs / signing.rs",
        "Check is_newer, verify_download and the built-in signing keys.",
        Box::new(|| {
            ensure!(
                updater::is_newer("v9.0.0", APP_VERSION, false)
                    && !updater::is_newer("v0.0.1", APP_VERSION, true),
                "newer detected, older never offered",
                "wrong comparison"
            );
            ensure!(
                !updater::is_newer("v9.0.0-beta.1", APP_VERSION, false)
                    && updater::is_newer("v9.0.0-beta.1", APP_VERSION, true),
                "Stable skips betas, Beta offers them",
                "wrong channel handling"
            );
            let f = tmp.join("bin");
            std::fs::write(&f, "x").ok();
            ensure!(
                updater::verify_checksum(&f, &format!("{}  bin", "0".repeat(64))).is_err(),
                "mismatch rejected",
                "accepted"
            );
            let keys = crate::signing::trusted_keys();
            ensure!(
                keys.len() == 2,
                "both signing keys built in",
                "signing keys missing"
            );
            let sums = format!(
                "{}  {}\n",
                updater::sha256_hex(&f).unwrap_or_default(),
                updater::ASSET
            );
            ensure!(
                updater::verify_download(&f, sums.as_bytes(), "not a signature", &keys, "v9.0.0")
                    .is_err(),
                "unsigned download rejected",
                "accepted without a signature"
            );
            Ok(None)
        }),
    );

    rn.check(
        "T21",
        "Uninstall",
        "Each level lists exactly its paths; Everything is guarded",
        "uninstall.rs",
        "Check plan() and workspace_refusals().",
        Box::new(|| {
            let home = tmp.join("home");
            let paths = AppPaths::under(&home.join("app"));
            paths.ensure().map_err(|e| fail("paths", e))?;
            let ws = home.join("GitHub_Projects");
            std::fs::create_dir_all(ws.join("p")).ok();
            let cfg = Config {
                workspace_dir: ws.display().to_string(),
                ..Config::default()
            };
            let inst = InstallPaths::under(&home);
            for lvl in [uninstall::Level::AppOnly, uninstall::Level::AppAndData] {
                let p = uninstall::plan(lvl, &paths, &cfg, &inst, &home);
                ensure!(
                    p.items.iter().all(|i| !i.path.starts_with(&ws)),
                    "workspace untouched",
                    format!("{lvl:?} includes workspace")
                );
            }
            let e = uninstall::plan(uninstall::Level::Everything, &paths, &cfg, &inst, &home);
            ensure!(
                uninstall::execute(&e, false, false).is_err(),
                "needs typed confirmation",
                "ran without it"
            );
            ensure!(
                !uninstall::workspace_refusals(&home, &home).is_empty(),
                "home folder refused",
                "allowed"
            );
            ensure!(ws.join("p").exists(), "projects intact", "deleted");
            Ok(None)
        }),
    );

    rn.check(
        "T22",
        "Settings & state",
        "Single-instance lock and pending hand-off",
        "instance.rs",
        "Check acquire() and take_pending().",
        Box::new(|| {
            let paths = AppPaths::under(&tmp.join("inst"));
            let g = instance::acquire(&paths).map_err(|e| fail("lock", e))?;
            ensure!(
                matches!(g, instance::Acquire::First(_)),
                "first copy gets the lock",
                "didn't"
            );
            ensure!(
                matches!(
                    instance::acquire(&paths),
                    Ok(instance::Acquire::AlreadyRunning)
                ),
                "second copy refused",
                "allowed"
            );
            let z = tmp.join("h.zip");
            std::fs::write(&z, "z").ok();
            instance::hand_off(&paths, &z).map_err(|e| fail("hand-off", e))?;
            ensure!(
                instance::take_pending(&paths).len() == 1,
                "zip handed over",
                "not received"
            );
            Ok(None)
        }),
    );

    let sb_paths = sb.ctx.paths.clone();
    let sb_log = sb.ctx.log.clone();
    rn.check(
        "T23",
        "Settings & state",
        "Privacy: no token in logs or reports; private files; clean Run environment",
        "report.rs / proc.rs",
        "Check write-time redaction, file modes and env scrubbing.",
        Box::new(|| {
            let tok = fake_token();
            sb_log.info("selftest", &format!("planted {tok}"));
            let log_text = std::fs::read_to_string(sb_paths.log_file()).unwrap_or_default();
            ensure!(
                !log_text.contains(&tok),
                "token absent from log file",
                "token found in log"
            );
            let rc = RedactContext::local(&ws);
            let rep = report::build_report(
                &report::ReportInput {
                    title: "privacy check".into(),
                    log_lines: sb_log.last_lines(500),
                    ..Default::default()
                },
                &rc,
            );
            ensure!(
                !rep.contains(&tok),
                "token absent from report",
                "token in report"
            );
            let home = util::home_dir().display().to_string();
            ensure!(
                home.len() < 2 || !rep.contains(&home),
                "home path absent",
                "home path in report"
            );
            ensure!(
                mode_of(&sb_paths.state_dir) == 0o700,
                "state folder 0700",
                format!("{:o}", mode_of(&sb_paths.state_dir))
            );
            ensure!(
                mode_of(&sb_paths.log_file()) == 0o600,
                "log 0600",
                format!("{:o}", mode_of(&sb_paths.log_file()))
            );
            ensure!(
                mode_of(&sb_paths.projects_file()) == 0o600,
                "projects.json 0600",
                format!("{:o}", mode_of(&sb_paths.projects_file()))
            );
            std::env::set_var("GITHUB_TOKEN_STOWLOCK_SELFTEST", &tok);
            let o = proc::run(
                "sh",
                &["-c", "echo ${GITHUB_TOKEN_STOWLOCK_SELFTEST:-none}"],
                &Opts {
                    scrub_env: true,
                    timeout: Some(std::time::Duration::from_secs(10)),
                    ..Default::default()
                },
            );
            std::env::remove_var("GITHUB_TOKEN_STOWLOCK_SELFTEST");
            ensure!(
                o.stdout_trim() == "none",
                "token-like variable removed",
                "variable passed through"
            );
            Ok(None)
        }),
    );

    let env = report::collect_env(None, None);
    let _ = std::fs::remove_dir_all(&root);
    let _ = Logger::default();
    SelfTestReport {
        checks: rn.results,
        env: env.tools,
        kernel: env.kernel,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn self_test_passes_here() {
        let d = tempfile::tempdir().unwrap();
        let paths = AppPaths::under(d.path());
        paths.ensure().unwrap();
        let r = run(&paths, false);
        let fails: Vec<String> = r
            .checks
            .iter()
            .filter(|c| c.outcome == Outcome::Fail)
            .map(|c| format!("{} {}: {}", c.id, c.name, c.detail))
            .collect();
        assert!(fails.is_empty(), "{fails:#?}");
        assert_eq!(r.checks.len(), 23);
        let md = r.to_markdown();
        assert!(md.contains("## Summary") && md.contains("## Environment"));
        r.apply_to_health(&paths);
        assert!(HealthState::load(&paths).get("Extraction").status == Health::Working);
        assert!(
            !d.path().join("cache").read_dir().unwrap().any(|e| e
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with("selftest-")),
            "sandbox removed"
        );
    }
}
