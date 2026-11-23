// SPDX-License-Identifier: GPL-3.0-or-later
//! Filing zips under `<workspace>/_archive/<slug>/`. Never overwrites.

use crate::util::{self, Ctx, Res};
use std::path::{Path, PathBuf};

/// Pick a free archive file name: `<slug>-<tag>.zip` or `<slug>-<stamp>.zip`,
/// then `-2`, `-3`… if taken.
pub fn archive_name(dir: &Path, slug: &str, tag: Option<&str>) -> PathBuf {
    let label = match tag.map(|t| t.trim()).filter(|t| !t.is_empty()) {
        Some(t) => t.to_string(),
        None => util::now_stamp(),
    };
    let base = format!("{slug}-{label}");
    let mut p = dir.join(format!("{base}.zip"));
    let mut n = 2;
    while p.exists() {
        p = dir.join(format!("{base}-{n}.zip"));
        n += 1;
    }
    p
}

/// Move or copy a zip into the archive. Folders are private (0700) because
/// archived zips are originals and may contain `.env` files.
pub fn archive_zip(
    zip: &Path,
    archive_root: &Path,
    slug: &str,
    tag: Option<&str>,
    move_it: bool,
) -> Res<PathBuf> {
    if !zip.is_file() {
        return Err(format!("The zip {} is no longer there.", util::tilde(zip)));
    }
    util::ensure_private_dir(archive_root)?;
    let dir = archive_dir(archive_root, slug);
    util::ensure_private_dir(&dir)?;
    let dest = archive_name(&dir, slug, tag);
    if move_it {
        util::move_file(zip, &dest)?;
    } else {
        std::fs::copy(zip, &dest).ctx("copying zip to the archive")?;
    }
    Ok(dest)
}

/// The archive folder for a project (an existing folder in any capitalisation).
pub fn archive_dir(archive_root: &Path, slug: &str) -> PathBuf {
    crate::detect::project_dir(archive_root, slug)
}

/// Would this tag's archive name already be taken?
pub fn already_archived(archive_root: &Path, slug: &str, tag: &str) -> bool {
    archive_dir(archive_root, slug)
        .join(format!("{slug}-{tag}.zip"))
        .exists()
}

pub fn count_archives(archive_root: &Path, slug: &str) -> usize {
    std::fs::read_dir(archive_dir(archive_root, slug))
        .map(|rd| {
            rd.flatten()
                .filter(|e| e.path().extension().map(|x| x == "zip").unwrap_or(false))
                .count()
        })
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn names_never_overwrite() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("_archive");
        for i in 0..3 {
            let z = d.path().join(format!("in{i}.zip"));
            std::fs::write(&z, "zip").unwrap();
            archive_zip(&z, &root, "sluice", Some("v0.2.0"), true).unwrap();
        }
        let dir = root.join("sluice");
        assert!(dir.join("sluice-v0.2.0.zip").exists());
        assert!(dir.join("sluice-v0.2.0-2.zip").exists());
        assert!(dir.join("sluice-v0.2.0-3.zip").exists());
        assert_eq!(count_archives(&root, "sluice"), 3);
        let mode = std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700);
    }

    #[test]
    fn copy_keeps_original() {
        let d = tempfile::tempdir().unwrap();
        let z = d.path().join("a.zip");
        std::fs::write(&z, "zip").unwrap();
        let out = archive_zip(&z, &d.path().join("_archive"), "a", None, false).unwrap();
        assert!(z.exists() && out.exists());
        assert!(out
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("a-20"));
    }
}
