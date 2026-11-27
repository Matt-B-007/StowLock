// SPDX-License-Identifier: GPL-3.0-or-later
//! Help: the Help menu, keyboard shortcuts, the command palette (Ctrl+K) and
//! the tour.
use super::*;

/// Something the palette, the Help menu or a shortcut can do.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(super) enum Cmd {
    Go(Tab),
    OpenZip,
    CheckUpdates,
    SelfTest,
    Shortcuts,
    Tour,
}

/// Every command: (what it's called, its shortcut, what it does).
pub(super) const COMMANDS: [(&str, &str, Cmd); 12] = [
    ("Open a project zip", "Ctrl+O", Cmd::OpenZip),
    ("Go to Stage", "Ctrl+1", Cmd::Go(Tab::Stage)),
    ("Go to Projects", "Ctrl+2", Cmd::Go(Tab::Projects)),
    ("Go to History", "Ctrl+3", Cmd::Go(Tab::History)),
    ("Go to Health", "Ctrl+4", Cmd::Go(Tab::Health)),
    ("Write a bug report", "Ctrl+5", Cmd::Go(Tab::Report)),
    ("Open Settings", "Ctrl+,", Cmd::Go(Tab::Settings)),
    ("Check for updates", "", Cmd::CheckUpdates),
    ("Run the self-test", "", Cmd::SelfTest),
    ("Keyboard shortcuts", "F1", Cmd::Shortcuts),
    ("Take the tour", "", Cmd::Tour),
    ("Push (on the review screen)", "Ctrl+Enter", Cmd::Shortcuts),
];

/// The tour: one card per part of the window.
pub(super) const TOUR: [(&str, &str); 6] = [
    (
        "Welcome to StowLock",
        "StowLock turns a project zip into a tested GitHub release. This short tour shows where things are. You can take it again any time from Help.",
    ),
    (
        "Stage",
        "Drop a zip here (or press Ctrl+O). StowLock unpacks it into its project folder, runs the same checks GitHub will, and scans for secrets before anything is pushed.",
    ),
    (
        "Projects",
        "Everything in your workspace and on GitHub, plus any stagings you didn't push. Nothing is lost until you delete it.",
    ),
    (
        "History and Health",
        "History lists every push and archive. Health shows what's working, the tools you have, and anything that needs fixing.",
    ),
    (
        "Bug reports",
        "If something goes wrong, the Bug report screen gathers what's needed with your name, folders and other private things hidden. Save or copy it.",
    ),
    (
        "Shortcuts",
        "Press Ctrl+K to find any command by typing, or F1 to see every shortcut.",
    ),
];

/// The command palette's state while it's open.
#[derive(Default)]
pub(super) struct Palette {
    pub(super) query: String,
    pub(super) selected: usize,
}

/// Commands whose name contains every word typed (any order, any case).
pub(super) fn matching(query: &str) -> Vec<(&'static str, &'static str, Cmd)> {
    let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    COMMANDS
        .iter()
        .filter(|(name, keys, _)| {
            let hay = format!("{} {}", name.to_lowercase(), keys.to_lowercase());
            words.iter().all(|w| hay.contains(w.as_str()))
        })
        .copied()
        .collect()
}

impl App {
    pub(super) fn run_cmd(&mut self, cmd: Cmd) {
        self.palette = None;
        match cmd {
            Cmd::Go(Tab::Report) => {
                self.open_bug_report();
                self.tab = Tab::Report;
            }
            Cmd::Go(Tab::Projects) => {
                self.refresh_attempts();
                self.tab = Tab::Projects;
            }
            Cmd::Go(t) => self.tab = t,
            Cmd::OpenZip => self.pick_zip(),
            Cmd::CheckUpdates => self.check_updates(true),
            Cmd::SelfTest => {
                self.tab = Tab::Health;
                self.run_selftest();
            }
            Cmd::Shortcuts => self.show_shortcuts = true,
            Cmd::Tour => self.tour_step = Some(0),
        }
    }

