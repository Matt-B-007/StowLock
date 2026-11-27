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

/// Is `tag` newer than `current`? Pre-releases count only when `beta` is on.
/// Nothing older is ever offered, whichever channel is chosen.
pub fn is_newer(tag: &str, current: &str, beta: bool) -> bool {
    let parse = |s: &str| semver::Version::parse(s.trim().trim_start_matches('v')).ok();
    match (parse(tag), parse(current)) {
        (Some(t), Some(c)) => (beta || t.pre.is_empty()) && t > c,
        _ => false,
    }
}

/// A newer release StowLock can offer.
#[derive(Debug, Clone, PartialEq)]
pub struct Release {
    pub tag: String,
    pub prerelease: bool,
    /// The release's notes ("What's new"), plain text, shortened.
    pub notes: String,
}

impl Release {
    /// "Beta 0.1.0-beta.3" or "0.2.0".
    pub fn label(&self) -> String {
        let v = self.tag.trim_start_matches('v');
        if self.prerelease {
            format!("Beta {v}")
        } else {
            v.to_string()
        }
    }
}

/// Keep release notes readable and bounded.
pub fn tidy_notes(body: &str) -> String {
    const MAX: usize = 4000;
    let text = body.replace("\r\n", "\n");
    let text = text.trim();
    if text.chars().count() <= MAX {
        return text.to_string();
    }
    let cut: String = text.chars().take(MAX).collect();
    format!("{cut}\n…(see the release page for the rest)")
}

/// From GitHub's public releases list (the REST API's JSON), the newest
/// release this channel may offer, with its notes, if it's newer than `current`.
pub fn pick_from_releases(json: &str, current: &str, beta: bool) -> Option<Release> {
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    let parse = |s: &str| semver::Version::parse(s.trim().trim_start_matches('v')).ok();
    v.as_array()?
        .iter()
        .filter(|r| !r["draft"].as_bool().unwrap_or(false))
        .filter(|r| has_app(r))
        .filter_map(|r| {
            let tag = r["tag_name"].as_str()?.to_string();
            let ver = parse(&tag)?;
            is_newer(&tag, current, beta).then(|| {
                (
                    ver,
                    Release {
                        tag,
                        prerelease: r["prerelease"].as_bool().unwrap_or(false),
                        notes: tidy_notes(r["body"].as_str().unwrap_or("")),
                    },
                )
            })
        })
        .max_by(|a, b| a.0.cmp(&b.0))
        .map(|(_, r)| r)
}

/// Download a public URL to a file (or to text when `out` is None) with curl.
/// No GitHub sign-in is needed, so updates work right after installing.
fn fetch(url: &str, out: Option<&Path>, secs: u64) -> Res<String> {
    if proc::which("curl").is_none() {
        return Err(
            "curl is needed to check for updates. Install it with: sudo pacman -S curl".into(),
        );
    }
    let mut args = vec!["-fsSL", "--retry", "2", "-H", "User-Agent: StowLock"];
    let out_s;
    if let Some(o) = out {
        out_s = o.display().to_string();
        args.push("-o");
        args.push(&out_s);
    }
    args.push(url);
    let o = proc::run("curl", &args, &Opts::default_timeout().with_timeout(secs));
    if o.ok() {
        Ok(o.stdout)
    } else {
        let why = o.stderr.trim();
        Err(if why.is_empty() {
            "couldn't reach GitHub".to_string()
        } else {
            why.to_string()
        })
    }
}

/// Is this release ready to install: the app attached, and its checksums
/// signed? A release whose build failed or is still running, or one without
/// a signature, counts as "not available yet" and is never offered.
fn has_app(r: &serde_json::Value) -> bool {
    match r["assets"].as_array() {
        Some(a) => {
            let has = |n: &str| a.iter().any(|x| x["name"].as_str() == Some(n));
            has(ASSET) && has(crate::signing::SUMS) && has(crate::signing::SUMS_SIG)
        }
        None => true, // an answer without asset details: don't block on it
    }
}

/// A newer beta than `current`, for telling Stable users one exists.
pub fn newer_beta(json: &str, current: &str) -> Option<String> {
    pick_from_releases(json, current, true)
        .filter(|r| r.prerelease)
        .map(|r| r.tag)
}

