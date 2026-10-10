#!/usr/bin/env bash
# Install this portable rembrowser for the current user (no root):
#   ~/.local/opt/rembrowser, ~/.local/bin/rembrowser, a menu entry and icon.
# Uninstall: ./install.sh --uninstall
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
data=${XDG_DATA_HOME:-$HOME/.local/share}
dest=$HOME/.local/opt/rembrowser

if [ "${1:-}" = "--uninstall" ]; then
  rm -rf "$dest" "$HOME/.local/bin/rembrowser" "$data/applications/rembrowser.desktop" \
    "$data/icons/hicolor/scalable/apps/rembrowser.svg"
  echo "rembrowser removed (profile kept in $data/rembrowser)"
  exit 0
fi

rm -rf "$dest"
mkdir -p "$dest" "$HOME/.local/bin" "$data/applications" "$data/icons/hicolor/scalable/apps"
cp -a "$here"/. "$dest/"
ln -sf "$dest/rembrowser" "$HOME/.local/bin/rembrowser"
cp "$here/rembrowser.svg" "$data/icons/hicolor/scalable/apps/rembrowser.svg"
cat > "$data/applications/rembrowser.desktop" <<DESKTOP
[Desktop Entry]
Type=Application
Name=rembrowser
GenericName=Web Browser
Comment=Vim-native, privacy-first Chromium browser
Exec=$dest/rembrowser %U
Icon=rembrowser
Terminal=false
Categories=Network;WebBrowser;
MimeType=text/html;application/xhtml+xml;x-scheme-handler/http;x-scheme-handler/https;
StartupWMClass=rembrowser
DESKTOP
command -v update-desktop-database >/dev/null && update-desktop-database "$data/applications" || true
echo "installed: $dest  (run: rembrowser)"
case ":$PATH:" in *":$HOME/.local/bin:"*) ;; *) echo "note: add ~/.local/bin to PATH" ;; esac
