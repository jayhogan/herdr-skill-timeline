#!/bin/sh
# Symlink the release binary into ~/.local/bin so pane commands can run
# `herdr-skill-timeline` by name.
set -eu

root="$(cd "$(dirname "$0")/.." && pwd)"
bin="$root/target/release/herdr-skill-timeline"

[ -x "$bin" ] || cargo build --release --manifest-path "$root/Cargo.toml"

mkdir -p "$HOME/.local/bin"
ln -sf "$bin" "$HOME/.local/bin/herdr-skill-timeline"
echo "linked $HOME/.local/bin/herdr-skill-timeline -> $bin"

case ":$PATH:" in
  *":$HOME/.local/bin:"*) ;;
  *) echo "note: $HOME/.local/bin is not on PATH" >&2 ;;
esac
