#!/usr/bin/env bash
# Claude Notch: install or update with one command.
#
#   curl -fsSL https://raw.githubusercontent.com/omidiyanto/Claude-Code-Linux-Notch/main/scripts/install.sh | bash
#
# It installs the system libraries, the right release build (GPU/Vulkan when a dedicated GPU is
# found, CPU otherwise), a speech model, and a systemd user service that starts with the desktop.
# Run it again to update. Nothing runs as root except the package manager.
#
# Options (with curl: ... | bash -s -- --cpu):
#   --cpu | --gpu        force the CPU or the GPU (Vulkan) build
#   --model NAME         speech model: large-v3-turbo-q5_0 (GPU default), small-q5_1 (CPU default), small, base
#   --version vX.Y.Z     install that release instead of the latest
#   --archive FILE       install from a release archive already downloaded (offline)
#   --from-source        build from this checkout instead of downloading (run inside the repository)
#   --no-deps            do not install system packages
#   --no-service         do not install or start the systemd user service
#   --uninstall          remove Claude Notch; settings, models and history are kept
#   --purge              with --uninstall: also delete Claude Notch's settings, models and backups
set -euo pipefail

REPO="${CLAUDE_NOTCH_REPO:-omidiyanto/Claude-Code-Linux-Notch}"
CURL=(curl --proto '=https' --tlsv1.2 --fail --location --silent --show-error)
# The release builds are made on Ubuntu 24.04.
MIN_GLIBC="2.39"

BIN_DIR="$HOME/.local/bin"
ICON="${XDG_DATA_HOME:-$HOME/.local/share}/icons/hicolor/128x128/apps/claude-notch.png"
UNIT_DIR="${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user"
AUTOSTART="${XDG_CONFIG_HOME:-$HOME/.config}/autostart/claude-notch.desktop"
CONFIG_DIR="${XDG_CONFIG_HOME:-$HOME/.config}/claude-notch"
DATA_DIR="${XDG_DATA_HOME:-$HOME/.local/share}/claude-notch"

if [[ -t 1 ]]; then
  bold=$'\e[1m' green=$'\e[32m' yellow=$'\e[33m' red=$'\e[31m' reset=$'\e[0m'
else
  bold="" green="" yellow="" red="" reset=""
fi
step() { printf '\n%s==> %s%s\n' "$bold" "$*" "$reset"; }
info() { printf '    %s\n' "$*"; }
warn() { printf '%s    ! %s%s\n' "$yellow" "$*" "$reset" >&2; }
die() {
  printf '%s    ✗ %s%s\n' "$red" "$*" "$reset" >&2
  exit 1
}

usage() {
  if [[ -f "$0" ]]; then
    sed -n '2,/^set -euo/p' "$0" | sed -e '/^set -euo/d' -e 's/^# \{0,1\}//'
  else
    echo "Options: --cpu --gpu --model NAME --version vX.Y.Z --archive FILE --from-source --no-deps --no-service --uninstall [--purge]"
    echo "Details: https://github.com/$REPO/blob/main/scripts/install.sh"
  fi
  exit "${1:-0}"
}

# "2.39" >= "2.36"?
version_at_least() { [[ "$(printf '%s\n%s\n' "$2" "$1" | sort -V | head -n 1)" == "$2" ]]; }

has_systemd_user() { command -v systemctl >/dev/null && systemctl --user show-environment >/dev/null 2>&1; }

has_dedicated_gpu() {
  if command -v vulkaninfo >/dev/null && vulkaninfo --summary 2>/dev/null | grep -q DISCRETE_GPU; then
    return 0
  fi
  command -v lspci >/dev/null && lspci 2>/dev/null | grep -iE 'vga|3d|display' | grep -qiE 'nvidia|amd|radeon|ati '
}

package_manager() {
  for pm in pacman apt-get dnf zypper; do
    command -v "$pm" >/dev/null && { echo "$pm"; return; }
  done
  echo none
}

