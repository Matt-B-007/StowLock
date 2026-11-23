// SPDX-License-Identifier: GPL-3.0-or-later
//! Push history (`history.json`) and the "Pushed today" list.

use crate::config::AppPaths;
use crate::util::{self, Res};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct PushRecord {
    pub slug: String,
    pub owner: String,
    pub github_url: String,
    /// "new" or "update"
    pub mode: String,
    pub tag: Option<String>,
    /// "stable" or "beta"
    pub channel: String,
    /// "private" or "public"
    pub visibility: String,
    pub pushed_at: String,
    pub archive_path: Option<String>,
}

impl PushRecord {
    pub fn date(&self) -> &str {
        self.pushed_at.get(..10).unwrap_or("")
    }
    pub fn time(&self) -> &str {
        self.pushed_at.get(11..16).unwrap_or("")
    }
    pub fn chip(&self) -> String {
        match &self.tag {
            Some(t) => format!("{} {} · {}", self.slug, t, self.time()),
            None => format!("{} · {}", self.slug, self.time()),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct History {
    pub pushes: Vec<PushRecord>,
}

impl History {
    pub fn load(paths: &AppPaths) -> Self {
        util::read_json_or_default(&paths.history_file())
    }

    pub fn append(paths: &AppPaths, rec: PushRecord) -> Res<()> {
        let mut h = Self::load(paths);
        h.pushes.push(rec);
        util::ensure_private_dir(&paths.state_dir)?;
        util::write_json(&paths.history_file(), &h)
    }

    pub fn clear(paths: &AppPaths) -> Res<()> {
        util::write_json(&paths.history_file(), &History::default())
    }

    pub fn on_date(&self, date: &str) -> Vec<PushRecord> {
        self.pushes
            .iter()
            .filter(|p| p.date() == date)
            .cloned()
            .collect()
    }

    pub fn today(&self) -> Vec<PushRecord> {
        self.on_date(&util::today())
    }

    pub fn last_for(&self, slug: &str) -> Option<PushRecord> {
        self.pushes.iter().rev().find(|p| p.slug == slug).cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_and_counts_today() {
        let d = tempfile::tempdir().unwrap();
        let paths = AppPaths::under(d.path());
        for slug in ["sluice", "clip-sorter", "md-toc"] {
            History::append(
                &paths,
                PushRecord {
                    slug: slug.into(),
                    pushed_at: util::now_iso(),
                    tag: Some("v0.1.0".into()),
                    ..Default::default()
                },
            )
            .unwrap();
        }
        History::append(
            &paths,
            PushRecord {
                slug: "old".into(),
                pushed_at: "2020-01-01T10:00:00+00:00".into(),
                ..Default::default()
            },
        )
        .unwrap();
        let h = History::load(&paths);
        assert_eq!(h.today().len(), 3);
        assert_eq!(h.last_for("old").unwrap().time(), "10:00");
        assert!(h.today()[0].chip().starts_with("sluice v0.1.0 · "));
    }
}
