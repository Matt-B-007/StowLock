// SPDX-License-Identifier: GPL-3.0-or-later
//! Help: keyboard shortcuts, the command palette (Ctrl+K) and
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
        "StowLock turns a project zip into a tested GitHub release. This short tour shows where things are. You can take it again any time from Settings › Help.",
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
        "Settings and help",
        "Settings (bottom of the sidebar) has Help: the shortcuts, this tour, and bug reports, which hide your name, folders and other private things before you save or copy them.",
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
            } else if self.show_uninstall {
                // Nothing has been removed until its button is pressed.
                self.show_uninstall = false;
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

    /// The palette, the shortcuts list and the tour.
    pub(super) fn help_windows(&mut self, ctx: &egui::Context) {
        if self.palette.is_some() {
            self.palette_window(ctx);
        }
        if self.show_shortcuts {
            let mut open = true;
            // Sized to its rows; scrolls when the window is too short for them.
            let room = (ctx.content_rect().height() - 150.0).max(160.0);
            ui_window(ctx, "Keyboard shortcuts", 420.0)
                .open(&mut open)
                .resizable(false)
                .vscroll(false)
                .show(ctx, |ui| {
                    egui::ScrollArea::vertical()
                        .max_height(room)
                        .min_scrolled_height(room)
                        .show(ui, |ui| {
                            egui::Grid::new("shortcuts")
                                .num_columns(2)
                                .spacing([24.0, 8.0])
                                .show(ui, |ui| {
                                    ui.label(ui::mono("Ctrl+K").color(ACCENT()));
                                    ui.label("Find any command");
                                    ui.end_row();
                                    for (name, keys, _) in
                                        COMMANDS.iter().filter(|c| !c.1.is_empty())
                                    {
                                        ui.label(ui::mono(*keys).color(ACCENT()));
                                        ui.label(*name);
                                        ui.end_row();
                                    }
                                    ui.label(ui::mono("Esc").color(ACCENT()));
                                    ui.label("Close this, the palette or the tour");
                                    ui.end_row();
                                });
                        });
                });
            self.show_shortcuts &= open;
        }
        if let Some(step) = self.tour_step {
            let (title, body) = TOUR[step.min(TOUR.len() - 1)];
            let mut open = true;
            let mut next = None;
            // One size for every card, in the bottom corner so it doesn't
            // cover what it's describing.
            let lift = if self.tab == Tab::Stage { 96.0 } else { 24.0 };
            ui_window(ctx, "Tour", 400.0)
                .open(&mut open)
                .resizable(false)
                .vscroll(false)
                .anchor(egui::Align2::RIGHT_BOTTOM, [-24.0, -lift])
                .show(ctx, |ui| {
                    ui.set_width(400.0_f32.min(ctx.content_rect().width() - 80.0));
                    ui.label(ui::muted(format!("{} of {}", step + 1, TOUR.len())).size(12.0));
                    ui.label(ui::semibold(title, 17.0));
                    ui.allocate_ui_with_layout(
                        egui::vec2(ui.available_width(), 96.0),
                        egui::Layout::top_down(egui::Align::Min),
                        |ui| {
                            ui.set_min_height(96.0);
                            ui.add(egui::Label::new(body).wrap());
                        },
                    );
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
                        4 => Tab::Settings,
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
        // Pinned near the top, so it doesn't move as the list gets shorter.
        let id = egui::Id::unique("command_palette");
        let modal = egui::Modal::new(id)
            .area(egui::Modal::default_area(id).anchor(egui::Align2::CENTER_TOP, [0.0, 96.0]))
            .show(ctx, |ui| {
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
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    ui.label(ui::mono(*keys).color(MUTED()).size(12.0));
                                },
                            );
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
