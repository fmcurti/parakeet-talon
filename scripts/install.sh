#!/usr/bin/env bash
# Install the local STT engines into Talon on macOS / Linux.
# 1. Symlinks this repo's plugin/ into ~/.talon/user/parakeet.
# 2. Downloads the prebuilt sidecar binaries (parakeet + qwen) from the latest
#    GitHub Release. If a prebuilt is missing, falls back to `cargo build
#    --release` (which builds the whole workspace).
#    Pass --build (or set FORCE_BUILD=1) to always build from source.

set -euo pipefail

GH_REPO="${PARAKEET_GH_REPO:-fmcurti/parakeet-talon}"

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_dir="$(cd "$script_dir/.." && pwd)"
plugin_src="$repo_dir/plugin"
sidecar_dir="$repo_dir/sidecar-rs"
release_dir="$sidecar_dir/target/release"
talon_user="$HOME/.talon/user"
target="$talon_user/parakeet"

# Sidecar binaries to install (one per engine).
BINS=(parakeet-sidecar qwen-sidecar)

force_build=0
for arg in "$@"; do
  case "$arg" in
    --build) force_build=1 ;;
    -h|--help)
      sed -n '2,7p' "$0" | sed 's/^# \{0,1\}//'
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

# --- Resolve the platform os/arch tags used in release asset names ---
platform_tag() {
  local os arch os_tag arch_tag
  os="$(uname -s)"
  arch="$(uname -m)"
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
  echo "${os_tag}-${arch_tag}"
}

install_prebuilt() {
  # $1 = binary name (e.g. parakeet-sidecar)
  local bin="$1" tag asset url tmp
  tag="$(platform_tag)" || return 1
  asset="${bin}-${tag}.tar.gz"
  url="https://github.com/${GH_REPO}/releases/latest/download/${asset}"
  tmp="$(mktemp -d)"
  echo "fetching $url"
  if ! curl -fLsS "$url" -o "$tmp/$asset"; then
    echo "  prebuilt not available for $bin"
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
  mkdir -p "$release_dir"
  tar xzf "$tmp/$asset" -C "$release_dir"
  chmod +x "$release_dir/$bin"
  # Strip the macOS quarantine flag so it runs without "right-click Open" prompt.
  if [[ "$(uname -s)" == "Darwin" ]]; then
    xattr -d com.apple.quarantine "$release_dir/$bin" 2>/dev/null || true
  fi
  rm -rf "$tmp"
  echo "installed prebuilt binary: $release_dir/$bin"
}

build_from_source() {
  if ! command -v cargo >/dev/null 2>&1; then
    echo "error: cargo not on PATH. Either install Rust from https://rustup.rs," >&2
    echo "       or ensure the prebuilt release is reachable." >&2
    exit 1
  fi
  echo "building sidecars (cargo build --release)"
  ( cd "$sidecar_dir" && cargo build --release )
}

if [[ "$force_build" == "1" ]]; then
  build_from_source
else
  need_build=0
  for bin in "${BINS[@]}"; do
    install_prebuilt "$bin" || need_build=1
  done
  if [[ "$need_build" == "1" ]]; then
    echo "one or more prebuilt binaries unavailable; building from source"
    build_from_source
  fi
fi

# --- Report what we ended up with ---
present=()
missing=()
for bin in "${BINS[@]}"; do
  if [[ -x "$release_dir/$bin" ]]; then present+=("$bin"); else missing+=("$bin"); fi
done
if [[ ${#present[@]} -eq 0 ]]; then
  echo "error: no sidecar binaries were installed under $release_dir" >&2
  exit 1
fi

echo
echo "done."
for bin in "${present[@]}"; do
  echo "binary: $release_dir/$bin ($(du -h "$release_dir/$bin" | cut -f1))"
done
if [[ ${#missing[@]} -gt 0 ]]; then
  echo "note: missing ${missing[*]} (that engine won't appear in Talon)"
fi
echo "restart Talon (or 'touch $plugin_src/engine.py'), then pick an engine from the tray menu."
echo "on first use each engine downloads its model: ~2.5 GB Parakeet, ~1.7 GB Qwen."
