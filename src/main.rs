// SPDX-License-Identifier: GPL-3.0-or-later
//! StowLock: drop in a project zip, get a clean, tested GitHub repo, with old
//! versions neatly archived. Copyright (C) 2026 Matt-B-007 and contributors.
//! This program is free software under the GNU GPL v3 or later.

mod app;
mod archive;
mod attempts;
mod auth;
mod cleanup;
mod config;
mod detect;
mod errors;
mod extract;
mod fixtures;
mod git;
mod github;
mod health;
mod history;
mod icon;
mod install;
mod instance;
mod logbook;
mod net;
mod pipeline;
mod preflight;
mod proc;
mod report;
mod rustcheck;
mod sandbox;
mod secrets;
mod selftest;
mod signing;
mod state;
mod tools;
mod ui;
mod uninstall;
mod updater;
mod util;
mod version;
mod workflows;
mod zipinfo;

use config::{AppPaths, Config, APP_VERSION};
use std::io::{BufRead, Write};
use std::path::PathBuf;

const HELP: &str = "StowLock — drop in a project zip, get a clean, tested GitHub repo.

Usage:
  stowlock [file.zip]            Open the app (optionally with a zip loaded)
  stowlock --install             Install to ~/.local/bin with an app menu entry
  stowlock --uninstall           Remove the app (keeps settings, history, projects)
  stowlock --uninstall --purge   Also remove settings, history, logs and reports
  stowlock --uninstall --everything
                                 Also remove your projects folder and archived zips
                                 (moved to the Trash; asks you to type DELETE)
      --dry-run                  Show what would be removed, remove nothing
      --permanent                With --everything: delete instead of Trash
      --yes                      Don't ask for confirmation (levels 1-2)
      --yes-delete-projects      Confirm --everything without typing (scripts)
  stowlock --self-test [--online]
                                 Check every feature in a sandbox; exits 1 on failure
  stowlock --self-test --quick   Seconds: can it start, load resources, read settings
  stowlock --report              Save a bug report to a file (private things hidden)
  stowlock --update              Check for a newer version and install it
  stowlock --rollback            Go back to the version kept from before the last update
  stowlock --version             Print the version
";

/// `--report`: the same redacted bug report as the app's, saved to a file
/// (for when the window won't open).
fn cli_report(paths: &AppPaths) -> Result<PathBuf, String> {
    let config = config::Config::load(paths);
    let tail: Vec<String> = std::fs::read_to_string(paths.log_file())
        .unwrap_or_default()
        .lines()
        .rev()
        .take(100)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .map(String::from)
        .collect();
    let problems: Vec<String> = tail
        .iter()
        .filter(|l| l.contains(" WARN ") || l.contains(" ERROR "))
        .cloned()
        .collect();
    let input = report::ReportInput {
        title: "bug report".into(),
        env: report::collect_env(None, None),
        health: health::HealthState::load(paths).lines(),
        problem_lines: problems,
        log_lines: tail,
        ..Default::default()
    };
    let rc = report::RedactContext::local(&config.workspace());
    let text = report::build_report(&input, &rc);
    report::save_report(paths, "bug-report", &text)
}

/// Can this build start? Loads the fonts and draws a frame without a window,
/// renders the icon, reads the settings and the signing keys.
fn quick_self_test(paths: &AppPaths) -> Result<(), String> {
    let ctx = eframe::egui::Context::default();
    ui::install_fonts(&ctx);
    let mut out = ctx.run_ui(eframe::egui::RawInput::default(), |root| {
        eframe::egui::CentralPanel::default().show(root, |ui| {
            ui.label(format!("StowLock {APP_VERSION} {}", ui::icon::REFRESH));
        });
    });
    // Nothing draws this frame: let go of the font pictures on purpose.
    out.textures_delta.clear();
    let rgba = icon::rgba(64);
    if rgba.len() != 64 * 64 * 4 {
        return Err("the app icon didn't render".into());
    }
    let _config = config::Config::load(paths);
    if signing::trusted_keys().len() != 2 {
        return Err("the update signing keys didn't load".into());
    }
    Ok(())
}

