#!/bin/sh
# SPDX-License-Identifier: GPL-3.0-or-later
# Install or update StowLock with one command:
#
#   curl -fsSL https://raw.githubusercontent.com/Matt-B-007/StowLock/main/install.sh | sh
#
# Options (after "sh -s --", for example: ... | sh -s -- --stable --no-launch):
#   --beta          the newest release, betas included (the default for now)
#   --stable        the newest full release only
#   --version X     a specific version, for example 0.1.0-beta.11
#   --uninstall     remove StowLock (keeps your settings, history and projects)
#   --no-launch     don't open StowLock afterwards
#   --from-source   build it from the source code (needs git and Rust)
#
# It installs for your user, so no password is needed. Downloads are checked
# against their published checksum, and against StowLock's signing keys when
# minisign is installed (sudo pacman -S minisign).
set -eu

REPO="Matt-B-007/StowLock"
ASSET="stowlock-linux-x86_64"
# StowLock's update signing keys (the same ones built into the app).
PRIMARY_KEY="RWQcs3jtP2jTiAMafwQX8UwciSjOH4o9tQcCVvYl2NkdnOeVupKPlhvC"
BACKUP_KEY="RWRObC9qlP598z1kmTFw7W+TKYKk3w2/63qWyRWkvRSkqGwGEgjmOlGe"

CHANNEL="${STOWLOCK_CHANNEL:-beta}"
WANT_TAG=""
MODE="install"
LAUNCH=1
SOURCE=0

say() { printf '%s\n' "$*"; }
fail() { printf 'StowLock install: %s\n' "$*" >&2; exit 1; }
usage() {
  say "Options: --beta (default), --stable, --version X, --uninstall, --no-launch, --from-source"
  say "Example: curl -fsSL https://raw.githubusercontent.com/$REPO/main/install.sh | sh -s -- --stable"
}

while [ "$#" -gt 0 ]; do
  case "$1" in
    --beta) CHANNEL="beta" ;;
    --stable) CHANNEL="stable" ;;
    --version)
      shift
      if [ "$#" -eq 0 ]; then
        fail "--version needs a version, for example: --version 0.1.0-beta.11"
      fi
      WANT_TAG="$1"
      ;;
    --version=*) WANT_TAG="${1#--version=}" ;;
    --uninstall) MODE="uninstall" ;;
    --no-launch) LAUNCH=0 ;;
    --from-source) SOURCE=1 ;;
    -h|--help) usage; exit 0 ;;
    *) fail "Unknown option: $1 (options: --beta --stable --version X --uninstall --no-launch --from-source)" ;;
  esac
  shift
done

if [ "$MODE" = "uninstall" ]; then
  BIN="${HOME}/.local/bin/stowlock"
  if [ -x "$BIN" ]; then
    exec "$BIN" --uninstall
  elif command -v stowlock >/dev/null 2>&1; then
    exec stowlock --uninstall
  fi
  fail "StowLock isn't installed for this user, so there's nothing to remove."
fi

[ "$(uname -s)" = "Linux" ] || fail "StowLock runs on Linux only."
[ "$(uname -m)" = "x86_64" ] || fail "StowLock is built for x86_64 computers only."
command -v curl >/dev/null 2>&1 || fail "curl is needed. Install it with: sudo pacman -S curl"
command -v sha256sum >/dev/null 2>&1 || fail "sha256sum is needed (part of coreutils)."

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT INT TERM

# Which release: a chosen version, the newest (betas included), or the newest
# full release.
if [ -n "$WANT_TAG" ]; then
  case "$WANT_TAG" in
    v*) TAG="$WANT_TAG" ;;
    *) TAG="v$WANT_TAG" ;;
  esac
