#!/usr/bin/env bash
# Tab behaviour test: Ctrl-T/Tab/W via wtype, DRM + adblock in the real tab, quit on last close.
pkill -x rembrowser; sleep 1
( LD_LIBRARY_PATH=/usr/lib/cef REMBROWSER_CDP=9222 timeout 90 ../target/release/rembrowser https://example.com >tabs.log 2>&1 & )
sleep 4
pages() { curl -s 127.0.0.1:9222/json/list | jq -r '[.[]|select(.type=="page")|.url]|join(" , ")'; }
wtitle() { niri msg -j windows | jq -r '.[]|select(.app_id=="rembrowser")|.title'; }
id=$(niri msg -j windows | jq -r '.[]|select(.app_id=="rembrowser")|.id')
niri msg action focus-window --id "$id"; sleep 0.5
echo "start:    [$(pages)] title=[$(wtitle)]"
wtype -M ctrl t -m ctrl; sleep 3; echo "ctrl-t:   [$(pages)] title=[$(wtitle)]"
wtype -M ctrl -k Tab -m ctrl; sleep 1; echo "ctrl-tab: title=[$(wtitle)]"
wtype -M ctrl -k Tab -m ctrl; sleep 1; echo "ctrl-tab: title=[$(wtitle)]"
wtype -M ctrl w -m ctrl; sleep 2; echo "ctrl-w:   [$(pages)] title=[$(wtitle)]"
echo "DRM: $(node cdp.mjs https://example.com "$(cat play.js)")"
echo "ads: $(node cdp.mjs https://example.com "$(cat adtest.js)")"
niri msg action focus-window --id "$id"; sleep 0.3
wtype -M ctrl w -m ctrl; sleep 2; echo "last ctrl-w → procs left: $(pgrep -xc rembrowser)"
pkill -x rembrowser; rg -v 'Gtk-WARNING|^$|DevTools' tabs.log | head -5
exit 0
