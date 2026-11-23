// SPDX-License-Identifier: GPL-3.0-or-later
//! One copy at a time. A second launch with a zip hands the path to the
//! running copy through the `pending/` folder, then exits.

use crate::config::AppPaths;
use crate::util::{self, Ctx, Res};
use std::fs::File;
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
    match f.try_lock() {
        Ok(()) => Ok(Acquire::First(InstanceGuard { _file: f })),
        Err(std::fs::TryLockError::WouldBlock) => Ok(Acquire::AlreadyRunning),
        Err(std::fs::TryLockError::Error(e)) => Err(format!("checking for another StowLock: {e}")),
    }
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
}