else
  if [ "$CHANNEL" = "stable" ]; then
    API="https://api.github.com/repos/$REPO/releases/latest"
  else
    API="https://api.github.com/repos/$REPO/releases?per_page=1"
  fi
  if ! JSON="$(curl -fsSL "$API" 2>"$TMP/err")"; then
    case "$(cat "$TMP/err" 2>/dev/null)" in
      *404*) fail "GitHub didn't find StowLock's releases. The repository may be private or have no releases yet." ;;
      *403*|*429*) fail "GitHub is limiting requests from your network right now. Try again in an hour." ;;
      *) fail "Couldn't reach GitHub. Check your internet connection and try again." ;;
    esac
  fi
  TAG="$(printf '%s\n' "$JSON" | grep -m1 '"tag_name"' | sed -E 's/.*"tag_name": *"([^"]+)".*/\1/')"
  [ -n "$TAG" ] || fail "There's no StowLock release to install yet."
fi

launch() {
  if [ "$LAUNCH" -eq 1 ] && { [ -n "${WAYLAND_DISPLAY:-}" ] || [ -n "${DISPLAY:-}" ]; }; then
    say "Opening StowLock…"
    nohup "$1" >/dev/null 2>&1 &
  fi
}

if [ "$SOURCE" -eq 1 ]; then
  command -v git >/dev/null 2>&1 || fail "git is needed to build from source. Install it with: sudo pacman -S git"
  command -v cargo >/dev/null 2>&1 || fail "Rust is needed to build from source. Install it with: sudo pacman -S rustup, then: rustup default stable"
  say "Building StowLock $TAG from source (a few minutes)…"
  git clone --quiet --depth 1 --branch "$TAG" "https://github.com/$REPO.git" "$TMP/src" \
    || fail "Couldn't download the source code for $TAG."
  (cd "$TMP/src" && cargo build --release --locked) || fail "The build failed."
  # Where cargo put it (CARGO_TARGET_DIR, if you've set one).
  BUILT="${CARGO_TARGET_DIR:-$TMP/src/target}/release/stowlock"
  "$BUILT" --install
  say ""
  say "Done: StowLock $TAG built from source and installed."
  launch "${HOME}/.local/bin/stowlock"
  exit 0
fi

BASE="https://github.com/$REPO/releases/download/$TAG"
say "Downloading StowLock $TAG…"
curl -fsSL -o "$TMP/$ASSET" "$BASE/$ASSET" \
  || fail "The download failed. Check the version and your connection, then try again."

# Checksums: the signed list when the release has one, else the older file.
if curl -fsSL -o "$TMP/SHA256SUMS" "$BASE/SHA256SUMS" 2>/dev/null \
  && curl -fsSL -o "$TMP/SHA256SUMS.minisig" "$BASE/SHA256SUMS.minisig" 2>/dev/null; then
  if command -v minisign >/dev/null 2>&1; then
    if minisign -V -q -P "$PRIMARY_KEY" -m "$TMP/SHA256SUMS" 2>/dev/null; then
      say "Signature OK (primary key)."
    elif minisign -V -q -P "$BACKUP_KEY" -m "$TMP/SHA256SUMS" 2>/dev/null; then
      say "Signature OK (backup key)."
    else
      fail "The release's signature doesn't match StowLock's signing keys, so it wasn't installed."
    fi
    COMMENT="$(minisign -V -P "$PRIMARY_KEY" -m "$TMP/SHA256SUMS" 2>/dev/null || minisign -V -P "$BACKUP_KEY" -m "$TMP/SHA256SUMS" 2>/dev/null)" || true
    case "$COMMENT" in
      *"StowLock $TAG"*) ;;
      *) fail "The signature was made for a different release, not $TAG, so it wasn't installed." ;;
    esac
  else
    say "Note: install minisign (sudo pacman -S minisign) to also check the signature."
  fi
  WANT="$(grep " $ASSET\$" "$TMP/SHA256SUMS" | cut -d' ' -f1)"
else
  curl -fsSL -o "$TMP/$ASSET.sha256" "$BASE/$ASSET.sha256" \
    || fail "The release has no checksum, so it wasn't installed."
  WANT="$(cut -d' ' -f1 "$TMP/$ASSET.sha256")"
fi
GOT="$(sha256sum "$TMP/$ASSET" | cut -d' ' -f1)"
if [ -z "$WANT" ] || [ "$WANT" != "$GOT" ]; then
  fail "The download doesn't match its checksum, so it wasn't installed."
fi
say "Checksum OK."

chmod +x "$TMP/$ASSET"
"$TMP/$ASSET" --install

say ""
say "Done. Open StowLock from your app menu."
say "From now on, StowLock updates itself: Settings > Updates > Check now."
launch "${HOME}/.local/bin/stowlock"
