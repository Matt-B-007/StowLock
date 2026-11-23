// SPDX-License-Identifier: GPL-3.0-or-later
//! Bug, crash and self-test reports, with identity protection: secrets,
//! home path, usernames, GitHub accounts, repo names, emails, hostname, IP
//! and MAC addresses are replaced before anything is shown or shared.

use crate::config::{AppPaths, APP_VERSION};
use crate::logbook::{Logger, Step, StepStatus};
use crate::proc::{self, Opts};
use crate::secrets::redact_secrets;
use crate::util::{self, Res};
use regex::Regex;
use std::path::PathBuf;
use std::sync::OnceLock;

#[derive(Debug, Clone, Default)]
pub struct RedactContext {
    pub home: String,
    pub username: String,
    pub hostname: String,
    pub workspace: String,
    pub logins: Vec<String>,
    pub orgs: Vec<String>,
    /// (name, private)
    pub repos: Vec<(String, bool)>,
    pub include_public_repo_names: bool,
    pub include_username: bool,
}

impl RedactContext {
    /// Fill in this machine's details (home, user, host).
    pub fn local(workspace: &std::path::Path) -> Self {
        let home = util::home_dir().display().to_string();
        let username = std::env::var("USER")
            .or_else(|_| std::env::var("LOGNAME"))
            .unwrap_or_default();
        let hostname = std::fs::read_to_string("/etc/hostname")
            .map(|s| s.trim().to_string())
            .ok()
            .filter(|s| !s.is_empty())
            .or_else(|| {
                proc::run("uname", &["-n"], &Opts::default_timeout())
                    .stdout
                    .trim()
                    .to_string()
                    .into()
            })
            .unwrap_or_default();
        RedactContext {
            home,
            username,
            hostname,
            workspace: workspace.display().to_string(),
            ..Default::default()
        }
    }
}

fn email_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}").unwrap())
}
fn ip_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"\b(?:\d{1,3}\.){3}\d{1,3}\b").unwrap())
}
fn ipv6_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"\b(?:[0-9a-fA-F]{1,4}:){4,7}[0-9a-fA-F]{1,4}\b").unwrap())
}
fn mac_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"\b(?:[0-9A-Fa-f]{2}[:-]){5}[0-9A-Fa-f]{2}\b").unwrap())
}
fn serial_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"(?i)serial(\s*(number|no\.?))?\s*[:=]\s*\S+").unwrap())
}
fn gh_url_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| {
        Regex::new(
            r"(?:https?://|git@)github\.com[/:]([A-Za-z0-9_.-]+)/([A-Za-z0-9_.-]+?)(?:\.git)?\b",
        )
        .unwrap()
    })
}

fn word(s: &str) -> Regex {
    Regex::new(&format!(
        r"(?i)(^|[^A-Za-z0-9_]){}($|[^A-Za-z0-9_])",
        regex::escape(s)
    ))
    .unwrap()
}

fn replace_word(text: &str, needle: &str, with: &str) -> String {
    if needle.trim().is_empty() {
        return text.to_string();
    }
    let re = word(needle);
    // Run twice so adjacent matches sharing a boundary are both replaced.
    let once = re
        .replace_all(text, |c: &regex::Captures| {
            format!("{}{with}{}", &c[1], &c[2])
        })
        .into_owned();
    re.replace_all(&once, |c: &regex::Captures| {
        format!("{}{with}{}", &c[1], &c[2])
    })
    .into_owned()
}

/// Apply every identity and secret rule to a piece of text.
/// Words that are never redacted: the app's own name.
const KEEP: [&str; 2] = ["StowLock", "stowlock"];

