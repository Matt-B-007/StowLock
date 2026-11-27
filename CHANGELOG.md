# Changelog

All notable changes to StowLock. When StowLock pushes a release, the section
for that version becomes the GitHub release notes and the "What's new" text in
the update notice.

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
