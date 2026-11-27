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
    /// The user's own words to hide ("Also hide these words").
    pub extra_words: Vec<String>,
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
fn steamid_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"\b7656119\d{10}\b").unwrap())
}
/// Windows-style home folders (Z:\home\matt under Wine/Proton, C:\Users\matt).
fn windows_home_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"(?i)\b[a-z]:\\(?:home|users)\\[^\\\s]+").unwrap())
}
fn gh_url_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| {
        Regex::new(r"(?:https?://|git@)github\.com[/:]([A-Za-z0-9_.-]+)/([A-Za-z0-9_.-]+)").unwrap()
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
    // Paths first: workspace → project-relative, then home → ~. (Before the
    // app's name is protected, so a home path containing "stowlock" is still
    // hidden whole.)
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
    s = windows_home_re().replace_all(&s, "~").into_owned();
    // Protect the app's own name (a user's repo may share it).
    for (i, k) in KEEP.iter().enumerate() {
        s = s.replace(k, &format!("\u{2}APP{i}\u{2}"));
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
    // Whole repo names only (my-app-preflight is not my-app), longest first,
    // so a name inside another can't leave part of the longer one behind.
    let mut url_owners: Vec<(String, String)> = gh_url_re()
        .captures_iter(&s)
        .map(|c| {
            let repo = c[2].trim_end_matches('.');
            let repo = repo.strip_suffix(".git").unwrap_or(repo);
            (c[1].to_string(), repo.to_string())
        })
        .collect();
    url_owners.sort_by_key(|(o, r)| std::cmp::Reverse(o.len() + r.len()));
    url_owners.dedup();
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
    let mut names: Vec<(usize, String)> = repo_names.iter().cloned().enumerate().collect();
    names.sort_by_key(|(_, n)| std::cmp::Reverse(n.len()));
    for (i, name) in names.iter() {
        let (i, name) = (*i, name);
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
    for (i, w) in rc
        .extra_words
        .iter()
        .filter(|w| !w.trim().is_empty())
        .enumerate()
    {
        s = replace_word(&s, w.trim(), &format!("<hidden-{}>", i + 1));
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
    s = mac_re().replace_all(&s, "<mac>").into_owned();
    s = ipv6_re().replace_all(&s, "<ip>").into_owned();
    // Version numbers like shellcheck-py==0.11.0.1 or v1.2.3.4 look like IP
    // addresses; keep those.
    s = ip_re()
        .replace_all(&s, |c: &regex::Captures| {
            let m = c.get(0).unwrap();
            let before = &s[..m.start()];
            let version = before.ends_with('=')
                || before.ends_with('v')
                || before.ends_with('-')
                || before.ends_with('@')
                || before.to_ascii_lowercase().ends_with("version ");
            if version {
                m.as_str().to_string()
            } else {
                "<ip>".to_string()
            }
        })
        .into_owned();
    s = serial_re().replace_all(&s, "serial: <serial>").into_owned();
    // Steam account ids (17 digits starting 7656119); game and app ids stay.
    s = steamid_re().replace_all(&s, "<steamid>").into_owned();
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
        let text = o.combined();
        let v = if o.ok() {
            text.lines().next().unwrap_or("").to_string()
        } else if text.contains("could not choose a version") || text.contains("no default") {
            // rustup is installed, but has no Rust version chosen yet.
            "rustup has no Rust version yet (run: rustup default stable)".into()
        } else if proc::which(name).is_some() {
            format!("installed, but '{name} {}' failed", args.join(" "))
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

/// Split "HH:MM:SS LEVEL [stage]   text" into its prefix and text.
fn split_line(l: &str) -> (&str, &str) {
    match l.find("] ") {
        Some(i) if l.starts_with(|c: char| c.is_ascii_digit()) => (&l[..=i], &l[i + 1..]),
        _ => ("", l),
    }
}

/// Keep a project's contents out of the report: git's per-file lines
/// ("create mode 100644 src/x.rs") are left out, and from a project's test
/// output only the command, its pass/fail lines and failure messages are
/// kept. Each command gets one short note of what was left out, at its end.
pub fn privacy_filter(lines: &[String]) -> Vec<String> {
    let file_line =
        Regex::new(r"^\s*(create|delete) mode \d+ |^\s*mode change \d+|^\s*rename .+ \(\d+%\)$")
            .unwrap();
    let bodies: Vec<&str> = lines.iter().map(|l| split_line(l).1.trim_start()).collect();
    let keep = crate::logbook::mark_important(&bodies);
    let mut out: Vec<String> = Vec::new();
    let (mut files, mut tests) = (0usize, 0usize);
    let mut prefix_for_note = String::new();
    // A group of left-out lines with no command line before it (the start of
    // "Latest log lines") takes the time of its first line instead.
    let mut first_hidden = String::new();
    let flush = |out: &mut Vec<String>, files: &mut usize, tests: &mut usize, prefix: &str| {
        let mut notes = Vec::new();
        if *files > 0 {
            notes.push(format!(
                "{files} file name{} left out",
                if *files == 1 { "" } else { "s" }
            ));
        }
        if *tests > 0 {
            notes.push(format!(
                "{tests} line{} of project test output left out",
                if *tests == 1 { "" } else { "s" }
            ));
        }
        if !notes.is_empty() {
            out.push(format!("{}  [{}]", prefix.trim_end(), notes.join(", ")));
        }
        *files = 0;
        *tests = 0;
    };
    for (i, l) in lines.iter().enumerate() {
        let (prefix, _) = split_line(l);
        let body = bodies[i];
        let is_header = body.starts_with("$ ");
        if is_header {
            {
                let p = if prefix_for_note.is_empty() {
                    first_hidden.clone()
                } else {
                    prefix_for_note.clone()
                };
                flush(&mut out, &mut files, &mut tests, &p);
                first_hidden.clear();
            }
            prefix_for_note = prefix.to_string();
            out.push(l.clone());
            continue;
        }
        let hidden_marker = body.starts_with('[') && body.contains("hidden]");
        if file_line.is_match(body) || (hidden_marker && prefix.contains("[push]")) {
            if first_hidden.is_empty() {
                first_hidden = prefix.to_string();
            }
            files += 1;
            continue;
        }
        if prefix.contains("[tests]") {
            if body.is_empty() || (body.starts_with('[') && body.contains("left out]")) {
                continue;
            }
            if !keep[i] && !hidden_marker {
                if first_hidden.is_empty() {
                    first_hidden = prefix.to_string();
                }
                tests += 1;
                continue;
            }
            out.push(l.clone());
            continue;
        }
        {
            let p = if prefix_for_note.is_empty() {
                first_hidden.clone()
            } else {
                prefix_for_note.clone()
            };
            flush(&mut out, &mut files, &mut tests, &p);
            first_hidden.clear();
        }
        out.push(l.clone());
    }
    {
        let p = if prefix_for_note.is_empty() {
            first_hidden.clone()
        } else {
            prefix_for_note.clone()
        };
        flush(&mut out, &mut files, &mut tests, &p);
        first_hidden.clear();
    }
    out
}

/// Exit codes that mean "a tool is missing here", not a failure: 127 (command
/// not found) and 3 (a House Standard check script's "only tools missing").
pub fn couldnt_run(code: Option<i32>) -> bool {
    matches!(code, Some(127) | Some(3))
}

/// The most recent failing test command and its lines (nothing from earlier
/// runs), for the AI fix prompt.
pub fn last_failure_block(lines: &[String]) -> Vec<String> {
    let is_header = |l: &str| split_line(l).1.trim_start().starts_with("$ ");
    let Some(start) = lines
        .iter()
        .rposition(|l| is_header(l) && split_line(l).0.contains("[tests]"))
    else {
        return Vec::new();
    };
    let mut out = vec![lines[start].clone()];
    for l in &lines[start + 1..] {
        if is_header(l) || !split_line(l).0.contains("[tests]") {
            break;
        }
        out.push(l.clone());
    }
    out
}

/// When a check is run again with the same result, show its output once:
/// the repeat becomes one line saying so.
pub fn fold_repeats(lines: &[String]) -> Vec<String> {
    let mut blocks: Vec<Vec<String>> = Vec::new();
    for l in lines {
        if split_line(l).1.trim_start().starts_with("$ ") || blocks.is_empty() {
            blocks.push(Vec::new());
        }
        blocks.last_mut().unwrap().push(l.clone());
    }
    let norm = |b: &Vec<String>| -> Vec<String> {
        b.iter()
            .skip(1)
            .map(|l| {
                let t = split_line(l).1.trim().to_string();
                // Thread ids and timings differ between runs.
                Regex::new(r"\(\d+\)|finished in [\d.]+s|\d+ ms")
                    .unwrap()
                    .replace_all(&t, "")
                    .to_string()
            })
            .collect()
    };
    let mut out = Vec::new();
    let mut prev: Option<Vec<String>> = None;
    for b in &blocks {
        let n = norm(b);
        if b.len() > 2 && prev.as_ref() == Some(&n) {
            out.push(format!("{}  (same output as the previous check)", b[0]));
        } else {
            out.extend(b.iter().cloned());
        }
        prev = Some(n);
    }
    out
}

/// A prompt for the user's AI when their *project's* tests fail: which tests,
/// what the failure said, and a request to fix the project (not StowLock).
pub fn project_fix_prompt(summary: &str, output: &[String]) -> String {
    let failures: Vec<String> = output
        .iter()
        .map(|l| split_line(l).1.trim().to_string())
        .filter(|t| !t.is_empty() && !t.starts_with('['))
        .filter(|t| crate::logbook::is_failure_line(t) || t.starts_with("thread '"))
        .take(40)
        .collect();
    let machine = summary.contains(crate::preflight::MACHINE_SPECIFIC);
    let fonts = summary.contains(crate::preflight::FONT_SPECIFIC);
    let mut p = String::from(if fonts {
        "My project's tests fail when they're run with only the fonts GitHub's Ubuntu machine has (DejaVu), so they'd fail on GitHub too. Text that fits with my own fonts doesn't fit with those.\n"
    } else if machine {
        "Some of my project's tests fail only on my own computer, not on GitHub's clean machine. The output shows they depend on things on my computer (a running copy of the app, my home folder or desktop settings).\n"
    } else {
        "My project's tests failed when StowLock checked it before pushing to GitHub.\n"
    });
    if !summary.trim().is_empty() {
        p.push_str(&format!("Result: {}\n", summary.trim()));
    }
    if !failures.is_empty() {
        p.push_str("Failure output:\n");
        for f in failures {
            p.push_str(&format!("  {f}\n"));
        }
    }
    if fonts {
        p.push_str("Fix it so the result doesn't depend on which fonts are installed: bundle the font(s) the app uses (an open-licence font, with its licence next to the others) and load them at startup before any window is built, so the app and its tests get them the same way. Let text that might not fit shorten with the whole text in a tooltip, or give it room. Don't loosen or skip the check. Prove it by running the tests with fontconfig limited to DejaVu (FONTCONFIG_FILE pointing to a fonts.conf that lists only a folder of DejaVu fonts). Give me the updated project as a zip.\n");
    } else if machine {
        p.push_str("Change the tests so they don't depend on the real machine: give them fakes or fixed values (for example a fake process list or a fixed effects setting). Don't change how the app behaves. Give me the updated project as a zip.\n");
    } else {
        p.push_str(
            "Please fix the project so these tests pass, without changing behaviour elsewhere. If a test itself is wrong, explain why before changing it, and give me the updated project as a zip.\n",
        );
    }
    p
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
    let runs: Vec<crate::logbook::Run> = if input.runs.iter().any(|r| !r.steps.is_empty()) {
        input
            .runs
            .iter()
            .filter(|r| !r.steps.is_empty())
            .cloned()
            .collect()
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
            if r.steps.is_empty() {
                continue;
            }
            s.push_str(&format!("\n### {}{when}\n\n", r.label));
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
        let label = if couldnt_run(*code) {
            "Couldn't run here (a tool is missing)"
        } else {
            "Failing command"
        };
        s.push_str(&format!(
            "\n**{label}:** `{cmd}` (exit code {})\n",
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
    let problems = fold_repeats(&privacy_filter(&input.problem_lines));
    if !problems.is_empty() {
        s.push_str(&format!(
            "\n## Warnings and errors ({})\n\n```text\n{}\n```\n",
            problems.len(),
            problems.join("\n")
        ));
    }
    // Lines already shown under warnings aren't repeated.
    let latest: Vec<String> = privacy_filter(&input.log_lines)
        .into_iter()
        .filter(|l| !input.problem_lines.contains(l))
        .collect();
    if !latest.is_empty() {
        s.push_str(&format!(
            "\n## Latest log lines ({})\n\n```text\n{}\n```\n",
            latest.len(),
            latest.join("\n")
        ));
    }
    let all_steps: Vec<Step> = runs.iter().flat_map(|r| r.steps.clone()).collect();
    let failed: Vec<&Step> = all_steps
        .iter()
        .filter(|s| s.status == StepStatus::Failed)
        .collect();
    let (project_tests, stowlock_faults): (Vec<&&Step>, Vec<&&Step>) =
        failed.iter().partition(|f| {
            f.name == "Pre-flight tests"
                && !crate::health::counts_against_stowlock(&f.name, &f.detail)
        });
    // A secret the scan found is in the project's files, not a StowLock bug.
    let stowlock_faults: Vec<&&Step> = stowlock_faults
        .into_iter()
        .filter(|f| !(f.name == "Secret scan" && f.detail.contains("possible secret")))
        .collect();
    // Only when the latest check of the project failed (not an older one).
    let latest_check_failed = all_steps
        .iter()
        .rev()
        .find(|st| st.name == "Pre-flight tests")
        .is_some_and(|st| st.status == StepStatus::Failed);
    // The latest failed check, and only the step that failed in it. No prompt
    // without failure output: an AI would only go looking for a problem that
    // isn't there.
    let last = last_failure_block(&privacy_filter(&input.problem_lines));
    let has_output = last
        .iter()
        .skip(1)
        .any(|l| !split_line(l).1.trim().starts_with('['));
    if !project_tests.is_empty() && latest_check_failed && has_output {
        s.push_str("\n## AI fix prompt (for your project)\n\n```text\n");
        let summary = project_tests
            .last()
            .map(|t| t.detail.lines().next().unwrap_or(""))
            .unwrap_or("");
        s.push_str(&project_fix_prompt(summary, &last));
        s.push_str("```\n");
    }
    if !stowlock_faults.is_empty() || input.panic.is_some() {
        s.push_str("\n## AI fix prompt (for StowLock)\n\n```text\n");
        if let Some(p) = &input.panic {
            s.push_str(&format!(
                "StowLock {APP_VERSION} crashed: {}\n",
                p.lines().next().unwrap_or("")
            ));
        }
        for f in stowlock_faults {
            s.push_str(&format!(
                "StowLock {APP_VERSION}: the step \"{}\" failed. Actual: {}\n",
                f.name,
                f.detail.lines().next().unwrap_or("")
            ));
        }
        if let Some((cmd, code)) = input
            .failing_command
            .as_ref()
            .filter(|(_, c)| !couldnt_run(*c))
        {
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

/// Crash reports kept (the House Standard: the last 10 sessions).
const CRASHES_KEPT: usize = 10;

/// The "StowLock is running" marker: written at start, removed on a clean
/// exit. Still there at the next start means StowLock closed unexpectedly,
/// even when nothing could be recorded (killed, out of memory, power cut).
pub fn running_marker(paths: &AppPaths) -> PathBuf {
    paths.state_dir.join("running")
}

/// Call when the window opens. Returns the version that closed unexpectedly
/// last time without leaving a crash report, if any, and marks this run.
pub fn mark_running(paths: &AppPaths) -> Option<String> {
    let marker = running_marker(paths);
    let unclean = std::fs::read_to_string(&marker).ok().and_then(|t| {
        let mut it = t.split_whitespace();
        let pid: u32 = it.next()?.parse().ok()?;
        let version = it.next().unwrap_or("?").to_string();
        // Still running (another copy that's handing over)? Not a crash.
        let alive = std::fs::read_to_string(format!("/proc/{pid}/comm"))
            .map(|c| c.trim().starts_with("stowlock"))
            .unwrap_or(false);
        (!alive).then_some(version)
    });
    let _ = util::ensure_private_dir(&paths.state_dir);
    let _ = util::atomic_write(
        &marker,
        format!("{} {}", std::process::id(), crate::config::APP_VERSION).as_bytes(),
    );
    // A panic already wrote its own report: that one is shown instead.
    unclean.filter(|_| !paths.crash_marker().exists())
}

/// Call on a clean exit.
pub fn mark_stopped(paths: &AppPaths) {
    let _ = std::fs::remove_file(running_marker(paths));
}

/// A report for a close nothing could record, with the last log lines, and
/// the marker the next launch shows a notice for.
pub fn record_unexpected_close(
    paths: &AppPaths,
    log: &Logger,
    version: &str,
    workspace: &std::path::Path,
) {
    let input = ReportInput {
        title: "crash report".into(),
        runs: log.runs(),
        problem_lines: log.report_lines(40, 100).0,
        log_lines: log.report_lines(40, 100).1,
        panic: Some(format!(
            "StowLock {version} closed unexpectedly and no error was recorded. The system may have stopped it, it may have run out of memory, or the computer lost power."
        )),
        ..Default::default()
    };
    let text = build_report(&input, &RedactContext::local(workspace));
    if let Ok(p) = save_report(paths, "crash", &text) {
        let _ = util::atomic_write(&paths.crash_marker(), p.display().to_string().as_bytes());
    }
    prune_crash_reports(paths);
}

/// Keep only the newest crash reports.
pub fn prune_crash_reports(paths: &AppPaths) {
    let Ok(rd) = std::fs::read_dir(paths.reports_dir()) else {
        return;
    };
    let mut crashes: Vec<PathBuf> = rd
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("crash-"))
        })
        .collect();
    crashes.sort();
    crashes.reverse();
    for old in crashes.iter().skip(CRASHES_KEPT) {
        let _ = std::fs::remove_file(old);
    }
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
        let _ = util::atomic_write(
            &crate::updater::crashed_version_file(&paths),
            crate::config::APP_VERSION.as_bytes(),
        );
        if let Ok(p) = save_report(&paths, "crash", &text) {
            let _ = util::atomic_write(&paths.crash_marker(), p.display().to_string().as_bytes());
        }
        prune_crash_reports(&paths);
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
            extra_words: vec![],
        }
    }

    /// Log lines shaped like the real reports from 2026-11-23, with this
    /// test's made-up identity in place of the real one.
    fn real_shaped_log() -> Vec<String> {
        [
            "11:14:37 INFO  [intake] Loaded /home/matt/Downloads/sluice-v0.12.1.zip",
            "11:14:38 INFO  [intake] Copied from /run/media/matt/USB/sluice-v0.12.1.zip",
            "17:37:35 WARN  [tests] $ cargo test --no-fail-fast  (exit Some(101), 15567 ms)",
            "17:37:35 INFO  [tests]   running 41 tests",
            "17:37:35 INFO  [tests]   test manage::tests::refuses_while_steam_runs ... ok",
            "17:37:35 INFO  [tests]   sluice: the game closed after 0s. To get help, run: sluice report 1240440",
            "17:37:35 WARN  [tests]   test parse::tests::reads_config ... FAILED",
            "17:37:35 WARN  [tests]   test result: FAILED. 40 passed; 1 failed",
            "17:38:16 INFO  [push] $ git commit -m \"Update to v0.12.1\"  (exit Some(0), 26 ms)",
            "17:38:16 INFO  [push]    48 files changed, 10770 insertions(+), 687 deletions(-)",
            "17:38:16 INFO  [push]    create mode 100644 crates/secret-plans/src/steam.rs",
            "17:38:16 INFO  [push]    create mode 100644 packaging/aur/sluice/PKGBUILD",
            "17:38:16 INFO  [push]   [7 lines hidden]",
            "17:38:18 INFO  [push]   To https://github.com/Matt-B-007/sluice.git",
            "17:38:20 INFO  [push] Pushed Matt-B-007/sluice from cachy-box as matt@cachy-box.lan",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect()
    }

    fn input_with(lines: Vec<String>, note: &str) -> ReportInput {
        ReportInput {
            title: "t".into(),
            what_doing: note.into(),
            expected: String::new(),
            env: EnvInfo::default(),
            session: None,
            steps: vec![],
            runs: vec![],
            failing_command: None,
            health: vec![],
            selftest_summary: None,
            problem_lines: lines
                .iter()
                .filter(|l| l.contains("WARN"))
                .cloned()
                .collect(),
            log_lines: lines,
            panic: None,
        }
    }

    #[test]
    fn real_shaped_report_keeps_private_things_private() {
        let mut ctx = rc();
        ctx.extra_words = vec!["Riverside Clinic".into()];
        let text = build_report(
            &input_with(
                real_shaped_log(),
                "Broke while syncing Riverside Clinic files",
            ),
            &ctx,
        );
        for leak in [
            "matt",
            "Matt-B-007",
            "cachy-box",
            "sluice",
            "secret-plans",
            "steam.rs",
            "PKGBUILD",
            "1240440",
            "Riverside",
            "refuses_while_steam_runs",
        ] {
            assert!(
                !text.to_lowercase().contains(&leak.to_lowercase()),
                "{leak} leaked:\n{text}"
            );
        }
        // What's needed to fix a bug stays.
        assert!(text.contains("48 files changed"));
        assert!(text.contains("reads_config ... FAILED"));
        assert!(text.contains("test result: FAILED. 40 passed; 1 failed"));
        assert!(text.contains("file names left out"));
        assert!(text.contains("project test output left out"));
        assert!(text.contains("StowLock"));
        // Failure lines appear once, not under both headings.
        assert_eq!(text.matches("reads_config ... FAILED").count(), 1, "{text}");
    }

    /// Shaped like the 2026-11-24 report: a project's e2e test fails, the
    /// check is run twice, and a passing test's name contains "failed".
    fn failing_project_log() -> Vec<String> {
        let mut v = Vec::new();
        for t in ["12:25:21", "12:25:47"] {
            for (lvl, line) in [
                ("WARN ", "$ cargo test --no-fail-fast  (exit Some(101), 5338 ms)"),
                ("INFO ", "  running 7 tests"),
                ("INFO ", "  test launch::starts_game ... ok"),
                ("INFO ", "  "),
                ("INFO ", "  test fsutil::tests::failed_write_keeps_original ... ok"),
                ("WARN ", "  test obs_capture_starts_the_game_through_obs_gamecapture ... FAILED"),
                ("WARN ", "  ---- obs_capture_starts_the_game_through_obs_gamecapture stdout ----"),
                ("WARN ", "  thread 'obs_capture_starts_the_game_through_obs_gamecapture' (316740) panicked at crates/sluice/tests/e2e.rs:528:5:"),
                ("INFO ", "  wrapper didn't launch obs-gamecapture: got [\"steam\"]"),
                ("INFO ", "  note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace"),
                ("INFO ", "  "),
                ("INFO ", "  test result: ok. 68 passed; 0 failed; 0 ignored; finished in 2.48s"),
                ("WARN ", "  test result: FAILED. 6 passed; 1 failed; 0 ignored; finished in 2.25s"),
            ] {
                v.push(format!("{t} {lvl} [tests] {line}"));
            }
        }
        v
    }

    #[test]
    fn project_test_failure_report_is_short_and_useful() {
        let lines = failing_project_log();
        let mut i = input_with(lines.clone(), "");
        i.problem_lines = lines.clone();
        i.runs = vec![crate::logbook::Run {
            label: "Stage sluice".into(),
            started: "12:24".into(),
            steps: vec![Step {
                name: "Pre-flight tests".into(),
                status: StepStatus::Failed,
                millis: 5742,
                detail: "cargo test --no-fail-fast failed · 74 passed, 1 failed: obs_capture_starts_the_game_through_obs_gamecapture".into(),
            }],
        }];
        i.steps = i.runs[0].steps.clone();
        i.failing_command = Some(("cargo test --no-fail-fast".into(), Some(101)));
        let text = build_report(&i, &rc());
        // The failure message survives, the passing test doesn't.
        assert!(
            text.contains("wrapper didn't launch obs-gamecapture"),
            "{text}"
        );
        assert!(!text.contains("failed_write_keeps_original"), "{text}");
        // The second identical run is folded into one line.
        assert!(
            text.contains("(same output as the previous check)"),
            "{text}"
        );
        // No blank lines or stacks of markers.
        assert!(!text.contains("[tests]   \n"));
        // The AI prompt is about the project.
        assert!(text.contains("AI fix prompt (for your project)"));
        assert!(text.contains("My project's tests failed"));
        assert!(
            !text.contains("Find the cause in the StowLock source"),
            "{text}"
        );
    }

    #[test]
    fn passing_lines_are_not_warnings() {
        use crate::logbook::is_failure_line;
        assert!(!is_failure_line("test result: ok. 68 passed; 0 failed"));
        assert!(!is_failure_line(
            "test fsutil::tests::failed_write_keeps_original ... ok"
        ));
        assert!(is_failure_line("test obs_capture ... FAILED"));
        assert!(is_failure_line("test result: FAILED. 6 passed; 1 failed"));
        assert!(is_failure_line("assertion `left == right` failed"));
    }

    #[test]
    fn empty_runs_are_left_out() {
        let mut i = input_with(vec![], "");
        i.runs = vec![
            crate::logbook::Run {
                label: "Stage x".into(),
                started: "11:14".into(),
                steps: vec![],
            },
            crate::logbook::Run {
                label: "Push x".into(),
                started: "11:15".into(),
                steps: vec![Step {
                    name: "Commit & push".into(),
                    status: StepStatus::Passed,
                    millis: 5,
                    detail: String::new(),
                }],
            },
        ];
        let text = build_report(&i, &rc());
        assert!(!text.contains("No steps recorded"));
        assert!(text.contains("Recent runs (last 1)"));
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

    #[test]
    fn a_repo_name_inside_another_never_leaks() {
        let mut c = rc();
        c.repos = vec![("Sluice".into(), false), ("Sluice-Preflight".into(), true)];
        let text = "Push Matt-B-007/Sluice-Preflight v0.1.0\n\
            From https://github.com/Matt-B-007/Sluice-Preflight\n\
            To https://github.com/Matt-B-007/Sluice-Preflight.git\n\
            https://github.com/Matt-B-007/Sluice/releases/tag/v1\n\
            Loaded ~/Downloads/sluice-preflight-v0.1.0.zip and sluice-v2.zip";
        let out = redact(text, &c);
        assert!(!out.to_lowercase().contains("preflight"), "{out}");
        assert!(!out.to_lowercase().contains("sluice"), "{out}");
        // One repo, one label, everywhere.
        let labels: std::collections::BTreeSet<&str> = out
            .match_indices("<repo-")
            .map(|(i, _)| &out[i..i + 8])
            .collect();
        assert_eq!(labels.len(), 2, "{out}");
        let preflight = out
            .lines()
            .next()
            .unwrap()
            .split('/')
            .nth(1)
            .unwrap()
            .split(' ')
            .next()
            .unwrap()
            .to_string();
        assert!(out.lines().nth(1).unwrap().ends_with(&preflight), "{out}");
    }

    #[test]
    fn the_ai_prompt_uses_only_the_latest_failure() {
        let lines: Vec<String> = [
            "18:07:46 WARN  [tests] $ [test] Check Qt libraries  (exit Some(1), 26 ms)",
            "18:07:46 WARN  [tests]   ::error title=The test machine is missing libraries",
            "18:22:33 WARN  [tests] $ [test] Tests  (exit Some(127), 3 ms)",
            "18:22:33 WARN  [tests]   bash: line 1: pytest: command not found",
            "18:22:33 INFO  [tests]   tests/test_secret_feature.py private detail",
        ]
        .iter()
        .map(|l| l.to_string())
        .collect();
        let kept = privacy_filter(&lines);
        assert!(
            kept.iter().any(|l| l.contains("pytest: command not found")),
            "{kept:?}"
        );
        assert!(
            !kept.iter().any(|l| l.contains("secret_feature")),
            "{kept:?}"
        );
        let last = last_failure_block(&kept);
        assert!(last[0].contains("[test] Tests"), "{last:?}");
        assert!(
            !last
                .iter()
                .any(|l| l.contains("Check Qt libraries") || l.contains("missing libraries")),
            "{last:?}"
        );
    }

    #[test]
    fn font_failures_ask_for_bundled_fonts() {
        let p = project_fix_prompt(
            &format!(
                "GitHub step \"Tests\" failed · 1 failed · {}",
                crate::preflight::FONT_SPECIFIC
            ),
            &["E  ('tile', 'text cut off', 'tilename', 'Broadcast', 87, 75)".to_string()],
        );
        assert!(p.contains("bundle the font"), "{p}");
        assert!(p.contains("Don't loosen"), "{p}");
        assert!(!p.contains("so these tests pass"), "{p}");
    }

    #[test]
    fn computer_specific_failures_get_a_different_prompt() {
        let p = project_fix_prompt(
            &format!(
                "GitHub step \"Tests\" failed · 1 failed · {}",
                crate::preflight::MACHINE_SPECIFIC
            ),
            &["18:46:59 WARN  [tests]   E  a demo program is still running".to_string()],
        );
        assert!(p.contains("don't depend on the real machine"), "{p}");
        assert!(p.contains("Don't change how the app behaves"), "{p}");
        assert!(!p.contains("so these tests pass"), "{p}");
    }

    #[test]
    fn left_out_notes_keep_their_time() {
        let lines: Vec<String> = [
            "18:49:10 INFO  [tests]   collected 1165 items",
            "18:49:10 INFO  [tests]   tests/test_a.py ....",
            "18:49:12 INFO  [tests] $ [test] Check libs  (exit Some(0), 16 ms)",
            "18:49:12 INFO  [tests]   Qt's libraries are there",
        ]
        .iter()
        .map(|l| l.to_string())
        .collect();
        let out = privacy_filter(&lines);
        assert!(out.iter().all(|l| l.starts_with("18:49:")), "{out:?}");
    }

    #[test]
    fn version_numbers_arent_taken_for_ip_addresses() {
        let out = redact(
            "pip install shellcheck-py==0.11.0.1 (v1.2.3.4) from 192.168.1.20",
            &rc(),
        );
        assert!(out.contains("shellcheck-py==0.11.0.1"), "{out}");
        assert!(out.contains("v1.2.3.4"), "{out}");
        assert!(!out.contains("192.168.1.20"), "{out}");
    }

    #[test]
    fn a_home_folder_named_after_the_app_is_still_hidden() {
        let mut c = rc();
        c.home = "/home/matt/.cache/stowlock/checks/stowlock-home".into();
        c.workspace = format!("{}/GitHub_Projects", c.home);
        let out = redact(
            "Report: /home/matt/.cache/stowlock/checks/stowlock-home/.local/state/stowlock/r.md",
            &c,
        );
        assert_eq!(out, "Report: ~/.local/state/stowlock/r.md");
    }

    #[test]
    fn a_missing_tool_isnt_called_a_failing_command() {
        assert!(couldnt_run(Some(3)) && couldnt_run(Some(127)));
        assert!(!couldnt_run(Some(1)) && !couldnt_run(None));
    }

    #[test]
    fn an_unexpected_close_is_noticed_next_time() {
        let d = tempfile::tempdir().unwrap();
        let paths = AppPaths::under(d.path());
        let log = Logger::new(None);
        // First start: nothing to report.
        assert_eq!(mark_running(&paths), None);
        // It never stopped cleanly; a dead process left the marker.
        std::fs::write(running_marker(&paths), "4194300 0.1.0-beta.10").unwrap();
        let v = mark_running(&paths).expect("noticed");
        assert_eq!(v, "0.1.0-beta.10");
        record_unexpected_close(&paths, &log, &v, d.path());
        let report = std::fs::read_to_string(paths.crash_marker()).unwrap();
        assert!(std::fs::read_to_string(report.trim())
            .unwrap()
            .contains("closed unexpectedly"));
        // A clean stop leaves nothing behind.
        mark_stopped(&paths);
        assert!(!running_marker(&paths).exists());
        // Only the newest 10 crash reports are kept.
        for i in 0..14 {
            std::fs::write(
                paths.reports_dir().join(format!("crash-2026-01-{i:02}.md")),
                "x",
            )
            .unwrap();
        }
        prune_crash_reports(&paths);
        let n = std::fs::read_dir(paths.reports_dir())
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().starts_with("crash-"))
            .count();
        assert_eq!(n, 10);
    }

    #[test]
    fn placeholders_follow_the_house_standard() {
        // Built at run time, so the source itself holds no token-shaped text
        // (StowLock's own secret scan would rightly flag it).
        let token = format!("ghp_{}", "abcdefghijklmnopqrstuvwxyz0123456789");
        let out = redact(
            &format!("steam 76561198012345678 app 1091500 at Z:\\home\\matt\\game, C:\\Users\\matt\\x; mac 00:1a:2b:3c:4d:5e ip 10.0.0.7 v0.11.0.1 token {token}"),
            &rc(),
        );
        assert!(out.contains("steam <steamid>"), "{out}");
        assert!(out.contains("app 1091500"), "game ids stay: {out}");
        assert!(
            out.contains("at ~\\game") && out.contains(", ~\\x"),
            "{out}"
        );
        assert!(out.contains("<mac>") && out.contains("ip <ip>"), "{out}");
        assert!(out.contains("v0.11.0.1"), "versions stay: {out}");
        assert!(out.contains("<secret>") && !out.contains("ghp_"), "{out}");
    }
}
