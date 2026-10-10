#!/usr/bin/env bash
# rembrowser installer
#
#   curl -fsSL https://raw.githubusercontent.com/nganaremba-rem/rembrowser/main/install.sh | bash
#
# Arch Linux (and Arch-based): builds the native package — system Chromium engine
#   (`cef`), all codecs incl. H.264. pacman pulls every dependency.
# Any other x86_64 Linux (glibc >= 2.34): the portable bundle from the latest release,
#   installed for your user in ~/.local (no root needed).
#
# REMBROWSER_PORTABLE=1  use the portable bundle on Arch too.
set -euo pipefail

REPO=nganaremba-rem/rembrowser

say() { printf '\033[1;35m==>\033[0m \033[1m%s\033[0m\n' "$*"; }
die() { printf '\033[1;31merror:\033[0m %s\n' "$*" >&2; exit 1; }
need() { command -v "$1" >/dev/null 2>&1 || die "'$1' is required — install it and run this again"; }

[ "$(uname -s)" = Linux ] || die "rembrowser runs on Linux only"
[ "$(uname -m)" = x86_64 ] || die "only x86_64 is supported for now"
need curl

arch_install() {
    say "Arch Linux: building the rembrowser package"
    echo "    (first build also downloads ~300 MB of Chromium headers for linking; takes a few minutes)"
    sudo pacman -S --needed --noconfirm base-devel git
    local src="${REMBROWSER_SRC:-$HOME/.local/src/rembrowser}"
    if [ -d "$src/.git" ]; then
        git -C "$src" pull --ff-only
    else
        mkdir -p "$(dirname "$src")"
        git clone --depth 1 "https://github.com/$REPO" "$src"
    fi
    # -s: pacman installs the dependencies (cef, cargo, ...); -i: installs the package.
    (cd "$src/packaging/arch" && makepkg -sif --noconfirm)
}

portable_install() {
    need tar
    need xz
    local glibc
    glibc=$(ldd --version 2>/dev/null | head -1 | grep -oE '[0-9]+\.[0-9]+$' || true)
    if [ -n "$glibc" ] && [ "$(printf '%s\n2.34\n' "$glibc" | sort -V | head -1)" != 2.34 ]; then
        die "glibc $glibc is too old (needs 2.34+: Ubuntu 22.04, Debian 12, Fedora 35 or newer)"
    fi
    say "Downloading the latest portable rembrowser"
    local url
    url=$(curl -fsSL "https://api.github.com/repos/$REPO/releases/latest" |
        grep -oE '"browser_download_url": *"[^"]*x86_64-linux\.tar\.xz"' | head -1 | cut -d'"' -f4)
    [ -n "$url" ] || die "no portable release found at github.com/$REPO/releases"
    # Global, not local: the EXIT trap runs after this function returns.
    tmp=$(mktemp -d)
    trap 'rm -rf "$tmp"' EXIT
    curl -fL --progress-bar "$url" -o "$tmp/rembrowser.tar.xz"
    say "Unpacking"
    tar -C "$tmp" -xJf "$tmp/rembrowser.tar.xz"
    local dir
    dir=$(find "$tmp" -maxdepth 1 -type d -name 'rembrowser-*-x86_64-linux' | head -1)
    [ -x "$dir/install.sh" ] || die "unexpected archive layout"
    "$dir/install.sh"
}

if [ -z "${REMBROWSER_PORTABLE:-}" ] && command -v pacman >/dev/null && command -v makepkg >/dev/null; then
    arch_install
else
    portable_install
fi

say "Done! Start it with:  rembrowser"
echo "    • first run sets up DRM (Widevine) in the background — restart once for Netflix & co."
echo "    • press <Space>sk inside rembrowser for every shortcut"
echo "    • rembrowser config | rembrowser update | rembrowser uninstall"