# System packages for running (and, with "build", for compiling) the given variant.
packages() {
  local pm="$1" variant="$2" build="$3"
  case "$pm" in
    pacman)
      echo webkit2gtk-4.1 gtk-layer-shell alsa-lib curl tar
      [[ "$variant" == vulkan ]] && echo vulkan-icd-loader
      [[ "$build" == build ]] && echo base-devel cmake clang
      [[ "$build" == build && "$variant" == vulkan ]] && echo vulkan-headers shaderc
      ;;
    apt-get)
      echo libwebkit2gtk-4.1-0 libgtk-layer-shell0 curl tar
      [[ "$variant" == vulkan ]] && echo libvulkan1
      [[ "$build" == build ]] && echo build-essential pkg-config cmake clang libclang-dev libwebkit2gtk-4.1-dev libgtk-layer-shell-dev libasound2-dev
      [[ "$build" == build && "$variant" == vulkan ]] && echo libvulkan-dev glslc
      ;;
    dnf)
      echo webkit2gtk4.1 gtk-layer-shell alsa-lib curl tar
      [[ "$variant" == vulkan ]] && echo vulkan-loader
      [[ "$build" == build ]] && echo gcc gcc-c++ make cmake clang pkgconf webkit2gtk4.1-devel gtk-layer-shell-devel alsa-lib-devel
      [[ "$build" == build && "$variant" == vulkan ]] && echo vulkan-headers vulkan-loader-devel glslc
      ;;
    zypper)
      echo libwebkit2gtk-4_1-0 gtk-layer-shell alsa curl tar
      [[ "$variant" == vulkan ]] && echo libvulkan1
      [[ "$build" == build ]] && echo gcc gcc-c++ make cmake clang pkgconf webkit2gtk3-soup2-devel gtk-layer-shell-devel alsa-devel
      [[ "$build" == build && "$variant" == vulkan ]] && echo vulkan-devel shaderc
      ;;
  esac
  return 0
}

install_packages() {
  local variant="$1" build="$2" pm
  pm="$(package_manager)"
  local -a pkgs
  read -r -a pkgs <<<"$(packages "$pm" "$variant" "$build" | tr '\n' ' ')"
  if [[ "$pm" == none || ${#pkgs[@]} -eq 0 ]]; then
    warn "Unknown package manager: install WebKitGTK 4.1, gtk-layer-shell and ALSA yourself."
    return
  fi
  local sudo=()
  [[ "$(id -u)" -ne 0 ]] && sudo=(sudo)
  info "${pm}: ${pkgs[*]}"
  case "$pm" in
    pacman) "${sudo[@]}" pacman -S --needed --noconfirm "${pkgs[@]}" ;;
    apt-get)
      "${sudo[@]}" apt-get update -qq
      # ALSA's runtime package was renamed in Ubuntu 24.04 / Debian 13.
      if apt-cache show libasound2t64 >/dev/null 2>&1; then pkgs+=(libasound2t64); else pkgs+=(libasound2); fi
      "${sudo[@]}" apt-get install -y --no-install-recommends "${pkgs[@]}"
      ;;
    dnf) "${sudo[@]}" dnf install -y "${pkgs[@]}" ;;
    zypper) "${sudo[@]}" zypper --non-interactive install "${pkgs[@]}" ;;
  esac
}

latest_version() {
  "${CURL[@]}" "https://api.github.com/repos/$REPO/releases/latest" |
    sed -n 's/.*"tag_name": *"\([^"]*\)".*/\1/p' | head -n 1
}

# Downloads the release archive for this variant and checks it against the release's SHA256SUMS.
download_release() {
  local version="$1" variant="$2" dir="$3" arch name
  arch="$(uname -m)"
  name="claude-notch-${version}-linux-${arch}-${variant}.tar.gz"
  local base="https://github.com/$REPO/releases/download/$version"
  info "$base/$name"
  "${CURL[@]}" -o "$dir/$name" "$base/$name" || die "No $variant build in release $version."
  "${CURL[@]}" -o "$dir/SHA256SUMS" "$base/SHA256SUMS" || die "Release $version has no SHA256SUMS."
  (cd "$dir" && grep -F " $name" SHA256SUMS | sha256sum --check --status) ||
    die "Checksum mismatch for $name: the download is corrupted or was tampered with."
  info "checksum verified"
  echo "$dir/$name"
}

