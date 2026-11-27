#!/bin/sh
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Every check StowLock's CI runs, in the same order, with the same flags.
# CI calls this script and nothing else for checks, so a pass here means a
# pass on GitHub. Run it before every release zip:
#
#   sh scripts/check.sh            # everything, in order
#   sh scripts/check.sh test       # one section (CI runs them one by one)
#
# Sections: fmt clippy test build selftest shell sign
#
# First it checks that every tool the chosen sections need is installed, and
# lists all the missing ones with the command to install them.
# Exit codes: 0 = passed, 1 = a check failed, 3 = only tools are missing
# (nothing was checked). No colour codes.

set -eu
cd "$(dirname "$0")/.."

# StowLock's own checks may build into a cache folder instead of ./target.
BIN="${CARGO_TARGET_DIR:-target}/release/stowlock"

# The shell scripts: all POSIX sh.
SHELL_SCRIPTS="install.sh packaging/aur/make-pkgbuild.sh scripts/check.sh scripts/sign.sh"

ALL_SECTIONS="fmt clippy test build selftest shell sign"

section() {
    printf '\n==> %s\n' "$1"
}

# ---- tools -------------------------------------------------------------

MISSING=""
PACKAGES=""
OTHER_FIXES=""

missing() {
    # missing TOOL PACKAGE [OTHER-FIX]
    MISSING="$MISSING $1"
    case " $PACKAGES " in
        *" $2 "*) ;;
        *) [ -z "$2" ] || PACKAGES="$PACKAGES $2" ;;
    esac
    if [ -n "${3:-}" ]; then
        OTHER_FIXES="$OTHER_FIXES
  $3"
    fi
}

check_rust() {
    if ! command -v cargo > /dev/null 2>&1; then
        missing cargo rustup "rustup default stable   (after installing rustup)"
        return
    fi
    if ! cargo --version > /dev/null 2>&1; then
        missing "a Rust version" "" "rustup default stable   (rustup has no Rust chosen yet)"
        return
    fi
    if [ "$1" = fmt ] && ! cargo fmt --version > /dev/null 2>&1; then
        missing rustfmt "" "rustup component add rustfmt"
    fi
    if [ "$1" = clippy ] && ! cargo clippy --version > /dev/null 2>&1; then
        missing clippy "" "rustup component add clippy"
    fi
}

need_tools() {
    for s in "$@"; do
        case "$s" in
            fmt|clippy|test|build) check_rust "$s" ;;
            selftest) ;;
            shell)
                command -v shellcheck > /dev/null 2>&1 || missing shellcheck shellcheck ;;
            sign)
                command -v minisign > /dev/null 2>&1 || missing minisign minisign ;;
            *)
                echo "Unknown section: $s (use: $ALL_SECTIONS)" >&2
                exit 2
                ;;
        esac
    done
    if [ -n "$MISSING" ]; then
        echo "Missing tools (nothing was checked):"
        if [ -n "$PACKAGES" ]; then
            echo "  Install them with: sudo pacman -S --needed$PACKAGES"
        fi
        if [ -n "$OTHER_FIXES" ]; then
            echo "  Then:$OTHER_FIXES"
        fi
        for t in $MISSING; do
            echo "  - $t"
        done
        exit 3
    fi
}

# ---- sections (each returns non-zero on the first problem) -------------

check_fmt() {
    section "Format (cargo fmt --check)"
    cargo fmt --all -- --check
}

check_clippy() {
    section "Lint (cargo clippy, warnings are errors)"
    cargo clippy --all-targets --locked -- -D warnings
}

check_test() {
    section "Tests (cargo test)"
    cargo test --locked
}

check_build() {
    section "Release build (cargo build --release)"
    cargo build --release --locked
}

check_selftest() {
    section "Self-test of the release build (--self-test --quick, then --self-test)"
    if [ ! -x "$BIN" ]; then
        echo "No release build at $BIN: run 'sh scripts/check.sh build' first."
        return 1
    fi
    # The quick one first: the updater runs it before swapping a new version
    # in, so every release must pass it.
    "$BIN" --self-test --quick || return 1
    "$BIN" --self-test
}

check_shell() {
    section "Shell scripts (sh -n + shellcheck -s sh)"
    for f in $SHELL_SCRIPTS; do
        sh -n "$f" || return 1
    done
    # Default severity: info-level findings fail too, as they do on GitHub.
    # shellcheck disable=SC2086 # the list is meant to split into file names
    shellcheck -s sh $SHELL_SCRIPTS
}

check_sign() {
    section "Signing dry run (throwaway key: sign, verify, refuse bad secrets)"
    sh scripts/sign.sh dry-run
}

run_section() {
    case "$1" in
        fmt) check_fmt ;;
        clippy) check_clippy ;;
        test) check_test ;;
        build) check_build ;;
        selftest) check_selftest ;;
        shell) check_shell ;;
        sign) check_sign ;;
    esac
}

if [ "$#" -eq 0 ]; then
    # shellcheck disable=SC2086 # split into section names on purpose
    set -- $ALL_SECTIONS
fi
need_tools "$@"
for s in "$@"; do
    if ! run_section "$s"; then
        printf '\nFAILED: %s\n' "$s"
        exit 1
    fi
done
printf '\nAll checks passed: %s\n' "$*"
