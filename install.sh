#!/usr/bin/env bash
# Installe Claude Vision : binaire, lanceur d'application et icône (dans $HOME, sans sudo).
set -euo pipefail
cd "$(dirname "$0")"

cargo install --path . --force
BIN="$HOME/.cargo/bin/claude-vision"

mkdir -p "$HOME/.local/bin"
ln -sf "$BIN" "$HOME/.local/bin/claude-vision"

install -Dm644 packaging/claude-vision.svg \
  "$HOME/.local/share/icons/hicolor/scalable/apps/claude-vision.svg"
mkdir -p "$HOME/.local/share/applications"
sed "s|@BIN@|$BIN|" packaging/claude-vision.desktop \
  > "$HOME/.local/share/applications/claude-vision.desktop"

update-desktop-database "$HOME/.local/share/applications" 2>/dev/null || true
gtk-update-icon-cache -q -t -f "$HOME/.local/share/icons/hicolor" 2>/dev/null || true

echo "Claude Vision est installé. Cherche « Claude Vision » dans tes applications."