    /// Keyboard shortcuts that work everywhere.
    pub(super) fn shortcuts(&mut self, ctx: &egui::Context) {
        use egui::{Key, Modifiers};
        let (k, f1, esc, comma, digit) = ctx.input_mut(|i| {
            let digits = [Key::Num1, Key::Num2, Key::Num3, Key::Num4, Key::Num5];
            (
                i.consume_key(Modifiers::COMMAND, Key::K),
                i.consume_key(Modifiers::NONE, Key::F1),
                i.key_pressed(Key::Escape),
                i.consume_key(Modifiers::COMMAND, Key::Comma),
                digits
                    .iter()
                    .position(|d| i.consume_key(Modifiers::COMMAND, *d)),
            )
        });
        if k {
            self.palette = match self.palette {
                Some(_) => None,
                None => Some(Palette::default()),
            };
        }
        if f1 {
            self.show_shortcuts = !self.show_shortcuts;
        }
        if comma {
            self.run_cmd(Cmd::Go(Tab::Settings));
        }
        if let Some(n) = digit {
            let tabs = [
                Tab::Stage,
                Tab::Projects,
                Tab::History,
                Tab::Health,
                Tab::Report,
            ];
            self.run_cmd(Cmd::Go(tabs[n]));
        }
        if esc {
            if self.palette.is_some() {
                self.palette = None;
            } else if self.show_shortcuts {
                self.show_shortcuts = false;
            } else if self.tour_step.is_some() {
                self.end_tour();
            }
        }
    }

    pub(super) fn end_tour(&mut self) {
        self.tour_step = None;
        // Not signed in yet: the tour ends on the GitHub account panel.
        if !self.env.ready() {
            self.tab = Tab::Settings;
        }
        if !self.config.tour_seen {
            self.config.tour_seen = true;
            self.save_config();
        }
    }

    /// The Help menu at the bottom of the sidebar.
    pub(super) fn help_menu(&mut self, ui: &mut egui::Ui, compact: bool) {
        let resp = ui::nav_item(ui, ui::icon::CIRCLE_HELP, "Help", false, None, compact)
            .on_hover_text("Shortcuts, the tour, bug reports and about");
        let mut pick = None;
        egui::Popup::menu(&resp).show(|ui| {
            ui.set_min_width(220.0);
            if ui.button("Find a command…  Ctrl+K").clicked() {
                self.palette = Some(Palette::default());
            }
            if ui.button("Keyboard shortcuts  F1").clicked() {
                pick = Some(Cmd::Shortcuts);
            }
            if ui.button("Take the tour").clicked() {
                pick = Some(Cmd::Tour);
            }
            ui.separator();
            if ui.button("Write a bug report").clicked() {
                pick = Some(Cmd::Go(Tab::Report));
            }
            if ui.button("Check for updates").clicked() {
                pick = Some(Cmd::CheckUpdates);
            }
            ui.separator();
            ui.label(ui::muted(format!("StowLock {APP_VERSION} · GPL-3.0-or-later")).size(12.0));
        });
        if let Some(c) = pick {
            self.run_cmd(c);
        }
    }