pub fn redact(text: &str, rc: &RedactContext) -> String {
    let mut s = redact_secrets(text);
    // Protect the app's own name (a user's repo may share it).
    for (i, k) in KEEP.iter().enumerate() {
        s = s.replace(k, &format!("\u{2}APP{i}\u{2}"));
    }
    // Paths: workspace → project-relative, then home → ~.
    if !rc.workspace.is_empty() {
        s = s.replace(&format!("{}/", rc.workspace), "");
        s = s.replace(&rc.workspace, "<workspace>");
        let tilde_ws = rc.workspace.replacen(&rc.home, "~", 1);
        if !rc.home.is_empty() && tilde_ws != rc.workspace {
            s = s.replace(&format!("{tilde_ws}/"), "");
            s = s.replace(&tilde_ws, "<workspace>");
        }
    }
    if !rc.home.is_empty() {
        s = s.replace(&rc.home, "~");
    }
    // GitHub URLs → placeholders (handled with the account/repo maps below).
    let mut accounts: Vec<String> = Vec::new();
    let account_of = |name: &str, accounts: &mut Vec<String>| -> usize {
        let lower = name.to_ascii_lowercase();
        if let Some(i) = accounts.iter().position(|a| *a == lower) {
            i + 1
        } else {
            accounts.push(lower);
            accounts.len()
        }
    };
    let mut repo_names: Vec<String> = Vec::new();
    for (name, private) in &rc.repos {
        if *private || !rc.include_public_repo_names {
            repo_names.push(name.to_ascii_lowercase());
        }
    }
    let repo_idx = |name: &str, list: &mut Vec<String>| -> usize {
        let lower = name.to_ascii_lowercase();
        if let Some(i) = list.iter().position(|r| *r == lower) {
            i + 1
        } else {
            list.push(lower);
            list.len()
        }
    };
    let url_owners: Vec<(String, String)> = gh_url_re()
        .captures_iter(&s)
        .map(|c| (c[1].to_string(), c[2].to_string()))
        .collect();
    for (owner, repo) in &url_owners {
        let is_org = rc.orgs.iter().any(|o| o.eq_ignore_ascii_case(owner));
        let owner_ph = if is_org {
            format!(
                "<org-{}>",
                rc.orgs
                    .iter()
                    .position(|o| o.eq_ignore_ascii_case(owner))
                    .unwrap()
                    + 1
            )
        } else {
            format!("<account-{}>", account_of(owner, &mut accounts))
        };
        let keep_repo = rc.include_public_repo_names
            && rc
                .repos
                .iter()
                .any(|(n, p)| n.eq_ignore_ascii_case(repo) && !p);
        let repo_ph = if keep_repo {
            repo.clone()
        } else {
            format!("<repo-{}>", repo_idx(repo, &mut repo_names))
        };
        s = s.replace(
            &format!("https://github.com/{owner}/{repo}"),
            &format!("https://github.com/{owner_ph}/{repo_ph}"),
        );
        s = s.replace(
            &format!("git@github.com:{owner}/{repo}"),
            &format!("https://github.com/{owner_ph}/{repo_ph}"),
        );
    }
    // Emails (including noreply) before account names, so they vanish whole.
    s = email_re().replace_all(&s, "<email>").into_owned();
    for login in &rc.logins {
        if rc.include_username {
            continue;
        }
        let n = account_of(login, &mut accounts);
        s = replace_word(&s, login, &format!("<account-{n}>"));
    }
    for (i, org) in rc.orgs.iter().enumerate() {
        s = replace_word(&s, org, &format!("<org-{}>", i + 1));
    }
    let names = repo_names.clone();
    for (i, name) in names.iter().enumerate() {
        s = replace_word(&s, name, &format!("<repo-{}>", i + 1));
    }
    // Kept (opted-in) GitHub logins must survive the Linux username/host rules.
    let kept: Vec<&String> = if rc.include_username {
        rc.logins.iter().collect()
    } else {
        vec![]
    };
    for (i, login) in kept.iter().enumerate() {
        s = s.replace(login.as_str(), &format!("\u{1}KEPT{i}\u{1}"));
    }
    if !rc.hostname.is_empty() {
        s = replace_word(&s, &rc.hostname, "<host>");
    }
    if !rc.username.is_empty() && rc.username.len() > 1 {
        s = replace_word(&s, &rc.username, "<user>");
    }
    for (i, login) in kept.iter().enumerate() {
        s = s.replace(&format!("\u{1}KEPT{i}\u{1}"), login.as_str());
    }
    s = mac_re().replace_all(&s, "[removed]").into_owned();
    s = ipv6_re().replace_all(&s, "[removed]").into_owned();
    s = ip_re().replace_all(&s, "[removed]").into_owned();
    s = serial_re().replace_all(&s, "[removed]").into_owned();
    for (i, k) in KEEP.iter().enumerate() {
        s = s.replace(&format!("\u{2}APP{i}\u{2}"), k);
    }
    s
}

// ------------------------------------------------------------------ env info

#[derive(Debug, Clone, Default)]
pub struct EnvInfo {
    pub kernel: String,
    pub tools: Vec<(String, String)>,
    pub gh_status: String,
}

