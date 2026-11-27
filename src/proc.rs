// SPDX-License-Identifier: GPL-3.0-or-later
//! Running external commands safely: separate arguments (never a shell
//! string), timeouts, cancellation, a fuller PATH and an optional cleaned
//! environment for running untrusted project code.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(Debug, Clone)]
pub struct Output {
    pub program: String,
    pub args: Vec<String>,
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub timed_out: bool,
    pub cancelled: bool,
    pub duration: Duration,
}

impl Output {
    pub fn ok(&self) -> bool {
        self.code == Some(0) && !self.timed_out && !self.cancelled
    }
    /// The command as a person would type it (for logs).
    pub fn command_line(&self) -> String {
        let mut s = self.program.clone();
        for a in &self.args {
            if a.contains(' ') || a.is_empty() {
                s.push_str(&format!(" \"{a}\""));
            } else {
                s.push(' ');
                s.push_str(a);
            }
        }
        s
    }
    /// stdout + stderr, trimmed.
    pub fn combined(&self) -> String {
        let mut s = self.stdout.trim_end().to_string();
        if !self.stderr.trim().is_empty() {
            if !s.is_empty() {
                s.push('\n');
            }
            s.push_str(self.stderr.trim_end());
        }
        s
    }
    pub fn stdout_trim(&self) -> String {
        self.stdout.trim().to_string()
    }
}

/// Options for one command.
#[derive(Clone, Default)]
pub struct Opts {
    pub cwd: Option<PathBuf>,
    pub timeout: Option<Duration>,
    pub cancel: Option<Arc<AtomicBool>>,
    /// Remove token-like environment variables (for running project code).
    pub scrub_env: bool,
    pub env: Vec<(String, String)>,
    pub stdin: Option<String>,
    /// Variables to leave out (for example the desktop session's).
    pub env_remove: Vec<String>,
}

impl Opts {
    pub fn in_dir(dir: &Path) -> Self {
        Opts {
            cwd: Some(dir.to_path_buf()),
            timeout: Some(Duration::from_secs(120)),
            ..Default::default()
        }
    }
    pub fn default_timeout() -> Self {
        Opts {
            timeout: Some(Duration::from_secs(120)),
            ..Default::default()
        }
    }
    pub fn with_timeout(mut self, secs: u64) -> Self {
        self.timeout = Some(Duration::from_secs(secs));
        self
    }
}

/// PATH with the user's tool folders added (menu launches get a minimal PATH).
/// PATH for GitHub steps on GitHub's terms: the system's folders first, and
/// your own installed programs (~/.local/bin, ~/.cargo/bin) only at the end,
/// so an installed copy of an app never stands in for the one being checked.
/// A desktop notification (notify-send, which KDE, GNOME and the rest
/// show). Nothing happens if notify-send isn't installed.
pub fn notify(title: &str, body: &str) {
    if which("notify-send").is_none() {
        return;
    }
    let _ = spawn_detached(
        "notify-send",
        &["--app-name=StowLock", "--icon=stowlock", title, body],
        &Opts::default(),
    );
}

pub fn checks_path() -> String {
    let home = crate::util::home_dir();
    let mine = [
        home.join(".local/bin").display().to_string(),
        home.join(".cargo/bin").display().to_string(),
    ];
    let mut parts: Vec<String> = full_path()
        .split(':')
        .filter(|s| !mine.iter().any(|m| m == s))
        .map(String::from)
        .collect();
    parts.extend(mine);
    parts.join(":")
}

pub fn full_path() -> String {
    let home = crate::util::home_dir();
    let mut parts: Vec<String> = vec![
        home.join(".cargo/bin").display().to_string(),
        home.join(".local/bin").display().to_string(),
    ];
    if let Ok(p) = std::env::var("PATH") {
        for seg in p.split(':') {
            if !seg.is_empty() && !parts.iter().any(|x| x == seg) {
                parts.push(seg.to_string());
            }
        }
    }
    for extra in ["/usr/local/bin", "/usr/bin", "/bin"] {
        if !parts.iter().any(|x| x == extra) {
            parts.push(extra.to_string());
        }
    }
    parts.join(":")
}

