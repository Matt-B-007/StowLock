// SPDX-License-Identifier: GPL-3.0-or-later
//! Click-through tests: the real app, driven by clicking its buttons (through
//! the accessibility tree, as a screen reader would). GitHub is the local
//! stand-in, so nothing ever reaches the real GitHub.
use super::*;
use egui_kittest::kittest::{NodeT, Queryable};
use egui_kittest::Harness;

/// The light/dark choice is one setting for the whole program, so the window
/// tests take turns instead of running at the same time.
static ONE_WINDOW_AT_A_TIME: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn one_window() -> std::sync::MutexGuard<'static, ()> {
    ONE_WINDOW_AT_A_TIME
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

fn wait_until(h: &mut Harness<'_, App>, what: &str, f: impl Fn(&App) -> bool) {
    for _ in 0..1500 {
        h.step();
        if f(h.state()) {
            h.step();
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    panic!("timed out waiting for {what}");
}

fn steps(h: &mut Harness<'_, App>, n: usize) {
    for _ in 0..n {
        h.step();
    }
}

/// Click the button whose name contains `text`.
fn click_button(h: &mut Harness<'_, App>, text: &str) {
    let n = h
        .get_all_by_label_contains(text)
        .find(|n| {
            n.accesskit_node().role() == egui::accesskit::Role::Button
                || n.accesskit_node().role() == egui::accesskit::Role::CheckBox
        })
        .unwrap_or_else(|| panic!("no button named like \"{text}\""));
    n.click_accesskit();
    steps(h, 2);
}

fn click_nav(h: &mut Harness<'_, App>, label: &str) {
    h.get_by_role_and_label(egui::accesskit::Role::Button, label)
        .click_accesskit();
    steps(h, 3);
}

#[test]
fn click_through_the_app() {
    let _window = one_window();
    let d = tempfile::tempdir().unwrap();
    let paths = AppPaths::under(&d.path().join("app"));
    paths.ensure().unwrap();
    let mut cfg = Config::load(&paths);
    cfg.workspace_dir = d.path().join("ws").display().to_string();
    cfg.check_for_updates = false;
    cfg.theme = "dark".into();
    cfg.tour_seen = true;
    cfg.fallback_git_name = "StowLock Test".into();
    cfg.fallback_git_email = "test@example.invalid".into();
    cfg.save(&paths).unwrap();
    let zip = crate::fixtures::Sandbox::new(&d.path().join("sb"))
        .unwrap()
        .zip(
            "demo-app-v0.2.0.zip",
            &[("main.py", b"print('hi')\n", 0o644)],
        );

    let p2 = paths.clone();
    let mut h = Harness::builder()
        .with_size(egui::vec2(1280.0, 860.0))
        .build_eframe(move |cc| App::new(cc, p2, Logger::new(None), None, None));
    // Let the start-up checks finish, then sign in to the stand-in GitHub.
    wait_until(&mut h, "start-up checks", |a| a.env.checked);
    {
        let a = h.state_mut();
        a.env.git = true;
        a.env.gh = true;
        a.env.login = Some("selftest-user".into());
        a.gh = Arc::new(crate::github::FakeGh::new(&d.path().join("gh")));
        a.build_poll = (0, 1);
    }
    steps(&mut h, 2);

    // Every screen opens from the sidebar.
    for (nav, marker) in [
        ("Projects", "projects in"),
        ("History", "Nothing pushed yet"),
        ("Health", "Run self-test"),
        ("Bug report", "Also hide these words"),
        ("Settings", "Update channel"),
    ] {
        click_nav(&mut h, nav);
        assert!(
            h.query_by_label_contains(marker).is_some(),
            "{nav} screen didn't show \"{marker}\""
        );
    }

    // Settings: the update channel and the theme switch.
    h.get_by_role_and_label(egui::accesskit::Role::Button, "Stable")
        .click_accesskit();
    steps(&mut h, 2);
    assert_eq!(h.state().config.update_channel, "stable");
    h.get_by_role_and_label(egui::accesskit::Role::Button, "Light")
        .click_accesskit();
    steps(&mut h, 3);
    assert!(ui::is_light(), "Light theme didn't apply");
    h.get_by_role_and_label(egui::accesskit::Role::Button, "Dark")
        .click_accesskit();
    steps(&mut h, 3);
    assert!(!ui::is_light());

    // Stage & check a zip.
    click_nav(&mut h, "Stage");
    // A newer Rust is out: the Stage screen says so; Later hides it.
    h.state_mut().rust = Some(crate::rustcheck::RustState::UpdateAvailable {
        have: "1.100.0".into(),
        newest: "1.101.0".into(),
    });
    steps(&mut h, 2);
    assert!(h
        .query_by_label_contains("Rust 1.101.0 is available (you have 1.100.0)")
        .is_some());
    click_button(&mut h, "Later");
    steps(&mut h, 2);
    assert!(h
        .query_by_label_contains("Rust 1.101.0 is available")
        .is_none());
    h.state_mut().load_zip(zip);
    steps(&mut h, 3);
    // The version comes from the zip: no typing.
    assert_eq!(h.state().session.tag, "v0.2.0");
    assert_eq!(h.state().session.channel, Channel::Stable);
    h.state_mut().session.private = false; // a new public repo
    click_button(&mut h, "Stage & check");
    wait_until(&mut h, "checks", |a| {
        a.session.checks.is_some() && a.busy.is_none()
    });
    steps(&mut h, 2);

    // The review window opens by itself; an instant click can't push.
    assert!(
        h.state().session.confirm,
        "review window didn't open by itself"
    );
    assert!(
        h.query_by_label_contains("Everyone using an older version")
            .is_some(),
        "Who gets this"
    );
    assert!(h
        .query_by_label_contains("I understand this will be public")
        .is_some());
    click_button(&mut h, "to demo-app");
    steps(&mut h, 2);
    assert!(
        h.state().session.outcome.is_none(),
        "pushed on a double-click"
    );
    // Armed after half a second, but a public repo still needs the tick.
    std::thread::sleep(std::time::Duration::from_millis(600));
    steps(&mut h, 2);
    click_button(&mut h, "to demo-app");
    steps(&mut h, 2);
    assert!(
        h.state().session.outcome.is_none(),
        "pushed a public repo without the tick"
    );
    click_button(&mut h, "I understand this will be public");
    steps(&mut h, 2);
    // GitHub's build will fail, to check the build card.
    std::fs::create_dir_all(d.path().join("gh")).unwrap();
    std::fs::write(
        d.path().join("gh/build.json"),
        r#"{"state":"failed","step":"arch (cargo test --locked)","log":"test x ... FAILED"}"#,
    )
    .unwrap();
    click_button(&mut h, "to demo-app");
    wait_until(&mut h, "push", |a| {
        a.session.outcome.is_some() && a.busy.is_none()
    });

    // Finished state: no "already exists" warning, the next steps instead.
    assert!(h.query_by_label_contains("View on GitHub").is_some());
    assert!(h.query_by_label_contains("Stage another zip").is_some());
    assert!(h.query_by_label_contains("already exists").is_none());
    assert!(
        d.path().join("gh").exists(),
        "nothing reached the stand-in GitHub"
    );

    // GitHub's build fails: say where, and offer to delete the release.
    wait_until(&mut h, "GitHub build", |a| {
        matches!(
            a.build.as_ref().and_then(|b| b.status.clone()),
            Some(crate::github::Build::Failed { .. })
        )
    });
    steps(&mut h, 2);
    assert!(h
        .query_by_label_contains("GitHub's build failed at arch (cargo test --locked)")
        .is_some());
    assert!(h
        .query_by_label_contains("It passed here but failed on GitHub")
        .is_some());

    // Re-run on GitHub: the re-run fails again, this time with a network
    // error, so StowLock says it's GitHub's problem, not the project's.
    std::fs::write(
        d.path().join("gh/rerun.json"),
        r#"{"state":"failed","step":"Install tools","log":"curl: (6) Could not resolve host: static.rust-lang.org"}"#,
    )
    .unwrap();
    click_button(&mut h, "Re-run on GitHub");
    wait_until(&mut h, "re-run watched", |a| {
        matches!(
            a.build.as_ref().and_then(|b| b.status.clone()),
            Some(crate::github::Build::Failed { ref step, .. }) if step == "Install tools"
        )
    });
    steps(&mut h, 2);
    assert!(h
        .query_by_label_contains("passing problem on GitHub's side")
        .is_some());
    click_button(&mut h, "Delete the release and tag");
    click_button(&mut h, "Delete v0.2.0 from GitHub");
    wait_until(&mut h, "release removed", |a| {
        a.busy.is_none() && a.session.outcome.as_ref().is_some_and(|o| o.tag.is_none())
    });
    let fake = crate::github::FakeGh::new(&d.path().join("gh"));
    assert!(
        fake.releases("selftest-user", "demo-app").is_empty(),
        "release still on GitHub"
    );

    // The push shows in History.
    click_nav(&mut h, "History");
    assert!(
        h.query_by_label_contains("demo-app").is_some(),
        "History is empty"
    );

    // The repo is deleted on GitHub: Projects flags the folder.
    std::fs::remove_dir_all(fake.bare("selftest-user", "demo-app")).unwrap();
    click_nav(&mut h, "Projects");
    click_button(&mut h, "Refresh");
    wait_until(&mut h, "repo list reloaded", |a| {
        a.repos_loading.is_empty()
            && a.github_repos("selftest-user")
                .is_some_and(|n| n.is_empty())
    });
    steps(&mut h, 2);
    assert!(
        h.query_by_label_contains("Not on GitHub anymore").is_some(),
        "deleted repo not flagged"
    );
    click_button(&mut h, "Remove the folder");
    assert!(h
        .query_by_label_contains("Move demo-app to the Trash")
        .is_some());
    click_button(&mut h, "Cancel");

    // Push it again: not an update any more, and the repo comes back.
    click_button(&mut h, "Push it again as a new repo");
    wait_until(&mut h, "re-check", |a| {
        a.session.checks.is_some() && a.busy.is_none()
    });
    assert_eq!(h.state().session.mode, Mode::New);
    assert!(h.state().session.deleted_on_github);
    assert!(h.state().session.confirm, "review window didn't open");
    std::thread::sleep(std::time::Duration::from_millis(600));
    steps(&mut h, 2);
    if h.query_by_label_contains("I understand this will be public")
        .is_some()
    {
        click_button(&mut h, "I understand this will be public");
    }
    click_button(&mut h, "to demo-app");
    wait_until(&mut h, "push again", |a| {
        a.session.outcome.is_some() && a.busy.is_none()
    });
    assert!(
        fake.bare("selftest-user", "demo-app").exists(),
        "repo not created again"
    );
    wait_until(&mut h, "repo list after push", |a| {
        a.github_repos("selftest-user")
            .is_some_and(|n| n.iter().any(|x| x == "demo-app"))
    });
}

/// A new install: the tour starts by itself and is remembered once done;
/// Ctrl+K finds commands, F1 lists the shortcuts, Esc closes, and the Help
/// menu offers the tour again.
#[test]
fn help_shortcuts_palette_and_tour() {
    let _window = one_window();
    let d = tempfile::tempdir().unwrap();
    let paths = AppPaths::under(&d.path().join("app"));
    paths.ensure().unwrap();
    let cfg = Config {
        workspace_dir: d.path().join("ws").display().to_string(),
        check_for_updates: false,
        rust_update_check: false,
        theme: "dark".into(),
        ..Config::default()
    };
    cfg.save(&paths).unwrap();
    let p2 = paths.clone();
    let mut h = Harness::builder()
        .with_size(egui::vec2(1280.0, 860.0))
        .build_eframe(move |cc| App::new(cc, p2, Logger::new(None), None, None));
    // Let the start-up checks finish first (they may switch to Settings).
    wait_until(&mut h, "start-up checks", |a| a.env.checked);
    steps(&mut h, 3);
    assert_eq!(
        h.state().tour_step,
        Some(0),
        "the tour starts on a new install"
    );
    assert!(h.query_by_label_contains("Welcome to StowLock").is_some());
    for _ in 1..help::TOUR.len() - 1 {
        click_button(&mut h, "Next");
    }
    assert_eq!(h.state().tab, Tab::Report, "the tour shows each part");
    click_button(&mut h, "Next");
    click_button(&mut h, "Done");
    assert_eq!(h.state().tour_step, None);
    assert!(Config::load(&paths).tour_seen, "the tour is remembered");

    // Ctrl+K, type, Enter.
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::K);
    steps(&mut h, 2);
    assert!(h.state().palette.is_some(), "Ctrl+K opens the palette");
    h.event(egui::Event::Text("health".into()));
    steps(&mut h, 2);
    h.key_press(egui::Key::Enter);
    steps(&mut h, 3);
    assert!(h.state().palette.is_none());
    assert_eq!(h.state().tab, Tab::Health, "Enter ran the command");

    // Ctrl+1 and Ctrl+, go to tabs.
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::Num1);
    steps(&mut h, 2);
    assert_eq!(h.state().tab, Tab::Stage);
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::Comma);
    steps(&mut h, 2);
    assert_eq!(h.state().tab, Tab::Settings);

    // F1 lists the shortcuts; Esc closes it.
    h.key_press(egui::Key::F1);
    steps(&mut h, 2);
    assert!(h.query_by_label_contains("Find any command").is_some());
    h.key_press(egui::Key::Escape);
    steps(&mut h, 2);
    assert!(!h.state().show_shortcuts);

    // The Help menu offers the tour again.
    click_button(&mut h, "Help");
    click_button(&mut h, "Take the tour");
    assert_eq!(h.state().tour_step, Some(0));
    h.key_press(egui::Key::Escape);
    steps(&mut h, 2);
    assert_eq!(h.state().tour_step, None);
}

