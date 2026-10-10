#!/usr/bin/env bash
# Portable rembrowser: one folder that runs on x86_64 Linux desktops with glibc >= 2.34
# (Ubuntu 22.04+, Debian 12+, Fedora 35+, Arch, ...). Nothing to install.
#
# Bundles Spotify's CEF build (the one the cef crate links against) next to the binary;
# the binary finds it through RUNPATH $ORIGIN/lib.
# Tradeoff vs the Arch package (system cef): Spotify's CEF has no H.264/AAC, so sites
# that only serve H.264 won't play. VP9/AV1/Opus and Widevine DRM work.
#
#   packaging/portable/build.sh      → dist/rembrowser-<ver>-x86_64-linux.tar.xz
set -euo pipefail
root=$(cd "$(dirname "$0")/../.." && pwd)
cd "$root"
ver=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
name="rembrowser-$ver-x86_64-linux"
out="$root/dist/$name"

export CEF_PATH="${CEF_PATH:-${XDG_CACHE_HOME:-$HOME/.cache}/rembrowser/cef}"
# Literal $ORIGIN: the dynamic loader expands it, not the shell.
# shellcheck disable=SC2016
export REMBROWSER_CEF_RPATH='$ORIGIN/lib'
export CARGO_TARGET_DIR="$root/target/portable"
cargo build --release --locked

cef=$(find "$CEF_PATH" -maxdepth 2 -type d -name cef_linux_x86_64 | sort -V | tail -1)
[ -f "$cef/libcef.so" ] || { echo "no CEF build under $CEF_PATH" >&2; exit 1; }

rm -rf "$out"
mkdir -p "$out/lib"
install -m755 "$CARGO_TARGET_DIR/release/rembrowser" "$out/rembrowser"
cp -a "$cef"/{libcef.so,libEGL.so,libGLESv2.so,libvk_swiftshader.so,libvulkan.so.1,vk_swiftshader_icd.json} "$out/lib/"
cp -a "$cef"/{icudtl.dat,v8_context_snapshot.bin,chrome_100_percent.pak,chrome_200_percent.pak,resources.pak,locales} "$out/lib/"
strip --strip-unneeded "$out/lib/libcef.so"
cp packaging/rembrowser.svg "$out/"
# Chromium's third-party licenses travel with its binaries.
for f in CREDITS.html LICENSE.txt; do if [ -f "$cef/$f" ]; then cp "$cef/$f" "$out/"; fi; done
install -m755 packaging/portable/install.sh "$out/install.sh"

mkdir -p dist
# xz: every distro can unpack it (zstd isn't installed everywhere).
XZ_OPT='-9 -T0' tar -C dist -cJf "dist/$name.tar.xz" "$name"
du -sh "$out" "dist/$name.tar.xz"
