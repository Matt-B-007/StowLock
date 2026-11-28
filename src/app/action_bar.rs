// SPDX-License-Identifier: GPL-3.0-or-later
//! The App's action bar code (part of the StowLock window).
use super::*;

impl App {
    pub(super) fn log_drawer(&mut self, ui: &mut egui::Ui) {
        ui.add_space(8.0);
        let r = egui::CollapsingHeader::new(ui::muted("Log"))
            .id_salt("log")
            .open(Some(self.show_log))
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    if ui.small_button("Copy log").clicked() {
                        ui.ctx()
                            .copy_text(report::redact(&self.log.text(), &self.redact_ctx()));
                        self.toast("Log copied (personal details removed).");
                    }
                });
                egui::ScrollArea::vertical()
                    .max_height(220.0)
                    .stick_to_bottom(true)
                    .id_salt("logscroll")
                    .show(ui, |ui| {
                        for e in self
                            .log
                            .entries()
                            .iter()
                            .rev()
                            .take(400)
                            .collect::<Vec<_>>()
                            .into_iter()
                            .rev()
                        {
                            let col = match e.level {
                                Level::Info => MUTED(),
                                Level::Warn => WARNING(),
                                Level::Error => ERROR(),
                            };
                            ui.label(ui::mono(e.line()).size(11.5).color(col));
                        }
                    });
            });
        if r.header_response.clicked() {
            self.show_log = !self.show_log;
        }
    }

    /// The text side of the action bar: wraps inside its own width so it
    /// never runs under the button.
    pub(super) fn action_text(ui: &mut egui::Ui, button_w: f32, add: impl FnOnce(&mut egui::Ui)) {
        let w = (ui.available_width() - button_w - 16.0).max(120.0);
        ui.vertical(|ui| {
            ui.set_max_width(w);
            ui.set_min_width(w);
            add(ui);
        });
    }

    pub(super) fn action_bar(&mut self, root: &mut egui::Ui) {
        egui::Panel::bottom("actions")
            .frame(
                egui::Frame::new()
                    .fill(ui::CARD())
                    .inner_margin(egui::Margin::symmetric(16, 12))
                    .stroke(egui::Stroke::new(1.0_f32, BORDER())),
            )
            .show(root, |ui| {
                let s = self.session.clone();
                let busy = self.busy.clone();
                const BTN: f32 = 180.0;
                if s.zip.is_none() && s.stage.is_none() {
                    ui.label(ui::muted("No project loaded."));
                    return;
                }
                let name_ok = detect::name_problem(&s.slug).is_none();
                if s.archive_only {
                    ui.horizontal(|ui| {
                        Self::action_text(ui, BTN, |ui| {
                            ui.add(
                                egui::Label::new(ui::muted(format!(
                                    "Will file the zip under _archive/{}/. Nothing will be pushed.",
                                    s.slug
                                )))
                                .wrap(),
                            );
                        });
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let ok = busy.is_none() && name_ok && s.zip.is_some();
                            if ui::primary_glow(ui, "Archive Zip", ok).clicked() {
                                self.archive_note_seen = true;
                                self.archive_only();
                            }
                        });
                    });
                    return;
                }
                let Some(stage) = s.stage.clone() else {
                    let folder_ok = s.mode == Mode::Update
                        || !s.local_exists
                        || s.local_is_ours
                        || s.replace_local;
                    // Why the button can't be pressed, said beside it.
                    let why = if let Some(p) = detect::name_problem(&s.slug) {
                        Some(p)
                    } else if !folder_ok && s.deleted_on_github {
                        Some("This repo was deleted on GitHub: tick \"Create it again\" above, or type another name.".to_string())
                    } else if !folder_ok {
                        Some("A folder with this name already exists: tick \"Replace the existing folder\" above, or type another name.".to_string())
                    } else {
                        None
                    };
                    ui.horizontal(|ui| {
                        Self::action_text(ui, BTN, |ui| {
                            if let Some(b) = &busy {
                                ui.horizontal(|ui| {
                                    ui.spinner();
                                    ui.label(b);
                                    if ui.small_button("Cancel").clicked() {
                                        self.cancel.store(true, Ordering::Relaxed);
                                    }
                                });
                            } else if let Some(why) = &why {
                                ui.horizontal(|ui| {
                                    ui.label(RichText::new(ui::icon::CIRCLE_X).size(16.0).color(ERROR()));
                                    ui.add(egui::Label::new(RichText::new(why).color(ERROR())).wrap());
                                });
                            } else {
                                ui.add(
                                    egui::Label::new(ui::muted(
                                        "Unpack and check the project. Nothing goes to GitHub yet.",
                                    ))
                                    .wrap(),
                                );
                            }
                        });
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let ok = busy.is_none() && why.is_none();
                            if ui::primary_glow(ui, "Stage & check", ok).clicked() {
                                self.stage();
                            }
                        });
                    });
                    return;
                };
                let pctx = self.pctx();
                let tags = self.existing_tags();
                let mut blockers = pipeline::push_blockers(
                    &pctx,
                    &stage,
                    s.checks.as_ref(),
                    !s.private && s.mode == Mode::New,
                    s.override_tests,
                );
                if !self.env.ready() {
                    blockers.push("Finish setup (sign in to GitHub)".into());
                }
                if s.outcome.is_none() {
                    if let Err(e) =
                        version::validate(&s.tag, s.channel, &self.config.tag_prefix, &tags)
                    {
                        blockers.push(e);
                    }
                }
                if s.outcome
                    .as_ref()
                    .map(|o| !o.nothing_to_push)
                    .unwrap_or(false)
                    && s.mode == Mode::New
                {
                    blockers.push("Already pushed".into());
                }
                let pushed = s.outcome.as_ref().map(|o| !o.nothing_to_push).unwrap_or(false);
                // When the checks you started pass, go straight to the review.
                if s.review_when_ready && busy.is_none() && s.checks.is_some() {
                    self.session.review_when_ready = false;
                    if blockers.is_empty() && s.outcome.is_none() {
                        self.session.confirm = true;
                        self.session.confirm_at = Some(std::time::Instant::now());
                        self.session.public_ack = false;
                        self.session.older_ack = false;
                    }
                }
                if self.session.confirm {
                    self.review_bar(ui);
                    return;
                }
                ui.horizontal(|ui| {
                    Self::action_text(ui, BTN + 140.0, |ui| {
                        if let Some(b) = &busy {
                            ui.horizontal(|ui| {
                                ui.spinner();
                                ui.label(b);
                                if ui.small_button("Cancel").clicked() {
                                    self.cancel.store(true, Ordering::Relaxed);
                                }
                            });
                        } else if pushed {
                            ui.horizontal(|ui| {
                                ui.label(RichText::new(ui::icon::CIRCLE_CHECK).size(16.0).color(ui::SUCCESS()));
                                ui.add(egui::Label::new(ui::semibold(
                                    format!("Pushed to github.com/{}/{}{}", s.owner, s.slug,
                                        s.outcome.as_ref().and_then(|o| o.tag.clone()).map(|t| format!(" · {t}")).unwrap_or_default()),
                                    14.0,
                                )).truncate());
                            });
                        } else if s.checks.is_none() {
                            ui.label(ui::muted("The checks haven't run: Re-check tests the project before anything is pushed."));
                        } else if let Some(b) = blockers.first() {
                            ui.horizontal(|ui| {
                                ui.label(RichText::new(ui::icon::CIRCLE_X).size(16.0).color(ERROR()));
                                ui.add(egui::Label::new(RichText::new(format!("Blocked: {b}")).color(ERROR())).wrap());
                            });
                            for more in blockers.iter().skip(1).take(2) {
                                ui.add(egui::Label::new(ui::muted(format!("Also: {more}")).size(12.5)).wrap());
                            }
                        } else {
                            ui.horizontal(|ui| {
                                ui.label(RichText::new(ui::icon::CIRCLE_CHECK).size(16.0).color(ui::SUCCESS()));
                                ui.label(ui::semibold("Ready to push", 14.0).color(ui::SUCCESS()));
                                ui.label(ui::muted(format!("· github.com/{}/{}", s.owner, s.slug)).size(12.5));
                            });
                        }
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if pushed {
                            if ui::primary_glow(ui, "Stage another zip", busy.is_none()).clicked() {
                                self.pick_zip();
                            }
                            if ui::button(ui, ui::icon::EXTERNAL, "View on GitHub").clicked() {
                                let url = format!("https://github.com/{}/{}", s.owner, s.slug);
                                let _ = proc::xdg_open(&url);
                            }
                            return;
                        }
                        let ok = busy.is_none() && blockers.is_empty();
                        if ui::primary_glow(ui, &ui::with_icon(ui::icon::UPLOAD, "Review & push…"), ok).clicked() {
                            self.session.confirm = true;
                            self.session.confirm_at = Some(std::time::Instant::now());
                            self.session.public_ack = false;
                            self.session.older_ack = false;
                        }
                    });
                });
            });
    }

    /// Is the version older than a release, and is this a new public repo?
    fn review_facts(&self) -> (Option<String>, bool) {
        let s = &self.session;
        let older =
            version::newer_existing(s.tag.trim(), &self.config.tag_prefix, &self.existing_tags());
        (older, s.mode == Mode::New && !s.private)
    }

    /// The bottom bar while reviewing: Back, and a button that says exactly
    /// what it will do.
    fn review_bar(&mut self, ui: &mut egui::Ui) {
        let s = self.session.clone();
        let tag = s.tag.trim().to_string();
        let armed = s
            .confirm_at
            .map(|t| t.elapsed().as_secs_f32() >= 0.5)
            .unwrap_or(true);
        if !armed {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(100));
        }
        let (older, public_new) = self.review_facts();
        let waiting = if public_new && !s.public_ack {
            Some("Tick the box above to confirm the repo will be public.")
        } else if older.is_some() && !s.older_ack {
            Some("Tick the box above to push this older version.")
        } else {
            None
        };
        let mut close = false;
        let mut go = false;
        ui.horizontal(|ui| {
            Self::action_text(ui, 420.0, |ui| match waiting {
                Some(w) => {
                    ui.add(egui::Label::new(RichText::new(w).color(WARNING())).wrap());
                }
                None => {
                    ui.add(
                        egui::Label::new(ui::muted(
                            "Check the details above, then push. Ctrl+Enter pushes, Esc goes back.",
                        ))
                        .wrap(),
                    );
                }
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                // A long name would push the button off the window.
                let to = if s.slug.chars().count() <= 24 {
                    s.slug.clone()
                } else {
                    "GitHub".to_string()
                };
                let label = if tag.is_empty() {
                    format!("Push to {to}")
                } else {
                    format!("Push {tag} to {to}")
                };
                let ready = armed && waiting.is_none() && self.busy.is_none();
                let clicked =
                    ui::primary_glow(ui, &ui::with_icon(ui::icon::UPLOAD, &label), ready).clicked();
                let keyed =
                    ready && ui.input(|i| i.modifiers.ctrl && i.key_pressed(egui::Key::Enter));
                go = clicked || keyed;
                close = ui::button(ui, "", "Back").clicked();
            });
        });
        let esc = self.palette.is_none()
            && !self.show_shortcuts
            && ui.input(|i| i.key_pressed(egui::Key::Escape));
        if close || esc {
            self.session.confirm = false;
        }
        if go {
            self.session.confirm = false;
            self.push();
        }
    }

    /// The review page: everything about the push in one place, with the
    /// files that changed and the release notes.
    pub(super) fn review_page(&mut self, ui: &mut egui::Ui) {
        if self.session.workflow_publishes.is_none() {
            if let Some(st) = &self.session.stage {
                self.session.workflow_publishes =
                    Some(crate::workflows::publishes_releases(&st.dir));
            }
        }
        let s = self.session.clone();
        let Some(stage) = s.stage.clone() else {
            self.session.confirm = false;
            return;
        };
        let tag = s.tag.trim().to_string();
        if self
            .session
            .review_notes
            .as_ref()
            .is_none_or(|(t, _)| *t != tag)
        {
            let notes = if tag.is_empty() {
                None
            } else {
                version::changelog_notes(&stage.dir, &tag)
            };
            self.session.review_notes = Some((tag.clone(), notes));
            self.session.review_files = if stage.changes.is_none() {
                git::files_to_commit(&stage.dir)
                    .iter()
                    .map(|f| f.display().to_string())
                    .collect()
            } else {
                Vec::new()
            };
        }
        let first_files = self.session.review_files.clone();
        let notes = self.session.review_notes.clone().and_then(|(_, n)| n);
        let (older_tag, public_new) = self.review_facts();
        ui.add_space(8.0);
        ui::card_active(ui, true, |ui| {
            ui.horizontal(|ui| {
                let (r, _) = ui.allocate_exact_size(egui::vec2(34.0, 34.0), egui::Sense::hover());
                ui.painter()
                    .rect_filled(r, egui::CornerRadius::same(8), ui::ACCENT_SOFT());
                ui.painter().text(
                    r.center(),
                    egui::Align2::CENTER_CENTER,
                    ui::icon::UPLOAD,
                    egui::FontId::proportional(18.0),
                    ACCENT(),
                );
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 0.0;
                    ui.label(ui::semibold("Ready to push", 18.0));
                    ui.label(ui::muted("Check the details, then push.").size(13.0));
                });
            });
            ui.add_space(8.0);
            let col_w = (ui.available_width() - 150.0).max(160.0);
            egui::Grid::new("review")
                .num_columns(2)
                .spacing([14.0, 10.0])
                .max_col_width(col_w)
                .show(ui, |ui| {
                    ui.label(ui::muted("Where"));
                    ui.add(
                        egui::Label::new(ui::mono(format!("github.com/{}/{}", s.owner, s.slug)))
                            .truncate(),
                    );
                    ui.end_row();
                    ui.label(ui::muted("Type"));
                    ui.horizontal_wrapped(|ui| {
                        ui::badge(
                            ui,
                            if s.mode == Mode::New {
                                "New repo"
                            } else {
                                "Update"
                            },
                            ui::SUCCESS(),
                        );
                        if s.mode == Mode::New {
                            if s.private {
                                ui::badge(ui, "Private", MUTED());
                            } else {
                                ui::badge(ui, "Public", WARNING());
                            }
                        }
                    });
                    ui.end_row();
                    ui.label(ui::muted("Version"));
                    ui.label(if tag.is_empty() {
                        "No release (code only)".to_string()
                    } else if s.channel == Channel::Beta {
                        format!("{tag} · Beta (pre-release)")
                    } else {
                        format!("{tag} · Stable")
                    });
                    ui.end_row();
                    ui.label(ui::muted("Commit"));
                    ui.add(egui::Label::new(s.commit_msg.trim()).truncate());
                    ui.end_row();
                    ui.label(ui::muted("Checks"));
                    let tests = s
                        .checks
                        .as_ref()
                        .map(|c| c.tests.summary.clone())
                        .unwrap_or_default();
                    if s.override_tests {
                        ui.label(
                            RichText::new(format!(
                                "{} Checks didn't pass, pushing anyway",
                                ui::icon::TRIANGLE_ALERT
                            ))
                            .color(WARNING()),
                        );
                    } else {
                        // Long summaries wrap inside the page.
                        ui.add(
                            egui::Label::new(
                                RichText::new(format!(
                                    "{} {}",
                                    ui::icon::CIRCLE_CHECK,
                                    if tests.is_empty() {
                                        "Passed".to_string()
                                    } else {
                                        tests
                                    }
                                ))
                                .color(ui::SUCCESS()),
                            )
                            .wrap(),
                        );
                    }
                    ui.end_row();
                    ui.label(ui::muted("Who gets this"));
                    ui.add(
                        egui::Label::new(match (&older_tag, tag.is_empty(), s.channel) {
                            (_, true, _) => {
                                "Nobody is offered an update (no version tag)".to_string()
                            }
                            (Some(n), _, _) => format!("Nobody: this counts as older than {n}"),
                            (None, _, Channel::Beta) => {
                                "People on the Beta update channel".to_string()
                            }
                            (None, _, Channel::Stable) => {
                                "Everyone using an older version".to_string()
                            }
                        })
                        .wrap(),
                    );
                    ui.end_row();
                    ui.label(ui::muted("After"));
                    let mut after = Vec::new();
                    if !tag.is_empty() {
                        if s.workflow_publishes == Some(true) {
                            after.push("the project's release workflow builds and publishes the release on GitHub");
                        } else {
                            after.push("release created");
                        }
                    }
                    if s.archive_after {
                        after.push(if s.archive_move {
                            "zip moved to the archive"
                        } else {
                            "zip copied to the archive"
                        });
                    }
                    ui.add(
                        egui::Label::new(if after.is_empty() {
                            "Nothing else".to_string()
                        } else {
                            util::capitalize(&after.join(" · "))
                        })
                        .wrap(),
                    );
                    ui.end_row();
                });
            if let Some(n) = &older_tag {
                ui.add_space(4.0);
                ui::banner(ui, WARNING(), |ui| {
                    ui.add(egui::Label::new(format!("{tag} counts as older than {n}, so nobody on {n} will be offered it. That's only right for tagging an old fix.")).wrap());
                    ui.checkbox(&mut self.session.older_ack, "Push it anyway");
                });
            }
            if public_new {
                ui.add_space(4.0);
                ui::banner(ui, WARNING(), |ui| {
                    ui.add(
                        egui::Label::new(
                            "This creates a public repository: anyone can see this code.",
                        )
                        .wrap(),
                    );
                    ui.checkbox(
                        &mut self.session.public_ack,
                        "I understand this will be public",
                    );
                });
            }
        });
        ui.add_space(8.0);
        ui::card(ui, |ui| match &stage.changes {
            Some(c) if c.total() > 0 => {
                ui.horizontal_wrapped(|ui| {
                    ui::section_title(ui, "CHANGED FILES");
                    ui.label(ui::muted(c.summary()).size(12.5));
                });
                egui::ScrollArea::vertical()
                    .max_height(200.0)
                    .id_salt("review_files")
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        for (list, sign, col) in [
                            (&c.added, "+", ui::SUCCESS()),
                            (&c.changed, "~", WARNING()),
                            (&c.removed, "−", ERROR()),
                        ] {
                            for f in list.iter() {
                                ui.add(
                                    egui::Label::new(
                                        ui::mono(format!("{sign} {f}")).size(12.5).color(col),
                                    )
                                    .truncate(),
                                );
                            }
                        }
                    });
            }
            Some(_) => {
                ui::section_title(ui, "CHANGED FILES");
                ui.label("No file changes (a new version tag only).");
            }
            None => {
                ui::section_title(ui, "FILES");
                ui.label(format!(
                    "{} files · {} (first push: everything is new).",
                    stage.files,
                    util::human_size(stage.size)
                ));
                egui::ScrollArea::vertical()
                    .max_height(200.0)
                    .id_salt("review_files")
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        for f in &first_files {
                            ui.add(
                                egui::Label::new(
                                    ui::mono(format!("+ {f}")).size(12.5).color(ui::SUCCESS()),
                                )
                                .truncate(),
                            );
                        }
                    });
            }
        });
        if !tag.is_empty() {
            ui.add_space(8.0);
            ui::card(ui, |ui| {
                ui::section_title(ui, "RELEASE NOTES");
                match &notes {
                    Some(n) => {
                        ui.label(
                            ui::muted(format!("From this project's CHANGELOG, under {tag}."))
                                .size(12.5),
                        );
                        egui::ScrollArea::vertical()
                            .max_height(180.0)
                            .id_salt("review_notes")
                            .auto_shrink([false, true])
                            .show(ui, |ui| {
                                ui.add(egui::Label::new(RichText::new(n.trim()).size(13.0)).wrap());
                            });
                    }
                    None if s.workflow_publishes == Some(true) => {
                        ui.add(
                            egui::Label::new(ui::muted(
                                "The project's release workflow writes them.",
                            ))
                            .wrap(),
                        );
                    }
                    None => {
                        ui.add(egui::Label::new(ui::muted(format!("No CHANGELOG entry for {tag}, so GitHub lists the commits since the last release. Add a \"{tag}\" section to CHANGELOG.md to write your own."))).wrap());
                    }
                }
            });
        }
    }
}
