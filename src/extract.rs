// SPDX-License-Identifier: GPL-3.0-or-later
//! Safe zip extraction. Every zip is treated as untrusted: no paths outside
//! the destination, no symlinks, size and entry limits (zip bomb guard),
//! executable bits restored, and a single wrapper folder unwrapped.

use crate::util::{Ctx, Res};
use std::fs;
use std::io::{Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

pub const MAX_TOTAL_BYTES: u64 = 2 * 1024 * 1024 * 1024;
pub const MAX_ENTRIES: usize = 50_000;

#[derive(Debug, Default, Clone)]
pub struct ExtractReport {
    pub files: usize,
    pub bytes: u64,
    pub skipped_unsafe: Vec<String>,
    pub skipped_symlinks: Vec<String>,
    pub unwrapped: Option<String>,
}

pub struct Limits {
    pub max_bytes: u64,
    pub max_entries: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            max_bytes: MAX_TOTAL_BYTES,
            max_entries: MAX_ENTRIES,
        }
    }
}

pub fn extract_zip(zip_path: &Path, dest: &Path) -> Res<ExtractReport> {
    extract_zip_with(zip_path, dest, &Limits::default())
}

pub fn extract_zip_with(zip_path: &Path, dest: &Path, limits: &Limits) -> Res<ExtractReport> {
    let file = fs::File::open(zip_path).ctx("opening the zip")?;
    let mut archive =
        zip::ZipArchive::new(file).map_err(|e| format!("This file isn't a valid zip ({e})."))?;
    if archive.len() > limits.max_entries {
        return Err(format!(
            "This zip has {} entries, more than the safety limit of {}. It wasn't extracted.",
            archive.len(),
            limits.max_entries
        ));
    }
    // Check the declared sizes first.
    let mut declared: u64 = 0;
    for i in 0..archive.len() {
        if let Ok(e) = archive.by_index_raw(i) {
            declared = declared.saturating_add(e.size());
        }
    }
    if declared > limits.max_bytes {
        return Err(format!(
            "This zip would expand to {}, more than the safety limit of {}. It wasn't extracted.",
            crate::util::human_size(declared),
            crate::util::human_size(limits.max_bytes)
        ));
    }
    fs::create_dir_all(dest).ctx("creating the extraction folder")?;
    let mut report = ExtractReport::default();
    let mut written: u64 = 0;
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).ctx("reading the zip")?;
        let raw_name = entry.name().to_string();
        let rel = match entry.enclosed_name() {
            Some(p) => p,
            None => {
                report.skipped_unsafe.push(raw_name);
                continue;
            }
        };
        if raw_name.starts_with('/') {
            report.skipped_unsafe.push(raw_name);
            continue;
        }
        if rel
            .components()
            .next()
            .map(|c| c.as_os_str() == "__MACOSX")
            .unwrap_or(false)
        {
            continue;
        }
        let mode = entry.unix_mode();
        if let Some(m) = mode {
            if m & 0o170000 == 0o120000 {
                report.skipped_symlinks.push(raw_name);
                continue;
            }
        }
        let out_path = dest.join(&rel);
        if !out_path.starts_with(dest) {
            report.skipped_unsafe.push(raw_name);
            continue;
        }
        if entry.is_dir() {
            fs::create_dir_all(&out_path).ctx("creating folder")?;
            continue;
        }
        if let Some(parent) = out_path.parent() {
            fs::create_dir_all(parent).ctx("creating folder")?;
        }
        let mut out = fs::File::create(&out_path).ctx(&format!("writing {}", rel.display()))?;
        let remaining = limits.max_bytes.saturating_sub(written);
        let mut limited = (&mut entry).take(remaining + 1);
        let mut buf = [0u8; 64 * 1024];
        let mut first = [0u8; 2];
        let mut first_len = 0;
        loop {
            let n = limited.read(&mut buf).ctx("reading the zip")?;
            if n == 0 {
                break;
            }
            if first_len < 2 {
                let take = (2 - first_len).min(n);
                first[first_len..first_len + take].copy_from_slice(&buf[..take]);
                first_len += take;
            }
            written += n as u64;
            if written > limits.max_bytes {
                drop(out);
                return Err(format!(
                    "This zip expands past the safety limit of {} (possible zip bomb). Extraction stopped.",
                    crate::util::human_size(limits.max_bytes)
                ));
            }
            out.write_all(&buf[..n]).ctx("writing file")?;
        }
        drop(out);
        let executable =
            mode.map(|m| m & 0o111 != 0).unwrap_or(false) || (first_len == 2 && &first == b"#!");
        let perm = if executable { 0o755 } else { 0o644 };
        let _ = fs::set_permissions(&out_path, fs::Permissions::from_mode(perm));
        report.files += 1;
    }
    report.bytes = written;
    report.unwrapped = unwrap_single_folder(dest)?;
    Ok(report)
}

/// If the folder holds exactly one directory (ignoring OS junk), move that
/// directory's contents up one level. Returns the unwrapped folder name.
pub fn unwrap_single_folder(dest: &Path) -> Res<Option<String>> {
    let entries: Vec<_> = fs::read_dir(dest)
        .ctx("reading folder")?
        .flatten()
        .filter(|e| {
            let n = e.file_name().to_string_lossy().to_string();
            n != "__MACOSX" && n != ".DS_Store" && n != "Thumbs.db"
        })
        .collect();
    if entries.len() != 1 || !entries[0].path().is_dir() {
        return Ok(None);
    }
    let name = entries[0].file_name().to_string_lossy().to_string();
    let inner = dest.join(format!(".stowlock-unwrap-{}", std::process::id()));
    fs::rename(entries[0].path(), &inner).ctx("unwrapping folder")?;
    for child in fs::read_dir(&inner).ctx("reading folder")?.flatten() {
        fs::rename(child.path(), dest.join(child.file_name())).ctx("unwrapping folder")?;
    }
    fs::remove_dir_all(&inner).ctx("unwrapping folder")?;
    Ok(Some(name))
}

