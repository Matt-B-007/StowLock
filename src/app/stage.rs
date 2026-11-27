// SPDX-License-Identifier: GPL-3.0-or-later
//! The App's stage code (part of the StowLock window).
use super::*;

impl App {
    pub(super) fn banners(&mut self, ui: &mut egui::Ui) {
        if self.env.checked && !self.env.ready() {
            ui::banner(ui, WARNING(), |ui| {
                ui.horizontal_wrapped(|ui| {
                    let what = if !self.env.git || !self.env.gh {
                        "Git and GitHub's gh tool need to be installed."
                    } else {
                        "Sign in to GitHub to push projects."
                    };
                    ui.label(RichText::new(format!("Finish setup: {what}")).color(WARNING()));
                    if ui.button("Finish setup").clicked() {
                        self.tab = Tab::Settings;
                    }
                });
            });
        }
        if self.env.offline {
            ui.label(ui::muted("Offline: using local projects only.").size(12.0));
        }
        if self.env.secure() == Some(false) {
            ui::banner(ui, WARNING(), |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label("Your GitHub sign-in is stored in a plain file instead of the system keyring.");
                    if ui.button("Move to secure storage").clicked() {
                        self.tab = Tab::Settings;
                        self.start_sign_in();
                    }
                });
            });
        }
        let mut dismiss_update = false;
        let mut do_update: Option<String> = None;
        let mut restart: Option<PathBuf> = None;
        match &self.update {
            UpdateState::None => {}
            UpdateState::Available(rel) => {
                ui::banner(ui, ACCENT(), |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(format!(
                            "StowLock {} is available (you have {APP_VERSION}).",
                            rel.label()
                        ));
                        if let Some(pkg) = &self.pacman_pkg {
                            ui.label(format!(
                                "Update it with your AUR helper ({pkg}), for example: paru -Syu"
                            ));
                        } else if ui
                            .button(RichText::new(format!("Update to {}", rel.label())).strong())
                            .clicked()
                        {
                            do_update = Some(rel.tag.clone());
                        }
                        if ui
                            .small_button(ui::icon::X)
                            .on_hover_text("Dismiss")
                            .clicked()
                        {
                            dismiss_update = true;
                        }
                    });
                    if !rel.notes.is_empty() {
                        egui::CollapsingHeader::new("What's new")
                            .id_salt("whats_new")
                            .show(ui, |ui| {
                                egui::ScrollArea::vertical()
                                    .max_height(200.0)
                                    .show(ui, |ui| {
                                        ui.add(egui::Label::new(rel.notes.as_str()).wrap());
                                    });
                            });
                    }
                });
            }
            UpdateState::Working(t) => {
                ui::banner(ui, ACCENT(), |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.spinner();
                        ui.label(format!("Downloading and checking StowLock {t}…"));
                    });
                });
            }
            UpdateState::Ready(t, p) => {
                ui::banner(ui, SUCCESS(), |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(format!("StowLock {t} is installed."));
                        if ui.button(RichText::new("Restart now").strong()).clicked() {
                            restart = Some(p.clone());
                        }
                    });
                });
            }
            UpdateState::Failed(e) => {
                let e = e.clone();
                ui::banner(ui, WARNING(), |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(format!(
                            "The update didn't install: {}. Your current version keeps working.",
                            e.lines().next().unwrap_or("")
                        ));
                        if ui
                            .small_button(ui::icon::X)
                            .on_hover_text("Dismiss")
                            .clicked()
                        {
                            dismiss_update = true;
                        }
                    });
                });
            }
        }
        if dismiss_update {
            self.update = UpdateState::None;
        }
        if let Some(t) = do_update {
            self.apply_update(t);
        }
        if let Some(p) = restart {
            // A clean exit: save settings and clear the "running" mark first,
            // so the new version doesn't think this one closed unexpectedly.
            self.save_config();
            report::mark_stopped(&self.paths);
            if updater::restart(&p).is_ok() {
                std::process::exit(0);
            }
            // The new version didn't start: this one keeps running.
            let _ = report::mark_running(&self.paths);
        }
        if let Some(prev) = self.rollback_offer.clone() {
            let mut go_back = false;
            ui::banner(ui, ERROR(), |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label(format!(
                        "StowLock {APP_VERSION} closed unexpectedly last time. Go back to {prev}?"
                    ));
                    if ui
                        .button(RichText::new(format!("Go back to {prev}")).strong())
                        .clicked()
                    {
                        go_back = true;
                    }
                    if ui
                        .small_button(ui::icon::X)
                        .on_hover_text("Dismiss")
                        .clicked()
                    {
                        self.rollback_offer = None;
                    }
                });
            });
            if go_back {
                self.start_rollback();
            }
        }
        if let Some(p) = self.crash_report.clone() {
            ui::banner(ui, ERROR(), |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label("StowLock closed unexpectedly last time.");
                    if ui::button(ui, ui::icon::BUG, "Report").clicked() {
                        self.tab = Tab::Report;
                    }
                    if ui.button("View the crash report").clicked() {
                        let _ = proc::xdg_open(&p.display().to_string());
                    }
                    if ui.button("Dismiss").clicked() {
                        self.crash_report = None;
                    }
                });
            });
        }
        if !self.installed {
            let older_installed = self
                .installed_version
                .clone()
                .filter(|v| instance::is_older(v, APP_VERSION));
            let newer_installed = self
                .installed_version
                .clone()
                .filter(|v| instance::is_older(APP_VERSION, v));
            let mut do_install = false;
            ui::banner(ui, ACCENT(), |ui| {
                ui.horizontal_wrapped(|ui| {
                    if let Some(v) = &newer_installed {
                        ui.label(format!(
                            "This is an older copy ({APP_VERSION}). The installed StowLock is {v}: open StowLock from your app menu to use it."
                        ));
                    } else if let Some(v) = &older_installed {
                        ui.label(format!(
                            "The installed StowLock is older ({v}). This copy is {APP_VERSION}."
                        ));
                        if ui.button("Replace the installed version").clicked() {
                            do_install = true;
                        }
                    } else {
                        ui.label("StowLock isn't installed yet, so it won't appear in your app menu.");
                        if ui.button("Install StowLock").clicked() {
                            do_install = true;
                        }
                    }
                    if ui.small_button(ui::icon::X).on_hover_text("Dismiss").clicked() {
                        self.installed = true;
                    }
                });
            });
            if do_install {
                match install::install(&self.install_inst) {
                    Ok(r) => {
                        for l in r.lines.iter().chain(r.warnings.iter()) {
                            self.log.info("install", l);
                        }
                        self.toast(
                            &r.lines
                                .first()
                                .cloned()
                                .unwrap_or_else(|| "Installed.".into()),
                        );
                        self.installed = true;
                    }
                    Err(e) => self.fail(&e, "install"),
                }
            }
        }
    }

    pub(super) fn error_card(&mut self, ui: &mut egui::Ui) {
        let Some((f, raw)) = self.error.clone() else {
            return;
        };
        ui::banner(ui, ERROR(), |ui| {
            ui.add(egui::Label::new(RichText::new(&f.message).color(ERROR()).strong()).wrap());
            ui.horizontal_wrapped(|ui| {
                match f.action {
                    Some(Action::SignIn) => {
                        if ui.button("Sign in").clicked() {
                            self.tab = Tab::Settings;
                            self.start_sign_in();
                        }
                    }
                    Some(Action::SetIdentity) => {
                        if ui.button("Set name and email").clicked() {
                            self.tab = Tab::Settings;
                        }
                    }
                    Some(Action::SwitchToUpdate) => {
                        if ui.button("Switch to Update").clicked() {
                            self.session.mode = Mode::Update;
                            self.error = None;
                        }
                    }
                    Some(Action::Retry) | Some(Action::BugReport) | None => {}
                }
                if ui.button("Bug Report").clicked() {
                    self.open_bug_report();
                }
                if ui.button("Dismiss").clicked() {
                    self.error = None;
                }
            });
            egui::CollapsingHeader::new(ui::muted("Details"))
                .id_salt("errdetails")
                .show(ui, |ui| {
                    ui.label(ui::mono(crate::secrets::redact_secrets(&raw)).size(12.0));
                });
        });
    }

    pub(super) fn stage_tab(&mut self, ui: &mut egui::Ui, hovering: bool) {
        self.error_card(ui);
        self.rust_notice(ui);
        // ---- drop zone
        let has_zip = self.session.zip.is_some() || self.session.stage.is_some();
        let (fill, stroke) = if hovering {
            (ACCENT().linear_multiply(0.12), ACCENT())
        } else {
            (ui::CARD(), BORDER())
        };
        let mut pick = false;
        let mut use_newer: Option<PathBuf> = None;
        egui::Frame::new()
            .fill(fill)
            .stroke(egui::Stroke::new(
                if hovering { 2.0_f32 } else { 1.0 },
                stroke,
            ))
            .corner_radius(egui::CornerRadius::same(16))
            .inner_margin(egui::Margin::same(if has_zip { 16 } else { 28 }))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                if has_zip {
                    ui.horizontal(|ui| {
                        let (r, _) =
                            ui.allocate_exact_size(egui::vec2(48.0, 48.0), egui::Sense::hover());
                        ui.painter().rect_filled(
                            r,
                            egui::CornerRadius::same(12),
                            ui::ACCENT_SOFT(),
                        );
                        let glyph = if self.session.zip.is_some() {
                            ui::icon::PACKAGE
                        } else {
                            ui::icon::FOLDER
                        };
                        ui.painter().text(
                            r.center(),
                            egui::Align2::CENTER_CENTER,
                            glyph,
                            egui::FontId::proportional(22.0),
                            ACCENT(),
                        );
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            pick |=
                                ui::button_enabled(ui, self.busy.is_none(), "", "Choose another…")
                                    .clicked();
                            ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
                                ui.spacing_mut().item_spacing.y = 2.0;
                                let (name, sub) = if let Some(z) = &self.session.zip {
                                    (
                                        z.file_name()
                                            .map(|n| n.to_string_lossy().to_string())
                                            .unwrap_or_default(),
                                        util::tilde(z),
                                    )
                                } else if let Some(st) = &self.session.stage {
                                    (
                                        util::tilde(&st.dir),
                                        "Loaded from your workspace".to_string(),
                                    )
                                } else {
                                    (String::new(), String::new())
                                };
                                ui.add(egui::Label::new(ui::semibold(&name, 15.0)).truncate())
                                    .on_hover_text(&name);
                                let meta = if let Some(st) = &self.session.stage {
                                    let n = crate::archive::count_archives(
                                        &self.config.archive_root(),
                                        &self.session.slug,
                                    );
                                    let mut m = format!(
                                        "{} · {} files · {}",
                                        st.stack().name(),
                                        st.files,
                                        util::human_size(st.size)
                                    );
                                    if n > 0 {
                                        m.push_str(&format!(
                                            " · {n} archived version{}",
                                            if n == 1 { "" } else { "s" }
                                        ));
                                    }
                                    m
                                } else {
                                    match &self.session.zip_info {
                                        Some(i) => format!("{} · {sub}", i.describe()),
                                        None => sub.clone(),
                                    }
                                };
                                ui.add(egui::Label::new(ui::muted(meta).size(13.0)).truncate())
                                    .on_hover_text(&sub);
                            });
                        });
                    });
                    // Make sure it's the zip you meant.
                    if self.session.stage.is_none() {
                        if let Some(newer) = self.session.newer_copy.clone() {
                            ui::banner(ui, WARNING(), |ui| {
                                ui.add(
                                    egui::Label::new(format!(
                                        "There's a newer copy of this download beside it: {}",
                                        newer
                                            .file_name()
                                            .map(|n| n.to_string_lossy().to_string())
                                            .unwrap_or_default()
                                    ))
                                    .wrap(),
                                );
                                if ui::button(ui, ui::icon::PACKAGE, "Use the newer copy").clicked() {
                                    use_newer = Some(newer.clone());
                                }
                            });
                        } else if self.session.same_zip {
                            ui::banner(ui, WARNING(), |ui| {
                                ui.add(
                                    egui::Label::new(
                                        "This is the same zip as last time (identical contents). If you downloaded a fixed one, choose it instead.",
                                    )
                                    .wrap(),
                                );
                            });
                        }
                    }
                } else {
                    ui.vertical_centered(|ui| {
                        let (r, _) =
                            ui.allocate_exact_size(egui::vec2(56.0, 56.0), egui::Sense::hover());
                        ui.painter().rect_filled(
                            r,
                            egui::CornerRadius::same(16),
                            ui::ACCENT_SOFT(),
                        );
                        ui.painter().text(
                            r.center(),
                            egui::Align2::CENTER_CENTER,
                            ui::icon::UPLOAD,
                            egui::FontId::proportional(26.0),
                            ACCENT(),
                        );
                        ui.add_space(6.0);
                        ui.label(ui::semibold(
                            if hovering {
                                "Drop the zip to load it"
                            } else {
                                "Choose a project .zip to get started"
                            },
                            17.0,
                        ));
                        ui.label(ui::muted("or drag it onto this window · Ctrl+O").size(13.0));
                        ui.add_space(6.0);
                        pick |= ui::primary(ui, "Choose a .zip…", self.busy.is_none()).clicked();
                    });
                }
            });
        if pick {
            self.pick_zip();
        }
        if let Some(z) = use_newer {
            self.load_zip(z);
        }
        if !has_zip {
            ui.add_space(8.0);
            ui.label(ui::muted("StowLock unpacks your zip into its own project folder, checks it, lets you try it, then pushes it to the right GitHub repo and files the zip away."));
            self.log_drawer(ui);
            return;
        }
        ui.add_space(8.0);
        self.routing_card(ui);
        ui.add_space(8.0);
        if !self.session.archive_only {
            self.details_card(ui);
            ui.add_space(8.0);
            self.results_card(ui);
        } else {
            ui::card(ui, |ui| {
                ui::section_title(ui, "ARCHIVE ONLY");
                ui.add(egui::Label::new("The zip is filed with this project. Nothing is extracted, checked or pushed to GitHub.").wrap());
                ui.add_space(4.0);
                ui.horizontal_wrapped(|ui| {
                    ui.label("Version");
                    let r = ui::named(
                        ui.add(
                            egui::TextEdit::singleline(&mut self.session.tag)
                                .hint_text("optional")
                                .desired_width(140.0)
                                .font(egui::TextStyle::Monospace),
                        ),
                        "Version",
                    );
                    if r.changed() {
                        self.session.tag_auto = false;
                    }
                });
                let note = match (
                    &self.session.found_version,
                    self.session.tag_auto,
                    self.session.tag.trim().is_empty(),
                ) {
                    (Some(v), true, false) => format!("Found {} in {}.", v.tag, v.source),
                    (_, _, true) => "No version found; the date and time will be used.".to_string(),
                    _ => "Using the version you typed.".to_string(),
                };
                ui.add(egui::Label::new(ui::muted(note).size(12.5)).wrap());
                let tag = self.session.tag.trim().to_string();
                if !tag.is_empty()
                    && crate::archive::already_archived(
                        &self.config.archive_root(),
                        &self.session.slug,
                        &tag,
                    )
                {
                    ui.add(egui::Label::new(RichText::new(format!("{tag} is already archived for this project. The new copy is kept alongside it (nothing is overwritten).")).color(WARNING()).size(12.5)).wrap());
                }
                ui.add_space(4.0);
                ui.horizontal_wrapped(|ui| {
                    ui.radio_value(
                        &mut self.session.archive_move,
                        true,
                        "Move (clears Downloads)",
                    );
                    ui.radio_value(&mut self.session.archive_move, false, "Copy");
                });
                if !self.archive_note_seen {
                    ui.label(ui::muted("Archived zips are kept exactly as downloaded, including any .env files.").size(12.0));
                }
            });
        }
        if let Some(out) = self.session.outcome.clone() {
            ui.add_space(8.0);
            self.after_push_cards(ui);
            self.completion_card(ui, &out);
        }
        self.log_drawer(ui);
    }

    pub(super) fn routing_card(&mut self, ui: &mut egui::Ui) {
        let mut slug_changed = false;
        let mut owner_changed = false;
        let staged = self.session.stage.is_some();
        ui::card(ui, |ui| {
            ui.horizontal(|ui| {
                ui::section_title(ui, "ROUTING");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let m = self.session.effective_mode();
                    let col = match m {
                        Mode::New => ACCENT(),
                        Mode::Update => SUCCESS(),
                        Mode::ArchiveOnly => MUTED(),
                    };
                    ui::badge(ui, m.badge(), col);
                    if self.session.channel == Channel::Beta && m != Mode::ArchiveOnly {
                        ui::badge(ui, "BETA", WARNING());
                    }
                });
            });
            let src = self
                .session
                .zip
                .as_ref()
                .and_then(|z| z.file_name())
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| "workspace".into());
            let dest = if self.session.archive_only {
                format!("~/…/_archive/{}/", self.session.slug)
            } else {
                format!(
                    "{}/{}",
                    util::tilde(&self.config.workspace()),
                    self.session.slug
                )
            };
            let remote = if self.session.archive_only {
                "not pushed".to_string()
            } else {
                format!(
                    "github.com/{}/{}",
                    if self.session.owner.is_empty() {
                        "<sign in>"
                    } else {
                        &self.session.owner
                    },
                    self.session.slug
                )
            };
            // One line per hop so long names shorten with … instead of
            // breaking mid-word.
            let remote_color = if self.session.archive_only {
                MUTED()
            } else {
                ACCENT()
            };
            egui::Grid::new("route_grid")
                .max_col_width((ui.available_width() - 130.0).max(160.0))
                .num_columns(2)
                .spacing([10.0, 4.0])
                .show(ui, |ui| {
                    for (label, text, color) in [
                        ("Zip", &src, TEXT()),
                        ("Folder", &dest, TEXT()),
                        ("GitHub", &remote, remote_color),
                    ] {
                        ui.label(ui::muted(label));
                        ui.add(egui::Label::new(ui::mono(text).color(color)).truncate())
                            .on_hover_text(text.as_str());
                        ui.end_row();
                    }
                });
            ui.add_space(4.0);
            egui::Grid::new("routing")
                .max_col_width((ui.available_width() - 130.0).max(160.0))
                .num_columns(2)
                .spacing([12.0, 8.0])
                .show(ui, |ui| {
                    ui.label("Repo name");
                    ui.horizontal_wrapped(|ui| {
                        // Only repos that are on GitHub now; local folders stand in
                        // only when GitHub's list couldn't be loaded.
                        let gh_list = self.github_repos(&self.session.owner).cloned();
                        let from_github = gh_list.is_some();
                        let mut names: Vec<String> = gh_list.unwrap_or_else(|| {
                            self.projects.iter().map(|p| p.slug.clone()).collect()
                        });
                        names.sort_by_key(|n| n.to_lowercase());
                        names.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
                        let field_w = (ui.available_width() - 150.0).clamp(140.0, 560.0);
                        let r = ui.add_enabled(
                            !staged,
                            egui::TextEdit::singleline(&mut self.session.slug)
                                .font(egui::TextStyle::Monospace)
                                .desired_width(field_w),
                        );
                        let r = ui::named(r, "Repository name");
                        if r.changed() {
                            // Existing repos keep their exact name; anything else is
                            // formatted as it's typed (no capitals, spaces → hyphens).
                            let typed = self.session.slug.clone();
                            if !names.iter().any(|n| n == &typed) {
                                self.session.slug = detect::format_typing(&typed);
                            }
                            slug_changed = true;
                        }
                        if r.lost_focus() && !names.iter().any(|n| n == &self.session.slug) {
                            let fin = detect::finish_name(&self.session.slug);
                            if fin != self.session.slug {
                                self.session.slug = fin;
                                slug_changed = true;
                            }
                        }
                        let owner_key = self.session.owner.to_ascii_lowercase();
                        let loading = self.repos_loading.contains(&owner_key);
                        let stale = self.repos_stale.contains(&owner_key);
                        let mut opened = false;
                        let mut reload = false;
                        ui.add_enabled_ui(!staged, |ui| {
                            let r = egui::ComboBox::from_id_salt("existing")
                                .selected_text("Existing…")
                                .width(110.0)
                                .show_ui(ui, |ui| {
                                    let note = if loading {
                                        "Loading from GitHub…".to_string()
                                    } else if !from_github {
                                        "Couldn't reach GitHub: showing folders on this computer"
                                            .to_string()
                                    } else if stale {
                                        "Couldn't reach GitHub: this list may be out of date"
                                            .to_string()
                                    } else {
                                        format!("On GitHub ({})", self.session.owner)
                                    };
                                    ui.label(ui::muted(note).size(12.0));
                                    if names.is_empty() && from_github {
                                        ui.label(ui::muted("No repos yet").size(12.5));
                                    }
                                    for n in names {
                                        if ui.selectable_label(n == self.session.slug, &n).clicked()
                                        {
                                            self.session.slug = n;
                                            slug_changed = true;
                                        }
                                    }
                                });
                            ui::named(r.response.clone(), "Existing repository");
                            opened = r.inner.is_some();
                            reload = ui
                                .add_enabled(!loading, egui::Button::new(ui::icon::REFRESH).small())
                                .on_hover_text("Reload your repos from GitHub")
                                .clicked();
                        });
                        if opened && !self.existing_open {
                            let o = self.session.owner.clone();
                            self.refresh_repos(&o, true);
                        }
                        self.existing_open = opened;
                        if reload {
                            let o = self.session.owner.clone();
                            self.refresh_repos(&o, true);
                        }
                    });
                    ui.end_row();
                    if !self.session.archive_only {
                        ui.label("Push to");
                        let mut owners: Vec<String> =
                            self.env.accounts.iter().map(|a| a.login.clone()).collect();
                        if let Some(l) = &self.env.login {
                            if !owners.contains(l) {
                                owners.insert(0, l.clone());
                            }
                        }
                        owners.extend(self.env.orgs.iter().cloned());
                        ui.add_enabled_ui(!staged, |ui| {
                            let cb = egui::ComboBox::from_id_salt("owner")
                                .selected_text(if self.session.owner.is_empty() {
                                    "Sign in first".to_string()
                                } else {
                                    self.session.owner.clone()
                                })
                                .show_ui(ui, |ui| {
                                    for o in owners {
                                        if ui
                                            .selectable_label(o == self.session.owner, &o)
                                            .clicked()
                                        {
                                            self.session.owner = o;
                                            owner_changed = true;
                                        }
                                    }
                                });
                            ui::named(cb.response, "Owner");
                        });
                        ui.end_row();
                    }
                });
            if let Some(problem) = detect::name_problem(&self.session.slug) {
                ui.add(egui::Label::new(RichText::new(problem).color(ERROR()).size(12.5)).wrap());
            }
            ui.horizontal_wrapped(|ui| {
                ui.add_enabled_ui(!staged, |ui| {
                    let r = ui.checkbox(
                        &mut self.session.archive_only,
                        "Archive only (already uploaded; nothing is pushed)",
                    );
                    if r.changed() && self.session.archive_only {
                        // Fill in the version found in the zip, if the user hasn't typed one.
                        if self.session.tag.trim().is_empty() || self.session.tag_auto {
                            if let Some(v) = &self.session.found_version {
                                self.session.tag = v.tag.clone();
                                self.session.tag_auto = true;
                            }
                        }
                    }
                });
            });
            if self.session.mode == Mode::New
                && self.session.local_exists
                && self.session.local_is_ours
                && !self.session.archive_only
                && !staged
            {
                ui.add(
                    egui::Label::new(ui::muted(
                        "This replaces your earlier staging of this project that wasn't pushed. The old copy goes to the Trash.",
                    ))
                    .wrap(),
                );
            } else if self.session.deleted_on_github && !self.session.archive_only && !staged {
                ui::banner(ui, WARNING(), |ui| {
                    ui.label(ui::semibold(
                        format!(
                            "This repo was deleted on GitHub: github.com/{}/{}",
                            self.session.owner, self.session.slug
                        ),
                        14.0,
                    ));
                    ui.add(
                        egui::Label::new(ui::muted(
                            "It isn't an update any more. Create it again, or type another name above.",
                        ))
                        .wrap(),
                    );
                    ui.checkbox(
                        &mut self.session.replace_local,
                        "Create it again from this zip (the old folder goes to the Trash)",
                    );
                });
            } else if self.session.mode == Mode::New
                && self.session.local_exists
                && !self.session.archive_only
                && !staged
            {
                ui.colored_label(
                    WARNING(),
                    "A folder with this name already exists in your workspace but isn't on GitHub.",
                );
                ui.checkbox(
                    &mut self.session.replace_local,
                    "Replace the existing folder (moved to the Trash)",
                );
            }
            if self.session.mode == Mode::Update && !self.session.archive_only {
                let last = History::load(&self.paths).last_for(&self.session.slug);
                let tag = self.existing_tags().into_iter().next();
                let mut txt = String::from("Update detected: this goes to the existing repo.");
                if let Some(t) = tag {
                    txt.push_str(&format!(" Latest tag {t}."));
                }
                if let Some(l) = last {
                    txt.push_str(&format!(" Last pushed {} {}.", l.date(), l.time()));
                }
                ui.colored_label(SUCCESS(), txt);
            }
        });
        if slug_changed || owner_changed {
            self.recompute_mode();
            if owner_changed && !self.session.owner.is_empty() {
                let owner = self.session.owner.clone();
                self.refresh_repos(&owner, true);
            }
        }
    }

    pub(super) fn details_card(&mut self, ui: &mut egui::Ui) {
        let mode = self.session.mode;
        let tags = self.existing_tags();
        ui::card(ui, |ui| {
            ui::section_title(ui, "DETAILS");
            egui::Grid::new("details")
                .max_col_width((ui.available_width() - 130.0).max(160.0))
                .num_columns(2)
                .spacing([12.0, 8.0])
                .show(ui, |ui| {
                    if mode == Mode::New {
                        ui.label("Description");
                        ui::named(ui.add(
                            egui::TextEdit::singleline(&mut self.session.description)
                                .desired_width(f32::INFINITY),
                        ), "Description");
                        ui.end_row();
                        ui.label("Topics");
                        ui::named(ui.add(
                            egui::TextEdit::singleline(&mut self.session.topics)
                                .hint_text("rust, cli, linux")
                                .desired_width(f32::INFINITY),
                        ), "Topics");
                        ui.end_row();
                        ui.label("License");
                        let cb = egui::ComboBox::from_id_salt("license")
                            .selected_text(match self.session.license.as_str() {
                                "MIT" => "MIT",
                                "GPL-3.0" => "GNU GPL v3",
                                _ => "None",
                            })
                            .show_ui(ui, |ui| {
                                ui.selectable_value(
                                    &mut self.session.license,
                                    "none".to_string(),
                                    "None",
                                );
                                ui.selectable_value(
                                    &mut self.session.license,
                                    "MIT".to_string(),
                                    "MIT",
                                );
                                ui.selectable_value(
                                    &mut self.session.license,
                                    "GPL-3.0".to_string(),
                                    "GNU GPL v3",
                                );
                            });
ui::named(cb.response, "License");
                        ui.end_row();
                        ui.label("Visibility");
                        ui.horizontal_wrapped(|ui| {
                            ui.selectable_value(&mut self.session.private, true, "Private");
                            ui.selectable_value(&mut self.session.private, false, "Public");
                        });
                        ui.end_row();
                    }
                    ui.label("Version");
                    let mut new_tag: Option<String> = None;
                    ui.vertical(|ui| {
                        ui.horizontal_wrapped(|ui| {
                            let sel = usize::from(self.session.channel == Channel::Beta);
                            if let Some(i) = ui::segmented(ui, &["Stable", "Beta"], sel, 180.0) {
                                self.session.channel = if i == 1 { Channel::Beta } else { Channel::Stable };
                            }
                            ui::named(ui.add(
                                egui::TextEdit::singleline(&mut self.session.tag)
                                    .hint_text("optional, e.g. v0.1.0")
                                    .desired_width(170.0)
                                    .font(egui::TextStyle::Monospace),
                            ), "Version");
                        });
                        let prefix = self.config.tag_prefix.clone();
                        if let Some(fv) = &self.session.found_version {
                            if self.session.tag.trim() == fv.tag {
                                ui.label(ui::muted(format!("From {} in the zip", fv.source)).size(12.5));
                            }
                        }
                        let (ls, lb) = version::latest(&tags, &prefix);
                        let latest = match (&ls, &lb) {
                            (None, None) => "No releases on GitHub yet".to_string(),
                            (Some(s), None) => format!("Latest on GitHub: {s}"),
                            (None, Some(b)) => format!("Latest on GitHub: {b} (beta)"),
                            (Some(s), Some(b)) => format!("Latest on GitHub: {s} · {b} (beta)"),
                        };
                        ui.label(ui::muted(latest).size(12.5));
                        let (fix, feature) = version::next_choices(&tags, self.session.channel, &prefix);
                        ui.horizontal_wrapped(|ui| {
                            if self.session.tag.trim() != fix
                                && ui.small_button(format!("Next fix: {fix}")).clicked()
                            {
                                new_tag = Some(fix.clone());
                            }
                            if self.session.tag.trim() != feature
                                && ui.small_button(format!("Next feature: {feature}")).clicked()
                            {
                                new_tag = Some(feature.clone());
                            }
                        });
                        if self.session.outcome.is_none() {
                            if let Some(newer) = version::newer_existing(&self.session.tag, &prefix, &tags) {
                                ui::banner(ui, WARNING(), |ui| {
                                    ui.add(egui::Label::new(format!(
                                        "{} counts as older than {newer}, so people on {newer} won't be offered it.",
                                        self.session.tag.trim()
                                    )).wrap());
                                    ui.horizontal_wrapped(|ui| {
                                        if ui::button(ui, "", &format!("Use {fix}")).clicked() {
                                            new_tag = Some(fix.clone());
                                        }
                                        if ui::button(ui, "", &format!("Use {feature}")).clicked() {
                                            new_tag = Some(feature.clone());
                                        }
                                    });
                                });
                            }
                        }
                    });
                    if let Some(t) = new_tag {
                        self.session.tag = t;
                    }
                    if self.session.tag != self.session.last_tag {
                        self.session.last_tag = self.session.tag.clone();
                        if let Some(v) = version::parse_tag(&self.session.tag, &self.config.tag_prefix) {
                            self.session.channel = if v.pre.is_empty() { Channel::Stable } else { Channel::Beta };
                        }
                        if !self.session.commit_edited {
                            self.session.commit_msg =
                                version::default_commit_message(mode == Mode::New, &self.session.tag);
                        }
                    }
                    ui.end_row();
                    ui.label("Commit message");
                    if ui::named(
                        ui.add(
                            egui::TextEdit::singleline(&mut self.session.commit_msg)
                                .desired_width(f32::INFINITY),
                        ),
                        "Commit message",
                    )
                    .changed()
                    {
                        self.session.commit_edited = true;
                    }
                    ui.end_row();
                    ui.label("After push");
                    ui.horizontal_wrapped(|ui| {
                        ui.checkbox(&mut self.session.archive_after, "Archive the zip");
                        ui.add_enabled_ui(self.session.archive_after, |ui| {
                            ui.radio_value(&mut self.session.archive_move, true, "Move");
                            ui.radio_value(&mut self.session.archive_move, false, "Copy");
                        });
                    });
                    ui.end_row();
                });
            if self.session.outcome.is_some() {
                // Pushed: the tag exists because this push just made it.
                let next = version::suggest(&tags, self.session.channel, &self.config.tag_prefix);
                ui.label(
                    RichText::new(format!(
                        "Pushed as {}. The next update would be {next}.",
                        self.session.tag
                    ))
                    .color(MUTED())
                    .size(12.5),
                );
            } else if let Err(e) = version::validate(
                &self.session.tag,
                self.session.channel,
                &self.config.tag_prefix,
                &tags,
            ) {
                ui.label(RichText::new(e).color(ERROR()).size(12.5));
            }
            if mode == Mode::New && !self.session.private {
                ui.label(
                    RichText::new(
                        "Public: anyone can see this repo. Secret-scan hits can't be overridden.",
                    )
                    .color(WARNING())
                    .size(12.5),
                );
            }
        });
    }

    pub(super) fn results_card(&mut self, ui: &mut egui::Ui) {
        let steps = self.log.steps();
        let Some(st) = self.session.stage.clone() else {
            let working = self.busy.is_some();
            ui::card_active(ui, working, |ui| {
                ui::section_title(ui, "PIPELINE");
                ui::stepper(ui, &STEPS, &steps, &WAITING);
                if self.busy.is_some() {
                    ui.horizontal_wrapped(|ui| {
                        ui.spinner();
                        ui.label(self.busy.clone().unwrap_or_default());
                    });
                } else {
                    ui.label(ui::muted("Click Stage & check to unpack the zip into its project folder and run the checks. Nothing goes to GitHub yet."));
                }
            });
            return;
        };
        ui::card(ui, |ui| {
            ui::section_title(ui, "PIPELINE");
            ui::stepper(ui, &STEPS, &steps, &WAITING);
            for n in st.notes.iter().chain(st.cleanup.lines().iter()) {
                ui.label(
                    RichText::new(format!("{} {n}", ui::icon::CHECK))
                        .color(SUCCESS())
                        .size(13.0),
                );
            }
            if let Some(ch) = &st.changes {
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new(format!("Changes: {}", ch.summary())).strong());
                    if ch.total() > 0
                        && ui
                            .small_button(if self.session.show_changes {
                                "Hide files"
                            } else {
                                "Show files"
                            })
                            .clicked()
                    {
                        self.session.show_changes = !self.session.show_changes;
                    }
                });
                if self.session.show_changes {
                    for f in &ch.added {
                        ui.label(ui::mono(format!("+ {f}")).color(SUCCESS()));
                    }
                    for f in &ch.changed {
                        ui.label(ui::mono(format!("~ {f}")).color(WARNING()));
                    }
                    for f in &ch.removed {
                        ui.label(ui::mono(format!("− {f}")).color(ERROR()));
                    }
                }
            }
            for (f, n) in &st.warn_files {
                ui.colored_label(
                    WARNING(),
                    format!("Large file: {f} ({})", util::human_size(*n)),
                );
            }
            for (f, n) in &st.block_files {
                ui.colored_label(
                    ERROR(),
                    format!(
                        "Over GitHub's 100 MB limit: {f} ({}). Remove it or add it to .gitignore.",
                        util::human_size(*n)
                    ),
                );
            }
        });
        ui.add_space(8.0);
        // ---- checks
        let working = self.busy.is_some();
        ui::card_active(ui, working, |ui| {
            ui::section_title(ui, "CHECKS");
            // Text here can't be selected with the mouse (that would copy it
            // unhidden): the Copy buttons hide private things first.
            ui.style_mut().interaction.selectable_labels = false;
            match self.session.checks.clone() {
                None => {
                    ui.horizontal_wrapped(|ui| {
                        if self.busy.is_some() {
                            ui.spinner();
                            ui.label("Running tests and the secret scan…");
                            if ui.button("Cancel").clicked() {
                                self.cancel.store(true, Ordering::Relaxed);
                            }
                        } else {
                            ui.label(ui::muted("Checks haven't run yet."));
                        }
                    });
                    // The newest output, so a slow or stuck step is easy to see.
                    if self.busy.is_some() {
                        let lines = proc::live_lines();
                        if !lines.is_empty() {
                            ui::inset(ui, |ui| {
                                for l in &lines {
                                    ui.add(
                                        egui::Label::new(ui::mono(l).size(12.0).color(MUTED()))
                                            .truncate(),
                                    );
                                }
                            });
                        }
                        ui.ctx()
                            .request_repaint_after(std::time::Duration::from_millis(400));
                    }
                }
                Some(c) => {
                    let (txt, col) = match c.tests.status {
                        TestStatus::Passed => ("Tests passed", SUCCESS()),
                        TestStatus::Failed => ("Tests failed", ERROR()),
                        TestStatus::Skipped if c.tests.summary.starts_with("Can't run here") => {
                            ("Can't run here", WARNING())
                        }
                        TestStatus::Skipped => ("Tests skipped", MUTED()),
                        TestStatus::Cancelled => ("Tests cancelled", WARNING()),
                    };
                    let cant_run = txt == "Can't run here";
                    let rust_project = self
                        .session
                        .stage
                        .as_ref()
                        .is_some_and(|st| st.stack() == crate::detect::Stack::Rust);
                    if let (
                        true,
                        Some(crate::rustcheck::RustState::UpdateAvailable { have, newest }),
                    ) = (rust_project, &self.rust)
                    {
                        ui.add(
                            egui::Label::new(ui::muted(format!(
                                "Your Rust ({have}) is older than the newest ({newest}), which GitHub uses: it may catch warnings this check didn't. Update Rust and Re-check to be sure."
                            )).size(12.5))
                            .wrap(),
                        );
                    }
                    ui.horizontal_wrapped(|ui| {
                        ui::badge(ui, txt, col);
                        ui.label(ui::muted(format!(
                            "{} · {:.1}s",
                            c.tests.summary,
                            c.tests.duration.as_secs_f32()
                        )));
                    });
                    if c.partial {
                        ui::banner(ui, WARNING(), |ui| {
                            ui.add(
                                egui::Label::new(
                                    "Only the failed job was re-checked. Run the full check before pushing.",
                                )
                                .wrap(),
                            );
                            if self.busy.is_none()
                                && ui::primary(
                                    ui,
                                    &ui::with_icon(ui::icon::REFRESH, "Run the full check"),
                                    true,
                                )
                                .clicked()
                            {
                                self.recheck();
                            }
                        });
                    }
                    if cant_run {
                        ui.add(
                            egui::Label::new(
                                "Something this step needs isn't on this computer, so it couldn't run here. That doesn't mean your project is broken. Install what's missing and Re-check, or push: GitHub runs this step with everything installed, and StowLock watches the build.",
                            )
                            .wrap(),
                        );
                        if c.tests.summary.contains("`") {
                            ui.add(
                                egui::Label::new(ui::muted(
                                    "If it's a Python tool, listing it in the project's requirements file lets StowLock install it here too, in its own environment.",
                                ).size(12.5))
                                .wrap(),
                            );
                        }
                    }
                    if c.tests.status == TestStatus::Passed
                        && c.tests.summary.contains("can't run here")
                    {
                        ui.add(
                            egui::Label::new(ui::muted(
                                "Everything that could run here passed. GitHub runs the rest, and StowLock watches the build. Nothing to install unless you want to run those steps here too.",
                            ).size(12.5))
                            .wrap(),
                        );
                    }
                    if let Some(h) = c
                        .tests
                        .hint
                        .clone()
                        .filter(|h| h.starts_with("sudo pacman -S"))
                    {
                        let packages = crate::preflight::pacman_packages(&h);
                        ui.horizontal_wrapped(|ui| {
                            self.install_button(ui, &packages, true);
                        });
                    }
                    if let Some(h) = &c.tests.hint {
                        ui.horizontal_wrapped(|ui| {
                            if c.tests.status == TestStatus::Passed || cant_run {
                                ui.label(ui::muted("Optional, to run it here too:").size(12.5));
                            }
                            ui.add(egui::Label::new(ui::mono(h)).wrap());
                            if ui.small_button("Copy").clicked() {
                                ui.ctx().copy_text(h.clone());
                            }
                        });
                    }
                    if !c.tests.tail.is_empty() {
                        egui::CollapsingHeader::new("Test output")
                            .id_salt("testout")
                            .default_open(c.tests.status == TestStatus::Failed)
                            .show(ui, |ui| {
                                egui::ScrollArea::vertical()
                                    .max_height(180.0)
                                    .id_salt("testscroll")
                                    .show(ui, |ui| {
                                        ui.label(ui::mono(&c.tests.tail).size(12.0));
                                    });
                                // Re-check only what failed: quicker while fixing.
                                let failed = crate::preflight::failed_jobs(&c.tests.tail);
                                let several = c.tests.summary.contains(" · jobs: ")
                                    || c.partial;
                                if c.tests.status == TestStatus::Failed && several && self.busy.is_none() {
                                    for job in &failed {
                                        if ui::button(ui, ui::icon::REFRESH, &format!("Re-check \"{job}\" only"))
                                            .on_hover_text("Runs just this job again: quicker while you fix it. The full check runs before you push.")
                                            .clicked()
                                        {
                                            self.recheck_jobs(vec![job.clone()]);
                                        }
                                    }
                                }
                                if ui
                                    .small_button(ui::with_icon(ui::icon::COPY, "Copy output"))
                                    .on_hover_text("Copies the result and output, with your name, folders and other private things hidden")
                                    .clicked()
                                {
                                    let text = format!("{}\n{}", c.tests.summary, c.tests.tail);
                                    ui.ctx().copy_text(report::redact(&text, &self.redact_ctx()));
                                    self.toast("Copied, with private things hidden.");
                                }
                            });
                    }
                    if c.tests.status == TestStatus::Failed
                        || c.tests.status == TestStatus::Cancelled
                    {
                        // Safer choices first; pushing anyway comes last.
                        let mut pick = false;
                        let mut recheck = false;
                        let mut report = false;
                        let mut open = false;
                        let mut copied = false;
                        ui::banner(ui, WARNING(), |ui| {
                            ui.label(ui::semibold("What to do next", 14.5));
                            if c.tests.summary.contains(crate::preflight::MACHINE_SPECIFIC) {
                                ui.add(egui::Label::new(RichText::new(
                                    "These failures look specific to this computer, not the project: they mention a running program or your home folder. They'll likely pass on GitHub. Quit the running copy and Re-check, or push anyway.",
                                ).color(WARNING())).wrap());
                            }
                            ui.add(egui::Label::new(
                                "1. Copy for AI, paste it into your AI chat, and load the fixed zip it gives you. The failed attempt is kept, never lost.",
                            ).wrap());
                            ui.add(
                                egui::Label::new(
                                    "2. Or fix it in the project folder yourself, then Re-check.",
                                )
                                .wrap(),
                            );
                            ui.add(
                                egui::Label::new(
                                    "3. If it looks like StowLock's fault, send a bug report.",
                                )
                                .wrap(),
                            );
                            ui.horizontal_wrapped(|ui| {
                                if ui::primary(ui, &ui::with_icon(ui::icon::COPY, "Copy for AI"), true)
                                    .on_hover_text("Copies the failed tests and their messages, with a request to fix the project")
                                    .clicked()
                                {
                                    let lines: Vec<String> = c.tests.tail.lines().map(|l| l.to_string()).collect();
                                    let prompt = report::project_fix_prompt(&c.tests.summary, &lines);
                                    let prompt = report::redact(&prompt, &self.redact_ctx());
                                    ui.ctx().copy_text(prompt);
                                    copied = true;
                                }
                                pick |=
                                    ui::button(ui, ui::icon::UPLOAD, "Load a fixed zip…").clicked();
                                open |=
                                    ui::button(ui, ui::icon::FOLDER_OPEN, "Open folder").clicked();
                                recheck |= ui::button(ui, ui::icon::REFRESH, "Re-check").clicked();
                                report |= ui::button(ui, ui::icon::BUG, "Bug report").clicked();
                            });
                            ui.add_space(2.0);
                            ui.checkbox(
                                &mut self.session.override_tests,
                                "Push anyway: I know why the tests fail and it's safe",
                            );
                        });
                        if copied {
                            self.toast(
                                "Copied. Paste it into your AI chat, then load the fixed zip.",
                            );
                        }
                        if pick {
                            self.pick_zip();
                        }
                        if open {
                            let _ = proc::xdg_open(&st.dir.display().to_string());
                        }
                        if recheck {
                            self.recheck();
                        }
                        if report {
                            self.open_bug_report();
                        }
                    }
                    ui.add_space(4.0);
                    if c.secrets.is_empty() {
                        ui.horizontal_wrapped(|ui| {
                            ui::badge(ui, "0 secrets found", SUCCESS());
                            if c.ignored_secrets > 0 {
                                ui.label(ui::muted(format!(
                                    "({} marked not a secret)",
                                    c.ignored_secrets
                                )));
                            }
                        });
                    } else {
                        ui::badge(
                            ui,
                            &format!(
                                "{} possible secret{}",
                                c.secrets.len(),
                                if c.secrets.len() == 1 { "" } else { "s" }
                            ),
                            ERROR(),
                        );
                        let mut mark: Option<String> = None;
                        for h in &c.secrets {
                            ui.horizontal_wrapped(|ui| {
                                ui.label(ui::mono(format!("{}:{}", h.file, h.line)));
                                ui.label(ui::muted(format!("{} {}", h.kind, h.masked)));
                                if ui.small_button("Not a secret").clicked() {
                                    mark = Some(h.id.clone());
                                }
                            });
                        }
                        ui.label(RichText::new("If this key was ever pushed before, removing it isn't enough. Revoke it and make a new one.").color(WARNING()).size(12.5));
                        if let Some(id) = mark {
                            let slug = self.session.slug.clone();
                            let _ = ProjectsState::update(&self.paths, &slug, |p| {
                                p.not_secret.push(id.clone())
                            });
                            if let Some(c) = self.session.checks.as_mut() {
                                c.secrets.retain(|h| h.id != id);
                                c.ignored_secrets += 1;
                            }
                        }
                    }
                }
            }
        });
        ui.add_space(8.0);
        // ---- try it
        ui::card(ui, |ui| {
            ui::section_title(ui, "TRY IT BEFORE PUSHING");
            ui.horizontal_wrapped(|ui| {
                ui.label("Run command");
                let r = ui::named(
                    ui.add(
                        egui::TextEdit::singleline(&mut self.session.run_cmd)
                            .font(egui::TextStyle::Monospace)
                            .desired_width(260.0),
                    ),
                    "Run command",
                );
                if r.lost_focus() {
                    let slug = self.session.slug.clone();
                    let cmd = self.session.run_cmd.clone();
                    let _ =
                        ProjectsState::update(&self.paths, &slug, |p| p.run_command = Some(cmd));
                }
            });
            ui.horizontal_wrapped(|ui| {
                if self.run_child.is_none() {
                    if ui::button(ui, ui::icon::PLAY, "Run").clicked() {
                        match pipeline::run_in_terminal(
                            &st.dir,
                            &self.session.run_cmd,
                            self.session.pass_env,
                        ) {
                            Ok(c) => {
                                self.run_child = Some(c);
                                HealthState::record(&self.paths, "Run/Editor", true, "");
                            }
                            Err(e) => {
                                HealthState::record(&self.paths, "Run/Editor", false, &e);
                                self.fail(&e, "run");
                            }
                        }
                    }
                } else if ui::button(ui, ui::icon::X, "Stop").clicked() {
                    if let Some(mut c) = self.run_child.take() {
                        let _ = c.kill();
                    }
                }
                if ui::button(ui, ui::icon::FOLDER_OPEN, "Open folder").clicked() {
                    let _ = proc::xdg_open(&st.dir.display().to_string());
                }
                if ui::button(ui, ui::icon::PEN, "Open in editor").clicked() {
                    match pipeline::open_in_editor(&st.dir) {
                        Ok(_) => HealthState::record(&self.paths, "Run/Editor", true, ""),
                        Err(e) => {
                            HealthState::record(&self.paths, "Run/Editor", false, &e);
                            self.fail(&e, "editor");
                        }
                    }
                }
                if ui
                    .add_enabled(
                        self.busy.is_none(),
                        egui::Button::new(ui::semibold(
                            ui::with_icon(ui::icon::REFRESH, "Re-check"),
                            14.0,
                        ))
                        .min_size(egui::vec2(0.0, 36.0)),
                    )
                    .clicked()
                {
                    self.recheck();
                }
            });
            let mut pe = self.session.pass_env;
            if ui
                .checkbox(
                    &mut pe,
                    "Pass my environment (keys and tokens) to Run and tests",
                )
                .changed()
            {
                self.session.pass_env = pe;
                let slug = self.session.slug.clone();
                let _ = ProjectsState::update(&self.paths, &slug, |p| p.pass_env = pe);
            }
            ui.label(ui::muted("Run executes this project's code on your computer.").size(12.0));
        });
    }

    pub(super) fn completion_card(&mut self, ui: &mut egui::Ui, out: &PushOutcome) {
        let color = if out.nothing_to_push {
            MUTED()
        } else {
            SUCCESS()
        };
        ui::banner(ui, color, |ui| {
            if out.nothing_to_push {
                ui.label(
                    RichText::new("No changes to push. GitHub already has this version.").strong(),
                );
            } else {
                let t = out
                    .tag
                    .clone()
                    .map(|t| format!(" as {t}"))
                    .unwrap_or_default();
                ui.label(
                    RichText::new(format!(
                        "Pushed {}/{}{t}",
                        self.session.owner, self.session.slug
                    ))
                    .color(SUCCESS())
                    .strong()
                    .size(15.0),
                );
            }
            if let Some(a) = &out.archived {
                ui.label(ui::muted(format!("Zip archived to {}", util::tilde(a))));
            }
            for w in &out.warnings {
                ui.colored_label(WARNING(), w);
            }
            ui.horizontal_wrapped(|ui| {
                if ui
                    .button(RichText::new("Open on GitHub").strong())
                    .clicked()
                {
                    let _ = proc::xdg_open(&out.url);
                }
                if ui.button("Open Folder").clicked() {
                    let _ = proc::xdg_open(
                        &self
                            .config
                            .workspace()
                            .join(&self.session.slug)
                            .display()
                            .to_string(),
                    );
                }
                if ui.button("Start another").clicked() {
                    self.session = Session::new(&self.config);
                    self.log.begin_run("Ready");
                }
            });
        });
    }
}
