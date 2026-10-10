#!/usr/bin/env bash
# Phase 4 e2e: new hints, gf/gF, harpoon/queue maps.
# Data files go to a throwaway XDG_DATA_HOME (rembrowser honours it).
cd "$(dirname "$0")" || exit 1
fails=0
X=$(mktemp -d "$PWD/xdg.XXXX"); mkdir -p "$X/rembrowser"
ev()   { NO_NAV=1 WAIT=300 node cdp.mjs x "$1"; }
nav()  { WAIT=2500 node cdp.mjs "$1" 1 >/dev/null; }
url()  { curl -s 127.0.0.1:9222/json/list | jq -r '[.[]|select(.type=="page" and (.url|startswith("data:text/html;base64,")|not))][0].url'; }
ntabs(){ curl -s 127.0.0.1:9222/json/list | jq '[.[]|select(.type=="page" and (.url|startswith("data:text/html;base64,")|not) and (.url|startswith("devtools")|not))]|length'; }
k()    { wtype "$@"; sleep 0.4; }
check(){ if [ "$2" = "$3" ]; then echo "PASS $1"; else echo "FAIL $1: got [$2] want [$3]"; fails=$((fails+1)); fi; }
win()  { niri msg -j windows | jq -r ".[]|select(.app_id==\"$1\")|.id" | head -1; }
focus(){ niri msg action focus-window --id "$(win rembrowser)"; sleep 0.3; }

printf 'https://example.org/\tExample Org\n' > "$X/rembrowser/harpoon"
pkill -x rembrowser; sleep 1
( XDG_DATA_HOME=$X LD_LIBRARY_PATH=/usr/lib/cef REMBROWSER_CDP=9222 timeout 120 ../target/release/rembrowser https://example.com >p4test.log 2>&1 & )
sleep 5; focus

k ';p'; k a; sleep 0.3
check ";p queues link"          "$(tail -1 "$X/rembrowser/queue" 2>/dev/null)" "https://iana.org/help/example-domains"
k ';k'; k a; sleep 0.3
check ";k removes element"      "$(ev 'document.querySelectorAll("a").length')" 0

k ' ma'; sleep 1.5
check "<Space>ma harpoon add (native)" "$(tail -1 "$X/rembrowser/harpoon" | cut -f1)" "https://example.com/"
k ' 1'; sleep 2.5
check "<Space>1 harpoon jump (native)" "$(url)" "https://example.org/"
k ' .'; sleep 0.3
check "<Space>. qpush"          "$(tail -1 "$X/rembrowser/queue")" "https://example.org/"

k gf; sleep 2
check "gf view-source tab"      "$(url | cut -c1-12)" "view-source:"
check "gf opened a tab"         "$(ntabs)" 2
k d; sleep 1

nav 'data:text/html,<img id=m src="https://www.iana.org/static/_img/2022/iana-logo-header.svg" width=200 height=80>'
k ';I'; k a; sleep 2.5
check ";I image in new tab"     "$(url)" "https://www.iana.org/static/_img/2022/iana-logo-header.svg"
k d; sleep 1

k ',d'; sleep 0.4
check ",d force-dark on (:eval!)" "$(ev '!!document.getElementById("rem-dark")')" true
k ',d'; sleep 0.4
check ",d force-dark off"       "$(ev '!!document.getElementById("rem-dark")')" false

nav https://example.com
k ' s.'; sleep 1; wtype 'Example Org'; sleep 0.5; k -k Return; sleep 2.5
check "<Space>s. history picker (popup)" "$(url)" "https://example.org/"
k ' sk'; sleep 1; wtype 'force-dark'; sleep 0.5; k -k Return; sleep 1
check "<Space>sk cheatsheet runs ,d"   "$(ev '!!document.getElementById("rem-dark")')" true

k M; sleep 1.5
check "M plays page in mpv"     "$(pgrep -a mpv | rg -c 'example.org')" 1
pkill -x mpv; focus

k gF; sleep 3
check "gF DevTools window"      "$(niri msg -j windows | jq -r '[.[]|select(.app_id=="rembrowser-devtools")]|length')" 1
focus; k -M ctrl q -m ctrl; sleep 3
check "C-q closes DevTools too" "$(pgrep -xc rembrowser)" 0

pkill -x rembrowser; rm -rf "$X"
echo "--- $fails failure(s)"