/// Every control on every screen has a name a screen reader can say (and the
/// tests can find). Checked on each screen, not only the one that happens to
/// be showing when the window opens.
#[test]
fn every_screen_names_its_controls() {
    let _window = one_window();
    let d = tempfile::tempdir().unwrap();
    let paths = AppPaths::under(&d.path().join("app"));
    paths.ensure().unwrap();
    let cfg = Config {
        workspace_dir: d.path().join("ws").display().to_string(),
        check_for_updates: false,
        rust_update_check: false,
        tour_seen: true,
        // The theme is one setting for the whole test run: keep every test dark.
        theme: "dark".into(),
        ..Config::default()
    };
    cfg.save(&paths).unwrap();
    let p2 = paths.clone();
    let mut h = Harness::builder()
        .with_size(egui::vec2(1280.0, 2400.0))
        .build_eframe(move |cc| App::new(cc, p2, Logger::new(None), None, None));
    wait_until(&mut h, "start-up checks", |a| a.env.checked);
    for tab in [
        Tab::Stage,
        Tab::Projects,
        Tab::History,
        Tab::Health,
        Tab::Report,
        Tab::Settings,
    ] {
        h.state_mut().tab = tab;
        steps(&mut h, 3);
        let unnamed = h.unnamed_widgets();
        assert!(unnamed.is_empty(), "{tab:?}: {unnamed:#?}");
    }
    // The palette and the tour too.
    h.state_mut().palette = Some(help::Palette::default());
    h.state_mut().tour_step = Some(0);
    steps(&mut h, 3);
    assert!(h.unnamed_widgets().is_empty(), "{:#?}", h.unnamed_widgets());
}