pub fn collect_env(signed_in: Option<bool>, secure: Option<bool>) -> EnvInfo {
    let kernel = proc::run("uname", &["-sr"], &Opts::default_timeout()).stdout_trim();
    let mut tools = Vec::new();
    for (name, args) in [
        ("git", vec!["--version"]),
        ("gh", vec!["--version"]),
        ("rustc", vec!["--version"]),
        ("cargo", vec!["--version"]),
        ("node", vec!["--version"]),
        ("python3", vec!["--version"]),
        ("go", vec!["version"]),
    ] {
        let o = proc::run(name, &args, &Opts::default_timeout().with_timeout(10));
        let v = if o.ok() {
            o.combined().lines().next().unwrap_or("").to_string()
        } else {
            "not installed".into()
        };
        tools.push((name.to_string(), v));
    }
    let gh_status = match (signed_in, secure) {
        (Some(true), Some(true)) => "signed in (token in system keyring)".into(),
        (Some(true), Some(false)) => "signed in (token in a plain file)".into(),
        (Some(true), None) => "signed in".into(),
        (Some(false), _) => "not signed in".into(),
        (None, _) => "unknown".into(),
    };
    EnvInfo {
        kernel,
        tools,
        gh_status,
    }
}

#[derive(Debug, Clone, Default)]
pub struct SessionInfo {
    pub owner: String,
    pub slug: String,
    pub mode: String,
    pub channel: String,
    pub tag: String,
    pub stack: String,
}

#[derive(Debug, Clone, Default)]
pub struct ReportInput {
    pub title: String,
    pub what_doing: String,
    pub expected: String,
    pub env: EnvInfo,
    pub session: Option<SessionInfo>,
    pub steps: Vec<Step>,
    /// Last runs (label, started, steps), oldest first. Preferred over `steps`.
    pub runs: Vec<crate::logbook::Run>,
    pub failing_command: Option<(String, Option<i32>)>,
    pub health: Vec<String>,
    pub selftest_summary: Option<String>,
    /// Warnings and errors, shown before the latest lines.
    pub problem_lines: Vec<String>,
    pub log_lines: Vec<String>,
    pub panic: Option<String>,
}

fn status_word(s: StepStatus) -> &'static str {
    match s {
        StepStatus::Running => "Running",
        StepStatus::Passed => "Passed",
        StepStatus::Failed => "Failed",
        StepStatus::Skipped => "Skipped",
    }
}

/// Build the report markdown, then redact it.
pub fn build_report(input: &ReportInput, rc: &RedactContext) -> String {
    let mut s = format!("# StowLock {}\n\n", input.title);
    s.push_str(&format!(
        "- **StowLock:** {APP_VERSION}\n- **Kernel:** {}\n- **GitHub sign-in:** {}\n",
        input.env.kernel, input.env.gh_status
    ));
    for (t, v) in &input.env.tools {
        s.push_str(&format!("- **{t}:** {v}\n"));
    }
    if !input.what_doing.trim().is_empty() {
        s.push_str(&format!(
            "\n## What I was doing\n\n{}\n",
            input.what_doing.trim()
        ));
    }
    if !input.expected.trim().is_empty() {
        s.push_str(&format!(
            "\n## What I expected\n\n{}\n",
            input.expected.trim()
        ));
    }
    if let Some(p) = &input.panic {
        s.push_str(&format!("\n## Crash\n\n```text\n{p}\n```\n"));
    }
    if let Some(se) = &input.session {
        s.push_str(&format!(
            "\n## Current project\n\n| Field | Value |\n| --- | --- |\n| Owner | {} |\n| Repo | {} |\n| Mode | {} |\n| Channel | {} |\n| Tag | {} |\n| Stack | {} |\n",
            se.owner, se.slug, se.mode, se.channel, if se.tag.is_empty() { "none" } else { &se.tag }, se.stack
        ));
    }
    let runs: Vec<crate::logbook::Run> = if !input.runs.is_empty() {
        input.runs.clone()
    } else if !input.steps.is_empty() {
        vec![crate::logbook::Run {
            label: "Last run".into(),
            started: String::new(),
            steps: input.steps.clone(),
        }]
    } else {
        vec![]
    };
    if !runs.is_empty() {
        s.push_str(&format!("\n## Recent runs (last {})\n", runs.len()));
        for r in runs.iter().rev() {
            let when = if r.started.is_empty() {
                String::new()
            } else {
                format!(", {}", r.started)
            };
            s.push_str(&format!("\n### {}{when}\n\n", r.label));
            if r.steps.is_empty() {
                s.push_str("No steps recorded.\n");
                continue;
            }
            s.push_str("| Step | Result | Time | Detail |\n| --- | --- | --- | --- |\n");
            for st in &r.steps {
                let detail = st.detail.lines().next().unwrap_or("").replace('|', "/");
                s.push_str(&format!(
                    "| {} | {} | {} ms | {} |\n",
                    st.name,
                    status_word(st.status),
                    st.millis,
                    detail
                ));
            }
        }
    }
    if let Some((cmd, code)) = &input.failing_command {
        s.push_str(&format!(
            "\n**Failing command:** `{cmd}` (exit code {})\n",
            code.map(|c| c.to_string()).unwrap_or_else(|| "none".into())
        ));
    }
    if !input.health.is_empty() {
        s.push_str("\n## Health\n\n");
        for h in &input.health {
            s.push_str(&format!("- {h}\n"));
        }
    }
    if let Some(st) = &input.selftest_summary {
        s.push_str(&format!("\n## Latest self-test\n\n{st}\n"));
    }
    if !input.problem_lines.is_empty() {
        s.push_str(&format!(
            "\n## Warnings and errors ({})\n\n```text\n{}\n```\n",
            input.problem_lines.len(),
            input.problem_lines.join("\n")
        ));
    }
    if !input.log_lines.is_empty() {
        s.push_str(&format!(
            "\n## Latest log lines ({})\n\n```text\n{}\n```\n",
            input.log_lines.len(),
            input.log_lines.join("\n")
        ));
    }
    let all_steps: Vec<Step> = runs.iter().flat_map(|r| r.steps.clone()).collect();
    let failed: Vec<&Step> = all_steps
        .iter()
        .filter(|s| s.status == StepStatus::Failed)
        .collect();
    if !failed.is_empty() || input.panic.is_some() {
        s.push_str("\n## AI fix prompt\n\n```text\n");
        if let Some(p) = &input.panic {
            s.push_str(&format!(
                "StowLock {APP_VERSION} crashed: {}\n",
                p.lines().next().unwrap_or("")
            ));
        }
        for f in failed {
            s.push_str(&format!(
                "StowLock {APP_VERSION}: the step \"{}\" failed. Actual: {}\n",
                f.name,
                f.detail.lines().next().unwrap_or("")
            ));
        }
        if let Some((cmd, code)) = &input.failing_command {
            s.push_str(&format!("Failing command: {cmd} (exit {:?})\n", code));
        }
        s.push_str("Find the cause in the StowLock source, fix it without changing behaviour elsewhere, and add a regression test.\n```\n");
    }
    redact(&s, rc)
}