fn ask(prompt: &str) -> String {
    print!("{prompt}");
    let _ = std::io::stdout().flush();
    let mut s = String::new();
    let _ = std::io::stdin().lock().read_line(&mut s);
    s.trim().to_string()
}

fn cli_uninstall(args: &[String], paths: &AppPaths) -> i32 {
    let has = |f: &str| args.iter().any(|a| a == f);
    let level = if has("--everything") {
        uninstall::Level::Everything
    } else if has("--purge") {
        uninstall::Level::AppAndData
    } else {
        uninstall::Level::AppOnly
    };
    let config = Config::load(paths);
    let plan = uninstall::plan(
        level,
        paths,
        &config,
        &install::InstallPaths::detect(),
        &util::home_dir(),
    );
    println!("{}", plan.describe());
    if !plan.refused.is_empty() {
        return 1;
    }
    if has("--dry-run") {
        println!("Dry run: nothing was removed.");
        return 0;
    }
    let confirmed = if level == uninstall::Level::Everything {
        has("--yes-delete-projects")
            || ask("Type DELETE to remove everything listed above: ") == "DELETE"
    } else {
        has("--yes") || matches!(ask("Continue? [y/N] ").to_lowercase().as_str(), "y" | "yes")
    };
    if !confirmed {
        println!("Cancelled. Nothing was removed.");
        return 1;
    }
    match uninstall::execute(&plan, confirmed, has("--permanent")) {
        Ok(out) => {
            for r in &out.removed {
                println!("removed {r}");
            }
            for t in &out.trashed {
                println!("moved to Trash: {t}");
            }
            for p in &out.problems {
                eprintln!("problem: {p}");
            }
            if !out.trashed.is_empty() {
                println!("\nThose files are still in the Trash until you empty it.");
            }
            if level != uninstall::Level::Everything {
                println!(
                    "Your projects and archived zips in {} were not touched.",
                    util::tilde(&config.workspace())
                );
            }
            i32::from(!out.problems.is_empty())
        }
        Err(e) => {
            eprintln!("{e}");
            1
        }
    }
}

/// Every option StowLock understands.
const OPTIONS: [&str; 20] = [
    "--dry-run",
    "--everything",
    "--help",
    "--install",
    "--online",
    "--open-screen",
    "--permanent",
    "--purge",
    "--quick",
    "--report",
    "--rollback",
    "--self-test",
    "--uninstall",
    "--update",
    "--version",
    "--window-size",
    "--yes",
    "--yes-delete-projects",
    "-V",
    "-h",
];

/// The first option that isn't one of StowLock's (a mistyped one, say).
fn unknown_option(args: &[String]) -> Option<&String> {
    args.iter()
        .find(|a| a.starts_with('-') && a.len() > 1 && !OPTIONS.contains(&a.as_str()))
}

