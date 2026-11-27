# Changelog

All notable changes to StowLock. When StowLock pushes a release, the section
for that version becomes the GitHub release notes and the "What's new" text in
the update notice.

## [0.2.0-beta.5] - 2026-11-27

### Fixed
- **Check scripts find the build in `./target`.** StowLock builds in its own cache folder so builds stay quick and the project folder stays clean, but scripts that run `./target/release/<app>` (as on GitHub) couldn't find it ("No such file or directory"). CachyPlay's release self-test and Stream Preflight's self-test ("the program wasn't built") failed this way. While a check runs, `./target` now leads to the cache, and it's removed when the check ends, so it's never committed. If the project has a real `./target` of its own, that one is used.
- **"Looks like a font difference" only when it is one.** It's now said only when tests actually failed and their output mentions text that doesn't fit ("text cut off", "doesn't fit", a missing glyph). A build or tool problem, like Stream Preflight's "the program wasn't built", is no longer called a font difference.
- **Cleaner repo names from zip names.** Words like "source", "src", "code", "project", "full", "complete" or "files" at the end are dropped, so "Gimbot 0.1.0 beta.14 source.zip" becomes `gimbot` for a new project (an existing Gimbot repo was already matched in 0.2.0-beta.4).

## [0.2.0-beta.4] - 2026-11-27

### Fixed
- **No "closed unexpectedly" notice after an update,** even when updating from a version that didn't clear its "running" mark before restarting (0.2.0-beta.2 and older): the hand-over from the version you just updated from isn't a crash.
- **A zip with a longer name goes to the existing project.** "gimbot-v0.4.2-house-standard.zip" or "Gimbot_fixed_fonts.zip" now picks your existing Gimbot repo, with its exact spelling, instead of making up a new, longer repo name. It matches names in your workspace and on GitHub; the longest match wins, and "gimbotics" isn't mistaken for "gimbot".

## [0.2.0-beta.3] - 2026-11-27

Fixes from the first day of beta.2.

### Fixed
- **Copy buttons work again.** Since the move to egui 0.37, clipboard support is a separate switch that wasn't on, so every Copy button did nothing. A test now checks it stays on. Web links open again too.
- **Cancel stops a running check straight away.** Cancel and the time limit only stopped the check script itself, while the programs it started (cargo, rustc, the tests) kept running until they finished. Now they stop too.
- **Updating no longer looks like a crash.** After an update, the new version thought the old one had closed unexpectedly, and Health marked "Settings & state" as broken. Quitting after an uninstall had the same problem.
- **Reports keep the reason a check script gives.** When a project's own check script ends with its verdict (for example "Check failed: the release build fails its quick self-test"), the few lines before it, which usually say why, are kept in the log and the bug report.

### Changed
- **"Only GitHub's fonts" matches GitHub's machine more closely:** DejaVu, Liberation and Noto Color Emoji (each when installed here), not DejaVu alone. Any other font name still falls back to DejaVu Sans, as on GitHub.

## [0.2.0-beta.2] - 2026-11-27

Checks that catch font problems before GitHub does, a clear view of which zip you loaded, and the exact House Standard v1.3 colours.

### Added
- **Only GitHub's fonts** (Settings → Checks, on by default). GitHub steps run with just the DejaVu fonts GitHub's Ubuntu machine has, with the same fallbacks: any other font name becomes DejaVu Sans. Text that only fits with your own fonts now fails here too, before you push. Fonts a project bundles and loads itself still work, as they do on GitHub.
- **Font failures are named.** When a failure looks like text not fitting ("text cut off" and similar) under GitHub's fonts, the checks card says "looks like a font difference", explains why, and Copy for AI asks the project to bundle its fonts and let text shorten with a tooltip.
- **DejaVu fonts in Your tools,** with Install missing tools when they're not installed (`ttf-dejavu`).
- **Which zip you loaded:** the Stage card shows the zip's size and when it was saved, and the log records them too.
- **"This is the same zip as last time"** when the zip you load is byte for byte the one you last staged for that project.
- **"There's a newer copy of this download beside it"** when the browser saved a later copy, such as `name(1).zip`, with a button to use it.

