#!/usr/bin/env bash
# Build, sign and publish rembrowser as a pacman repo: the files are assets of the GitHub
# release "repo", so users add   Server = <releases>/download/repo   to pacman.conf.
set -euo pipefail

key=FAC3D400E106D05407A0504FC6ED9221C017576E          # "rembrowser repo" signing key
tag=repo
repo=nganaremba-rem/rembrowser                        # gh -R: we run from a non-git cache dir
out="${XDG_CACHE_HOME:-$HOME/.cache}/rembrowser/repo"

cd "$(dirname "${BASH_SOURCE[0]}")"
makepkg -fd --noconfirm --sign --key "$key"           # -d: cef needs not be installed to build

pkg=$(makepkg --packagelist | head -n1)               # only the build just made, not old ones
rm -rf "$out"; mkdir -p "$out"                        # staging only: always start from an empty db
cp -f "$pkg" "$pkg.sig" "$out"
gpg --armor --export "$key" > "$out/rembrowser-repo.asc"

cd "$out"
repo-add --sign --key "$key" rembrowser.db.tar.gz ./*.pkg.tar.zst
# repo-add leaves symlinks (rembrowser.db -> .db.tar.gz); release assets must be real files.
for ext in db files; do
  for sig in "" .sig; do
    rm -f "rembrowser.$ext$sig"
    cp "rembrowser.$ext.tar.gz$sig" "rembrowser.$ext$sig"
  done
done

assets=(./*.pkg.tar.zst ./*.pkg.tar.zst.sig rembrowser.db rembrowser.db.sig
        rembrowser.files rembrowser.files.sig rembrowser-repo.asc)
echo "ready in $out:"; ls -l "${assets[@]}"

read -rp "Upload to GitHub release '$tag'? [y/N] " yn
[ "$yn" = y ] || { echo "not uploaded"; exit 0; }

gh -R "$repo" release view "$tag" >/dev/null 2>&1 ||
  gh -R "$repo" release create "$tag" --latest=false --title "pacman repo" \
    --notes "Binary pacman repo. See README for how to add it."
gh -R "$repo" release upload "$tag" --clobber "${assets[@]}"