/// Why the window can't open, in plain words.
fn window_problem(error: &str) -> String {
    let no_display =
        std::env::var_os("WAYLAND_DISPLAY").is_none() && std::env::var_os("DISPLAY").is_none();
    if no_display {
        "StowLock couldn't open its window: there's no desktop session here (no DISPLAY or WAYLAND_DISPLAY). Start it from your desktop, or use the command-line options: stowlock --help".to_string()
    } else {
        format!("StowLock couldn't open its window. The desktop refused it: {error}")
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let has = |f: &str| args.iter().any(|a| a == f);
    if let Some(bad) = unknown_option(&args) {
        eprintln!("StowLock doesn't have an option called {bad}. See: stowlock --help");
        std::process::exit(2);
    }
    let paths = AppPaths::detect();

    if has("--version") || has("-V") {
        println!("StowLock {APP_VERSION}");
        return;
    }
    if has("--help") || has("-h") {
        print!("{HELP}");
        return;
    }
    if has("--uninstall") {
        std::process::exit(cli_uninstall(&args, &paths));
    }
    if has("--report") {
        match cli_report(&paths) {
            Ok(p) => {
                println!("Bug report saved: {}", util::tilde(&p));
                println!("Private things are hidden. Nothing was sent; attach the file where you want help.");
                return;
            }
            Err(e) => {
                eprintln!("Couldn't write the report: {e}");
                std::process::exit(1);
            }
        }
    }
    if has("--install") {
        match install::install(&install::InstallPaths::detect()) {
            Ok(r) => {
                for l in r.lines {
                    println!("{l}");
                }
                for w in r.warnings {
                    println!("note: {w}");
                }
                if let Some(v) = instance::running_pid(&paths).and_then(instance::running_version) {
                    if v != APP_VERSION {
                        println!(
                            "note: StowLock {v} is still open. Close it, then open StowLock again to use {APP_VERSION}."
                        );
                    }
                }
                return;
            }
            Err(e) => {
                eprintln!("Install failed: {e}");
                std::process::exit(1);
            }
        }
    }
    if let Err(e) = paths.ensure() {
        eprintln!("StowLock couldn't create its folders: {e}");
        std::process::exit(1);
    }
    if has("--update") {
        let config = config::Config::load(&paths);
        let beta = config.beta_updates();
        let log = logbook::Logger::new(None);
        match updater::check(&paths, &config.update_repo, beta).map(|c| (c.newer, c.newest_tag)) {
            Ok((None, newest)) => println!(
                "You have the newest {} version ({APP_VERSION}). Newest on GitHub: {}.",
                if beta { "beta or stable" } else { "stable" },
                newest.unwrap_or_else(|| "none yet".into())
            ),
            Ok((Some(rel), _)) => {
                println!("Updating StowLock {APP_VERSION} to {}…", rel.label());
                match updater::apply(&paths, &config.update_repo, &rel.tag, &log) {
                    Ok(p) => {
                        println!("Installed {} at {}.", rel.label(), util::tilde(&p));
                        println!("Close StowLock if it's open, then open it again. If anything's wrong: stowlock --rollback");
                    }
                    Err(e) => {
                        eprintln!("{e}");
                        std::process::exit(1);
                    }
                }
            }
            Err(e) => {
                eprintln!("{e}");
                std::process::exit(1);
            }
        }
        return;
    }
    if has("--rollback") {
        let log = logbook::Logger::new(None);
        let r = std::env::current_exe()
            .map_err(|e| e.to_string())
            .and_then(|exe| updater::rollback(&paths, &exe, &log));
        match r {
            Ok((_, v)) => {
                println!("Went back to StowLock {v}. Open StowLock again to use it.");
                println!("The version you left ({APP_VERSION}) is kept; run --rollback again to return to it.");
            }
            Err(e) => {
                eprintln!("{e}");
                std::process::exit(1);
            }
        }
        return;
    }
    if has("--self-test") && has("--quick") {
        // Seconds, no network, no other tools: run by the updater before it
        // swaps a new version in.
        match quick_self_test(&paths) {
            Ok(()) => {
                println!("StowLock {APP_VERSION}: quick self-test passed (starts, resources load, settings read)");
                return;
            }
            Err(e) => {
                eprintln!("StowLock {APP_VERSION}: quick self-test failed: {e}");
                std::process::exit(1);
            }
        }
    }
    if has("--self-test") {
        println!("StowLock {APP_VERSION} self-test (sandboxed; never touches your projects or GitHub)…\n");
        let r = selftest::run(&paths, has("--online"));
        for c in &r.checks {
            let tag = match c.outcome {
                selftest::Outcome::Pass => "PASS",
                selftest::Outcome::Fail => "FAIL",
                selftest::Outcome::Skip => "SKIP",
            };
            let extra = if c.detail.is_empty() {
                String::new()
            } else {
                format!("  ({})", c.detail)
            };
            println!("{tag}  {}  {:<18} {}{extra}", c.id, c.area, c.name);
        }
        r.apply_to_health(&paths);
        let saved = report::save_report(&paths, "selftest", &r.to_markdown());
        println!("\n{}", r.summary_line());
        if let Ok(p) = saved {
            println!("Report: {}", util::tilde(&p));
        }
        std::process::exit(i32::from(r.counts().1 > 0));
    }

    // The zip is the first argument that isn't an option or an option's value.
    let zip_arg: Option<PathBuf> = args
        .iter()
        .enumerate()
        .find(|(i, a)| {
            !a.starts_with('-')
                && (*i == 0 || (args[i - 1] != "--window-size" && args[i - 1] != "--open-screen"))
        })
        .map(|(_, a)| PathBuf::from(a));
    let guard = match instance::acquire(&paths) {
        Ok(instance::Acquire::First(g)) => Some(g),
        Ok(instance::Acquire::AlreadyRunning) => {
            // An older copy is open: offer to switch to this newer one.
            let older = instance::running_pid(&paths).and_then(|pid| {
                instance::running_version(pid)
                    .filter(|v| instance::is_older(v, APP_VERSION))
                    .map(|v| (pid, v))
            });
            let mut switched = None;
            if let Some((pid, v)) = older {
                let q = format!(
                    "An older StowLock ({v}) is open. Close it and switch to {APP_VERSION}?"
                );
                if proc::ask_yes_no("StowLock", &q) {
                    switched = instance::take_over(&paths, pid, 8);
                    if switched.is_none() {
                        eprintln!("The older StowLock didn't close. Close it yourself, then open StowLock again.");
                        std::process::exit(1);
                    }
                }
            }
            if switched.is_some() {
                switched
            } else {
                if let Some(z) = &zip_arg {
                    match instance::hand_off(&paths, z) {
                        Ok(_) => println!("StowLock is already open; the zip was sent to it."),
                        Err(e) => eprintln!("{e}"),
                    }
                } else {
                    println!("StowLock is already open.");
                }
                return;
            }
        }
        Err(e) => {
            eprintln!("{e}");
            None
        }
    };

    let config = Config::load(&paths);
    let log = logbook::Logger::new(Some(paths.log_file()));
    report::install_panic_hook(paths.clone(), log.clone(), config.workspace());
    // Closed unexpectedly last time without a crash report (killed, out of
    // memory, power cut)? Write one now, so the notice can show it.
    if let Some(v) = report::mark_running(&paths) {
        report::record_unexpected_close(&paths, &log, &v, &config.workspace());
    }
    let paths_at_exit = paths.clone();

    // --window-size WIDTHxHEIGHT: open at a given size (used for layout checks).
    let size = args
        .iter()
        .position(|a| a == "--window-size")
        .and_then(|i| args.get(i + 1))
        .and_then(|v| v.split_once('x'))
        .and_then(|(w, h)| Some([w.parse::<f32>().ok()?, h.parse::<f32>().ok()?]))
        .unwrap_or([1120.0, 800.0]);
    let icon = eframe::egui::IconData {
        rgba: icon::rgba(128),
        width: 128,
        height: 128,
    };
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("StowLock")
            .with_app_id("stowlock")
            .with_inner_size(size)
            .with_min_inner_size([760.0, 560.0])
            .with_drag_and_drop(true)
            .with_icon(std::sync::Arc::new(icon)),
        ..Default::default()
    };
    let result = eframe::run_native(
        "StowLock",
        options,
        Box::new(move |cc| Ok(Box::new(app::App::new(cc, paths, log, guard, zip_arg)))),
    );
    report::mark_stopped(&paths_at_exit);
    if let Err(e) = result {
        eprintln!("{}", window_problem(&e.to_string()));
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_mistyped_option_is_named() {
        let args = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(unknown_option(&args(&["--self-test", "--quick"])), None);
        assert_eq!(unknown_option(&args(&["my app.zip"])), None);
        assert_eq!(
            unknown_option(&args(&["--window-size", "1120x800", "--selftest"])).map(String::as_str),
            Some("--selftest")
        );
        assert_eq!(
            unknown_option(&args(&["-x"])).map(String::as_str),
            Some("-x")
        );
        // Every option in the help text is one StowLock knows.
        for word in HELP.split_whitespace() {
            let w = word.trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '-');
            if w.starts_with("--") && w.len() > 2 {
                assert!(OPTIONS.contains(&w), "{w} is in --help but not in OPTIONS");
            }
        }
    }
}
