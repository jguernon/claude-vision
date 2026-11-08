#!/usr/bin/env bash
# Adds a launcher for another Claude Vision instance, with its own app identity so GNOME
# (or any window manager) can place it on its own workspace.
#
#   packaging/install-instance.sh NAME [ACCOUNT] [COLOR]
#
# Example: packaging/install-instance.sh work work red
#   -> "Claude Vision WORK" (app id claude-vision-work), showing only the account whose
#      label or folder contains "work", with a red banner and a red-banner icon.
#
# COLOR: blue, red, green, cyan, magenta, brown, gray or #RRGGBB (default: red).
set -euo pipefail

name="${1:?usage: install-instance.sh NAME [ACCOUNT] [COLOR]}"
account="${2:-}"
color="${3:-red}"
[[ "$name" =~ ^[a-zA-Z0-9-]+$ ]] || { echo "NAME: letters, digits and dashes only" >&2; exit 1; }
name="${name,,}"
case "${color,,}" in
  red|rouge) hex='#AA0000' ;;
  blue|bleu) hex='#0000AA' ;;
  green|vert) hex='#00AA00' ;;
  cyan) hex='#00AAAA' ;;
  magenta) hex='#AA00AA' ;;
  brown|brun|orange) hex='#AA5500' ;;
  gray|grey|gris) hex='#555555' ;;
  \#[0-9a-fA-F][0-9a-fA-F][0-9a-fA-F][0-9a-fA-F][0-9a-fA-F][0-9a-fA-F]) hex="$color" ;;
  *) echo "COLOR: blue, red, green, cyan, magenta, brown, gray or #RRGGBB" >&2; exit 1 ;;
esac

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
bin="${CLAUDE_VISION_BIN:-$HOME/.cargo/bin/claude-vision}"
id="claude-vision-$name"
upper="${name^^}"

icons="$HOME/.local/share/icons/hicolor/scalable/apps"
apps="$HOME/.local/share/applications"
mkdir -p "$icons" "$apps"
sed "s/#0000AA/$hex/" "$here/claude-vision.svg" > "$icons/$id.svg"

exec_line="$bin --window --instance $name --accent $color"
[[ -n "$account" ]] && exec_line+=" --account $account"

cat > "$apps/$id.desktop" <<DESKTOP
[Desktop Entry]
Type=Application
Name=Claude Vision $upper
GenericName=Claude session monitor
GenericName[fr]=Moniteur de sessions Claude
Comment=Live Claude Code sessions, agents, data flow and quotas (EGA palette)
Comment[fr]=Sessions Claude Code, agents, flux et quotas en temps réel (palette EGA)
Exec=$exec_line
Icon=$id
Terminal=false
Categories=Development;Monitor;
Keywords=claude;agents;sessions;quotas;ega;
StartupWMClass=$id
StartupNotify=true
DESKTOP

update-desktop-database "$apps" 2>/dev/null || true
gtk-update-icon-cache -q -t -f "$HOME/.local/share/icons/hicolor" 2>/dev/null || true
echo "Installed \"Claude Vision $upper\" ($apps/$id.desktop)"
echo "  Exec: $exec_line"
