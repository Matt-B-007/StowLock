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
        let ctx = &root.ctx().clone();
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
                            if ui::primary(ui, "Archive Zip", ok).clicked() {
                                self.archive_note_seen = true;
                                self.archive_only();
                            }
                        });
                    });
                    return;
                }
                let Some(stage) = s.stage.clone() else {
                    ui.horizontal(|ui| {
                        Self::action_text(ui, BTN, |ui| {
                            if let Some(b) = &busy {
                                ui.horizontal(|ui| {
                                    ui.spinner();
                                    ui.label(b);
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
                            let ok = busy.is_none()
                                && name_ok
                                && (s.mode == Mode::Update
                                    || !s.local_exists
                                    || s.local_is_ours
                                    || s.replace_local);
                            if ui::primary(ui, "Stage & check", ok).clicked() {
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
                            ui.label(ui::muted("Run checks first: Stage & check tests the project before anything is pushed."));
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
                            if ui::primary(ui, "Stage another zip", busy.is_none()).clicked() {
                                self.pick_zip();
                            }
                            if ui::button(ui, ui::icon::EXTERNAL, "View on GitHub").clicked() {
                                let url = format!("https://github.com/{}/{}", s.owner, s.slug);
                                let _ = proc::xdg_open(&url);
                            }
                            return;
                        }
                        let ok = busy.is_none() && blockers.is_empty();
                        if ui::primary(ui, &ui::with_icon(ui::icon::UPLOAD, "Push to GitHub…"), ok).clicked() {
                            self.session.confirm = true;
                            self.session.confirm_at = Some(std::time::Instant::now());
                            self.session.public_ack = false;
                            self.session.older_ack = false;
                        }
                    });
                });
            });
        if self.session.confirm {
            self.push_review(ctx);
        }
    }

    /// The "Ready to push" window: everything about the push in one place,
    /// and a confirm button that says exactly what it will do.
    pub(super) fn push_review(&mut self, ctx: &egui::Context) {
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
        let public_new = s.mode == Mode::New && !s.private;
        let tag = s.tag.trim().to_string();
        let armed = s
            .confirm_at
            .map(|t| t.elapsed().as_secs_f32() >= 0.5)
            .unwrap_or(true);
        if !armed {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
        let mut close = false;
        let mut go = false;
        let mut show_files = false;
        let modal = egui::Modal::new(egui::Id::unique("push_review")).show(ctx, |ui| {
            ui.set_width(520.0_f32.min(ctx.content_rect().width() - 80.0));
            ui.horizontal(|ui| {
                let (r, _) = ui.allocate_exact_size(egui::vec2(34.0, 34.0), egui::Sense::hover());
                ui.painter()
                    .rect_filled(r, egui::CornerRadius::same(10), ui::ACCENT_SOFT());
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
            egui::Grid::new("review")
                .num_columns(2)
                .spacing([14.0, 10.0])
                .max_col_width(400.0)
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
                    ui.label(ui::muted("Changes"));
                    ui.horizontal_wrapped(|ui| match &stage.changes {
                        Some(c) if c.total() > 0 => {
                            ui.label(format!(
                                "{} file{}:",
                                c.total(),
                                if c.total() == 1 { "" } else { "s" }
                            ));
                            if !c.added.is_empty() {
                                ui.label(
                                    RichText::new(format!("{} added", c.added.len()))
                                        .color(ui::SUCCESS()),
                                );
                            }
                            if !c.changed.is_empty() {
                                ui.label(
                                    RichText::new(format!("{} changed", c.changed.len()))
                                        .color(WARNING()),
                                );
                            }
                            if !c.removed.is_empty() {
                                ui.label(
                                    RichText::new(format!("{} removed", c.removed.len()))
                                        .color(ERROR()),
                                );
                            }
                            show_files = ui
                                .link(if s.show_changes {
                                    "hide list"
                                } else {
                                    "show list"
                                })
                                .clicked();
                        }
                        Some(_) => {
                            ui.label("No file changes (a new version tag only)");
                        }
                        None => {
                            ui.label(format!("{} files (first push)", stage.files));
                        }
                    });
                    ui.end_row();
                    ui.label(ui::muted("Checks"));
                    let tests = s
                        .checks
                        .as_ref()
                        .map(|c| c.tests.summary.clone())
                        .unwrap_or_default();
                    ui.horizontal_wrapped(|ui| {
                        if s.override_tests {
                            ui.label(
                                RichText::new(format!(
                                    "{} Tests failed, pushing anyway",
                                    ui::icon::TRIANGLE_ALERT
                                ))
                                .color(WARNING()),
                            );
                        } else {
                            ui.label(
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
                            );
                        }
                    });
                    ui.end_row();
                    ui.label(ui::muted("Who gets this"));
                let older = version::newer_existing(&tag, &self.config.tag_prefix, &self.existing_tags());
                ui.add(egui::Label::new(match (&older, tag.is_empty(), s.channel) {
                    (_, true, _) => "Nobody is offered an update (no version tag)".to_string(),
                    (Some(n), _, _) => format!("Nobody: this counts as older than {n}"),
                    (None, _, Channel::Beta) => "People on the Beta update channel".to_string(),
                    (None, _, Channel::Stable) => "Everyone using an older version".to_string(),
                }).wrap());
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
                    ui.label(if after.is_empty() {
                        "Nothing else".to_string()
                    } else {
                        util::capitalize(&after.join(" · "))
                    });
                    ui.end_row();
                });
            if s.show_changes {
                if let Some(c) = &stage.changes {
                    egui::ScrollArea::vertical()
                        .max_height(160.0)
                        .id_salt("review_files")
                        .show(ui, |ui| {
                            for (list, sign, col) in [
                                (&c.added, "+", ui::SUCCESS()),
                                (&c.changed, "~", WARNING()),
                                (&c.removed, "−", ERROR()),
                            ] {
                                for f in list.iter() {
                                    ui.label(ui::mono(format!("{sign} {f}")).color(col));
                                }
                            }
                        });
                }
            }
            let older_tag = version::newer_existing(&tag, &self.config.tag_prefix, &self.existing_tags());
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
            ui.add_space(6.0);
            ui.separator();
            ui.horizontal(|ui| {
                if ui::button(ui, "", "Back").clicked() {
                    close = true;
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let label = if tag.is_empty() {
                        format!("Push to {}", s.slug)
                    } else {
                        format!("Push {tag} to {}", s.slug)
                    };
                    let ready = armed
                        && (!public_new || self.session.public_ack)
                        && (older_tag.is_none() || self.session.older_ack);
                    let clicked =
                        ui::primary(ui, &ui::with_icon(ui::icon::UPLOAD, &label), ready).clicked();
                    let keyed =
                        ready && ui.input(|i| i.modifiers.ctrl && i.key_pressed(egui::Key::Enter));
                    if clicked || keyed {
                        go = true;
                    }
                });
            });
        });
        if show_files {
            self.session.show_changes = !self.session.show_changes;
        }
        if modal.should_close() || close {
            self.session.confirm = false;
        }
        if go {
            self.session.confirm = false;
            self.push();
        }
    }
}
