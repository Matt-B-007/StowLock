# Changelog

All notable changes to StowLock. When StowLock pushes a release, the section
for that version becomes the GitHub release notes and the "What's new" text in
the update notice.

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
