// SPDX-License-Identifier: GPL-3.0-or-later
//! GitHub sign-in without a terminal: StowLock drives `gh auth login --web`,
//! shows the one-time code, opens the browser itself and finishes setup.
//! Also installs missing tools through the desktop's password dialog.

use crate::proc::{self, Opts, Output};
use regex::Regex;
use std::io::{BufRead, BufReader, Write};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

pub const DEVICE_URL: &str = "https://github.com/login/device";

#[derive(Debug, Clone)]
pub enum SignInEvent {
    Code(String),
    Line(String),
    Done(Result<String, String>),
}

/// Find the one-time code in a line of gh's output.
pub fn parse_code(line: &str) -> Option<String> {
    static R: OnceLock<Regex> = OnceLock::new();
    let re =
        R.get_or_init(|| Regex::new(r"(?i)one-time code:?\s*([A-Z0-9]{4}-[A-Z0-9]{4})").unwrap());
    re.captures(line).map(|c| c[1].to_ascii_uppercase())
}

/// Run the whole sign-in on the calling (worker) thread, reporting events.
pub fn sign_in(
    events: Sender<SignInEvent>,
    cancel: Arc<AtomicBool>,
    notify: impl Fn() + Send + Sync + 'static,
) {
    let send = |e: SignInEvent| {
        let _ = events.send(e);
        notify();
    };
    let opts = Opts {
        env: vec![
            ("GH_BROWSER".into(), "true".into()),
            ("BROWSER".into(), "true".into()),
        ],
        ..Default::default()
    };
    let mut cmd = proc::command(
        "gh",
        &[
            "auth",
            "login",
            "--hostname",
            "github.com",
            "--git-protocol",
            "https",
            "--web",
        ],
        &opts,
    );
    cmd.env_remove("GH_PROMPT_DISABLED");
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            send(SignInEvent::Done(Err(format!(
                "Couldn't start gh: {e}. Install GitHub's gh tool first."
            ))));
            return;
        }
    };
    let mut stdin = child.stdin.take();
    let (ltx, lrx) = std::sync::mpsc::channel::<String>();
    for stream in [
        child
            .stdout
            .take()
            .map(|s| Box::new(s) as Box<dyn std::io::Read + Send>),
        child
            .stderr
            .take()
            .map(|s| Box::new(s) as Box<dyn std::io::Read + Send>),
    ]
    .into_iter()
    .flatten()
    {
        let ltx = ltx.clone();
        std::thread::spawn(move || {
            // gh prints "Press Enter…" without a newline; read in small chunks.
            let mut reader = BufReader::new(stream);
            let mut buf = Vec::new();
            loop {
                buf.clear();
                match reader.read_until(b'\n', &mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {
                        let _ = ltx.send(String::from_utf8_lossy(&buf).trim_end().to_string());
                    }
                }
            }
        });
    }
    drop(ltx);
    let start = Instant::now();
    let mut code_sent = false;
    let mut last_enter = Instant::now() - Duration::from_secs(10);
    loop {
        while let Ok(line) = lrx.try_recv() {
            if let Some(code) = parse_code(&line) {
                if !code_sent {
                    code_sent = true;
                    send(SignInEvent::Code(code));
                }
            }
            if !line.trim().is_empty() {
                send(SignInEvent::Line(line));
            }
        }
        // Press Enter for gh (it waits before "opening" the browser).
        if code_sent
            && last_enter.elapsed() > Duration::from_secs(2)
            && start.elapsed() < Duration::from_secs(20)
        {
            if let Some(si) = stdin.as_mut() {
                let _ = si.write_all(b"\n");
                let _ = si.flush();
            }
            last_enter = Instant::now();
        }
        match child.try_wait() {
            Ok(Some(status)) => {
                while let Ok(line) = lrx.try_recv() {
                    send(SignInEvent::Line(line));
                }
                if status.success() {
                    let setup = proc::run("gh", &["auth", "setup-git"], &Opts::default_timeout());
                    let login = proc::run(
                        "gh",
                        &["api", "user", "--jq", ".login"],
                        &Opts::default_timeout(),
                    )
                    .stdout_trim();
                    if !setup.ok() {
                        send(SignInEvent::Line(format!(
                            "gh auth setup-git: {}",
                            setup.combined()
                        )));
                    }
                    send(SignInEvent::Done(Ok(login)));
                } else {
                    send(SignInEvent::Done(Err(
                        "Sign-in didn't finish. You can try again.".into(),
                    )));
                }
                return;
            }
            Ok(None) => {}
            Err(e) => {
                send(SignInEvent::Done(Err(e.to_string())));
                return;
            }
        }
        if cancel.load(Ordering::Relaxed) || start.elapsed() > Duration::from_secs(15 * 60) {
            let _ = child.kill();
            let _ = child.wait();
            let why = if cancel.load(Ordering::Relaxed) {
                "Sign-in cancelled."
            } else {
                "Sign-in timed out after 15 minutes."
            };
            send(SignInEvent::Done(Err(why.into())));
            return;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Install git and gh through the desktop's password dialog (polkit).
pub fn install_tools() -> Output {
    if proc::which("pkexec").is_none() {
        let mut o = proc::run("true", &[], &Opts::default());
        o.code = Some(1);
        o.stderr = "pkexec isn't available, so StowLock can't ask for your password. Install with: sudo pacman -S git github-cli".into();
        return o;
    }
    proc::run(
        "pkexec",
        &[
            "pacman",
            "-S",
            "--needed",
            "--noconfirm",
            "git",
            "github-cli",
        ],
        &Opts::default_timeout().with_timeout(900),
    )
}

pub fn sign_out(login: &str) -> Output {
    proc::run(
        "gh",
        &[
            "auth",
            "logout",
            "--hostname",
            "github.com",
            "--user",
            login,
        ],
        &Opts::default_timeout(),
    )
}

pub fn switch_account(login: &str) -> Output {
    proc::run(
        "gh",
        &[
            "auth",
            "switch",
            "--hostname",
            "github.com",
            "--user",
            login,
        ],
        &Opts::default_timeout(),
    )
}

/// GitHub's private commit email for the signed-in account.
pub fn noreply_email() -> Option<String> {
    let o = proc::run(
        "gh",
        &[
            "api",
            "user",
            "--jq",
            "\"\\(.id)+\\(.login)@users.noreply.github.com\"",
        ],
        &Opts::default_timeout(),
    );
    let v = o.stdout_trim().trim_matches('"').to_string();
    if o.ok() && v.contains("@users.noreply.github.com") {
        Some(v)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_gh_device_code() {
        assert_eq!(
            parse_code("! First copy your one-time code: AB12-CD34").as_deref(),
            Some("AB12-CD34")
        );
        assert_eq!(
            parse_code("! Your one-time code: 9F3A-1B2C").as_deref(),
            Some("9F3A-1B2C")
        );
        assert_eq!(
            parse_code("Press Enter to open github.com in your browser..."),
            None
        );
        assert_eq!(
            parse_code("something changed: code is ABCD1234"),
            None,
            "unknown formats fall back to raw text"
        );
    }
}
