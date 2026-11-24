#!/bin/sh
# SPDX-License-Identifier: GPL-3.0-or-later
# Install or update StowLock with one command:
#
#   curl -fsSL https://raw.githubusercontent.com/Matt-B-007/StowLock/main/install.sh | sh
#
# It downloads the newest release (betas included while StowLock is in beta;
# set STOWLOCK_CHANNEL=stable for full releases only), checks it against its
# published checksum, and installs it for your user (no password needed).
set -eu

REPO="Matt-B-007/StowLock"
CHANNEL="${STOWLOCK_CHANNEL:-beta}"
ASSET="stowlock-linux-x86_64"

say() { printf '%s\n' "$*"; }
fail() { printf 'StowLock install: %s\n' "$*" >&2; exit 1; }

[ "$(uname -s)" = "Linux" ] || fail "StowLock runs on Linux only."
[ "$(uname -m)" = "x86_64" ] || fail "StowLock is built for x86_64 computers only."
command -v curl >/dev/null 2>&1 || fail "curl is needed. Install it with: sudo pacman -S curl"
command -v sha256sum >/dev/null 2>&1 || fail "sha256sum is needed (part of coreutils)."

# Which release: the newest one (betas included), or the newest full release.
if [ "$CHANNEL" = "stable" ]; then
  API="https://api.github.com/repos/$REPO/releases/latest"
else
  API="https://api.github.com/repos/$REPO/releases?per_page=1"
fi
TAG="$(curl -fsSL "$API" | grep -m1 '"tag_name"' | sed -E 's/.*"tag_name": *"([^"]+)".*/\1/')" \
  || fail "Couldn't reach GitHub to find the newest release."
[ -n "$TAG" ] || fail "Couldn't find a release to install."

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT INT TERM
BASE="https://github.com/$REPO/releases/download/$TAG"

say "Downloading StowLock $TAG…"
curl -fsSL -o "$TMP/$ASSET" "$BASE/$ASSET" || fail "The download failed. Check your connection and try again."
curl -fsSL -o "$TMP/$ASSET.sha256" "$BASE/$ASSET.sha256" || fail "The release has no checksum, so it wasn't installed."

WANT="$(cut -d' ' -f1 "$TMP/$ASSET.sha256")"
GOT="$(sha256sum "$TMP/$ASSET" | cut -d' ' -f1)"
[ "$WANT" = "$GOT" ] || fail "The download doesn't match its checksum, so it wasn't installed."
say "Checksum OK."

chmod +x "$TMP/$ASSET"
"$TMP/$ASSET" --install

say ""
say "Done. Open StowLock from your app menu."
say "From now on, StowLock updates itself: Settings > Updates > Check now."
