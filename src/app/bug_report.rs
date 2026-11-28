// SPDX-License-Identifier: GPL-3.0-or-later
//! The App's bug report code (part of the StowLock window).
use super::*;

impl App {
    pub(super) fn redact_ctx(&self) -> RedactContext {
        let mut rc = RedactContext::local(&self.config.workspace());
        rc.logins = self.env.accounts.iter().map(|a| a.login.clone()).collect();
        if let Some(l) = &self.env.login {
            if !rc.logins.contains(l) {
                rc.logins.push(l.clone());
            }
        }
        rc.orgs = self.env.orgs.clone();
        // Everything that could name a project: GitHub repos (with their real
        // visibility), workspace folders, archive folders, push history and the
        // loaded zip. Unknown visibility is treated as private.
        let mut repos: Vec<(String, bool)> = self.env.repos.clone();
        let mut add = |name: &str| {
            if !name.is_empty() && !repos.iter().any(|(n, _)| n.eq_ignore_ascii_case(name)) {
                repos.push((name.to_string(), true));
            }
        };
        for p in &self.projects {
            add(&p.slug);
        }
        if let Ok(rd) = std::fs::read_dir(self.config.archive_root()) {
            for e in rd.flatten() {
                add(&e.file_name().to_string_lossy());
            }
        }
        for h in &self.history.pushes {
            add(&h.slug);
        }
        add(&self.session.slug);
        if let Some(z) = &self.session.zip {
            let name = z
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            add(&detect::slug_from_filename(&name));
        }
        for e in self.log.entries() {
            if let Some(rest) = e.message.strip_prefix("Loaded ") {
                let name = rest.rsplit('/').next().unwrap_or(rest);
                add(&detect::slug_from_filename(name));
            }
        }
        rc.repos = repos;
        rc.include_public_repo_names = self.bug.include_public;
        rc.include_username = self.bug.include_username;
        rc.extra_words = self.config.report_hide_words.clone();
        rc
    }

    pub(super) fn open_bug_report(&mut self) {
        self.tab = Tab::Report;
        self.bug.created = chrono::Local::now();
        self.bug.saved_to = None;
        self.bug.confirm_issue = false;
        self.bug.preview = self.build_bug_report();
    }

    /// Download the previewed report as a .txt file, to a folder the user picks.
    pub(super) fn download_bug_report(&mut self) {
        let text = self.bug.preview.clone();
        let name = report::bug_report_file_name(&self.bug.created);
        let start = dirs::download_dir().unwrap_or_else(util::home_dir);
        self.bug.saving = true;
        self.spawn_bg(move || {
            let r = match save_file_dialog(&start, &name) {
                None => Ok(None),
                Some(path) => util::atomic_write(&path, text.as_bytes())
                    .map(|_| Some(path.clone()))
                    .map_err(|e| format!("Couldn't save {}: {e}", util::tilde(&path))),
            };
            Msg::BugDownloaded(r)
        });
    }

    pub(super) fn excluded(&self, section: &str) -> bool {
        self.bug.excluded.iter().any(|x| x == section)
    }

    pub(super) fn refresh_attempts(&mut self) {
        self.attempts = crate::attempts::list(&self.config.workspace());
    }

    /// How checks run on this computer, for bug reports.
    pub(super) fn checks_mode(&self) -> String {
        let c = &self.config;
        let mut v = Vec::new();
        if c.clean_test_home {
            if c.sandbox_checks && crate::sandbox::available() {
                v.push("sandboxed");
            } else if c.sandbox_checks {
                v.push("not sandboxed (bubblewrap not available)");
            } else {
                v.push("not sandboxed");
            }
            v.push("clean home");
        } else {
            v.push("your own home folder");
        }
        if c.github_fonts {
            v.push("GitHub's fonts");
        }
        if c.offline_checks {
            v.push("offline");
        }
        if c.exact_github_checks {
            v.push("exact mode");
        }
        v.join(" · ")
    }

    pub(super) fn build_bug_report(&self) -> String {
        let s = &self.session;
        let input = ReportInput {
            title: "bug report".into(),
            what_doing: self.bug.what.clone(),
            expected: self.bug.expected.clone(),
            env: report::EnvInfo {
                checks: self.checks_mode(),
                ..report::collect_env(Some(self.env.login.is_some()), self.env.secure())
            },
            session: if s.slug.is_empty()
                || self.bug.excluded.iter().any(|x| x == "Current project")
            {
                None
            } else {
                Some(SessionInfo {
                    owner: s.owner.clone(),
                    slug: s.slug.clone(),
                    mode: s.effective_mode().badge().into(),
                    channel: s.channel.label().into(),
                    tag: s.tag.clone(),
                    stack: s
                        .stage
                        .as_ref()
                        .map(|x| x.stack().name().to_string())
                        .unwrap_or_else(|| "not staged".into()),
                })
            },
            steps: Vec::new(),
            runs: if self.excluded("Recent runs") {
                vec![]
            } else {
                self.log.runs()
            },
            failing_command: self.log.failing_command(),
            health: if self.excluded("Health") {
                vec![]
            } else {
                self.health.lines()
            },
            selftest_summary: self.last_selftest.as_ref().map(|(r, _)| r.summary_line()),
            problem_lines: if self.excluded("Warnings and errors") {
                vec![]
            } else {
                self.log.report_lines(40, 100).0
            },
            log_lines: if self.excluded("Latest log lines") {
                vec![]
            } else {
                self.log.report_lines(40, 100).1
            },
            panic: None,
        };
        let text = report::build_report(&input, &self.redact_ctx());
        let created = format!(
            "- **Created:** {}\n",
            self.bug.created.format("%Y-%m-%d %H:%M:%S")
        );
        match text.find("\n\n") {
            Some(i) => format!("{}{}{}", &text[..i + 2], created, &text[i + 2..]),
            None => text,
        }
    }
}