### Changed
- **House Standard v1.3 colours, exactly:** the navy neutrals for both themes from the colour table. StowLock's accent stays emerald.
- **The contrast test checks every text colour on every surface** (background, sidebar, card, control and hover) in both themes: text at least 12.3:1, muted at least 5.0:1, accent and status colours at least 4.5:1. It also checks filled buttons: ink at least 7.2:1 in dark, white at least 5.1:1 in light.
- **Family details:**
  - Cards have a faint emerald edge.
  - The card that's working, or holds the control you're typing in, gets small emerald corner brackets.
  - The single main action has a soft glow.
  - Corners are 8, 12 and 16 px only, and pop-ups have the same faint edge.
- **The icon's background is #0C1016 with a thin emerald edge at 35 %.**

## [0.2.0-beta.1] - 2026-11-27

The family look, keyboard shortcuts and a tour, a newer egui underneath, and checks that behave more like GitHub.

### Fixed
- **Each workflow job gets its own fresh setup,** as on GitHub's separate machines: its own clean home folder and its own Python environment. One job can't leave anything behind for another.
- **One failing job no longer stops the others.** Only the rest of the failing job is skipped (and listed as "Not run"). The other jobs still run, and the result says how each job went, for example "jobs: test failed, rust passed".
- **Copying from the checks card hides private things,** the same way bug reports do. "Copy output" copies the result and output with your name, folders and other private things hidden. The checks text can no longer be copied by selecting it with the mouse, which would have skipped the hiding.
- A few symbols drew as empty boxes. Every symbol the app uses is now checked against its fonts by a test.
- **Every text field and dropdown has a name a screen reader can say** (several in Settings and Stage had none). A test now checks every screen.
- **A test failing inside a Rust library is no longer called "specific to this computer".** Paths under `~/.cargo` and `~/.rustup` are where libraries and toolchains live, so they say nothing about your computer.

### Added
- **Help menu** at the bottom of the sidebar: find a command, keyboard shortcuts, the tour, bug report, check for updates, and the version.
- **Keyboard shortcuts:** Ctrl+K finds any command by typing. Ctrl+1 to Ctrl+5 go to Stage, Projects, History, Health and Bug report. Ctrl+, opens Settings, Ctrl+O opens a zip, F1 lists every shortcut, and Esc closes the palette, the list or the tour.
- **A short tour** of the window. It starts by itself only on a new install; if you're upgrading, you can take it any time from Help.

### Changed
- **House Standard v1.3 colours:** a navy base with StowLock's emerald (#34D399 dark, #047857 light). Filled buttons use ink text in dark mode and white in light. Every text colour is tested for at least 4.5:1 contrast in both themes, including on hovered rows.
- **A bolder icon that still reads at 16 px:** a bigger keyhole, a thicker lid line, no small teeth, and a navy background.
- **The bug report's main actions are Save… and Copy.** "Keep a copy" and "Open on GitHub…" come after them, and GitHub still asks first because issues are public.
- **egui 0.31 → 0.37** (the toolkit StowLock is drawn with).
- **The window's code is split into smaller files** (Stage, Projects, Health, Settings, the action bar and so on). Nothing changes in use; all the click-through tests pass unchanged.
- Settings files move to layout version 2, migrated on their own.

## [0.1.0-beta.11] - 2026-11-27

Fixes from real use, one-click tool installs, and a sturdier core.