/// The newest release on GitHub for this channel, newer or not.
pub fn newest_tag(json: &str, beta: bool) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    let parse = |s: &str| semver::Version::parse(s.trim().trim_start_matches('v')).ok();
    v.as_array()?
        .iter()
        .filter(|r| !r["draft"].as_bool().unwrap_or(false))
        .filter_map(|r| {
            let tag = r["tag_name"].as_str()?.to_string();
            let ver = parse(&tag)?;
            (beta || ver.pre.is_empty()).then_some((ver, tag))
        })
        .max_by(|a, b| a.0.cmp(&b.0))
        .map(|(_, t)| t)
}

/// What an update check found.
#[derive(Debug, Clone, PartialEq)]
pub struct CheckResult {
    /// A newer release this channel may offer.
    pub newer: Option<Release>,
    /// The newest release on GitHub for this channel (may be this version).
    pub newest_tag: Option<String>,
    /// The anonymous check was refused, so the user's GitHub sign-in was used.
    pub via_sign_in: bool,
    /// On Stable: a newer beta exists (switch the channel to try it).
    pub newer_beta: Option<String>,
}

/// Turn a failed download into words a person can act on.
pub fn explain_fetch_error(err: &str) -> String {
    let e = err.to_ascii_lowercase();
    if e.contains("404") {
        "StowLock couldn't see its releases on GitHub. If the StowLock repository is private, sign in to GitHub (Settings) so updates can use your account.".into()
    } else if e.contains("403") || e.contains("429") || e.contains("rate limit") {
        "GitHub is limiting requests from your network right now. Try again in an hour.".into()
    } else if e.contains("resolve host") || e.contains("connect") || e.contains("timed out") {
        "Couldn't reach GitHub. Check your internet connection and try again.".into()
    } else if e.contains("curl is needed") {
        err.to_string()
    } else {
        format!(
            "Couldn't check for updates ({}).",
            err.lines().next().unwrap_or("").trim()
        )
    }
}

/// The releases list: anonymously first (works for anyone with a public repo),
/// then through the user's GitHub sign-in if GitHub refused (a private repo).
fn releases_json(repo: &str) -> Res<(String, bool)> {
    let url = format!("https://api.github.com/repos/{repo}/releases?per_page=30");
    match fetch(&url, None, 30) {
        Ok(j) => Ok((j, false)),
        Err(e) => {
            if proc::which("gh").is_some() {
                let o = proc::run(
                    "gh",
                    &["api", &format!("repos/{repo}/releases?per_page=30")],
                    &Opts::default_timeout().with_timeout(30),
                );
                if o.ok() {
                    return Ok((o.stdout, true));
                }
            }
            Err(explain_fetch_error(&e))
        }
    }
}

/// Check GitHub for this channel's newest release.
pub fn check(paths: &AppPaths, repo: &str, beta: bool) -> Res<CheckResult> {
    let _ = util::write_json(
        &last_check_file(paths),
        &LastCheck {
            at: chrono::Local::now().timestamp(),
        },
    );
    let (json, via_sign_in) = releases_json(repo)?;
    if serde_json::from_str::<serde_json::Value>(&json)
        .map(|v| !v.is_array())
        .unwrap_or(true)
    {
        return Err("GitHub sent an unexpected answer. Try again later.".into());
    }
    Ok(CheckResult {
        newer: pick_from_releases(&json, APP_VERSION, beta),
        newest_tag: newest_tag(&json, beta),
        via_sign_in,
        newer_beta: if beta {
            None
        } else {
            newer_beta(&json, APP_VERSION)
        },
    })
}

// ---- Going back to the previous version ----

fn previous_dir(paths: &AppPaths) -> PathBuf {
    paths.state_dir.join("previous")
}

/// The version kept from before the last update, if there is one.
pub fn previous_version(paths: &AppPaths) -> Option<String> {
    let d = previous_dir(paths);
    if !d.join("stowlock").is_file() {
        return None;
    }
    std::fs::read_to_string(d.join("version"))
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

/// Copy `exe` (this version) into the previous-version slot.
fn keep_previous(paths: &AppPaths, exe: &Path, version: &str) -> Res<()> {
    let d = previous_dir(paths);
    util::ensure_private_dir(&d)?;
    let tmp = d.join("stowlock.new");
    std::fs::copy(exe, &tmp).ctx("keeping the current version")?;
    std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755)).ok();
    std::fs::rename(&tmp, d.join("stowlock")).ctx("keeping the current version")?;
    util::atomic_write(&d.join("version"), version.as_bytes())
}

