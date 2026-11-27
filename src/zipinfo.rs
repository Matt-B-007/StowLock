// SPDX-License-Identifier: GPL-3.0-or-later
//! Which zip was loaded: its size, when it was saved, a fingerprint of its
//! contents, whether it's the same as the last one staged for that project,
//! and whether the browser saved a newer copy beside it ("name(1).zip").

use crate::config::AppPaths;
use crate::util;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

#[derive(Debug, Clone, PartialEq)]
pub struct ZipInfo {
    pub size: u64,
    pub modified: Option<SystemTime>,
    /// SHA-256 of the file, hex.
    pub sha: String,
}

/// Read a zip's size, time and fingerprint.
pub fn read(path: &Path) -> Option<ZipInfo> {
    let meta = std::fs::metadata(path).ok()?;
    let mut f = std::fs::File::open(path).ok()?;
    let mut h = Sha256::new();
    std::io::copy(&mut f, &mut h).ok()?;
    Some(ZipInfo {
        size: meta.len(),
        modified: meta.modified().ok(),
        sha: format!("{:x}", h.finalize()),
    })
}

impl ZipInfo {
    /// "870 KB · saved today 10:59" (or "saved 26 Nov 18:38").
    pub fn describe(&self) -> String {
        let when = self.modified.map(|t| {
            let t: chrono::DateTime<chrono::Local> = t.into();
            let today = chrono::Local::now().date_naive();
            if t.date_naive() == today {
                format!("saved today {}", t.format("%H:%M"))
            } else {
                format!("saved {}", t.format("%-d %b %H:%M"))
            }
        });
        match when {
            Some(w) => format!("{} · {w}", util::human_size(self.size)),
            None => util::human_size(self.size),
        }
    }
}

/// "app-v1.zip", "app-v1(1).zip" and "app-v1 (2).zip" are copies of one download.
fn base_name(file: &str) -> Option<String> {
    let stem = file
        .strip_suffix(".zip")
        .or_else(|| file.strip_suffix(".ZIP"))?;
    let re = regex::Regex::new(r"^(.*?) ?\(\d+\)$").unwrap();
    Some(match re.captures(stem) {
        Some(c) => c[1].to_string(),
        None => stem.to_string(),
    })
}

/// A newer copy of the same download in the same folder, if there is one.
pub fn newer_copy(path: &Path) -> Option<PathBuf> {
    let name = path.file_name()?.to_str()?;
    let base = base_name(name)?;
    let mine = std::fs::metadata(path).ok()?.modified().ok()?;
    let mut best: Option<(SystemTime, PathBuf)> = None;
    for e in std::fs::read_dir(path.parent()?).ok()?.flatten() {
        let p = e.path();
        if p == path {
            continue;
        }
        let Some(n) = p.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if base_name(n).as_deref() != Some(base.as_str()) {
            continue;
        }
        let Ok(t) = e.metadata().and_then(|m| m.modified()) else {
            continue;
        };
        if t > mine && best.as_ref().is_none_or(|(bt, _)| t > *bt) {
            best = Some((t, p));
        }
    }
    best.map(|(_, p)| p)
}

fn memory_file(paths: &AppPaths) -> PathBuf {
    paths.state_dir.join("last-zips.json")
}

/// Was this exact zip the last one staged for the project?
pub fn same_as_last(paths: &AppPaths, slug: &str, sha: &str) -> bool {
    let m: BTreeMap<String, String> = util::read_json_or_default(&memory_file(paths));
    m.get(slug).is_some_and(|s| s == sha)
}

/// Remember the zip staged for a project.
pub fn remember(paths: &AppPaths, slug: &str, sha: &str) {
    let mut m: BTreeMap<String, String> = util::read_json_or_default(&memory_file(paths));
    m.insert(slug.to_string(), sha.to_string());
    let _ = util::write_json(&memory_file(paths), &m);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copies_of_one_download_are_recognised() {
        assert_eq!(base_name("app-v1.zip").unwrap(), "app-v1");
        assert_eq!(base_name("app-v1(1).zip").unwrap(), "app-v1");
        assert_eq!(base_name("app-v1 (12).zip").unwrap(), "app-v1");
        assert!(base_name("notes.txt").is_none());
    }

    #[test]
    fn the_newest_copy_beside_it_is_found() {
        let d = tempfile::tempdir().unwrap();
        let old = d.path().join("stowlock-v0.2.0-beta.1.zip");
        let newer = d.path().join("stowlock-v0.2.0-beta.1(1).zip");
        let other = d.path().join("sluice-v1.zip");
        for (p, secs) in [(&old, 100), (&newer, 200), (&other, 300)] {
            std::fs::write(p, b"zip").unwrap();
            let t = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000 + secs);
            std::fs::File::options()
                .write(true)
                .open(p)
                .unwrap()
                .set_modified(t)
                .unwrap();
        }
        assert_eq!(newer_copy(&old), Some(newer.clone()));
        assert_eq!(newer_copy(&newer), None, "it's already the newest");
    }

    #[test]
    fn the_same_zip_twice_is_noticed() {
        let d = tempfile::tempdir().unwrap();
        let paths = AppPaths::under(d.path());
        std::fs::create_dir_all(&paths.state_dir).unwrap();
        let z = d.path().join("a.zip");
        std::fs::write(&z, b"one").unwrap();
        let first = read(&z).unwrap();
        assert!(!same_as_last(&paths, "app", &first.sha));
        remember(&paths, "app", &first.sha);
        assert!(same_as_last(&paths, "app", &first.sha));
        std::fs::write(&z, b"two").unwrap();
        assert!(!same_as_last(&paths, "app", &read(&z).unwrap().sha));
        assert!(
            first.describe().contains("saved today"),
            "{}",
            first.describe()
        );
    }
}