    /// The palette, the shortcuts list and the tour.
    pub(super) fn help_windows(&mut self, ctx: &egui::Context) {
        if self.palette.is_some() {
            self.palette_window(ctx);
        }
        if self.show_shortcuts {
            let mut open = true;
            ui_window(ctx, "Keyboard shortcuts", 420.0)
                .open(&mut open)
                .show(ctx, |ui| {
                    egui::Grid::new("shortcuts")
                        .num_columns(2)
                        .spacing([24.0, 8.0])
                        .show(ui, |ui| {
                            ui.label(ui::mono("Ctrl+K").color(ACCENT()));
                            ui.label("Find any command");
                            ui.end_row();
                            for (name, keys, _) in COMMANDS.iter().filter(|c| !c.1.is_empty()) {
                                ui.label(ui::mono(*keys).color(ACCENT()));
                                ui.label(*name);
                                ui.end_row();
                            }
                            ui.label(ui::mono("Esc").color(ACCENT()));
                            ui.label("Close this, the palette or the tour");
                            ui.end_row();
                        });
                });
            self.show_shortcuts &= open;
        }
        if let Some(step) = self.tour_step {
            let (title, body) = TOUR[step.min(TOUR.len() - 1)];
            let mut open = true;
            let mut next = None;
            ui_window(ctx, "Tour", 440.0)
                .open(&mut open)
                .show(ctx, |ui| {
                    ui.label(ui::muted(format!("{} of {}", step + 1, TOUR.len())).size(12.0));
                    ui.label(ui::semibold(title, 17.0));
                    ui.add(egui::Label::new(body).wrap());
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        let last = step + 1 >= TOUR.len();
                        if ui::primary(ui, if last { "Done" } else { "Next" }, true).clicked() {
                            next = Some(if last { None } else { Some(step + 1) });
                        }
                        if step > 0 && ui.button("Back").clicked() {
                            next = Some(Some(step - 1));
                        }
                        if !last && ui.button("Skip the tour").clicked() {
                            next = Some(None);
                        }
                    });
                });
            match next {
                Some(Some(s)) => {
                    self.tour_step = Some(s);
                    // Show the part of the window each card talks about.
                    self.tab = match s {
                        2 => Tab::Projects,
                        3 => Tab::Health,
                        4 => Tab::Report,
                        _ => Tab::Stage,
                    };
                }
                Some(None) => self.end_tour(),
                None if !open => self.end_tour(),
                None => {}
            }
        }
    }

    fn palette_window(&mut self, ctx: &egui::Context) {
        let Some(mut p) = self.palette.take() else {
            return;
        };
        let found = matching(&p.query);
        p.selected = p.selected.min(found.len().saturating_sub(1));
        let (down, up, enter) = ctx.input_mut(|i| {
            (
                i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown),
                i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp),
                i.consume_key(egui::Modifiers::NONE, egui::Key::Enter),
            )
        });
        if down && p.selected + 1 < found.len() {
            p.selected += 1;
        }
        if up {
            p.selected = p.selected.saturating_sub(1);
        }
        let mut run = enter.then(|| found.get(p.selected).map(|f| f.2)).flatten();
        let mut close = false;
        let modal = egui::Modal::new(egui::Id::unique("command_palette")).show(ctx, |ui| {
            ui.set_width(460.0_f32.min(ctx.content_rect().width() - 80.0));
            let edit = ui::named(
                ui.add(
                    egui::TextEdit::singleline(&mut p.query)
                        .hint_text("Type a command…")
                        .desired_width(f32::INFINITY),
                ),
                "Find a command",
            );
            edit.request_focus();
            ui.add_space(4.0);
            if found.is_empty() {
                ui.label(ui::muted("Nothing matches."));
            }
            for (i, (name, keys, cmd)) in found.iter().enumerate() {
                let on = i == p.selected;
                let r = ui.horizontal(|ui| {
                    let r = ui.selectable_label(on, *name);
                    if !keys.is_empty() {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(ui::mono(*keys).color(MUTED()).size(12.0));
                        });
                    }
                    r
                });
                if r.inner.clicked() {
                    run = Some(*cmd);
                }
            }
        });
        if modal.should_close() {
            close = true;
        }
        if let Some(c) = run {
            self.run_cmd(c);
        } else if !close {
            self.palette = Some(p);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_palette_finds_commands_by_any_words() {
        let names = |q: &str| matching(q).iter().map(|c| c.0).collect::<Vec<_>>();
        assert_eq!(names("zip"), ["Open a project zip"]);
        assert_eq!(names("SETTINGS"), ["Open Settings"]);
        assert!(names("tour take").contains(&"Take the tour"));
        assert!(names("ctrl+3").contains(&"Go to History"));
        assert!(names("nothing like this").is_empty());
        assert_eq!(names("").len(), COMMANDS.len());
    }

    #[test]
    fn every_tour_card_and_shortcut_is_distinct() {
        let mut keys: Vec<&str> = COMMANDS
            .iter()
            .map(|c| c.1)
            .filter(|k| !k.is_empty())
            .collect();
        let n = keys.len();
        keys.sort();
        keys.dedup();
        assert_eq!(keys.len(), n, "two commands share a shortcut");
        assert!(TOUR.iter().all(|(t, b)| !t.is_empty() && b.len() > 20));
    }
}
