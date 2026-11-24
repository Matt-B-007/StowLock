// SPDX-License-Identifier: GPL-3.0-or-later
//! Fixture zips and a sandboxed pipeline context backed by a fake GitHub.
//! Used by the self-test and the automated tests; never touches real data.

use crate::config::{AppPaths, Config};
use crate::extract::write_zip;
use crate::github::FakeGh;
use crate::logbook::Logger;
use crate::pipeline::Ctx;
use crate::util::Res;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

pub struct Sandbox {
    pub root: PathBuf,
    pub ctx: Ctx,
    pub fake: Arc<FakeGh>,
    pub owner: String,
}

impl Sandbox {
    pub fn new(root: &Path) -> Res<Sandbox> {
        std::fs::create_dir_all(root).map_err(|e| e.to_string())?;
        let paths = AppPaths::under(&root.join("app"));
        paths.ensure()?;
        let config = Config {
            workspace_dir: root.join("GitHub_Projects").display().to_string(),
            run_tests_before_push: true,
            test_timeout_secs: 120,
            fallback_git_name: "StowLock Self-Test".into(),
            fallback_git_email: "selftest@example.invalid".into(),
            ..Config::default()
        };
        let fake = Arc::new(FakeGh::new(&root.join("fake-github")));
        let log = Logger::new(Some(paths.log_file()));
        let ctx = Ctx {
            paths,
            config,
            gh: fake.clone(),
            log,
            cancel: Arc::new(AtomicBool::new(false)),
            trash_dir: Some(root.join("Trash")),
        };
        Ok(Sandbox {
            root: root.to_path_buf(),
            ctx,
            fake,
            owner: "selftest-user".into(),
        })
    }

    pub fn zip(&self, name: &str, entries: &[(&str, &[u8], u32)]) -> PathBuf {
        let dir = self.root.join("downloads");
        let _ = std::fs::create_dir_all(&dir);
        let p = dir.join(name);
        write_zip(&p, entries).expect("fixture zip");
        p
    }

    /// A small Python project wrapped in a `<name>-main/` folder.
    pub fn python_zip(&self, file: &str, wrapper: &str, extra: &[(&str, &[u8])]) -> PathBuf {
        let mut entries: Vec<(String, Vec<u8>, u32)> = vec![
            (
                format!("{wrapper}/main.py"),
                b"print('hello from stowlock')\n".to_vec(),
                0o644,
            ),
            (
                format!("{wrapper}/util.py"),
                b"def add(a, b):\n    return a + b\n".to_vec(),
                0o644,
            ),
            (format!("{wrapper}/.DS_Store"), b"junk".to_vec(), 0o644),
            (
                format!("{wrapper}/.env"),
                b"API_KEY=local-only\n".to_vec(),
                0o644,
            ),
            (
                format!("{wrapper}/run.sh"),
                b"#!/bin/sh\npython3 main.py\n".to_vec(),
                0o644,
            ),
        ];
        for (n, b) in extra {
            entries.push((format!("{wrapper}/{n}"), b.to_vec(), 0o644));
        }
        let refs: Vec<(&str, &[u8], u32)> = entries
            .iter()
            .map(|(n, b, m)| (n.as_str(), b.as_slice(), *m))
            .collect();
        self.zip(file, &refs)
    }
}

/// A token-shaped string that matches the GitHub pattern but is fake.
pub fn fake_token() -> String {
    format!(
        "ghp_{}",
        "St0wL0ckFakeT0ken"
            .repeat(3)
            .chars()
            .take(36)
            .collect::<String>()
    )
}
