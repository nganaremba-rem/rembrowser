#!/usr/bin/env bash
# Move rembrowser to the cef version Arch ships: pin the matching cef crate, widen the
# PKGBUILD dependency, then build, test and install. Run it BEFORE the system upgrade
# (rembrowser update --system calls it). Stops on any failure; API breaks need a human.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$root"

repo=$(pacman -Si cef | awk '/^Version/ {print $3}')   # 154.0.34-2
chromium=${repo%-*}                                     # 154.0.34
major=${chromium%%.*}                                   # 154

crate=$(curl -fsS -H 'User-Agent: rembrowser-bump' https://crates.io/api/v1/crates/cef |
  jq -r --arg c "+$chromium" '[.versions[] | select((.yanked|not) and (.num|endswith($c)))][0].num // empty')
crate=${crate%%+*}                                      # 154.5.0
[ -n "$crate" ] || { echo "no cef crate for Chromium $chromium yet; try again later" >&2; exit 1; }

if rg -q "version = \"=$crate\"" Cargo.toml; then
  echo "already on cef crate $crate"
else
  sd 'cef = \{ version = "=[^"]+"' "cef = { version = \"=$crate\"" Cargo.toml
  sd "'cef>=[0-9]+' 'cef<[0-9]+'" "'cef>=$major' 'cef<$((major + 1))'" packaging/arch/PKGBUILD
  rel=$(awk -F= '/^pkgrel=/ {print $2}' packaging/arch/PKGBUILD)
  sd "(?m)^pkgrel=$rel\$" "pkgrel=$((rel + 1))" packaging/arch/PKGBUILD
  cargo update -p cef
fi

export CEF_PATH="${CEF_PATH:-$HOME/.cache/rembrowser/cef}"
cd packaging/arch
makepkg -fd --noconfirm
# The new package needs cef>=$major, which -Syu installs next; the old one blocks cef
# with cef<$major. Skip the dependency check once so both can land (-Syu fixes the rest).
sudo_args=()
[ -n "${SUDO_ASKPASS:-}" ] && sudo_args=(-A)   # no tty (GUI password prompt)
sudo "${sudo_args[@]}" pacman -U --noconfirm --nodeps --nodeps "$(makepkg --packagelist | head -n1)"