/// File name for a downloaded bug report: app name, date and time the
/// report was created. No colons, so it works on every file system.
pub fn bug_report_file_name(created: &chrono::DateTime<chrono::Local>) -> String {
    format!(
        "StowLock-bug-report-{}.txt",
        created.format("%Y-%m-%d_%H-%M-%S")
    )
}

/// Save a report into the reports folder (0600). Returns its path.
pub fn save_report(paths: &AppPaths, kind: &str, text: &str) -> Res<PathBuf> {
    util::ensure_private_dir(&paths.reports_dir())?;
    let p = paths
        .reports_dir()
        .join(format!("{kind}-{}.md", util::now_stamp_secs()));
    util::atomic_write(&p, text.as_bytes())?;
    Ok(p)
}

/// Delete reports and the old log older than `days`.
pub fn apply_retention(paths: &AppPaths, days: u32) -> usize {
    let limit = std::time::Duration::from_secs(days as u64 * 86_400);
    let mut n = 0;
    let mut candidates: Vec<PathBuf> = std::fs::read_dir(paths.reports_dir())
        .map(|rd| rd.flatten().map(|e| e.path()).collect())
        .unwrap_or_default();
    candidates.push(paths.log_file().with_extension("log.old"));
    for p in candidates {
        let old = std::fs::metadata(&p)
            .and_then(|m| m.modified())
            .map(|t| t.elapsed().map(|d| d > limit).unwrap_or(false))
            .unwrap_or(false);
        if old && std::fs::remove_file(&p).is_ok() {
            n += 1;
        }
    }
    n
}

/// Remove every log and report now.
pub fn clear_logs_and_reports(paths: &AppPaths, log: &Logger) -> Res<usize> {
    let mut n = 0;
    for p in std::fs::read_dir(paths.reports_dir())
        .map(|rd| rd.flatten().map(|e| e.path()).collect::<Vec<_>>())
        .unwrap_or_default()
    {
        if std::fs::remove_file(&p).is_ok() {
            n += 1;
        }
    }
    for p in [paths.log_file(), paths.log_file().with_extension("log.old")] {
        if std::fs::remove_file(&p).is_ok() {
            n += 1;
        }
    }
    log.clear();
    Ok(n)
}