build_from_source() {
  local variant="$1" dir="$2" repo
  repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." 2>/dev/null && pwd)" || true
  if [[ ! -f "$repo/Cargo.toml" ]] || ! grep -q '^name = "claude-notch"' "$repo/Cargo.toml"; then
    die "--from-source must be run from inside the repository (scripts/install.sh)."
  fi
  command -v cargo >/dev/null ||
    die "Rust is not installed: see https://rustup.rs, then run this again."
  local features=()
  [[ "$variant" == vulkan ]] && features=(--features vulkan)
  info "cargo build --release --locked ${features[*]}"
  (cd "$repo" && cargo build --release --locked "${features[@]}" >&2)
  (cd "$repo" && scripts/package.sh "$variant" target/release/claude-notch "$dir" | tail -n 1)
}

# Points config.toml at the speech model, keeping every other setting.
set_model_path() {
  local path="$1" file="$CONFIG_DIR/config.toml"
  mkdir -p "$CONFIG_DIR"
  chmod 700 "$CONFIG_DIR"
  if [[ ! -f "$file" ]]; then
    printf 'model_path = "%s"\n' "$path" >"$file"
  elif grep -q '^model_path *=' "$file"; then
    sed -i "s|^model_path *=.*|model_path = \"$path\"|" "$file"
  else
    # Top-level keys must come before any [table], so the new line goes first.
    sed -i "1i model_path = \"$path\"" "$file"
  fi
  chmod 600 "$file"
}

uninstall() {
  local purge="$1"
  step "Removing Claude Notch"
  if has_systemd_user; then
    systemctl --user disable --now claude-notch.service 2>/dev/null || true
  fi
  pkill -x claude-notch 2>/dev/null || true
  rm -f "$BIN_DIR/claude-notch" "$ICON" "$UNIT_DIR/claude-notch.service" "$AUTOSTART"
  has_systemd_user && systemctl --user daemon-reload
  if [[ "$purge" == yes ]]; then
    rm -rf "$CONFIG_DIR" "$DATA_DIR"
    info "Deleted $CONFIG_DIR and $DATA_DIR."
  else
    info "Kept your settings ($CONFIG_DIR) and models, workspace and backups ($DATA_DIR)."
  fi
  info "Claude Code and its settings (~/.claude) were not touched."
  printf '\n%sClaude Notch is uninstalled.%s\n' "$green" "$reset"
}

