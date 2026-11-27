// SPDX-License-Identifier: GPL-3.0-or-later
//! The tools the family's checks use, whether they're installed, and
//! installing missing ones with pacman through the desktop's own password
//! prompt (polkit), only when you click Install.

use crate::logbook::Logger;
use crate::proc::{self, Opts};
use crate::util::Res;

#[derive(Debug, Clone, PartialEq)]
pub struct ToolInfo {
    pub name: &'static str,
    /// The Arch package that provides it ("" when the fix isn't a package).
    pub package: &'static str,
    pub why: &'static str,
    /// The installed version, or None when it's missing.
    pub version: Option<String>,
    /// A fix that isn't a package install (rustup without a Rust chosen).
    pub other_fix: Option<&'static str>,
}

fn version_of(program: &str, args: &[&str]) -> Option<String> {
    proc::which(program)?;
    let o = proc::run(program, args, &Opts::default_timeout().with_timeout(15));
    if !o.ok() {
        return None;
    }
    let text = o.combined();
    let re = regex::Regex::new(r"\b(\d+\.\d+(?:\.\d+)?)\b").unwrap();
    re.captures(&text).map(|c| c[1].to_string())
}

/// Check every tool (runs programs: call it off the UI thread).
pub fn survey() -> Vec<ToolInfo> {
    let mut out = Vec::new();
    // Rust: through rustup, so each project's Rust version is followed.
    let rustup = proc::which("rustup").is_some();
    let rustc = version_of("rustc", &["--version"]);
    out.push(ToolInfo {
        name: "Rust (rustup)",
        package: "rustup",
        why: "builds and checks Rust projects",
        version: if rustup { rustc.clone() } else { None },
        other_fix: (rustup && rustc.is_none()).then_some("rustup default stable"),
    });
    for (name, program, args, package, why) in [
        (
            "shellcheck",
            "shellcheck",
            &["--version"][..],
            "shellcheck",
            "checks shell scripts",
        ),
        (
            "cargo-audit",
            "cargo",
            &["audit", "--version"][..],
            "cargo-audit",
            "checks Rust dependencies for known security problems",
        ),
        (
            "cargo-deny",
            "cargo",
            &["deny", "--version"][..],
            "cargo-deny",
            "checks Rust dependencies' licences and sources",
        ),
        (
            "minisign",
            "minisign",
            &["-v"][..],
            "minisign",
            "the signing dry run",
        ),
        ("git", "git", &["--version"][..], "git", "everything"),
        (
            "gh",
            "gh",
            &["--version"][..],
            "github-cli",
            "GitHub sign-in, repos and releases",
        ),
    ] {
        out.push(ToolInfo {
            name,
            package,
            why,
            version: version_of(program, args),
            other_fix: None,
        });
    }
    out
}

/// The packages to install for everything that's missing.
pub fn missing_packages(tools: &[ToolInfo]) -> Vec<String> {
    tools
        .iter()
        .filter(|t| t.version.is_none() && t.other_fix.is_none() && !t.package.is_empty())
        .map(|t| t.package.to_string())
        .collect()
}

/// The command to install them by hand (shown when there's no password prompt).
pub fn pacman_command(packages: &[String]) -> String {
    format!("sudo pacman -S --needed {}", packages.join(" "))
}

fn valid_package(p: &str) -> bool {
    !p.is_empty()
        && !p.starts_with('-')
        && p.chars()
            .all(|c| c.is_ascii_alphanumeric() || "@._+-".contains(c))
}

/// Install `packages` with pacman, asking for your password through the
/// desktop's prompt (pkexec). Nothing is installed without that approval.
pub fn install(packages: &[String], log: &Logger) -> Res<String> {
    if packages.is_empty() {
        return Ok("Nothing to install".into());
    }
    if let Some(bad) = packages.iter().find(|p| !valid_package(p)) {
        return Err(format!(
            "\"{bad}\" isn't a package name, so nothing was installed."
        ));
    }
    if proc::which("pkexec").is_none() {
        return Err(format!(
            "This computer has no password prompt for apps (polkit). Install them in a terminal instead:\n{}",
            pacman_command(packages)
        ));
    }
    let mut args = vec!["pacman", "-S", "--needed", "--noconfirm"];
    args.extend(packages.iter().map(String::as_str));
    log.info(
        "tools",
        &format!("Installing with pacman: {}", packages.join(" ")),
    );
    let o = proc::run("pkexec", &args, &Opts::default_timeout().with_timeout(900));
    log.command("tools", &o);
    match o.code {
        Some(0) => Ok(format!("Installed {}", packages.join(", "))),
        // pkexec: 126 = you closed the password prompt, 127 = not allowed.
        Some(126) | Some(127) => Err("The install was cancelled at the password prompt.".into()),
        _ => Err(format!(
            "pacman couldn't install {}: {}",
            packages.join(", "),
            o.combined().lines().last().unwrap_or("").trim()
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool(name: &'static str, package: &'static str, version: Option<&str>) -> ToolInfo {
        ToolInfo {
            name,
            package,
            why: "",
            version: version.map(String::from),
            other_fix: None,
        }
    }

    #[test]
    fn lists_only_what_is_missing() {
        let tools = vec![
            tool("shellcheck", "shellcheck", Some("0.11.0")),
            tool("cargo-audit", "cargo-audit", None),
            tool("cargo-deny", "cargo-deny", None),
        ];
        assert_eq!(missing_packages(&tools), ["cargo-audit", "cargo-deny"]);
        assert_eq!(
            pacman_command(&missing_packages(&tools)),
            "sudo pacman -S --needed cargo-audit cargo-deny"
        );
        // rustup without a Rust chosen is fixed with a command, not a package.
        let mut r = tool("Rust (rustup)", "rustup", None);
        r.other_fix = Some("rustup default stable");
        assert!(missing_packages(&[r]).is_empty());
    }

    #[test]
    fn refuses_anything_that_isnt_a_package_name() {
        let log = Logger::new(None);
        for bad in ["--overwrite=*", "a;rm -rf /", "x y", ""] {
            assert!(install(&[bad.to_string()], &log).is_err(), "{bad}");
        }
        assert!(valid_package("python-pipx") && valid_package("lib32-glibc"));
    }

    #[test]
    fn survey_finds_the_tools_this_computer_has() {
        let tools = survey();
        let git = tools.iter().find(|t| t.name == "git").unwrap();
        assert!(git.version.is_some(), "git is installed wherever tests run");
    }
}
