#!/usr/bin/env bash
# Packs a built binary with everything an install needs into a release archive.
# Used by CI for releases, and by scripts/install.sh --from-source.
#   scripts/package.sh <cpu|vulkan> [binary] [out-dir]   → prints the archive path
set -euo pipefail
cd "$(dirname "$0")/.."

variant="${1:-}"
case "$variant" in
  cpu | vulkan) ;;
  *)
    echo "usage: scripts/package.sh <cpu|vulkan> [binary] [out-dir]" >&2
    exit 2
    ;;
esac
binary="${2:-target/release/claude-notch}"
out="${3:-dist}"
[[ -x "$binary" ]] || { echo "no binary at $binary: build it first" >&2; exit 1; }

version="$(sed -n 's/^version = "\(.*\)"$/\1/p' Cargo.toml | head -n 1)"
name="claude-notch-v${version}-linux-$(uname -m)-${variant}"
stage="$(mktemp -d)"
trap 'rm -rf "$stage"' EXIT

dir="$stage/$name"
install -Dm755 "$binary" "$dir/claude-notch"
install -Dm644 icons/icon.png "$dir/icon.png"
install -Dm644 packaging/claude-notch.service "$dir/claude-notch.service"
install -Dm644 packaging/claude-notch.desktop "$dir/claude-notch.desktop"
install -Dm755 scripts/download-model.sh "$dir/download-model.sh"
install -Dm644 README.md LICENSE -t "$dir"

mkdir -p "$out"
tar -C "$stage" --owner=0 --group=0 --numeric-owner -czf "$out/$name.tar.gz" "$name"
echo "$out/$name.tar.gz"
