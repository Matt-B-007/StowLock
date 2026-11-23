<p align="center"><img src="assets/stowlock.svg" width="128" alt="StowLock icon"></p>

<h1 align="center">StowLock</h1>

<p align="center"><b>Drop in a project zip, get a clean, tested GitHub repo, with old versions neatly archived.</b></p>

StowLock is a small native Linux app (Rust + egui) for people who build projects with AI tools and get them back as zip files. Instead of a Downloads folder full of `project-final-v2 (3).zip` and git commands typed by hand, StowLock puts each project in its own folder, checks it, lets you try it, pushes it to the right GitHub repo (new or existing) and files the zip away.

Built for CachyOS and other Arch-based distributions; it runs on any x86_64 Linux desktop.

## What it does

- **Routes every zip to the right repo.** `sluice-v2.zip` updates your existing `sluice` repo instead of creating `sluice-v2`. The full target (`zip → folder → github.com/you/repo`) is shown before anything happens.
- **Three modes:** **New repo**, **Update** (syncs files into the existing repo, removes deleted files, keeps history) and **Archive only** (files an old zip without pushing).
- **Safe extraction.** Zip paths that try to escape the folder, symlinks and zip bombs are refused. A single wrapper folder (`myapp-main/`) is unwrapped.
- **Clean repos.** Removes OS junk, `node_modules`, `target`, virtualenvs and nested `.git` folders; writes a `.gitignore`; adds a starter README and an optional license for new repos. `.env` files are never deleted and never committed.
- **Checks before push.** Runs your project's tests (Rust, Node, Python, Go) and scans for leaked keys and tokens. Failing checks block the push unless you explicitly override (secret hits can't be overridden on a public repo).
- **Try it first.** Run the staged project in a terminal, fix it in your editor, re-check, then push exactly what's in the folder.
- **Stable or beta releases.** Optional version tags (`v0.2.0`, `v0.3.0-beta.1`) with GitHub releases; betas are marked as pre-releases.
- **Keeps Downloads empty.** After a successful push the zip moves to `~/GitHub_Projects/_archive/<project>/`.
- **Never force-pushes**, never overwrites unsaved local work, and stops if GitHub has changes you don't.
- **Pushed today** tracker, a **Projects** tab, a **Health** view and a built-in **self-test**.
- **No terminal needed:** installs git and GitHub's `gh` tool through your desktop's password dialog and signs you in with a one-time code in your browser.

## Install

### Download (recommended)

1. Download `stowlock-linux-x86_64` from the [latest release](https://github.com/Matt-B-007/stowlock/releases/latest).
2. Make it executable and install it:

   ```bash
   chmod +x stowlock-linux-x86_64
   ./stowlock-linux-x86_64 --install
   ```

StowLock now appears in your app menu, and "Open with StowLock" works on zip files. Future updates take two clicks inside the app.

### Build from source

```bash
sudo pacman -S --needed rust git github-cli
git clone https://github.com/Matt-B-007/stowlock.git
cd stowlock
cargo build --release
./target/release/stowlock --install
```

## First run

StowLock opens on the **GitHub account** panel:

1. **Install missing tools** (git and GitHub's `gh`), if needed. You'll see your desktop's normal password dialog.
2. **Sign in with GitHub.** StowLock shows a one-time code (already copied) and opens GitHub in your browser. Paste it and approve.
3. **Name and email for commits.** Optionally use GitHub's private email so your real address isn't on your commits.

Then choose a zip, click **Stage & check**, try it, and click **Push to GitHub**.

## Where things live

| Path | What |
| --- | --- |
| `~/GitHub_Projects/<project>/` | Your projects, one git repo each (configurable) |
| `~/GitHub_Projects/_archive/<project>/` | Uploaded and archived zips |
| `~/.config/stowlock/config.toml` | Settings |
| `~/.local/state/stowlock/` | Push history, logs, bug and self-test reports |
| `~/.cache/stowlock/` | Update downloads and the self-test sandbox |

StowLock never writes its own files into a project folder.

## Privacy

- Nothing leaves your computer except what you push to GitHub, the sign-in, your repo list and a release check against this repository (no personal data). No tracking, analytics or automatic crash uploads.
- StowLock never reads, stores or logs your GitHub token; sign-in is handled by `gh`.
- Its folders are readable only by your account. Secrets are blanked out the moment they're written to the log, and logs and reports are deleted after 30 days.
- Bug reports replace your username, GitHub account, repo names, email, computer name and folder paths with placeholders. You always see exactly what will be shared before you copy it or open an issue.

## Self-test

```bash
stowlock --self-test            # 23 checks in a sandbox; never touches your projects or GitHub
stowlock --self-test --online   # also read-only checks of your GitHub sign-in
```

The same test runs from **Health → Run self-test**. Failures come with a ready-to-paste "AI fix prompt".

## Uninstall

```bash
stowlock --uninstall                # app only: keeps settings, history and projects
stowlock --uninstall --purge        # also settings, history, logs and reports
stowlock --uninstall --everything   # also your projects folder and archived zips (to the Trash; type DELETE)
stowlock --uninstall --dry-run      # show what would be removed
```

The same three choices are in **Settings → Uninstall StowLock**. Your repos on GitHub, git, gh and your GitHub sign-in are never touched.

## Development

```bash
cargo test       # unit and end-to-end tests (against a fake GitHub made of local bare repos)
cargo run        # start the app
```

Pushing a `v*` tag builds `stowlock-linux-x86_64` and its checksum in GitHub Actions and attaches them to the release.

## License

StowLock is free software, licensed under the [GNU General Public License v3.0 or later](LICENSE).
