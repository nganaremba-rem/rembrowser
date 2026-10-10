#!/usr/bin/env bash
# shot.sh OUT.png [url] [keys...]: launch, wait, optional wtype keys, screenshot window (50%).
out=$1; url=${2:-https://example.com}; shift; shift
pkill -x rembrowser; sleep 0.5
( LD_LIBRARY_PATH=/usr/lib/cef timeout 60 ../target/release/rembrowser "$url" >shot.log 2>&1 & )
sleep 4
id=$(niri msg -j windows | jq -r '.[]|select(.app_id=="rembrowser")|.id')
niri msg action focus-window --id "$id"; sleep 0.4
for k in "$@"; do eval "wtype $k"; sleep 0.6; done
niri msg action screenshot-window --id "$id" --path "$PWD/full.png"; sleep 0.5
magick full.png -resize 50% "$out"
pkill -x rembrowser
rg -v 'Gtk|zygote|DevTools|^$|Fontconfig' shot.log | head -5
exit 0