/// Build a zip in memory/on disk from (path, contents, unix mode) — used by
/// tests and the self-test to make fixture zips.
pub fn write_zip(path: &Path, entries: &[(&str, &[u8], u32)]) -> Res<()> {
    let f = fs::File::create(path).ctx("creating zip")?;
    let mut z = zip::ZipWriter::new(f);
    for (name, data, mode) in entries {
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated)
            .unix_permissions(*mode);
        if name.ends_with('/') {
            z.add_directory(name.trim_end_matches('/'), opts)
                .ctx("zip dir")?;
        } else {
            z.start_file(*name, opts).ctx("zip file")?;
            z.write_all(data).ctx("zip write")?;
        }
    }
    z.finish().ctx("finishing zip")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Write a zip whose entry name is taken verbatim (for traversal tests).
    fn raw_zip(path: &Path, name: &str) {
        let f = fs::File::create(path).unwrap();
        let mut z = zip::ZipWriter::new(f);
        z.start_file(name, zip::write::SimpleFileOptions::default())
            .unwrap();
        z.write_all(b"evil").unwrap();
        z.start_file("ok.txt", zip::write::SimpleFileOptions::default())
            .unwrap();
        z.write_all(b"fine").unwrap();
        z.finish().unwrap();
    }

    #[test]
    fn rejects_traversal_and_absolute() {
        let d = tempfile::tempdir().unwrap();
        for bad in ["../evil.txt", "/etc/evil.txt", "a/../../evil.txt"] {
            let zp = d.path().join("bad.zip");
            raw_zip(&zp, bad);
            let out = d.path().join("out");
            let _ = fs::remove_dir_all(&out);
            let r = extract_zip(&zp, &out).unwrap();
            assert!(!d.path().join("evil.txt").exists(), "{bad} escaped");
            assert!(!Path::new("/etc/evil.txt").exists());
            assert!(out.join("ok.txt").exists());
            assert!(
                !r.skipped_unsafe.is_empty()
                    || !out.join("etc/evil.txt").exists()
                    || bad.starts_with('/'),
                "{bad}"
            );
            // Nothing is ever written outside the destination.
            for e in walkdir::WalkDir::new(d.path()).into_iter().flatten() {
                if e.file_name() == "evil.txt" {
                    assert!(e.path().starts_with(&out), "{bad} wrote {:?}", e.path());
                }
            }
        }
    }

    #[test]
    fn unwraps_single_folder_only() {
        let d = tempfile::tempdir().unwrap();
        let zp = d.path().join("a.zip");
        write_zip(
            &zp,
            &[
                ("myapp-main/", b"", 0o755),
                ("myapp-main/src/main.rs", b"fn main(){}", 0o644),
                ("myapp-main/run.sh", b"#!/bin/sh\necho hi\n", 0o644),
            ],
        )
        .unwrap();
        let out = d.path().join("out");
        let r = extract_zip(&zp, &out).unwrap();
        assert_eq!(r.unwrapped.as_deref(), Some("myapp-main"));
        assert!(out.join("src/main.rs").exists());
        let mode = fs::metadata(out.join("run.sh"))
            .unwrap()
            .permissions()
            .mode();
        assert!(mode & 0o111 != 0, "shebang file should be executable");

        let zp2 = d.path().join("b.zip");
        write_zip(&zp2, &[("a/x.txt", b"1", 0o644), ("b.txt", b"2", 0o644)]).unwrap();
        let out2 = d.path().join("out2");
        let r2 = extract_zip(&zp2, &out2).unwrap();
        assert!(r2.unwrapped.is_none());
        assert!(out2.join("a/x.txt").exists());
    }

    #[test]
    fn keeps_exec_bit_from_zip() {
        let d = tempfile::tempdir().unwrap();
        let zp = d.path().join("x.zip");
        write_zip(
            &zp,
            &[("tool", b"binary", 0o755), ("data.txt", b"x", 0o644)],
        )
        .unwrap();
        let out = d.path().join("o");
        extract_zip(&zp, &out).unwrap();
        assert!(fs::metadata(out.join("tool")).unwrap().permissions().mode() & 0o111 != 0);
        assert!(
            fs::metadata(out.join("data.txt"))
                .unwrap()
                .permissions()
                .mode()
                & 0o111
                == 0
        );
    }

    #[test]
    fn zip_bomb_guard() {
        let d = tempfile::tempdir().unwrap();
        let zp = d.path().join("big.zip");
        let data = vec![b'a'; 200_000];
        write_zip(&zp, &[("big.txt", &data, 0o644)]).unwrap();
        let limits = Limits {
            max_bytes: 100_000,
            max_entries: 10,
        };
        let err = extract_zip_with(&zp, &d.path().join("o"), &limits).unwrap_err();
        assert!(err.contains("safety limit"));
        let many: Vec<(String, Vec<u8>)> =
            (0..20).map(|i| (format!("f{i}.txt"), vec![b'x'])).collect();
        let refs: Vec<(&str, &[u8], u32)> = many
            .iter()
            .map(|(n, b)| (n.as_str(), b.as_slice(), 0o644))
            .collect();
        let zp2 = d.path().join("many.zip");
        write_zip(&zp2, &refs).unwrap();
        assert!(extract_zip_with(&zp2, &d.path().join("o2"), &limits)
            .unwrap_err()
            .contains("entries"));
    }
}