/// Is this environment variable name something that looks like a credential?
pub fn is_sensitive_env(name: &str) -> bool {
    let up = name.to_ascii_uppercase();
    const WORDS: [&str; 6] = [
        "TOKEN",
        "SECRET",
        "PASSWORD",
        "API_KEY",
        "PRIVATE_KEY",
        "CREDENTIAL",
    ];
    WORDS.iter().any(|w| up.contains(w))
        || up.starts_with("GH_")
        || up.starts_with("GITHUB_")
        || up.starts_with("AWS_")
}

/// Find a program on the full PATH.
pub fn which(program: &str) -> Option<PathBuf> {
    for dir in full_path().split(':') {
        let p = Path::new(dir).join(program);
        if p.is_file() {
            return Some(p);
        }
    }
    None
}

/// Build a Command with our PATH and environment rules applied.
pub fn command(program: &str, args: &[&str], opts: &Opts) -> Command {
    let mut cmd = Command::new(program);
    cmd.args(args);
    if opts.scrub_env {
        for (k, _) in std::env::vars_os() {
            if let Some(k) = k.to_str() {
                if is_sensitive_env(k) {
                    cmd.env_remove(k);
                }
            }
        }
    }
    cmd.env("PATH", full_path());
    // Never let git or gh stop and wait for typed input.
    cmd.env("GIT_TERMINAL_PROMPT", "0");
    cmd.env("GH_PROMPT_DISABLED", "1");
    cmd.env("GH_NO_UPDATE_NOTIFIER", "1");
    for k in &opts.env_remove {
        cmd.env_remove(k);
    }
    for (k, v) in &opts.env {
        cmd.env(k, v);
    }
    if let Some(d) = &opts.cwd {
        cmd.current_dir(d);
    }
    cmd
}

/// Run a command to completion (or timeout / cancel) and capture its output.
pub fn run(program: &str, args: &[&str], opts: &Opts) -> Output {
    let start = Instant::now();
    let mut out = Output {
        program: program.to_string(),
        args: args.iter().map(|s| s.to_string()).collect(),
        code: None,
        stdout: String::new(),
        stderr: String::new(),
        timed_out: false,
        cancelled: false,
        duration: Duration::ZERO,
    };
    let mut cmd = command(program, args, opts);
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    cmd.stdin(if opts.stdin.is_some() {
        Stdio::piped()
    } else {
        Stdio::null()
    });
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            out.stderr = if e.kind() == std::io::ErrorKind::NotFound {
                format!("{program}: command not found")
            } else {
                format!("{program}: {e}")
            };
            out.duration = start.elapsed();
            return out;
        }
    };
    if let Some(input) = &opts.stdin {
        if let Some(mut si) = child.stdin.take() {
            use std::io::Write;
            let _ = si.write_all(input.as_bytes());
        }
    }
    let mut so = child.stdout.take().expect("piped");
    let mut se = child.stderr.take().expect("piped");
    let t_out = std::thread::spawn(move || {
        let mut v = Vec::new();
        let _ = so.read_to_end(&mut v);
        v
    });
    let t_err = std::thread::spawn(move || {
        let mut v = Vec::new();
        let _ = se.read_to_end(&mut v);
        v
    });
    // Check often at first (most commands finish in a few milliseconds),
    // then settle at every 25 ms for long ones.
    let mut pause = Duration::from_millis(1);
    let mut ended = None;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                out.code = status.code();
                ended = Some(start.elapsed());
                break;
            }
            Ok(None) => {}
            Err(_) => break,
        }
        if let Some(t) = opts.timeout {
            if start.elapsed() > t {
                let _ = child.kill();
                let _ = child.wait();
                out.timed_out = true;
                break;
            }
        }
        if let Some(c) = &opts.cancel {
            if c.load(Ordering::Relaxed) {
                let _ = child.kill();
                let _ = child.wait();
                out.cancelled = true;
                break;
            }
        }
        std::thread::sleep(pause);
        pause = (pause * 2).min(Duration::from_millis(25));
    }
    out.stdout = String::from_utf8_lossy(&t_out.join().unwrap_or_default()).to_string();
    out.stderr = String::from_utf8_lossy(&t_err.join().unwrap_or_default()).to_string();
    if out.timed_out {
        let secs = opts.timeout.map(|t| t.as_secs()).unwrap_or(0);
        out.stderr.push_str(&format!(
            "\n[stowlock] stopped after {secs} seconds (timeout)"
        ));
    }
    out.duration = ended.unwrap_or_else(|| start.elapsed());
    out
}

