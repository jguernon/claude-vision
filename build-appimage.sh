#!/usr/bin/env bash
# Builds dist/ClaudeVision-<version>-linux-x86_64.AppImage.
#
# An AppImage is a small startup program (the AppImage type-2 runtime) followed by a
# squashfs image of the app folder. Requires cargo and mksquashfs (squashfs-tools).
set -euo pipefail

RUNTIME_RELEASE="20251108"

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$here"
version="$(grep -m1 '^version' Cargo.toml | cut -d'"' -f2)"
cache="${XDG_CACHE_HOME:-$HOME/.cache}/claude-vision-build"
runtime="$cache/runtime-x86_64-$RUNTIME_RELEASE"
mkdir -p "$cache"
if [[ ! -f "$runtime" ]]; then
  curl -fsSL -o "$runtime.part" \
    "https://github.com/AppImage/type2-runtime/releases/download/$RUNTIME_RELEASE/runtime-x86_64"
  mv "$runtime.part" "$runtime"
fi

cargo build --release

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
app="$work/AppDir"
mkdir -p "$app/usr/bin" "$app/usr/share/applications" \
         "$app/usr/share/icons/hicolor/256x256/apps" "$app/usr/share/icons/hicolor/scalable/apps"

install -m755 target/release/claude-vision "$app/usr/bin/claude-vision"
cp packaging/claude-vision.png "$app/claude-vision.png"
cp packaging/claude-vision.png "$app/usr/share/icons/hicolor/256x256/apps/claude-vision.png"
cp packaging/claude-vision.svg "$app/usr/share/icons/hicolor/scalable/apps/claude-vision.svg"
ln -s claude-vision.png "$app/.DirIcon"
sed "s|@BIN@|claude-vision|" packaging/claude-vision.desktop > "$app/claude-vision.desktop"
cp "$app/claude-vision.desktop" "$app/usr/share/applications/claude-vision.desktop"

cat > "$app/AppRun" <<'APPRUN'
#!/bin/sh
here="${APPDIR:-$(dirname "$(readlink -f "$0")")}"
# Without arguments, open the window (the AppImage must stay alive while it runs).
[ $# -eq 0 ] && set -- --window
exec "$here/usr/bin/claude-vision" "$@"
APPRUN
chmod +x "$app/AppRun"

mkdir -p dist
output="dist/ClaudeVision-$version-linux-x86_64.AppImage"
mksquashfs "$app" "$work/image.squashfs" -root-owned -noappend -comp zstd -Xcompression-level 19 -quiet -no-progress
cat "$runtime" "$work/image.squashfs" > "$output"
chmod +x "$output"
( cd dist && sha256sum "$(basename "$output")" > "$(basename "$output").sha256" )
echo "$output ($(du -h "$output" | cut -f1))"
cat "$output.sha256"
