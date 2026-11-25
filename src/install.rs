// SPDX-License-Identifier: GPL-3.0-or-later
//! First install and desktop integration: binary in `~/.local/bin`, launcher
//! entry with "Open with StowLock" for zips, and the icon.

use crate::proc::{self, Opts};
use crate::util::{self, Ctx, Res};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

pub const ICON_SVG: &[u8] = include_bytes!("../assets/stowlock.svg");

#[derive(Debug, Clone)]
pub struct InstallPaths {
    pub bin: PathBuf,
    pub desktop: PathBuf,
    pub icon: PathBuf,
}

impl InstallPaths {
    pub fn detect() -> Self {
        Self::under(&util::home_dir())
    }
    pub fn under(home: &Path) -> Self {
        InstallPaths {
            bin: home.join(".local/bin/stowlock"),
            desktop: home.join(".local/share/applications/stowlock.desktop"),
            icon: home.join(".local/share/icons/hicolor/scalable/apps/stowlock.svg"),
        }
    }
}

pub fn desktop_entry(bin: &Path) -> String {
    format!(
        "[Desktop Entry]\nType=Application\nName=StowLock\nGenericName=Project Zip Organizer\nComment=Drop in a project zip, get a clean, tested GitHub repo\nExec={} %f\nIcon=stowlock\nTerminal=false\nCategories=Development;\nMimeType=application/zip;\nKeywords=github;git;zip;archive;\nStartupWMClass=stowlock\n",
        bin.display()
    )
}

/// Is the running program the installed copy?
pub fn is_installed(inst: &InstallPaths) -> bool {
    let exe = std::env::current_exe()
        .ok()
        .and_then(|p| std::fs::canonicalize(p).ok());
    let bin = std::fs::canonicalize(&inst.bin).ok();
    exe.is_some() && exe == bin
}

pub fn bin_dir_on_path(inst: &InstallPaths) -> bool {
    let dir = inst
        .bin
        .parent()
        .map(|p| p.display().to_string())
        .unwrap_or_default();
    std::env::var("PATH")
        .map(|p| p.split(':').any(|s| s == dir))
        .unwrap_or(false)
}

/// Ask a StowLock program file for its version (`--version`). Only ever run
/// on StowLock's own installed or running program.
pub fn program_version(path: &Path) -> Option<String> {
    if !path.exists() {
        return None;
    }
    let o = proc::run(
        &path.display().to_string(),
        &["--version"],
        &Opts::default_timeout().with_timeout(5),
    );
    o.stdout
        .lines()
        .find_map(|l| l.trim().strip_prefix("StowLock "))
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

/// Other StowLock downloads in the same folder as `exe`, e.g.
/// `stowlock-linux-x86_64(1)`, so the user knows which one they're installing.
pub fn other_downloads(exe: &Path) -> Vec<(String, String)> {
    let (Some(dir), Some(me)) = (exe.parent(), exe.file_name()) else {
        return Vec::new();
    };
    let Ok(rd) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<(String, String)> = rd
        .flatten()
        .filter(|e| e.file_name() != me)
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            if !name.starts_with("stowlock-linux-x86_64") || name.ends_with(".sha256") {
                return None;
            }
            let when = e
                .metadata()
                .and_then(|m| m.modified())
                .map(|t| {
                    chrono::DateTime::<chrono::Local>::from(t)
                        .format("%b %-d, %H:%M")
                        .to_string()
                })
                .unwrap_or_default();
            Some((name, when))
        })
        .collect();
    out.sort();
    out
}

/// A downloaded copy whose name shows the browser renamed it, e.g. "(1)".
pub fn looks_like_duplicate(name: &str) -> bool {
    let n = name.trim_end_matches(".AppImage");
    n.ends_with(')') && n.contains('(')
        || n.rsplit_once('.')
            .is_some_and(|(_, tail)| tail.chars().all(|c| c.is_ascii_digit()))
}

/// The command that puts ~/.local/bin on PATH for the user's shell.
pub fn path_fix_command(shell: &str) -> &'static str {
    let name = shell.rsplit('/').next().unwrap_or("");
    match name {
        "fish" => "fish_add_path ~/.local/bin",
        "zsh" => {
            "echo 'export PATH=\"$HOME/.local/bin:$PATH\"' >> ~/.zshrc  (then open a new terminal)"
        }
        _ => {
            "echo 'export PATH=\"$HOME/.local/bin:$PATH\"' >> ~/.bashrc  (then open a new terminal)"
        }
    }
}

#[derive(Debug, Default)]
pub struct InstallReport {
    pub lines: Vec<String>,
    pub warnings: Vec<String>,
}

/// Write the launcher entry and icon (used by install and on first run).
pub fn write_desktop_files(inst: &InstallPaths) -> Res<()> {
    std::fs::create_dir_all(inst.desktop.parent().unwrap()).ctx("creating applications folder")?;
    std::fs::create_dir_all(inst.icon.parent().unwrap()).ctx("creating icons folder")?;
    util::atomic_write(&inst.desktop, desktop_entry(&inst.bin).as_bytes())?;
    std::fs::set_permissions(&inst.desktop, std::fs::Permissions::from_mode(0o644)).ok();
    util::atomic_write(&inst.icon, ICON_SVG)?;
    std::fs::set_permissions(&inst.icon, std::fs::Permissions::from_mode(0o644)).ok();
    if proc::which("update-desktop-database").is_some() {
        let dir = inst.desktop.parent().unwrap().display().to_string();
        let _ = proc::run(
            "update-desktop-database",
            &[&dir],
            &Opts::default_timeout().with_timeout(20),
        );
    }
    Ok(())
}