/// Put `new_bin` in place of `exe` atomically (Linux keeps the running file open).
fn swap_in(new_bin: &Path, exe: &Path) -> Res<()> {
    let target_dir = exe.parent().ok_or("no folder for the program")?;
    let staged = target_dir.join(format!(".stowlock.update-{}", std::process::id()));
    std::fs::copy(new_bin, &staged).ctx("staging the new version")?;
    std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o755)).ok();
    std::fs::rename(&staged, exe).ctx("swapping in the new version")
}

/// Go back to the kept version. The version being left is kept in its place,
/// so the user can go forward again. Returns (program path, version now installed).
pub fn rollback(paths: &AppPaths, exe: &Path, log: &Logger) -> Res<(PathBuf, String)> {
    let prev = previous_version(paths).ok_or("There's no previous version to go back to.")?;
    let exe = std::fs::canonicalize(exe).unwrap_or_else(|_| exe.to_path_buf());
    if let Some(pkg) = pacman_owner(&exe) {
        return Err(format!(
            "StowLock was installed from the AUR ({pkg}). Go back with your package manager, for example: sudo downgrade {pkg}"
        ));
    }
    let d = previous_dir(paths);
    let old_bin = d.join("stowlock.restore");
    std::fs::copy(d.join("stowlock"), &old_bin).ctx("reading the previous version")?;
    keep_previous(paths, &exe, APP_VERSION)?;
    swap_in(&old_bin, &exe)?;
    let _ = std::fs::remove_file(&old_bin);
    log.info("update", &format!("Went back to StowLock {prev}"));
    Ok((exe, prev))
}

/// Written by the crash handler: which version crashed.
pub fn crashed_version_file(paths: &AppPaths) -> PathBuf {
    paths.state_dir.join("crashed-version")
}

