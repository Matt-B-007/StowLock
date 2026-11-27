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
# Sections: fmt clippy test build selftest shell
# Tools: Rust from rust-toolchain.toml (with clippy and rustfmt) and the
# pinned ShellCheck (in CI: pip install shellcheck-py==0.11.0.1).

set -eu
cd "$(dirname "$0")/.."

# StowLock's own checks may build into a cache folder instead of ./target.
BIN="${CARGO_TARGET_DIR:-target}/release/stowlock"

# The shell scripts: all POSIX sh.
SHELL_SCRIPTS="install.sh packaging/aur/make-pkgbuild.sh scripts/check.sh"

section() {
    printf '\n==> %s\n' "$1"
}

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
    section "Self-test of the release build (--self-test)"
    if [ ! -x "$BIN" ]; then
        echo "No release build at $BIN: run 'sh scripts/check.sh build' first." >&2
        exit 1
    fi
    "$BIN" --self-test
}

check_shell() {
    section "Shell scripts (sh -n + shellcheck -s sh)"
    for f in $SHELL_SCRIPTS; do
        sh -n "$f"
    done
    if ! command -v shellcheck >/dev/null 2>&1; then
        # Same wording and exit code as the shell, so StowLock can tell
        # "not installed here" apart from a finding.
        echo "shellcheck: command not found" >&2
        exit 127
    fi
    # Default severity: info-level findings fail too, as they do on GitHub.
    # shellcheck disable=SC2086 # the list is meant to split into file names
    shellcheck -s sh $SHELL_SCRIPTS
}

run_section() {
    case "$1" in
        fmt) check_fmt ;;
        clippy) check_clippy ;;
        test) check_test ;;
        build) check_build ;;
        selftest) check_selftest ;;
        shell) check_shell ;;
        *)
            echo "Unknown section: $1 (use fmt, clippy, test, build, selftest or shell)" >&2
            exit 2
            ;;
    esac
}

if [ "$#" -eq 0 ]; then
    set -- fmt clippy test build selftest shell
fi
for s in "$@"; do
    run_section "$s"
done
printf '\nAll checks passed: %s\n' "$*"
