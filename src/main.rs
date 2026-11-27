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
mod pipeline;
mod preflight;
mod proc;
mod report;
mod secrets;
mod selftest;
mod signing;
mod state;
mod ui;
mod uninstall;
mod updater;
mod util;
mod version;
mod workflows;

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
  stowlock --update              Check for a newer version and install it
  stowlock --rollback            Go back to the version kept from before the last update
  stowlock --version             Print the version
";

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

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let has = |f: &str| args.iter().any(|a| a == f);
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
            !a.starts_with("--")
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

    // --window-size WIDTHxHEIGHT: open at a given size (used for layout checks).
    let size = args
        .iter()
        .position(|a| a == "--window-size")
        .and_then(|i| args.get(i + 1))
        .and_then(|v| v.split_once('x'))
        .and_then(|(w, h)| Some([w.parse::<f32>().ok()?, h.parse::<f32>().ok()?]))
        .unwrap_or([680.0, 840.0]);
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
            .with_min_inner_size([520.0, 600.0])
            .with_drag_and_drop(true)
            .with_icon(std::sync::Arc::new(icon)),
        ..Default::default()
    };
    let result = eframe::run_native(
        "StowLock",
        options,
        Box::new(move |cc| Ok(Box::new(app::App::new(cc, paths, log, guard, zip_arg)))),
    );
    if let Err(e) = result {
        eprintln!("StowLock couldn't open its window: {e}");
        std::process::exit(1);
    }
}