/// Write a redacted crash report and a marker the next launch notices.
pub fn install_panic_hook(paths: AppPaths, log: Logger, workspace: PathBuf) {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let msg = format!("{info}");
        let backtrace = std::backtrace::Backtrace::force_capture().to_string();
        let input = ReportInput {
            title: "crash report".into(),
            env: EnvInfo {
                kernel: proc::run("uname", &["-sr"], &Opts::default_timeout()).stdout_trim(),
                ..Default::default()
            },
            runs: log.runs(),
            failing_command: log.failing_command(),
            problem_lines: log.report_lines(40, 100).0,
            log_lines: log.report_lines(40, 100).1,
            panic: Some(format!(
                "{msg}\n\n{}",
                backtrace.lines().take(40).collect::<Vec<_>>().join("\n")
            )),
            ..Default::default()
        };
        let rc = RedactContext::local(&workspace);
        let text = build_report(&input, &rc);
        if let Ok(p) = save_report(&paths, "crash", &text) {
            let _ = util::atomic_write(&paths.crash_marker(), p.display().to_string().as_bytes());
        }
        default(info);
    }));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rc() -> RedactContext {
        RedactContext {
            home: "/home/matt".into(),
            username: "matt".into(),
            hostname: "cachy-box".into(),
            workspace: "/home/matt/GitHub_Projects".into(),
            logins: vec!["Matt-B-007".into()],
            orgs: vec!["acme-tools".into()],
            repos: vec![("sluice".into(), true), ("json-diff-ui".into(), false)],
            include_public_repo_names: false,
            include_username: false,
        }
    }

    #[test]
    fn removes_identity_and_device_details() {
        let tok = crate::fixtures::fake_token();
        let raw = format!(
            "matt@cachy-box: cd /home/matt/GitHub_Projects/sluice/src && git push https://github.com/Matt-B-007/sluice.git\n\
             email 123+Matt-B-007@users.noreply.github.com token {tok} ip 192.168.1.20 mac aa:bb:cc:dd:ee:ff\n\
             also json-diff-ui and acme-tools, Serial Number: XYZ123 and /home/matt/.config"
        );
        let out = redact(&raw, &rc());
        for bad in [
            "matt",
            "cachy-box",
            "/home/",
            "Matt-B-007",
            "sluice",
            "json-diff-ui",
            "acme-tools",
            "192.168",
            "aa:bb",
            "XYZ123",
            "noreply",
            &tok[4..],
        ] {
            assert!(!out.contains(bad), "leaked {bad}: {out}");
        }
        assert!(
            out.contains("<account-1>")
                && out.contains("<repo-1>")
                && out.contains("<email>")
                && out.contains("<host>")
        );
        assert!(out.contains("src"), "project-relative path kept: {out}");
        assert!(out.contains("~/.config"));
    }

    #[test]
    fn download_file_name_format() {
        use chrono::TimeZone;
        let t = chrono::Local
            .with_ymd_and_hms(2026, 11, 23, 7, 43, 5)
            .unwrap();
        let n = bug_report_file_name(&t);
        assert_eq!(n, "StowLock-bug-report-2026-11-23_07-43-05.txt");
        assert!(!n.contains(':') && !n.contains(' '));
    }

    #[test]
    fn app_name_survives_a_repo_with_the_same_name() {
        let mut c = rc();
        c.repos.push(("stowlock".into(), false));
        let out = redact("# StowLock bug report\n- StowLock: 0.1.0\npushed stowlock to https://github.com/Matt-B-007/stowlock", &c);
        assert!(
            out.contains("# StowLock bug report") && out.contains("- StowLock: 0.1.0"),
            "{out}"
        );
        assert!(!out.contains("Matt-B-007"));
    }

    #[test]
    fn opt_in_keeps_public_names_only() {
        let mut c = rc();
        c.include_public_repo_names = true;
        c.include_username = true;
        let out = redact("json-diff-ui sluice Matt-B-007", &c);
        assert!(out.contains("json-diff-ui"));
        assert!(!out.contains("sluice"), "private repos always hidden");
        assert!(out.contains("Matt-B-007"));
    }

    #[test]
    fn report_has_sections_and_fix_prompt() {
        let input = ReportInput {
            title: "bug report".into(),
            steps: vec![Step {
                name: "Commit & push".into(),
                status: StepStatus::Failed,
                millis: 10,
                detail: "rejected by matt".into(),
            }],
            log_lines: vec!["line from /home/matt/GitHub_Projects/sluice".into()],
            ..Default::default()
        };
        let text = build_report(&input, &rc());
        assert!(
            text.contains("## Recent runs")
                && text.contains("## AI fix prompt")
                && text.contains("Failed")
        );
        assert!(!text.contains("matt") && !text.contains("sluice"));
    }
}