/// If this version crashed last time and an older one is kept, offer it.
/// Reading clears the note.
pub fn take_rollback_offer(paths: &AppPaths) -> Option<String> {
    let f = crashed_version_file(paths);
    let crashed = std::fs::read_to_string(&f).ok()?;
    let _ = std::fs::remove_file(&f);
    let prev = previous_version(paths)?;
    (crashed.trim() == APP_VERSION && prev != APP_VERSION).then_some(prev)
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

/// Check a download before installing it: the signature on SHA256SUMS must be
/// valid for one of `keys` and made for `tag`, and the download must match the
/// checksum SHA256SUMS lists for it.
pub fn verify_download(
    bin: &Path,
    sums: &[u8],
    sig: &str,
    keys: &[minisign_verify::PublicKey],
    tag: &str,
) -> Res<()> {
    crate::signing::verify(sums, sig, keys, tag)?;
    let text = String::from_utf8_lossy(sums);
    let want = crate::signing::checksum_for(&text, ASSET).ok_or_else(|| {
        "The release's checksum list doesn't include the app, so it wasn't installed.".to_string()
    })?;
    if sha256_hex(bin)? == want {
        Ok(())
    } else {
        Err("The download doesn't match its checksum, so it wasn't installed.".into())
    }
}

/// Download, verify and install `tag`. The version being replaced is kept so
/// the user can go back. Returns the path that was replaced.
pub fn apply(paths: &AppPaths, repo: &str, tag: &str, log: &Logger) -> Res<PathBuf> {
    let exe = std::env::current_exe().ctx("finding the running program")?;
    let exe = std::fs::canonicalize(&exe).unwrap_or(exe);
    if let Some(pkg) = pacman_owner(&exe) {
        return Err(format!(
            "StowLock was installed from the AUR ({pkg}). Update it with your AUR helper, for example: paru -Syu"
        ));
    }
    let dir = paths.update_dir();
    let _ = std::fs::remove_dir_all(&dir);
    util::ensure_private_dir(&dir)?;
    let base = format!("https://github.com/{repo}/releases/download/{tag}");
    let bin = dir.join(ASSET);
    let sums_file = dir.join(crate::signing::SUMS);
    let sig_file = dir.join(crate::signing::SUMS_SIG);
    let direct = fetch(&format!("{base}/{ASSET}"), Some(&bin), 300)
        .and_then(|_| {
            fetch(
                &format!("{base}/{}", crate::signing::SUMS),
                Some(&sums_file),
                60,
            )
        })
        .and_then(|_| {
            fetch(
                &format!("{base}/{}", crate::signing::SUMS_SIG),
                Some(&sig_file),
                60,
            )
        });
    if let Err(e) = direct {
        // A private repo: download through the user's GitHub sign-in instead.
        let d = dir.display().to_string();
        let o = proc::run(
            "gh",
            &[
                "release",
                "download",
                tag,
                "--repo",
                repo,
                "--pattern",
                ASSET,
                "--pattern",
                "SHA256SUMS*",
                "--dir",
                &d,
                "--clobber",
            ],
            &Opts::default_timeout().with_timeout(300),
        );
        if !o.ok() {
            return Err(format!("The download failed. {}", explain_fetch_error(&e)));
        }
    }
    let sums = std::fs::read(&sums_file)
        .map_err(|_| "The release has no checksum list, so it wasn't installed.".to_string())?;
    let sig = std::fs::read_to_string(&sig_file)
        .map_err(|_| "The release isn't signed, so it wasn't installed.".to_string())?;
    log.info("update", &format!("Downloaded StowLock {tag}"));
    verify_download(&bin, &sums, &sig, &crate::signing::trusted_keys(), tag)?;
    log.info("update", "Checksum and signature verified");
    std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755))
        .ctx("making it executable")?;
    keep_previous(paths, &exe, APP_VERSION)?;
    swap_in(&bin, &exe)?;
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
        assert!(is_newer("v0.2.0", "0.1.0", false));
        assert!(!is_newer("v0.1.0", "0.1.0", false));
        assert!(
            !is_newer("v0.0.9", "0.1.0", true),
            "never older, even on Beta"
        );
        assert!(
            !is_newer("v0.2.0-beta.1", "0.1.0", false),
            "Stable skips betas"
        );
        assert!(
            is_newer("v0.2.0-beta.1", "0.1.0", true),
            "Beta offers betas"
        );
        assert!(is_newer("v0.1.0-beta.3", "0.1.0-beta.2", true));
        assert!(
            is_newer("v0.1.0", "0.1.0-beta.3", false),
            "stable beats its betas"
        );
        assert!(!is_newer("junk", "0.1.0", true));
    }

    #[test]
    fn reads_githubs_public_release_list() {
        let json = r#"[
            {"tag_name":"v0.1.0-beta.5","prerelease":true,"draft":true,"body":"unfinished"},
            {"tag_name":"v0.1.0-beta.4","prerelease":true,"draft":false,"body":"Faster checks\r\n- one"},
            {"tag_name":"v0.1.0-beta.3","prerelease":true,"draft":false,"body":""}
        ]"#;
        let r = pick_from_releases(json, "0.1.0-beta.3", true).unwrap();
        assert_eq!(r.tag, "v0.1.0-beta.4", "drafts are never offered");
        assert!(r.prerelease);
        assert_eq!(r.notes, "Faster checks\n- one");
        assert_eq!(r.label(), "Beta 0.1.0-beta.4");
        assert_eq!(pick_from_releases(json, "0.1.0-beta.3", false), None);
        assert_eq!(pick_from_releases(json, "0.1.0-beta.4", true), None);
        assert_eq!(
            pick_from_releases("<html>rate limited</html>", "0.1.0", true),
            None
        );
    }

    #[test]
    fn releases_without_the_app_are_never_offered() {
        let json = r#"[
            {"tag_name":"v9.0.1","prerelease":false,"draft":false,"assets":[{"name":"stowlock-linux-x86_64.sha256"}]},
            {"tag_name":"v9.0.0","prerelease":false,"draft":false,"assets":[{"name":"stowlock-linux-x86_64"},{"name":"stowlock-linux-x86_64.sha256"},{"name":"SHA256SUMS"},{"name":"SHA256SUMS.minisig"}]}
        ]"#;
        assert_eq!(
            pick_from_releases(json, "0.1.0", false).unwrap().tag,
            "v9.0.0"
        );
    }

    #[test]
    fn unsigned_releases_are_not_available_yet() {
        let json = r#"[
            {"tag_name":"v9.0.1","prerelease":false,"draft":false,"assets":[{"name":"stowlock-linux-x86_64"},{"name":"stowlock-linux-x86_64.sha256"}]}
        ]"#;
        assert!(pick_from_releases(json, "0.1.0", false).is_none());
    }

    #[test]
    fn downloads_are_checked_against_signed_checksums() {
        use crate::signing::test_keys::TestKey;
        let d = tempfile::tempdir().unwrap();
        let bin = d.path().join(ASSET);
        std::fs::write(&bin, b"new stowlock").unwrap();
        let key = TestKey::new();
        let keys = [key.public()];
        let sums = format!("{}  {ASSET}\n", sha256_hex(&bin).unwrap());
        let sig = key.sign(sums.as_bytes(), "StowLock v9.0.0");
        assert!(verify_download(&bin, sums.as_bytes(), &sig, &keys, "v9.0.0").is_ok());
        // A swapped download fails the checksum, even with a good signature.
        std::fs::write(&bin, b"tampered").unwrap();
        let e = verify_download(&bin, sums.as_bytes(), &sig, &keys, "v9.0.0").unwrap_err();
        assert!(e.contains("doesn't match its checksum"), "{e}");
        // A checksum list edited to match the tampered file fails the signature.
        let forged = format!("{}  {ASSET}\n", sha256_hex(&bin).unwrap());
        assert!(verify_download(&bin, forged.as_bytes(), &sig, &keys, "v9.0.0").is_err());
        // The built-in keys don't trust a test key.
        assert!(verify_download(
            &bin,
            forged.as_bytes(),
            &key.sign(forged.as_bytes(), "StowLock v9.0.0"),
            &crate::signing::trusted_keys(),
            "v9.0.0"
        )
        .is_err());
    }

    #[test]
    fn stable_users_hear_about_newer_betas() {
        let json = r#"[{"tag_name":"v9.1.0-beta.1","prerelease":true,"draft":false}]"#;
        assert_eq!(newer_beta(json, "0.1.0").as_deref(), Some("v9.1.0-beta.1"));
        assert_eq!(newer_beta(json, "9.1.0"), None);
    }

    #[test]
    fn newest_on_github_per_channel() {
        let json = r#"[{"tag_name":"v0.1.0-beta.4","prerelease":true,"draft":false},
                       {"tag_name":"v0.0.9","prerelease":false,"draft":false},
                       {"tag_name":"v0.2.0","prerelease":false,"draft":true}]"#;
        assert_eq!(newest_tag(json, true).as_deref(), Some("v0.1.0-beta.4"));
        assert_eq!(newest_tag(json, false).as_deref(), Some("v0.0.9"));
    }

    #[test]
    fn errors_are_explained() {
        assert!(
            explain_fetch_error("curl: (22) The requested URL returned error: 404")
                .contains("private")
        );
        assert!(
            explain_fetch_error("curl: (22) The requested URL returned error: 403")
                .contains("limiting")
        );
        assert!(
            explain_fetch_error("curl: (6) Could not resolve host: api.github.com")
                .contains("internet")
        );
    }

    #[test]
    fn notes_are_bounded() {
        assert_eq!(tidy_notes("  Fixes\r\n- one  "), "Fixes\n- one");
        let long = "x".repeat(10_000);
        let t = tidy_notes(&long);
        assert!(t.chars().count() < 4100 && t.ends_with("the rest)"));
    }

    #[test]
    fn rollback_swaps_versions_both_ways() {
        let d = tempfile::tempdir().unwrap();
        let paths = AppPaths::under(&d.path().join("app"));
        paths.ensure().unwrap();
        let exe = d.path().join("bin").join("stowlock");
        std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
        std::fs::write(&exe, b"old build").unwrap();
        keep_previous(&paths, &exe, "0.0.1").unwrap();
        std::fs::write(&exe, b"new build").unwrap();
        assert_eq!(previous_version(&paths).as_deref(), Some("0.0.1"));
        let log = Logger::new(None);
        let (_, now) = rollback(&paths, &exe, &log).unwrap();
        assert_eq!(now, "0.0.1");
        assert_eq!(std::fs::read(&exe).unwrap(), b"old build");
        // The version we left is kept, so going forward again works.
        assert_eq!(previous_version(&paths).as_deref(), Some(APP_VERSION));
        assert_eq!(
            std::fs::read(paths.state_dir.join("previous/stowlock")).unwrap(),
            b"new build"
        );
    }

    #[test]
    fn crash_offer_only_for_this_version() {
        let d = tempfile::tempdir().unwrap();
        let paths = AppPaths::under(d.path());
        paths.ensure().unwrap();
        let exe = d.path().join("stowlock");
        std::fs::write(&exe, b"x").unwrap();
        keep_previous(&paths, &exe, "0.0.1").unwrap();
        std::fs::write(crashed_version_file(&paths), "9.9.9").unwrap();
        assert_eq!(take_rollback_offer(&paths), None, "another version crashed");
        std::fs::write(crashed_version_file(&paths), APP_VERSION).unwrap();
        assert_eq!(take_rollback_offer(&paths).as_deref(), Some("0.0.1"));
        assert_eq!(take_rollback_offer(&paths), None, "offered once");
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
