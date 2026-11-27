// SPDX-License-Identifier: GPL-3.0-or-later
//! Is this computer's Rust up to date? Projects follow the newest stable Rust
//! (rust-toolchain.toml: channel = "stable"), and so does GitHub, so an older
//! Rust here can miss warnings GitHub would catch. StowLock checks once a day
//! (in the background) and offers to run `rustup update`.

use crate::config::AppPaths;
use crate::proc::{self, Opts};
use crate::util::{self, Res};
use serde::{Deserialize, Serialize};

const CHECK_EVERY_SECS: i64 = 24 * 60 * 60;

#[derive(Debug, Clone, PartialEq)]
pub enum RustState {
    /// rustup isn't installed (`system` is Arch's own Rust, if that's there).
    NoRustup {
        system: Option<String>,
    },
    /// rustup is installed but has no Rust version chosen yet.
    NoDefault,
    UpToDate {
        version: String,
    },
    UpdateAvailable {
        have: String,
        newest: String,
    },
    /// Couldn't tell (offline, say).
    Unknown(String),
}

impl RustState {
    /// One line for Settings.
    pub fn line(&self) -> String {
        match self {
            RustState::NoRustup { system: Some(v) } => format!(
                "Rust {v} from Arch's rust package (projects' Rust versions aren't followed; rustup is recommended)"
            ),
            RustState::NoRustup { system: None } => "Rust isn't installed".into(),
            RustState::NoDefault => "rustup has no Rust version yet".into(),
            RustState::UpToDate { version } => format!("Rust {version} · up to date"),
            RustState::UpdateAvailable { have, newest } => {
                format!("Rust {newest} is available (you have {have})")
            }
            RustState::Unknown(why) => format!("Couldn't check Rust: {why}"),
        }
    }
}

#[derive(Default, Serialize, Deserialize)]
struct LastCheck {
    at: i64,
}

fn last_check_file(paths: &AppPaths) -> std::path::PathBuf {
    paths.state_dir.join("rust-check.json")
}

/// Time for the daily check?
pub fn check_due(paths: &AppPaths) -> bool {
    let lc: LastCheck = util::read_json_or_default(&last_check_file(paths));
    chrono::Local::now().timestamp() - lc.at >= CHECK_EVERY_SECS
}

fn first_version(text: &str) -> Option<String> {
    text.split_whitespace()
        .find(|w| w.chars().next().is_some_and(|c| c.is_ascii_digit()) && w.contains('.'))
        .map(|w| w.to_string())
}

/// Read `rustup check` output for the stable toolchain:
///   stable-x86_64-unknown-linux-gnu - Update available : 1.100.0 (… 2026-11-09) -> 1.101.0 (… 2026-12-18)
///   stable-x86_64-unknown-linux-gnu - Up to date : 1.100.0 (… 2026-11-09)
pub fn parse_check(output: &str) -> Option<RustState> {
    let line = output
        .lines()
        .find(|l| l.trim_start().starts_with("stable-"))?;
    let (_, rest) = line.split_once(" - ")?;
    if rest.starts_with("Update available") {
        let (_, vers) = rest.split_once(':')?;
        let (have, newest) = vers.split_once("->")?;
        Some(RustState::UpdateAvailable {
            have: first_version(have)?,
            newest: first_version(newest)?,
        })
    } else if rest.starts_with("Up to date") {
        let (_, v) = rest.split_once(':')?;
        Some(RustState::UpToDate {
            version: first_version(v)?,
        })
    } else {
        None
    }
}

/// What this computer's Rust is like now (asks rustup; needs the network).
pub fn check(paths: &AppPaths) -> RustState {
    let _ = util::write_json(
        &last_check_file(paths),
        &LastCheck {
            at: chrono::Local::now().timestamp(),
        },
    );
    let opts = Opts::default_timeout().with_timeout(60);
    if proc::which("rustup").is_none() {
        let o = proc::run("rustc", &["--version"], &opts);
        return RustState::NoRustup {
            system: o.ok().then(|| first_version(&o.stdout)).flatten(),
        };
    }
    let v = proc::run("rustc", &["--version"], &opts);
    let text = v.combined();
    if text.contains("could not choose a version") || text.contains("no default") {
        return RustState::NoDefault;
    }
    let o = proc::run("rustup", &["check"], &opts);
    match parse_check(&o.combined()) {
        Some(s) => s,
        None if o.ok() => RustState::Unknown("rustup's answer had no stable Rust in it".into()),
        None => RustState::Unknown(
            o.combined()
                .lines()
                .find(|l| !l.trim().is_empty())
                .unwrap_or("rustup couldn't reach the internet")
                .trim()
                .to_string(),
        ),
    }
}

/// `rustup update stable` (or set stable as the default when there's none).
/// Updating replaces the old stable in place, so no old copy is left behind.
pub fn update(state: &RustState) -> Res<String> {
    let opts = Opts::default_timeout().with_timeout(900);
    let args: &[&str] = if *state == RustState::NoDefault {
        &["default", "stable"]
    } else {
        &["update", "stable", "--no-self-update"]
    };
    let o = proc::run("rustup", args, &opts);
    if !o.ok() {
        return Err(format!(
            "rustup couldn't update Rust: {}",
            o.combined().lines().last().unwrap_or("").trim()
        ));
    }
    let v = proc::run("rustc", &["--version"], &Opts::default_timeout());
    Ok(first_version(&v.stdout).unwrap_or_else(|| "the newest version".into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_rustups_answer() {
        let out = "stable-x86_64-unknown-linux-gnu - Update available : 1.100.0 (0ef1bc386 2026-11-09) -> 1.101.0 (a1b2c3d4e 2026-12-18)\nrustup - Up to date : 1.29.3\n";
        assert_eq!(
            parse_check(out),
            Some(RustState::UpdateAvailable {
                have: "1.100.0".into(),
                newest: "1.101.0".into()
            })
        );
        let out = "stable-x86_64-unknown-linux-gnu - Up to date : 1.101.0 (a1b2c3d4e 2026-12-18)\n";
        assert_eq!(
            parse_check(out),
            Some(RustState::UpToDate {
                version: "1.101.0".into()
            })
        );
        assert_eq!(parse_check("error: no toolchains installed"), None);
    }

    #[test]
    fn settings_lines_say_what_to_do() {
        assert!(RustState::NoDefault.line().contains("no Rust version"));
        assert_eq!(
            RustState::UpToDate {
                version: "1.101.0".into()
            }
            .line(),
            "Rust 1.101.0 · up to date"
        );
    }
}