/// Start a program detached from us (terminal for Run, xdg-open, restart).
/// Ask a yes/no question with the desktop's dialog (KDE, then zenity), or in
/// the terminal if there's no dialog. No way to ask means "no".
pub fn ask_yes_no(title: &str, question: &str) -> bool {
    if which("kdialog").is_some() {
        if let Ok(s) = std::process::Command::new("kdialog")
            .args(["--title", title, "--yesno", question])
            .status()
        {
            return s.success();
        }
    }
    if which("zenity").is_some() {
        if let Ok(s) = std::process::Command::new("zenity")
            .args(["--question", "--title", title, "--text", question])
            .status()
        {
            return s.success();
        }
    }
    use std::io::{BufRead, IsTerminal, Write};
    if std::io::stdin().is_terminal() {
        print!("{question} [y/N] ");
        let _ = std::io::stdout().flush();
        let mut line = String::new();
        let _ = std::io::stdin().lock().read_line(&mut line);
        return matches!(line.trim(), "y" | "Y" | "yes" | "Yes");
    }
    false
}

pub fn spawn_detached(
    program: &str,
    args: &[&str],
    opts: &Opts,
) -> std::io::Result<std::process::Child> {
    let mut cmd = command(program, args, opts);
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    cmd.spawn()
}

/// Open a URL or folder with the desktop's default app.
pub fn xdg_open(target: &str) -> Result<(), String> {
    spawn_detached("xdg-open", &[target], &Opts::default())
        .map(|_| ())
        .map_err(|e| format!("couldn't open {target}: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runs_and_captures() {
        let o = run(
            "sh",
            &["-c", "echo hi; echo err 1>&2; exit 3"],
            &Opts::default_timeout(),
        );
        assert_eq!(o.code, Some(3));
        assert_eq!(o.stdout.trim(), "hi");
        assert!(o.stderr.contains("err"));
    }

    #[test]
    fn times_out() {
        let o = run(
            "sleep",
            &["5"],
            &Opts {
                timeout: Some(Duration::from_millis(200)),
                ..Default::default()
            },
        );
        assert!(o.timed_out);
        assert!(!o.ok());
    }

    #[test]
    fn missing_program_is_reported() {
        let o = run("definitely-not-a-program-xyz", &[], &Opts::default());
        assert!(o.stderr.contains("command not found"));
    }

    #[test]
    fn scrubs_sensitive_env() {
        assert!(is_sensitive_env("GITHUB_TOKEN"));
        assert!(is_sensitive_env("my_api_key"));
        assert!(is_sensitive_env("AWS_REGION"));
        assert!(!is_sensitive_env("HOME"));
        let opts = Opts {
            scrub_env: true,
            env: vec![],
            timeout: Some(Duration::from_secs(5)),
            ..Default::default()
        };
        // SAFETY: tests in this module don't read this variable concurrently.
        std::env::set_var("STOWLOCK_TEST_SECRET", "abc");
        let o = run("sh", &["-c", "echo ${STOWLOCK_TEST_SECRET:-none}"], &opts);
        assert_eq!(o.stdout.trim(), "none");
    }

    #[test]
    fn quick_commands_are_timed_honestly() {
        let o = run("true", &[], &Opts::default_timeout());
        assert!(o.ok());
        assert!(o.duration < Duration::from_millis(20), "{:?}", o.duration);
    }
}
