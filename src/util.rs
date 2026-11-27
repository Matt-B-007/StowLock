// SPDX-License-Identifier: GPL-3.0-or-later
//! Small shared helpers: result type, private file writes, trash, sizes.

use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

pub type Res<T> = Result<T, String>;

/// Turn any displayable error into our string error with context.
pub trait Ctx<T> {
    fn ctx(self, what: &str) -> Res<T>;
}

impl<T, E: std::fmt::Display> Ctx<T> for Result<T, E> {
    fn ctx(self, what: &str) -> Res<T> {
        self.map_err(|e| format!("{what}: {e}"))
    }
}

/// Create a directory (and parents) readable only by the current user (0700).
pub fn ensure_private_dir(dir: &Path) -> Res<()> {
    fs::create_dir_all(dir).ctx(&format!("creating {}", dir.display()))?;
    fs::set_permissions(dir, fs::Permissions::from_mode(0o700))
        .ctx(&format!("securing {}", dir.display()))
}

/// Write a file atomically (temp file + rename) with permissions 0600.
pub fn atomic_write(path: &Path, bytes: &[u8]) -> Res<()> {
    let dir = path.parent().ok_or("file has no parent folder")?;
    fs::create_dir_all(dir).ctx("creating folder")?;
    let tmp = dir.join(format!(
        ".{}.tmp-{}",
        path.file_name().and_then(|n| n.to_str()).unwrap_or("file"),
        std::process::id()
    ));
    {
        let mut f = fs::File::create(&tmp).ctx("creating temp file")?;
        f.set_permissions(fs::Permissions::from_mode(0o600))
            .ctx("securing temp file")?;
        f.write_all(bytes).ctx("writing temp file")?;
        f.sync_all().ok();
    }
    fs::rename(&tmp, path).ctx(&format!("saving {}", path.display()))
}

/// Read a JSON file; a corrupt file is moved aside and the default returned.
pub fn read_json_or_default<T: serde::de::DeserializeOwned + Default>(path: &Path) -> T {
    match fs::read_to_string(path) {
        Ok(text) => match serde_json::from_str(&text) {
            Ok(v) => v,
            Err(_) => {
                let aside = path.with_extension(format!("corrupt-{}", now_stamp()));
                let _ = fs::rename(path, aside);
                T::default()
            }
        },
        Err(_) => T::default(),
    }
}

pub fn write_json<T: serde::Serialize>(path: &Path, value: &T) -> Res<()> {
    let text = serde_json::to_string_pretty(value).ctx("encoding JSON")?;
    atomic_write(path, text.as_bytes())
}

/// Local timestamp used in file names: 2026-11-23-0642.
pub fn now_stamp() -> String {
    chrono::Local::now().format("%Y-%m-%d-%H%M").to_string()
}

/// Local timestamp with seconds, for unique report names.
pub fn now_stamp_secs() -> String {
    chrono::Local::now().format("%Y-%m-%d-%H%M%S").to_string()
}

pub fn now_iso() -> String {
    chrono::Local::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, false)
}

pub fn today() -> String {
    chrono::Local::now().format("%Y-%m-%d").to_string()
}

pub fn home_dir() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from("/tmp"))
}

/// Expand a leading `~` to the home folder.
pub fn expand_tilde(p: &str) -> PathBuf {
    if p == "~" {
        home_dir()
    } else if let Some(rest) = p.strip_prefix("~/") {
        home_dir().join(rest)
    } else {
        PathBuf::from(p)
    }
}

/// Show a path with the home folder replaced by `~`.
pub fn tilde(p: &Path) -> String {
    let home = home_dir();
    match p.strip_prefix(&home) {
        Ok(rest) if rest.as_os_str().is_empty() => "~".into(),
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => p.display().to_string(),
    }
}

pub fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = bytes as f64;
    let mut i = 0;
    while v >= 1024.0 && i < UNITS.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{bytes} B")
    } else {
        format!("{v:.1} {}", UNITS[i])
    }
}

/// Total size and file count of a folder (follows no symlinks).
pub fn dir_stats(dir: &Path) -> (u64, u64) {
    let mut size = 0;
    let mut count = 0;
    for e in walkdir::WalkDir::new(dir)
        .follow_links(false)
        .into_iter()
        .flatten()
    {
        if e.file_type().is_file() {
            count += 1;
            size += e.metadata().map(|m| m.len()).unwrap_or(0);
        }
    }
    (size, count)
}

/// Move a path to the desktop Trash. Tries `gio trash`, then the freedesktop
/// Trash folder directly. Returns where it went.
pub fn move_to_trash(path: &Path) -> Res<String> {
    if crate::proc::which("gio").is_some() {
        let out = std::process::Command::new("gio")
            .arg("trash")
            .arg(path)
            .output();
        if let Ok(o) = out {
            if o.status.success() {
                return Ok("Trash".into());
            }
        }
    }
    let trash = home_dir().join(".local/share/Trash");
    let files = trash.join("files");
    let info = trash.join("info");
    fs::create_dir_all(&files).ctx("creating Trash folder")?;
    fs::create_dir_all(&info).ctx("creating Trash folder")?;
    let base = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("item")
        .to_string();
    let mut name = base.clone();
    let mut n = 1;
    while files.join(&name).exists() || info.join(format!("{name}.trashinfo")).exists() {
        n += 1;
        name = format!("{base}.{n}");
    }
    let abs = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let date = chrono::Local::now().format("%Y-%m-%dT%H:%M:%S");
    let info_text = format!(
        "[Trash Info]\nPath={}\nDeletionDate={date}\n",
        abs.display()
    );
    fs::write(info.join(format!("{name}.trashinfo")), info_text).ctx("writing Trash info")?;
    fs::rename(path, files.join(&name)).map_err(|e| {
        let _ = fs::remove_file(info.join(format!("{name}.trashinfo")));
        format!(
            "couldn't move {} to the Trash (it may be on another drive): {e}",
            tilde(path)
        )
    })?;
    Ok("Trash".into())
}

/// Copy a file, falling back gracefully; used when rename crosses drives.
pub fn move_file(from: &Path, to: &Path) -> Res<()> {
    if fs::rename(from, to).is_ok() {
        return Ok(());
    }
    fs::copy(from, to).ctx("copying file")?;
    fs::remove_file(from).ctx("removing original file")
}

/// "release created" -> "Release created".
pub fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

/// Remove terminal colour codes ("\x1b[1;31m") from program output.
pub fn strip_ansi(text: &str) -> String {
    static R: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    R.get_or_init(|| regex::Regex::new(r"\x1b\[[0-9;?]*[A-Za-z]|\x1b\][^\x07]*\x07").unwrap())
        .replace_all(text, "")
        .into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_are_human() {
        assert_eq!(human_size(512), "512 B");
        assert_eq!(human_size(2048), "2.0 KB");
        assert_eq!(human_size(5 * 1024 * 1024), "5.0 MB");
    }

    #[test]
    fn atomic_write_is_private() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("x.json");
        atomic_write(&p, b"{}").unwrap();
        let mode = fs::metadata(&p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }

    #[test]
    fn corrupt_json_recovers() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("s.json");
        fs::write(&p, "{not json").unwrap();
        let v: std::collections::HashMap<String, String> = read_json_or_default(&p);
        assert!(v.is_empty());
        assert!(!p.exists());
    }
}