main() {
  local variant="" model="" version="" archive="" from_source=no deps=yes service=yes remove=no purge=no
  while [[ $# -gt 0 ]]; do
    case "$1" in
      --cpu) variant=cpu ;;
      --gpu) variant=vulkan ;;
      --model) model="${2:?--model needs a name}"; shift ;;
      --version) version="${2:?--version needs a tag like v0.3.0}"; shift ;;
      --archive) archive="${2:?--archive needs a file}"; shift ;;
      --from-source) from_source=yes ;;
      --no-deps) deps=no ;;
      --no-service) service=no ;;
      --uninstall) remove=yes ;;
      --purge) purge=yes ;;
      -h | --help) usage 0 ;;
      *) warn "unknown option $1"; usage 2 ;;
    esac
    shift
  done

  [[ "$(uname -s)" == Linux ]] || die "Claude Notch runs on Linux only."
  [[ "$(id -u)" -ne 0 ]] || die "Run this as your own user, not root: the notch runs in your desktop session."
  if [[ "$remove" == yes ]]; then
    uninstall "$purge"
    return
  fi
  [[ "$(uname -m)" == x86_64 || "$from_source" == yes ]] ||
    die "Release builds are for x86_64; on $(uname -m) use --from-source."

  if [[ -z "$variant" ]]; then
    if has_dedicated_gpu; then variant=vulkan; else variant=cpu; fi
  fi
  if [[ -z "$model" ]]; then
    if [[ "$variant" == vulkan ]]; then model=large-v3-turbo-q5_0; else model=small-q5_1; fi
  fi
  step "Claude Notch: $variant build, speech model $model"

  if [[ "$deps" == yes ]]; then
    step "Installing system packages"
    install_packages "$variant" "$([[ "$from_source" == yes ]] && echo build || echo run)"
  fi
  if ! command -v sha256sum >/dev/null || ! command -v tar >/dev/null; then
    die "sha256sum and tar are needed."
  fi

  local tmp
  tmp="$(mktemp -d)"
  trap 'rm -rf "$tmp"' EXIT

  step "Getting Claude Notch"
  if [[ -n "$archive" ]]; then
    [[ -f "$archive" ]] || die "No archive at $archive."
  elif [[ "$from_source" == yes ]]; then
    archive="$(build_from_source "$variant" "$tmp")"
  else
    local glibc
    glibc="$(ldd --version 2>/dev/null | head -n 1 | grep -oE '[0-9]+\.[0-9]+$' || true)"
    if [[ -n "$glibc" ]] && ! version_at_least "$glibc" "$MIN_GLIBC"; then
      die "This system has glibc $glibc; the release builds need $MIN_GLIBC or newer. Use --from-source."
    fi
    [[ -n "$version" ]] || version="$(latest_version)"
    [[ -n "$version" ]] || die "Could not find the latest release of $REPO."
    archive="$(download_release "$version" "$variant" "$tmp")"
  fi

  tar -xzf "$archive" -C "$tmp" --no-same-owner
  local src
  src="$(find "$tmp" -mindepth 2 -maxdepth 2 -name claude-notch -type f -printf '%h\n' | head -n 1)"
  [[ -n "$src" && -x "$src/claude-notch" ]] || die "The archive does not contain claude-notch."

  step "Installing"
  local running=no
  pgrep -x claude-notch >/dev/null && running=yes
  install -Dm755 "$src/claude-notch" "$BIN_DIR/claude-notch"
  install -Dm644 "$src/icon.png" "$ICON"
  info "$BIN_DIR/claude-notch ($("$BIN_DIR/claude-notch" --version))"

  step "Speech model"
  "$src/download-model.sh" "$model"
  [[ "$model" == small-q5_1 ]] || set_model_path "$DATA_DIR/models/ggml-$model.bin"

  if [[ "$service" == yes ]]; then
    step "Starting with your desktop"
    if has_systemd_user; then
      install -Dm644 "$src/claude-notch.service" "$UNIT_DIR/claude-notch.service"
      if [[ -f "$AUTOSTART" ]]; then
        rm -f "$AUTOSTART"
        info "Removed the old autostart entry: the systemd service replaces it."
      fi
      systemctl --user daemon-reload
      systemctl --user enable claude-notch.service >/dev/null
      systemctl --user restart claude-notch.service
      info "systemd user service enabled and (re)started: systemctl --user status claude-notch"
    else
      install -Dm644 "$src/claude-notch.desktop" "$AUTOSTART"
      info "No systemd user session: added an autostart entry instead ($AUTOSTART)."
      [[ "$running" == yes ]] && info "Restart Claude Notch to use the new version."
    fi
  fi

  step "Checks"
  if command -v claude >/dev/null || [[ -x "$HOME/.local/bin/claude" ]]; then
    info "Claude Code: $( (claude --version || "$HOME/.local/bin/claude" --version) 2>/dev/null | head -n 1)"
  else
    warn "Claude Code is not installed. Install it and sign in once:"
    warn "  curl -fsSL https://claude.ai/install.sh | bash && claude   (then /login)"
  fi
  case ":$PATH:" in
    *":$BIN_DIR:"*) ;;
    *) warn "$BIN_DIR is not on your PATH; add it to use the claude-notch command." ;;
  esac

  printf '\n%sClaude Notch is installed.%s Say "Hi Claude", or click the notch at the top of your screen.\n' "$green$bold" "$reset"
  cat <<EOF

    Push-to-talk shortcut:  bind the command "claude-notch listen" to a key in your desktop's
                            keyboard settings (KDE: System Settings → Keyboard → Shortcuts).
    Browser control:        install the Claude in Chrome extension in a Chromium-based browser,
                            then run "claude --chrome" once and choose /chrome → Enabled by default.
    Settings:               the ⚙ on the notch, or "claude-notch settings".
    Update:                 run this installer again.  Remove: add --uninstall.
EOF
}

main "$@"
