#!/usr/bin/env bash
# Install the Parakeet engine into Talon on macOS / Linux.
# Symlinks this repo's plugin/ into ~/.talon/user/parakeet and builds the Rust sidecar.

set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_dir="$(cd "$script_dir/.." && pwd)"
plugin_src="$repo_dir/plugin"
sidecar_dir="$repo_dir/sidecar-rs"
talon_user="$HOME/.talon/user"
target="$talon_user/parakeet"

if [[ ! -d "$HOME/.talon" ]]; then
  echo "error: ~/.talon not found. Install Talon first." >&2
  exit 1
fi

if ! command -v cargo >/dev/null 2>&1; then
  echo "error: cargo not on PATH. Install Rust from https://rustup.rs and re-run." >&2
  exit 1
fi

mkdir -p "$talon_user"

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

echo "building sidecar (cargo build --release)"
( cd "$sidecar_dir" && cargo build --release )

bin="$sidecar_dir/target/release/parakeet-sidecar"
if [[ ! -x "$bin" ]]; then
  echo "error: expected binary at $bin" >&2
  exit 1
fi

echo
echo "done."
echo "binary: $bin ($(du -h "$bin" | cut -f1))"
echo "restart Talon (or 'touch $plugin_src/engine.py') to activate."
echo "on first run the sidecar downloads ~480 MB of Parakeet model weights."
