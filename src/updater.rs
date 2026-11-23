// SPDX-License-Identifier: GPL-3.0-or-later
//! Two-click self-update from StowLock's own GitHub releases: check quietly,
//! download the release binary, verify its SHA-256, swap it in atomically.

use crate::config::{AppPaths, APP_VERSION};
use crate::logbook::Logger;
use crate::proc::{self, Opts};
use crate::uninstall::pacman_owner;
use crate::util::{self, Ctx, Res};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

pub const ASSET: &str = "stowlock-linux-x86_64";
const CHECK_EVERY_SECS: i64 = 6 * 3600;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct LastCheck {
    at: i64,
}

fn last_check_file(paths: &AppPaths) -> PathBuf {
    paths.state_dir.join("update-check.json")
}

/// Has it been 6 hours since the last check?
pub fn check_due(paths: &AppPaths) -> bool {
    let lc: LastCheck = util::read_json_or_default(&last_check_file(paths));
    chrono::Local::now().timestamp() - lc.at >= CHECK_EVERY_SECS
}

/// Is `tag` newer than this build? Pre-releases are never offered.
pub fn is_newer(tag: &str, current: &str) -> bool {
    let parse = |s: &str| semver::Version::parse(s.trim().trim_start_matches('v')).ok();
    match (parse(tag), parse(current)) {
        (Some(t), Some(c)) => t.pre.is_empty() && t > c,
        _ => false,
    }
}

/// Latest stable release tag, if newer than this build.
pub fn check(paths: &AppPaths, repo: &str) -> Res<Option<String>> {
    let _ = util::write_json(
        &last_check_file(paths),
        &LastCheck {
            at: chrono::Local::now().timestamp(),
        },
    );
    let o = proc::run(
        "gh",
        &[
            "release",
            "list",
            "--repo",
            repo,
            "--exclude-pre-releases",
            "--limit",
            "1",
            "--json",
            "tagName",
        ],
        &Opts::default_timeout().with_timeout(30),
    );
    if !o.ok() {
        return Err(o.combined());
    }
    let v: serde_json::Value = serde_json::from_str(&o.stdout).ctx("reading release list")?;
    let tag = v
        .as_array()
        .and_then(|a| a.first())
        .and_then(|r| r["tagName"].as_str())
        .map(|s| s.to_string());
    Ok(tag.filter(|t| is_newer(t, APP_VERSION)))
}

pub fn sha256_hex(path: &Path) -> Res<String> {
    let bytes = std::fs::read(path).ctx("reading the download")?;
    Ok(Sha256::digest(&bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}

/// Compare a file against a `.sha256` file's text (`<hex>  <name>`).
pub fn verify_checksum(file: &Path, sha_text: &str) -> Res<()> {
    let want = sha_text
        .split_whitespace()
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    if want.len() != 64 {
        return Err("The checksum file is malformed.".into());
    }
    let got = sha256_hex(file)?;
    if got == want {
        Ok(())
    } else {
        Err("The download doesn't match its checksum, so it wasn't installed.".into())
    }
}

/// Download, verify and install `tag`. Returns the path that was replaced.
pub fn apply(paths: &AppPaths, repo: &str, tag: &str, log: &Logger) -> Res<PathBuf> {
    let exe = std::env::current_exe().ctx("finding the running program")?;
    let exe = std::fs::canonicalize(&exe).unwrap_or(exe);
    if let Some(pkg) = pacman_owner(&exe) {
        return Err(format!(
            "StowLock was installed by your package manager. Update with: sudo pacman -Syu {pkg}"
        ));
    }
    let dir = paths.update_dir();
    let _ = std::fs::remove_dir_all(&dir);
    util::ensure_private_dir(&dir)?;
    let d = dir.display().to_string();
    let pattern = format!("{ASSET}*");
    let o = proc::run(
        "gh",
        &[
            "release",
            "download",
            tag,
            "--repo",
            repo,
            "--pattern",
            &pattern,
            "--dir",
            &d,
            "--clobber",
        ],
        &Opts::default_timeout().with_timeout(300),
    );
    log.command("update", &o);
    if !o.ok() {
        return Err(o.combined());
    }
    let bin = dir.join(ASSET);
    let sha = std::fs::read_to_string(dir.join(format!("{ASSET}.sha256")))
        .map_err(|_| "The release has no checksum file, so it wasn't installed.".to_string())?;
    verify_checksum(&bin, &sha)?;
    std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755))
        .ctx("making it executable")?;
    let target_dir = exe.parent().ok_or("no folder for the program")?;
    let staged = target_dir.join(format!(".stowlock.update-{}", std::process::id()));
    std::fs::copy(&bin, &staged).ctx("staging the new version")?;
    std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o755)).ok();
    // Linux keeps the running program's file open; new launches get the new one.
    std::fs::rename(&staged, &exe).ctx("swapping in the new version")?;
    let _ = std::fs::remove_dir_all(&dir);
    log.info("update", &format!("Installed StowLock {tag}"));
    Ok(exe)
}

/// Start the new binary and let the caller exit.
pub fn restart(exe: &Path) -> Res<()> {
    proc::spawn_detached(&exe.display().to_string(), &[], &Opts::default())
        .map(|_| ())
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_comparison() {
        assert!(is_newer("v0.2.0", "0.1.0"));
        assert!(!is_newer("v0.1.0", "0.1.0"));
        assert!(!is_newer("v0.0.9", "0.1.0"));
        assert!(!is_newer("v0.2.0-beta.1", "0.1.0"), "betas never offered");
        assert!(!is_newer("junk", "0.1.0"));
    }

    #[test]
    fn checksum_mismatch_rejected() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join(ASSET);
        std::fs::write(&f, b"binary").unwrap();
        let good = format!("{}  {ASSET}\n", sha256_hex(&f).unwrap());
        assert!(verify_checksum(&f, &good).is_ok());
        let bad = format!("{}  {ASSET}\n", "0".repeat(64));
        assert!(verify_checksum(&f, &bad).is_err());
        assert!(verify_checksum(&f, "nonsense").is_err());
    }
}
