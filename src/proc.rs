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
    /// Show the output as it comes (the newest lines, for the checks card).
    pub live: bool,
}

/// The newest output lines of the command running with `live` (one check
/// runs at a time), for showing progress while it runs.
static LIVE: std::sync::Mutex<std::collections::VecDeque<String>> =
    std::sync::Mutex::new(std::collections::VecDeque::new());
const LIVE_LINES: usize = 6;

/// The newest lines of the running check's output.
pub fn live_lines() -> Vec<String> {
    LIVE.lock()
        .map(|l| l.iter().cloned().collect())
        .unwrap_or_default()
}

fn live_clear() {
    if let Ok(mut l) = LIVE.lock() {
        l.clear();
    }
}

fn live_push(text: &str) {
    let line = crate::util::strip_ansi(text);
    // A progress line rewritten with \r: keep its newest version.
    let line = line
        .rsplit('\r')
        .next()
        .unwrap_or("")
        .trim_end()
        .to_string();
    if line.trim().is_empty() {
        return;
    }
    if let Ok(mut l) = LIVE.lock() {
        l.push_back(line);
        while l.len() > LIVE_LINES {
            l.pop_front();
        }
    }
}

/// Read a pipe to the end; with `live`, also pass on each line as it comes.
fn read_all(mut r: impl Read, live: bool) -> Vec<u8> {
    let mut all = Vec::new();
    if !live {
        let _ = r.read_to_end(&mut all);
        return all;
    }
    let mut buf = [0u8; 8192];
    let mut start = 0;
    loop {
        match r.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                all.extend_from_slice(&buf[..n]);
                while let Some(i) = all[start..].iter().position(|b| *b == b'\n') {
                    live_push(&String::from_utf8_lossy(&all[start..start + i]));
                    start += i + 1;
                }
            }
        }
    }
    all
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

/// Stop a command and everything it started: ask nicely, then insist.
fn stop_group(child: &mut std::process::Child) {
    let group = format!("-{}", child.id());
    let signal = |sig: &str| {
        let _ = Command::new("kill")
            .args([sig, "--", &group])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    };
    signal("-TERM");
    for _ in 0..20 {
        if let Ok(Some(_)) = child.try_wait() {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    // Whatever is left (a test program that ignores TERM) is stopped for sure.
    signal("-KILL");
    let _ = child.kill();
    let _ = child.wait();
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
    // Its own process group, so Cancel and the time limit stop everything it
    // started (a check script's cargo, rustc and test programs), not only
    // the script itself.
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
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
    let so = child.stdout.take().expect("piped");
    let se = child.stderr.take().expect("piped");
    let live = opts.live;
    if live {
        live_clear();
    }
    let t_out = std::thread::spawn(move || read_all(so, live));
    let t_err = std::thread::spawn(move || read_all(se, live));
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
                stop_group(&mut child);
                out.timed_out = true;
                break;
            }
        }
        if let Some(c) = &opts.cancel {
            if c.load(Ordering::Relaxed) {
                stop_group(&mut child);
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

    #[test]
    fn live_output_keeps_the_whole_output_too() {
        let text = "one\ntwo\r50%\rthree\nfour (no newline)";
        let plain = read_all(text.as_bytes(), false);
        let live = read_all(text.as_bytes(), true);
        assert_eq!(plain, live);
        assert_eq!(String::from_utf8(live).unwrap(), text);
    }

    #[test]
    fn cancel_stops_everything_the_command_started() {
        // Like a check script running cargo: the script waits on programs it
        // started. Cancel must stop them too, not wait for them to finish.
        let cancel = Arc::new(AtomicBool::new(false));
        let c2 = cancel.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            c2.store(true, Ordering::Relaxed);
        });
        let mut opts = Opts::default_timeout();
        opts.cancel = Some(cancel);
        let t = Instant::now();
        let o = run("sh", &["-c", "sleep 30 & sleep 30; wait"], &opts);
        assert!(o.cancelled);
        assert!(
            t.elapsed() < Duration::from_secs(5),
            "took {:?}",
            t.elapsed()
        );
    }

    #[test]
    fn the_time_limit_stops_everything_too() {
        let t = Instant::now();
        let o = run(
            "sh",
            &["-c", "sh -c 'trap \"\" TERM; sleep 30' & sleep 30; wait"],
            &Opts::default_timeout().with_timeout(1),
        );
        assert!(o.timed_out);
        assert!(
            t.elapsed() < Duration::from_secs(6),
            "took {:?}",
            t.elapsed()
        );
    }

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