/// Copy the running binary to `~/.local/bin/stowlock` (atomically) and add
/// the launcher entry and icon.
pub fn install(inst: &InstallPaths) -> Res<InstallReport> {
    let mut r = InstallReport::default();
    let exe = std::env::current_exe().ctx("finding the running program")?;
    let here = crate::config::APP_VERSION;
    let name = exe
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    if looks_like_duplicate(&name) {
        r.warnings.push(format!(
            "You're installing from \"{name}\". Your browser renamed it because another download had the same name. This file is StowLock {here}."
        ));
    }
    for (other, when) in other_downloads(&exe) {
        r.warnings.push(format!(
            "There's another StowLock download next to it: \"{other}\" ({when}). Delete the one you don't need so the wrong one isn't installed later."
        ));
    }
    let bin_dir = inst.bin.parent().unwrap();
    std::fs::create_dir_all(bin_dir).ctx("creating ~/.local/bin")?;
    if !is_installed(inst) {
        let replaced = program_version(&inst.bin);
        let tmp = bin_dir.join(format!(".stowlock.new-{}", std::process::id()));
        std::fs::copy(&exe, &tmp).ctx("copying StowLock")?;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755))
            .ctx("making it executable")?;
        std::fs::rename(&tmp, &inst.bin).ctx("installing StowLock")?;
        r.lines.push(match replaced {
            Some(old) if old != here => format!(
                "Installed StowLock {here} to {} (replaced {old})",
                util::tilde(&inst.bin)
            ),
            Some(_) => format!("Reinstalled StowLock {here} to {}", util::tilde(&inst.bin)),
            None => format!("Installed StowLock {here} to {}", util::tilde(&inst.bin)),
        });
    } else {
        r.lines
            .push(format!("StowLock {here} is already installed"));
    }
    write_desktop_files(inst)?;
    r.lines.push(format!(
        "Added the launcher entry {}",
        util::tilde(&inst.desktop)
    ));
    r.lines
        .push(format!("Added the icon {}", util::tilde(&inst.icon)));
    if !bin_dir_on_path(inst) {
        r.warnings.push(format!(
            "{} isn't on your PATH, so typing `stowlock` in a terminal won't work yet. The app menu entry works either way. To fix it, run: {}",
            util::tilde(bin_dir),
            path_fix_command(&std::env::var("SHELL").unwrap_or_default())
        ));
    }
    Ok(r)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_fix_per_shell() {
        assert_eq!(
            path_fix_command("/usr/bin/fish"),
            "fish_add_path ~/.local/bin"
        );
        assert!(path_fix_command("/bin/zsh").contains(".zshrc"));
        assert!(path_fix_command("/bin/bash").contains(".bashrc"));
        assert!(path_fix_command("").contains(".bashrc"));
    }

    #[test]
    fn spots_renamed_downloads() {
        assert!(looks_like_duplicate("stowlock-linux-x86_64(1)"));
        assert!(looks_like_duplicate("stowlock-linux-x86_64 (2)"));
        assert!(looks_like_duplicate("stowlock-linux-x86_64.1"));
        assert!(!looks_like_duplicate("stowlock-linux-x86_64"));
        assert!(!looks_like_duplicate("stowlock"));
    }

    #[test]
    fn lists_other_downloads_but_not_checksums() {
        let d = tempfile::tempdir().unwrap();
        for n in [
            "stowlock-linux-x86_64",
            "stowlock-linux-x86_64(1)",
            "stowlock-linux-x86_64.sha256",
            "notes.txt",
        ] {
            std::fs::write(d.path().join(n), b"x").unwrap();
        }
        let others = other_downloads(&d.path().join("stowlock-linux-x86_64(1)"));
        let names: Vec<_> = others.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, ["stowlock-linux-x86_64"]);
    }

    #[test]
    fn reads_a_programs_version() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("fake-stowlock");
        std::fs::write(&f, "#!/bin/sh\necho 'StowLock 9.8.7-beta.1'\n").unwrap();
        std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(program_version(&f).as_deref(), Some("9.8.7-beta.1"));
        assert_eq!(program_version(&d.path().join("missing")), None);
    }

    #[test]
    fn desktop_files_written() {
        let d = tempfile::tempdir().unwrap();
        let inst = InstallPaths::under(d.path());
        write_desktop_files(&inst).unwrap();
        let text = std::fs::read_to_string(&inst.desktop).unwrap();
        assert!(text.contains("MimeType=application/zip;"));
        assert!(text.contains(&format!("Exec={} %f", inst.bin.display())));
        assert!(std::fs::read(&inst.icon).unwrap().starts_with(b"<svg"));
    }
}