### Fixed
- **Checks no longer reuse an old build of your project.** Each check starts with a fresh Python environment, as GitHub does (pip's download cache keeps it quick). A cached environment could keep an older build of a project, so a test comparing versions failed only here.
- **Your installed programs no longer stand in for the project being checked.** During GitHub steps, `~/.local/bin` and `~/.cargo/bin` come after the system's folders, so an installed copy of an app can't be found before the freshly built one. StowLock's own tools folder comes last, so a copy it built earlier never hides the version you installed.
- **The daily Rust check reads rustup's answer whatever its wording, capitals or colours** (it said "Couldn't check Rust" with rustup 1.29).
- **The real missing tool is named.** When a check script prints a `sudo pacman -S …` line, those packages are named, never an earlier "Install Rust" step.
- **Reports show a check script's whole missing-tools list,** and call a step that only couldn't run here "Couldn't run here", not a failing command.
- Setting up Rust for the first time says "Rust set up (1.100.0)".
- **A possible secret the scan finds isn't reported as a StowLock bug:** it's in the project's files, so the report no longer offers a "fix StowLock" AI prompt for it.
- **StowLock's own source must pass its own secret scan** (a permanent test). A redaction test held a token-shaped example and blocked the push of this very release.

### Added
- **Your tools,** in Health: what the family's checks use (Rust through rustup, shellcheck, cargo-audit, cargo-deny, minisign, git, gh), which are installed, and their versions.
- **Install missing tools:** one button, in Your tools and on checks that couldn't run, that installs exactly what's missing with pacman after you approve your desktop's normal password prompt. Without a password prompt, it shows the command to copy instead.
- **Desktop notifications** when a push or GitHub's build finishes, while StowLock isn't the window in front (Settings → Updates; on by default).
- **`stowlock --report`** saves a bug report to a file (private things hidden), for when the window won't open.
- **The one-line installer has the standard options:** `--beta` (the default for now), `--stable`, `--version X`, `--uninstall`, `--no-launch` and `--from-source`. It checks the release's signature too, when minisign is installed. Without options it now opens StowLock when it's done. `STOWLOCK_CHANNEL` still works.
- **"Closed unexpectedly" is noticed even when nothing could be recorded** (the system stopped StowLock, it ran out of memory, the power went). The notice has Report and Dismiss. Crash reports from the last 10 sessions are kept.

### Changed
- **Updates no longer need curl.** StowLock downloads with its own HTTPS client, which uses your system's certificates and proxy settings (including `NO_PROXY`) and retries passing network problems. If the system's DNS doesn't answer, it asks a DNS-over-HTTPS service (1.1.1.1, then 8.8.8.8), sending only the host name. Private repos still update through your gh sign-in.
- **A new version must pass `--self-test --quick`** (it starts, loads its resources and reads settings, in seconds) before it replaces the current one. CI checks this on every push.
- **Settings have a version number and migrate themselves** when a future version changes them. A backup is kept at most every 15 minutes (the newest 5), and a damaged settings file falls back to the newest backup.
- **Bug report placeholders follow the House Standard:** `~`, `<user>`, `<host>`, `<ip>`, `<mac>`, `<secret>`, `<steamid>`, plus StowLock's own `<email>`, `<repo-N>`, `<account-N>`, `<workspace>` and `<serial>`. Windows-style home paths (`Z:\home\…`) are hidden too, and version numbers are never mistaken for IP addresses.

## [0.1.0-beta.10] - 2026-11-27

Checks that tell the truth, re-runs from StowLock, and safer releases.

### Fixed
- **Exit code 3 from a project's `scripts/check.sh` means "tools missing", not "failed"** (the House Standard). The result says "Can't run here", shows the script's full list of missing tools and its `sudo pacman -S --needed …` command, and the push isn't blocked. Before, it read as a failure with the list hidden.
- **No AI prompt when nothing failed.** The bug report only includes one when there's failure output to show.
- **Missing tools are named correctly,** whatever the wording ("is needed (found: none)", "isn't installed", "no such command"). A note no longer blames a GitHub-only install step when the output names the tool.
- **rustup without a chosen Rust** says so ("run: rustup default stable") instead of "rustc: not installed".
- **A project whose workflow can't run here still gets checked:** when it has `scripts/check.sh`, StowLock runs that. It never says "no tests" when the workflow has steps; it lists what only GitHub runs.
- **Clearer step counts:** jobs that run only on GitHub (an Arch container, macOS) are named instead of counted as steps not run ("Ran 8 of 10 GitHub steps here · "Arch" runs only on GitHub").
- **A lint failure counts as a real failure** ("could not compile"), with no misleading notes.
- **Staging the same zip again doesn't pile up saved attempts.** An identical attempt is kept once.

### Added
- **Re-run on GitHub** in the failed-build card: re-runs only the failed jobs, and StowLock watches again. **Check again** picks up a re-run started on GitHub. When the log shows a download or network error, the card says it's probably a passing GitHub problem and suggests the re-run first.
- **Tools a workflow installs with Cargo** (`cargo install cargo-audit --locked`) are installed here too, in StowLock's own shared tools folder, unless you already have them. `${{ env.NAME }}` values in tool-install steps are filled in, so pinned tools install as intended.
- **Rust update notice.** Once a day StowLock asks rustup whether a newer stable Rust is out, and offers **Update Rust** (it runs `rustup update` in the background). Settings → Updates shows your Rust, with Check Rust. The checks card says when your Rust is older than the one GitHub uses.
- **Check caches:** Settings → Checks shows how much space they use, with **Clear**. A project's cache is removed after 30 days without a check.

### Changed
- **Rust follows the newest stable release** (`rust-toolchain.toml`: `channel = "stable"`) on GitHub and, with rustup, on your computer.
- **`scripts/check.sh` follows the House Standard:** it checks every tool first and lists all the missing ones with the install command, and exits 0 (passed), 1 (a check failed) or 3 (only tools missing). New section: **sign**, a signing dry run.
- **Releases are safer:**
  - The signing secret's shape is checked in seconds, before any building. It says exactly what's wrong (only one line, the public key, a password-protected key) without printing it.
  - Signing lives in one script, `scripts/sign.sh`, used by the release and by a **signing dry run on every push**, so a broken signing step shows up on an ordinary push.
  - GitHub's machine is pinned to **Ubuntu 24.04** instead of "latest", so it only changes when we change it.

## [0.1.0-beta.9] - 2026-11-26

The first signed release. From here on, StowLock only installs updates that are signed with its own key.

### Added
- **Signed updates.** Every release's checksum list (`SHA256SUMS`) is signed with minisign in the release workflow, which fails before building if the signing secret is missing and checks the signatures against both public keys before anything is published. StowLock builds in two public keys, a primary and an offline backup, and accepts a signature from either.
- **StowLock refuses an update unless the checksum and the signature are both valid,** and the signature was made for that exact version (so an older signed release can't be passed off as a newer one). A release without a signature counts as "not available yet". Going back to the previous version still works, because it uses the copy kept on your computer.
- The self-test also checks that both signing keys are built in and that an unsigned download is refused.

### Notes
- This release is installed by the previous version's checksum-only updater; the signature check starts with the next release.
- The old `stowlock-linux-x86_64.sha256` file is still published (now signed too), so versions up to 0.1.0-beta.8 and the one-line installer keep working. It will stay until no supported version needs it; the cut-off will be noted here.

## [0.1.0-beta.8] - 2026-11-26

### Changed
- **GitHub steps run like GitHub's clean machine.** They get a fresh home folder and none of your desktop session's settings (display, theme, scaling, session bus), so tests don't pick up your own settings, app data or desktop. Tool caches (Rust, pip, npm, Go) are kept, so checks stay fast. You can turn this off in Settings → Checks.
- **When a project's release workflow publishes its own release, StowLock pushes the tag only** and shows "GitHub is building and publishing v1.2.0". Projects without such a workflow work as before.
- StowLock's own release workflow now creates a draft release with the CHANGELOG notes, attaches the files, then publishes, so a release is never visible without its files.
- **A step that can't run here no longer stops the others.** If one GitHub step needs a tool this computer doesn't have (shellcheck, say), StowLock notes it and runs the remaining steps. When everything that ran passed, the result is "Passed · 1 step can't run here (needs `shellcheck`; GitHub runs it)", and the install command is shown as optional. Before, the whole check read "Skipped" and the install command read like an order.
- Steps that write to GitHub's own files (`$GITHUB_PATH`, `$GITHUB_ENV`, `$GITHUB_OUTPUT`) are recognised as GitHub-only instead of failing here.
- **Tools a workflow installs with pip are installed here too,** in StowLock's own Python environment (for example the pinned `shellcheck-py`), so the checks that need them run on this computer instead of being skipped.
- A project's own check script that stops because a tool "isn't installed" (or "no such command") counts as "can't run here", with the project's own install advice, unless something really failed first (failed tests, a compile error, a formatting diff).
- Terminal colour codes no longer end up in logs and bug reports.
- Bug reports no longer mistake version numbers like `0.11.0.1` for IP addresses.
- Bug reports hide your home folder path even when it contains "stowlock" (it was missed when StowLock checked its own repo in its fresh home).
- **CI must pass first time (House Standard v1.2).** All checks live in `scripts/check.sh`. CI and the release workflow run its sections one by one (format, lint with warnings as errors, tests, release build, self-test, shell scripts with `sh -n` + shellcheck), so a local pass means a GitHub pass and a failure names what broke. The Rust toolchain (1.97.0, in `rust-toolchain.toml`) and shellcheck (0.11.0.1) are pinned. The release workflow also checks that the tag, Cargo.toml and the newest CHANGELOG entry agree, and deletes its own unfinished draft if anything fails.

### Fixed
- **Failures that look specific to this computer are flagged,** for example a test finding a running copy of the app or a path in your home folder. Copy for AI then asks to fix the tests' dependence on the machine, not the app's behaviour.
- The note "this step comes after a step only GitHub can run" only appears when that step could be the cause. It no longer appears when the tests ran and most of them passed.
- pytest's short summary ("5 failed, 1154 passed in 116s") now shows in the check result.
- In bug reports, every "lines left out" note keeps its time.

## [0.1.0-beta.7] - 2026-11-26

### Fixed
- **A missing tool no longer counts as your tests failing.** When a GitHub step needs a command that isn't installed here (exit code 127, for example `pytest`), Stage & check says "Can't run here", names the command and shows how to install it. The push isn't blocked: GitHub runs the step with everything installed, and StowLock watches the build. (Before beta.5, StowLock skipped pytest when it wasn't installed; matching GitHub's steps lost that.)
- A step that checks the result of an install step only GitHub can run (for example, a Qt library check after `apt-get install`) is marked "Can't run here" when it fails for something missing, instead of "your tests failed". A different failure after such a step still fails, with a note.
- **The AI fix prompt uses only the step that just failed.** It no longer includes output from earlier runs. Check output now shows earlier steps as one line each and the failing step in full.
- **Bug reports no longer reveal part of a private repo name** when one repo name contains another (for example `name` and `name-Preflight`). Repo names are matched whole and longest first, so each repo keeps one label.
- Bug reports keep the lines that explain a failure ("command not found", "No module named …", missing libraries). The rest of your project's output stays private.
- Logs show a GitHub step's name ("[test] Tests") instead of its whole script.
- Quick commands are timed correctly (they all showed about 26 ms) and finish without the extra wait.

### Changed
- **Python packages are installed the way GitHub does.** Steps like `pip install -r requirements.txt` now run here too, in StowLock's own Python environment for the project, never system-wide. Later steps use that environment.

## [0.1.0-beta.6] - 2026-11-26

### Changed
- **The "Existing…" repo menu lists only repos that are on GitHub right now.** It reloads when you open it, when StowLock comes back to the front, and after every push, and has a reload button. If GitHub can't be reached it says so and the list may be out of date. Folders on this computer only stand in when GitHub's list couldn't be loaded at all.

### Added
- **Projects flags folders whose repo was deleted on GitHub** ("Not on GitHub anymore"), with Remove the folder (moved to the Trash), Push it again as a new repo (keeps the folder's history), or Keep it.

### Fixed
- A zip whose repo was deleted on GitHub is no longer treated as an update (which failed with "repository not found"). StowLock says the repo was deleted and offers to create it again or use another name.

## [0.1.0-beta.5] - 2026-11-25

### Added
- **One flow from zip to release.** The version (and Stable or Beta) is read from the zip's own files. When the checks pass, the review window opens by itself. After pushing a new StowLock release, StowLock offers to update to it.
- **Latest on GitHub** beside the version box, with one-click **Next fix** and **Next feature**.
- **Older-version warning.** A version that counts as older than one already released (for example `v0.1.1-beta.1` after `v0.1.1`) is flagged, with one-click fixes. The review window says **Who gets this**, and an older version needs a "Push it anyway" tick.
- **Watches GitHub's build after every push:** live status, and on failure the failing step, Open the build log, Copy for AI, Delete the release and tag, or Keep it. A build that passed here but failed on GitHub is called out.
- **Checks that match GitHub's.** When a project has GitHub workflows, Stage & check runs their steps with the same commands and settings, and lists the steps only GitHub can run (installs, publishing, other systems). Optional exact mode runs them with `act` (needs Docker). Both are in Settings → Checks.
- **Clean up failed releases.** If the tag reached GitHub but the release didn't, choose Finish the release or Remove the tag. History has **Delete release…** (type the tag to confirm; the code stays).
- The one-line install is now at the very top of the README.

### Changed
- The update section explains itself: it tells Stable users when a newer beta exists and how to switch, and explains when GitHub's newest release counts as older than yours.
- Updates only offer releases that have the app attached, so a release whose build failed is never offered.

### Fixed
- A tag created locally is removed again if pushing it fails.

## [0.1.0-beta.4] - 2026-11-24

### Added
- **Push review:** a "Ready to push" window shows where the push goes, new or update, public or private, the version, what changed and the check results. The confirm button says exactly what it will do ("Push v0.2.0 to my-app") and ignores clicks for half a second, so a double-click can't push. New public repos need an "I understand this will be public" tick.
- A status label beside the push button: "Run checks first", "Blocked: …" or "Ready to push". After a push: View on GitHub and Stage another zip.
- **Copy for AI** when a project's tests fail: the failed tests and their messages, with a request to fix the project, ready to paste.
- Release notes come from the project's `CHANGELOG.md` section for the version, when there is one.
- `stowlock --update` checks for and installs a newer version from a terminal.
- Settings shows when StowLock last checked for updates and the newest version on GitHub.

### Changed
- **Graphite and emerald colours** in light and dark. Every text colour meets 4.5:1 contrast on every background, checked by a test.
- Updates download over plain HTTPS, so they work before you sign in to GitHub. For a private repository they fall back to your GitHub sign-in.
- Update errors say what's wrong (private repo, rate limit, no connection) instead of "error 404".
- The install command and `--install` give the exact PATH fix for your shell (for fish: `fish_add_path ~/.local/bin`).

### Fixed
- The AI fix prompt for a project's failing tests is about the project, not StowLock.
- Bug reports keep the failure message itself ("assertion failed …"), mark only failures as warnings, give one short note per command instead of blank lines and "[1 line left out]" markers, and show a repeated failure once.
- Health records Settings and the editor when you use them.

### Quality
- Automated click-through tests drive the real app: every screen, the theme and update channel, Stage & check, the review window's safeguards, the push and History.
- The Arch Linux test job builds and installs the AUR package with Arch's own tools.

## [0.1.0-beta.3] - 2026-11-24

### Added
- New design: a sidebar with Stage, Projects, History, Health, Bug report and Settings; Inter font and Lucide icons; light and dark themes that follow your desktop.
- One-line install: `curl -fsSL https://raw.githubusercontent.com/Matt-B-007/StowLock/main/install.sh | sh`.
- Update channel (Stable or Beta), "What's new" in the update notice, and going back to the previous version (Settings, or `stowlock --rollback`), offered automatically after a crash.
- Saved earlier attempts in Projects, with Restore and Delete.
- AUR package files with every release, and an Arch Linux test job.
- Safer next steps when tests fail, and the failed tests named in the summary.

### Changed
- Bug reports hide private details wherever they appear, leave out your project's file names and test chatter, hide your typed note, and accept your own words to hide.
- Build files are kept between checks of the same project, so re-checks are faster.

### Fixed
- No "tag already exists" warning right after a successful push.
- Health no longer blames StowLock for a project's failing tests or for safety refusals.
- Opening a newer copy while an older one is open now offers to switch.

## [0.1.0-beta.2] - 2026-11-23

### Added
- Download a bug report as a .txt file, named with the date and time.
- The version is read from the zip for Archive Only.

### Fixed
- Repo names with capital letters work, and existing repos keep their exact name.
- Layout: nothing overlaps at any window size.
- Loading a fixed zip after a failed check no longer gets stuck on "unsaved changes".
- Tests pass with newer git versions and on fast machines.

## [0.1.0-beta.1] - 2026-11-23

- First beta.
