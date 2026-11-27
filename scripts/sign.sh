#!/bin/sh
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Signing for StowLock releases (minisign). One script for the release
# workflow and for the dry run CI does on every push, so a broken signing
# step shows up on an ordinary push, not on release day.
#
#   sh scripts/sign.sh check-secret      is MINISIGN_SECRET_KEY shaped right?
#   sh scripts/sign.sh sign TAG FILE...  sign with MINISIGN_SECRET_KEY
#   sh scripts/sign.sh verify FILE...    check FILE.minisig against the
#                                        built-in public keys (either one)
#   sh scripts/sign.sh dry-run           all of the above with a throwaway key
#
# The secret key is only ever read from the environment, written to a
# private temporary file while signing, and deleted. It is never printed.

set -eu
cd "$(dirname "$0")/.."

APP="StowLock"
PRIMARY_PUB_FILE="${PRIMARY_PUB_FILE:-packaging/minisign/primary.pub}"
BACKUP_PUB_FILE="${BACKUP_PUB_FILE:-packaging/minisign/backup.pub}"
REPO="${GITHUB_REPOSITORY:-Matt-B-007/StowLock}"

say() {
    printf '%s\n' "$*"
}

# The key line of a .pub or .key file (the line that isn't the comment).
key_line() {
    grep -v '^untrusted comment:' "$1" | grep -m1 .
}

secret_fix_hint() {
    say "Set it straight from the key file so nothing gets cut off:"
    say "  gh secret set MINISIGN_SECRET_KEY -R $REPO < primary.key"
}

# Check the secret's shape without printing any of it.
check_secret() {
    secret="${MINISIGN_SECRET_KEY:-}"
    if [ -z "$secret" ]; then
        say "MINISIGN_SECRET_KEY secret is missing, so this release can't be signed."
        say "Add it in Settings > Secrets and variables > Actions, then re-run."
        secret_fix_hint
        return 1
    fi
    clean="$(printf '%s\n' "$secret" | tr -d '\r')"
    lines="$(printf '%s\n' "$clean" | grep -c .)" || true
    first="$(printf '%s\n' "$clean" | grep -m1 .)" || true
    second="$(printf '%s\n' "$clean" | grep . | sed -n 2p)" || true
    problem=""
    case "$first" in
        "untrusted comment: minisign public key"*)
            problem="it holds the PUBLIC key; it needs the secret key file (primary.key)" ;;
        "untrusted comment:"*)
            if [ "$lines" -lt 2 ]; then
                problem="it has only the comment line; the key line under it is missing"
            elif [ "$lines" -gt 2 ]; then
                problem="it has $lines lines; a minisign key file has 2"
            fi
            ;;
        RW*)
            problem="it has only the key line; the 'untrusted comment:' line above it is missing" ;;
        *)
            problem="it doesn't start with 'untrusted comment:', so it isn't a minisign key file" ;;
    esac
    if [ -z "$problem" ]; then
        case "$second" in
            RWRTY*) problem="the key has a password; the workflow needs a key made without one (minisign -G -W)" ;;
            RWQAAEIy*) ;;
            RW*) problem="the second line isn't a minisign secret key (it may be a public key)" ;;
            *) problem="the second line isn't a minisign key" ;;
        esac
    fi
    if [ -n "$problem" ]; then
        say "The MINISIGN_SECRET_KEY secret is wrong: $problem."
        secret_fix_hint
        return 1
    fi
    say "The MINISIGN_SECRET_KEY secret looks right (2 lines, no password)."
}

sign_files() {
    tag="$1"
    shift
    check_secret || return 1
    key="$(mktemp "${RUNNER_TEMP:-${TMPDIR:-/tmp}}/minisign.XXXXXX")"
    trap 'rm -f "$key"' EXIT
    chmod 600 "$key"
    printf '%s\n' "$MINISIGN_SECRET_KEY" | tr -d '\r' > "$key"
    for f in "$@"; do
        if ! minisign -S -s "$key" -m "$f" -t "$APP $tag" < /dev/null; then
            say "Signing $f failed. If it said the secret key couldn't be loaded, the"
            say "MINISIGN_SECRET_KEY secret's contents are wrong; the workflow is fine."
            secret_fix_hint
            rm -f "$key"
            return 1
        fi
        say "Signed $f"
    done
    rm -f "$key"
}

verify_files() {
    primary="$(key_line "$PRIMARY_PUB_FILE")"
    backup="$(key_line "$BACKUP_PUB_FILE")"
    for f in "$@"; do
        if minisign -V -q -P "$primary" -m "$f" 2>/dev/null; then
            say "$f: signed with the primary key"
        elif minisign -V -q -P "$backup" -m "$f" 2>/dev/null; then
            say "$f: signed with the backup key"
        else
            say "$f.minisig doesn't verify with either public key, so nothing was published."
            return 1
        fi
    done
}

# Sign and verify with a throwaway key, and make sure bad secrets and
# strangers' signatures are refused. Runs on every push.
dry_run() {
    work="$(mktemp -d "${TMPDIR:-/tmp}/sign-dry-run.XXXXXX")"
    trap 'rm -rf "$work"' EXIT
    minisign -G -W -f -p "$work/test.pub" -s "$work/test.key" > /dev/null
    printf 'abc  stowlock-linux-x86_64\n' > "$work/SHA256SUMS"
    MINISIGN_SECRET_KEY="$(cat "$work/test.key")"
    export MINISIGN_SECRET_KEY
    sign_files v0.0.0-dry-run "$work/SHA256SUMS" > /dev/null || return 1
    if ! PRIMARY_PUB_FILE="$work/test.pub" verify_files "$work/SHA256SUMS" > /dev/null; then
        say "Dry run: a signature from the test key didn't verify."
        return 1
    fi
    # The real keys must refuse a stranger's signature.
    if verify_files "$work/SHA256SUMS" > /dev/null 2>&1; then
        say "Dry run: StowLock's public keys accepted a stranger's signature."
        return 1
    fi
    # Common paste mistakes must be caught before signing.
    only_key="$(key_line "$work/test.key")"
    for bad in "" "$only_key" "$(cat "$work/test.pub")"; do
        if MINISIGN_SECRET_KEY="$bad" check_secret > /dev/null 2>&1; then
            say "Dry run: a wrongly pasted secret wasn't caught."
            return 1
        fi
    done
    say "Signing dry run passed: sign, verify with either key, refuse strangers and bad secrets."
}

case "${1:-}" in
    check-secret) check_secret ;;
    sign)
        shift
        [ "$#" -ge 2 ] || { say "Usage: sh scripts/sign.sh sign TAG FILE..."; exit 2; }
        sign_files "$@"
        ;;
    verify)
        shift
        verify_files "$@"
        ;;
    dry-run) dry_run ;;
    *)
        say "Usage: sh scripts/sign.sh check-secret | sign TAG FILE... | verify FILE... | dry-run"
        exit 2
        ;;
esac
