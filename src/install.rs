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
    let bin_dir = inst.bin.parent().unwrap();
    std::fs::create_dir_all(bin_dir).ctx("creating ~/.local/bin")?;
    if !is_installed(inst) {
        let tmp = bin_dir.join(format!(".stowlock.new-{}", std::process::id()));
        std::fs::copy(&exe, &tmp).ctx("copying StowLock")?;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755))
            .ctx("making it executable")?;
        std::fs::rename(&tmp, &inst.bin).ctx("installing StowLock")?;
        r.lines.push(format!(
            "Installed the program to {}",
            util::tilde(&inst.bin)
        ));
    } else {
        r.lines.push("Program already installed".into());
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
            "{} isn't on your PATH, so typing `stowlock` in a terminal won't work yet. The app menu entry works either way.",
            util::tilde(bin_dir)
        ));
    }
    Ok(r)
}

#[cfg(test)]
mod tests {
    use super::*;

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
