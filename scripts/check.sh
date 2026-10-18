#!/usr/bin/env bash
# Every gate a change must pass: formatting, lints (warnings are errors), unit tests and
# dependency audits, for the Rust app and the notch page. Any failure stops the script.
#   scripts/check.sh          the full gate
#   scripts/check.sh --slow   plus the end-to-end speech tests (needs the model and espeak-ng)
set -euo pipefail
cd "$(dirname "$0")/.."

step() { printf '\n\033[1m==> %s\033[0m\n' "$*"; }

step "rustfmt"
cargo fmt --all -- --check

step "clippy (all + pedantic, warnings are errors)"
cargo clippy --all-targets --locked -- -D warnings

step "rust tests"
cargo test --locked

if [[ "${1:-}" == "--slow" ]]; then
  step "end-to-end speech tests"
  cargo test --release --locked -- --ignored --test-threads 1
fi

# Fails on any known vulnerability. Two notices remain, both pulled in by Tauri 2's GTK 3 stack
# and not fixable here: proc-macro-error is unmaintained (RUSTSEC-2024-0370), and glib's
# VariantStrIter is unsound (RUSTSEC-2024-0429), an API this app never calls.
step "cargo audit"
cargo audit

step "npm ci"
npm ci --no-audit --no-fund --silent

step "eslint"
npm run --silent lint

step "ui tests"
npm test --silent

step "npm audit"
npm audit --audit-level=low

step "shellcheck"
shellcheck scripts/*.sh

step "actionlint"
actionlint

step "systemd unit + desktop entry"
unit_dir="$(mktemp -d)"
trap 'rm -rf "$unit_dir"' EXIT
# Verified against /usr/bin/true: the real binary only exists once installed. System mode, so it
# also runs where there is no user session (CI).
sed 's|^ExecStart=.*|ExecStart=/usr/bin/true|' packaging/claude-notch.service > "$unit_dir/claude-notch.service"
systemd-analyze verify "$unit_dir/claude-notch.service"
desktop-file-validate packaging/claude-notch.desktop

printf '\n\033[1;32mAll checks passed.\033[0m\n'
