#!/usr/bin/env bash
# Installe et active l'extension GNOME Shell « Claude Vision — quotas ».
#
# Copie le dossier claude-vision-tray@jguernon dans
# ~/.local/share/gnome-shell/extensions/ puis l'active.
# Sous Wayland, GNOME Shell ne découvre une nouvelle extension qu'à la
# prochaine ouverture de session : se déconnecter puis se reconnecter.

set -euo pipefail

UUID="claude-vision-tray@jguernon"
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SRC="$HERE/$UUID"
DEST_DIR="${XDG_DATA_HOME:-$HOME/.local/share}/gnome-shell/extensions"
DEST="$DEST_DIR/$UUID"

if [[ ! -f "$SRC/metadata.json" ]]; then
    echo "Source introuvable : $SRC" >&2
    exit 1
fi

mkdir -p "$DEST_DIR"
# Remplace seulement le dossier de cette extension-ci.
rm -rf -- "$DEST"
cp -r -- "$SRC" "$DEST"
echo "Copiée dans $DEST"

# Ajoute seulement cet UUID à la liste enabled-extensions (ce que fait le Shell
# lui-même pour « gnome-extensions enable »), sans toucher aux autres entrées.
enable_via_gsettings() {
    local cur new
    cur="$(gsettings get org.gnome.shell enabled-extensions)"
    if [[ "$cur" == *"'$UUID'"* ]]; then
        return 0
    fi
    if [[ "$cur" == "@as []" || "$cur" == "[]" ]]; then
        new="['$UUID']"
    else
        new="${cur%]}, '$UUID']"
    fi
    gsettings set org.gnome.shell enabled-extensions "$new"
}

if gnome-extensions enable "$UUID" 2>/dev/null; then
    echo "Activée : $UUID"
else
    # Sous Wayland, le Shell en cours ne voit pas une extension copiée après
    # l'ouverture de session : gnome-extensions enable répond « does not exist ».
    # On l'inscrit donc directement ; elle sera chargée à la prochaine session.
    enable_via_gsettings
    echo "Inscrite dans enabled-extensions : $UUID (chargée à la prochaine session)"
fi

if gsettings get org.gnome.shell disabled-extensions | grep -q "'$UUID'"; then
    echo "Attention : $UUID figure aussi dans disabled-extensions, qui a priorité." >&2
fi

if [[ "${XDG_SESSION_TYPE:-}" == "wayland" ]]; then
    echo "Session Wayland : se déconnecter puis se reconnecter pour que GNOME Shell charge l'extension."
fi
