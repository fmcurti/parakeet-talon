#!/usr/bin/env bash
# Install the Parakeet engine into Talon on macOS / Linux.
# 1. Symlinks this repo's plugin/ into ~/.talon/user/parakeet.
# 2. Downloads the prebuilt sidecar binary from the latest GitHub Release.
#    If no release matches this platform, falls back to `cargo build --release`.
#    Pass --build (or set FORCE_BUILD=1) to always build from source.

set -euo pipefail

GH_REPO="${PARAKEET_GH_REPO:-fmcurti/parakeet-talon}"

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_dir="$(cd "$script_dir/.." && pwd)"
plugin_src="$repo_dir/plugin"
sidecar_dir="$repo_dir/sidecar-rs"
talon_user="$HOME/.talon/user"
target="$talon_user/parakeet"

force_build=0
for arg in "$@"; do
  case "$arg" in
    --build) force_build=1 ;;
    -h|--help)
      sed -n '2,6p' "$0" | sed 's/^# \{0,1\}//'
      exit 0 ;;
  esac
done
if [[ "${FORCE_BUILD:-0}" == "1" ]]; then
  force_build=1
fi

if [[ ! -d "$HOME/.talon" ]]; then
  echo "error: ~/.talon not found. Install Talon first." >&2
  exit 1
fi

mkdir -p "$talon_user"

# --- Symlink plugin/ into Talon's user dir ---
if [[ -e "$target" || -L "$target" ]]; then
  if [[ -L "$target" && "$(readlink "$target")" == "$plugin_src" ]]; then
    echo "link already in place: $target -> $plugin_src"
  else
    backup="$target.bak.$(date +%s)"
    echo "moving existing $target -> $backup"
    mv "$target" "$backup"
    ln -s "$plugin_src" "$target"
    echo "linked $target -> $plugin_src"
  fi
else
  ln -s "$plugin_src" "$target"
  echo "linked $target -> $plugin_src"
fi

# --- Resolve release asset for this platform ---
detect_asset() {
  local os arch
  os="$(uname -s)"
  arch="$(uname -m)"
  local os_tag arch_tag
  case "$os" in
    Darwin) os_tag="macos" ;;
    Linux)  os_tag="linux" ;;
    *) echo "error: unsupported OS: $os" >&2; return 1 ;;
  esac
  case "$arch" in
    arm64|aarch64) arch_tag="aarch64" ;;
    x86_64|amd64)  arch_tag="x86_64" ;;
    *) echo "error: unsupported arch: $arch" >&2; return 1 ;;
  esac
  echo "parakeet-sidecar-${os_tag}-${arch_tag}.tar.gz"
}

bin_out="$sidecar_dir/target/release/parakeet-sidecar"

install_prebuilt() {
  local asset url tmp
  asset="$(detect_asset)" || return 1
  url="https://github.com/${GH_REPO}/releases/latest/download/${asset}"
  tmp="$(mktemp -d)"
  echo "fetching $url"
  if ! curl -fLsS "$url" -o "$tmp/$asset"; then
    echo "  prebuilt not available (repo may have no release yet)"
    rm -rf "$tmp"
    return 1
  fi
  # Optional checksum verification.
  if curl -fLsS "${url}.sha256" -o "$tmp/$asset.sha256" 2>/dev/null; then
    ( cd "$tmp" && shasum -a 256 -c "$asset.sha256" ) >/dev/null || {
      echo "error: checksum mismatch for $asset" >&2
      rm -rf "$tmp"; return 1
    }
  fi
  mkdir -p "$sidecar_dir/target/release"
  tar xzf "$tmp/$asset" -C "$sidecar_dir/target/release"
  chmod +x "$bin_out"
  # Strip the macOS quarantine flag so it runs without "right-click Open" prompt.
  if [[ "$(uname -s)" == "Darwin" ]]; then
    xattr -d com.apple.quarantine "$bin_out" 2>/dev/null || true
  fi
  rm -rf "$tmp"
  echo "installed prebuilt binary: $bin_out"
}

build_from_source() {
  if ! command -v cargo >/dev/null 2>&1; then
    echo "error: cargo not on PATH. Either install Rust from https://rustup.rs," >&2
    echo "       or ensure the prebuilt release is reachable." >&2
    exit 1
  fi
  echo "building sidecar (cargo build --release)"
  ( cd "$sidecar_dir" && cargo build --release )
}

if [[ "$force_build" == "1" ]]; then
  build_from_source
else
  install_prebuilt || build_from_source
fi

if [[ ! -x "$bin_out" ]]; then
  echo "error: expected binary at $bin_out" >&2
  exit 1
fi

echo
echo "done."
echo "binary: $bin_out ($(du -h "$bin_out" | cut -f1))"
echo "restart Talon (or 'touch $plugin_src/engine.py') to activate."
echo "on first run the sidecar downloads ~2.5 GB of Parakeet model weights."
