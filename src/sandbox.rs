// SPDX-License-Identifier: GPL-3.0-or-later
//! Sandboxed checks with bubblewrap: a project's own scripts and tests run in
//! a sealed-off view of the computer, like GitHub's clean machine. They see
//! the project, the build tools and caches, a fresh home and the system's
//! programs (read-only), and nothing else: not your home folder, your keys,
//! your running apps or your desktop. Stopping the check stops everything
//! inside it.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// Is bubblewrap installed and allowed to make a sandbox here? (Checked once.)
pub fn available() -> bool {
    static OK: OnceLock<bool> = OnceLock::new();
    *OK.get_or_init(|| {
        std::process::Command::new("bwrap")
            .args([
                "--ro-bind",
                "/",
                "/",
                "--dev",
                "/dev",
                "--proc",
                "/proc",
                "--unshare-pid",
                "--die-with-parent",
                "true",
            ])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
    })
}

/// What a sandboxed step may use.
#[derive(Debug, Clone, Default)]
pub struct Sandbox {
    /// The real home folder (hidden inside).
    pub home: PathBuf,
    /// Folders the step may change: the project, build caches, tool caches.
    pub writable: Vec<PathBuf>,
    /// Folders it may read (only when they exist): tools on the PATH.
    pub readable: Vec<PathBuf>,
    /// Files hidden even inside writable folders (cargo's login token).
    pub hidden_files: Vec<PathBuf>,
    /// One folder shown at another place (the build cache as ./target).
    pub binds: Vec<(PathBuf, PathBuf)>,
    /// The step's own /tmp.
    pub tmp: PathBuf,
    /// No network at all.
    pub offline: bool,
}

impl Sandbox {
    /// The bubblewrap command line that runs `program args` in `dir`.
    pub fn wrap(&self, dir: &Path, program: &str, args: &[&str]) -> Vec<String> {
        let s = |p: &Path| p.display().to_string();
        let mut a: Vec<String> = [
            "--die-with-parent",
            "--new-session",
            "--unshare-pid",
            "--unshare-ipc",
            "--unshare-uts",
            "--hostname",
            "stowlock-check",
            // The system's programs and settings, read-only.
            "--ro-bind",
            "/",
            "/",
            "--dev",
            "/dev",
            "--proc",
            "/proc",
        ]
        .iter()
        .map(|x| x.to_string())
        .collect();
        // Every home folder is hidden, and the desktop session's sockets.
        for hide in [
            Path::new("/home"),
            self.home.as_path(),
            Path::new("/run/user"),
            // Removable drives and other mounts.
            Path::new("/run/media"),
            Path::new("/media"),
            Path::new("/mnt"),
        ] {
            if hide.is_dir() {
                a.extend(["--tmpfs".into(), s(hide)]);
            }
        }
        a.extend(["--bind".into(), s(&self.tmp), "/tmp".into()]);
        for p in &self.readable {
            a.extend(["--ro-bind-try".into(), s(p), s(p)]);
        }
        for p in &self.writable {
            a.extend(["--bind-try".into(), s(p), s(p)]);
        }
        for f in &self.hidden_files {
            if f.is_file() {
                a.extend(["--ro-bind".into(), "/dev/null".into(), s(f)]);
            }
        }
        if !dir.as_os_str().is_empty() {
            a.extend(["--bind".into(), s(dir), s(dir), "--chdir".into(), s(dir)]);
        }
        for (from, to) in &self.binds {
            a.extend(["--bind".into(), s(from), s(to)]);
        }
        if self.offline {
            a.push("--unshare-net".into());
        }
        a.push("--".into());
        a.push(program.into());
        a.extend(args.iter().map(|x| x.to_string()));
        a
    }
}

/// Does a failure look like the sandbox stopped the step (it tried to write
/// outside the project, or read your home folder)?
pub fn blocked(output: &str) -> bool {
    let l = output.to_ascii_lowercase();
    l.contains("read-only file system") || l.contains("os error 30")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proc::{self, Opts};

    fn sandbox(root: &Path) -> Sandbox {
        let tmp = root.join("tmp");
        std::fs::create_dir_all(&tmp).unwrap();
        Sandbox {
            home: crate::util::home_dir(),
            writable: vec![root.join("cache")],
            readable: vec![],
            hidden_files: vec![],
            binds: vec![],
            tmp,
            offline: false,
        }
    }

    fn run(sb: &Sandbox, dir: &Path, script: &str) -> proc::Output {
        let a = sb.wrap(dir, "sh", &["-c", script]);
        let args: Vec<&str> = a.iter().map(String::as_str).collect();
        proc::run("bwrap", &args, &Opts::default_timeout())
    }

    #[test]
    fn a_sandboxed_step_sees_only_what_it_should() {
        if !available() {
            return;
        }
        let root = tempfile::tempdir().unwrap();
        let project = root.path().join("project");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::create_dir_all(root.path().join("cache")).unwrap();
        let sb = sandbox(root.path());
        // The project and cache can be changed.
        let o = run(
            &sb,
            &project,
            &format!(
                "touch made-here && touch {}/cache/built",
                root.path().display()
            ),
        );
        assert!(o.ok(), "{}", o.combined());
        assert!(project.join("made-here").exists());
        assert!(root.path().join("cache/built").exists());
        // The real home folder isn't there.
        let home = crate::util::home_dir();
        if home.join(".bashrc").exists() || home.join(".profile").exists() {
            let o = run(&sb, &project, &format!("ls -A {}", home.display()));
            assert!(o.stdout.trim().is_empty(), "home is visible: {}", o.stdout);
        }
        // Nothing outside can be changed.
        let o = run(&sb, &project, "touch /usr/stowlock-should-not-exist");
        assert!(!o.ok());
        assert!(
            blocked(&o.combined()) || o.combined().contains("ermission"),
            "{}",
            o.combined()
        );
        // Only its own programs are running.
        let o = run(&sb, &project, "ls /proc | grep -c '^[0-9]'");
        let n: u32 = o.stdout.trim().parse().unwrap_or(99);
        assert!(n <= 4, "it saw {n} processes");
    }

    #[test]
    fn offline_means_no_network() {
        if !available() {
            return;
        }
        let root = tempfile::tempdir().unwrap();
        let mut sb = sandbox(root.path());
        sb.offline = true;
        let o = run(&sb, root.path(), "cat /proc/net/dev | grep -c ':'");
        assert_eq!(o.stdout.trim(), "1", "only loopback: {}", o.combined());
    }
}
