#!/usr/bin/env bash
# Install the Parakeet engine into Talon on macOS / Linux.
# Symlinks this repo's plugin/ into ~/.talon/user/parakeet and sets up a venv.

set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_dir="$(cd "$script_dir/.." && pwd)"
plugin_src="$repo_dir/plugin"
talon_user="$HOME/.talon/user"
target="$talon_user/parakeet"

if [[ ! -d "$HOME/.talon" ]]; then
  echo "error: ~/.talon not found. Install Talon first." >&2
  exit 1
fi

if ! command -v python3 >/dev/null 2>&1; then
  echo "error: python3 not on PATH." >&2
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

venv_dir="$plugin_src/.venv"
if [[ ! -x "$venv_dir/bin/python" ]]; then
  echo "creating venv at $venv_dir"
  python3 -m venv "$venv_dir"
fi

"$venv_dir/bin/pip" install --upgrade pip
"$venv_dir/bin/pip" install -r "$plugin_src/requirements.txt"

echo
echo "done."
echo "restart Talon (or 'touch $plugin_src/engine.py') to activate."
echo "select 'parakeet' in the tray Active Engine menu if it doesn't auto-pick."
