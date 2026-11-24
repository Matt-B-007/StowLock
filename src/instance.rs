// SPDX-License-Identifier: GPL-3.0-or-later
//! One copy at a time. A second launch with a zip hands the path to the
//! running copy through the `pending/` folder, then exits.

use crate::config::AppPaths;
use crate::util::{self, Ctx, Res};
use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};

pub struct InstanceGuard {
    _file: File,
}

pub enum Acquire {
    First(InstanceGuard),
    AlreadyRunning,
}

pub fn acquire(paths: &AppPaths) -> Res<Acquire> {
    util::ensure_private_dir(&paths.state_dir)?;
    let f = File::options()
        .create(true)
        .truncate(false)
        .write(true)
        .open(paths.lock_file())
        .ctx("opening the lock file")?;
    // A lock can look busy for a moment after its owner let go: any program
    // started at that instant briefly holds a copy of the lock file. So a busy
    // lock is retried for up to half a second before deciding another
    // StowLock is really open.
    for attempt in 0..LOCK_TRIES {
        match f.try_lock() {
            Ok(()) => {
                // Record our process id so a later launch can find this copy.
                let _ = f.set_len(0);
                let _ = (&f).write_all(std::process::id().to_string().as_bytes());
                return Ok(Acquire::First(InstanceGuard { _file: f }));
            }
            Err(std::fs::TryLockError::WouldBlock) => {
                if attempt + 1 < LOCK_TRIES {
                    std::thread::sleep(std::time::Duration::from_millis(25));
                }
            }
            Err(std::fs::TryLockError::Error(e)) => {
                return Err(format!("checking for another StowLock: {e}"))
            }
        }
    }
    Ok(Acquire::AlreadyRunning)
}

const LOCK_TRIES: u32 = 20;

/// The process id of the StowLock that's open, if any. Newer copies write it
/// into the lock file; older ones are found by name.
pub fn running_pid(paths: &AppPaths) -> Option<u32> {
    let me = std::process::id();
    let alive = |pid: u32| pid != me && Path::new(&format!("/proc/{pid}")).exists();
    if let Some(pid) = pid_in_lock_file(paths) {
        return Some(pid);
    }
    let o = crate::proc::run(
        "pgrep",
        &["-x", "stowlock"],
        &crate::proc::Opts::default_timeout().with_timeout(5),
    );
    o.stdout
        .lines()
        .filter_map(|l| l.trim().parse::<u32>().ok())
        .find(|p| alive(*p))
}

/// The process id written in the lock file, if that process is another one
/// that's still running.
pub fn pid_in_lock_file(paths: &AppPaths) -> Option<u32> {
    let me = std::process::id();
    std::fs::read_to_string(paths.lock_file())
        .ok()
        .and_then(|t| t.trim().parse::<u32>().ok())
        .filter(|p| *p != me && Path::new(&format!("/proc/{p}")).exists())
}

/// The version of the StowLock running as `pid` (asks its own program file).
pub fn running_version(pid: u32) -> Option<String> {
    crate::install::program_version(Path::new(&format!("/proc/{pid}/exe")))
}

/// Is version `a` older than version `b`?
pub fn is_older(a: &str, b: &str) -> bool {
    let parse = |s: &str| semver::Version::parse(s.trim().trim_start_matches('v')).ok();
    matches!((parse(a), parse(b)), (Some(x), Some(y)) if x < y)
}

/// Ask the open copy to close, then wait up to `secs` for its lock.
pub fn take_over(paths: &AppPaths, pid: u32, secs: u64) -> Option<InstanceGuard> {
    let _ = std::process::Command::new("kill")
        .args(["-TERM", &pid.to_string()])
        .status();
    let until = std::time::Instant::now() + std::time::Duration::from_secs(secs);
    while std::time::Instant::now() < until {
        if let Ok(Acquire::First(g)) = acquire(paths) {
            return Some(g);
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
    None
}

/// Give a zip to the copy that's already running.
pub fn hand_off(paths: &AppPaths, zip: &Path) -> Res<()> {
    util::ensure_private_dir(&paths.pending_dir())?;
    let abs = std::fs::canonicalize(zip).unwrap_or_else(|_| zip.to_path_buf());
    let name = format!("{}-{}.txt", util::now_stamp_secs(), std::process::id());
    util::atomic_write(
        &paths.pending_dir().join(name),
        abs.display().to_string().as_bytes(),
    )
}

/// Collect (and remove) zips handed over by other launches.
pub fn take_pending(paths: &AppPaths) -> Vec<PathBuf> {
    let mut v = Vec::new();
    let Ok(rd) = std::fs::read_dir(paths.pending_dir()) else {
        return v;
    };
    let mut files: Vec<PathBuf> = rd
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().map(|x| x == "txt").unwrap_or(false))
        .collect();
    files.sort();
    for f in files {
        if let Ok(t) = std::fs::read_to_string(&f) {
            let p = PathBuf::from(t.trim());
            if p.is_file() {
                v.push(p);
            }
        }
        let _ = std::fs::remove_file(&f);
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lock_and_hand_off() {
        let d = tempfile::tempdir().unwrap();
        let paths = AppPaths::under(d.path());
        let first = acquire(&paths).unwrap();
        assert!(matches!(first, Acquire::First(_)));
        assert!(matches!(acquire(&paths).unwrap(), Acquire::AlreadyRunning));
        let z = d.path().join("p.zip");
        std::fs::write(&z, "z").unwrap();
        hand_off(&paths, &z).unwrap();
        let got = take_pending(&paths);
        assert_eq!(got.len(), 1);
        assert!(take_pending(&paths).is_empty());
        drop(first);
        assert!(matches!(acquire(&paths).unwrap(), Acquire::First(_)));
    }

    #[test]
    fn lock_file_names_the_open_copy() {
        let d = tempfile::tempdir().unwrap();
        let paths = AppPaths::under(d.path());
        let _g = acquire(&paths).unwrap();
        let text = std::fs::read_to_string(paths.lock_file()).unwrap();
        assert_eq!(text, std::process::id().to_string());
        // Our own process never counts as "another copy". (Only the lock file
        // is checked here: a real StowLock may be open on this computer while
        // its tests run, and must not make this test fail.)
        assert_eq!(pid_in_lock_file(&paths), None);
    }

    #[test]
    fn version_order() {
        assert!(is_older("0.1.0-beta.2", "0.1.0-beta.3"));
        assert!(is_older("0.1.0", "0.2.0"));
        assert!(!is_older("0.1.0-beta.3", "0.1.0-beta.3"));
        assert!(!is_older("junk", "0.1.0"));
    }

    #[test]
    fn released_lock_is_free_while_other_programs_start() {
        // Starting programs at the same moment used to make a just-released
        // lock look busy (seen on a fast machine during the tests).
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let starters: Vec<_> = (0..4)
            .map(|_| {
                let s = stop.clone();
                std::thread::spawn(move || {
                    while !s.load(std::sync::atomic::Ordering::Relaxed) {
                        let _ = std::process::Command::new("true").status();
                    }
                })
            })
            .collect();
        let d = tempfile::tempdir().unwrap();
        let paths = AppPaths::under(d.path());
        let mut busy = 0;
        for _ in 0..200 {
            drop(acquire(&paths).unwrap());
            if !matches!(acquire(&paths).unwrap(), Acquire::First(_)) {
                busy += 1;
            }
        }
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        for h in starters {
            let _ = h.join();
        }
        assert_eq!(busy, 0, "a released lock looked busy {busy} times");
    }
}
