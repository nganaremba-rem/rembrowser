#!/usr/bin/env bash
# End-to-end vim engine test: real keystrokes via wtype, page state via CDP.
# Usage: vimtest.sh   (prints PASS/FAIL per check)
cd "$(dirname "$0")" || exit 1
fails=0
ev()   { NO_NAV=1 WAIT=${W:-300} node cdp.mjs x "$1"; }
nav()  { WAIT=${W:-2500} node cdp.mjs "$1" "${2:-1}" >/dev/null; }
url()  { curl -s 127.0.0.1:9222/json/list | jq -r '[.[]|select(.type=="page" and (.url|startswith("data:text/html;base64,")|not))][0].url'; }
ntabs(){ curl -s 127.0.0.1:9222/json/list | jq '[.[]|select(.type=="page" and (.url|startswith("data:text/html;base64,")|not))]|length'; }
k()    { wtype "$@"; sleep 0.35; }
check() { if [ "$2" = "$3" ]; then echo "PASS $1"; else echo "FAIL $1: got [$2] want [$3]"; fails=$((fails+1)); fi; }
truthy(){ if [ "$2" = "true" ]; then echo "PASS $1"; else echo "FAIL $1 ($2)"; fails=$((fails+1)); fi; }

pkill -x rembrowser; sleep 1
( LD_LIBRARY_PATH=/usr/lib/cef REMBROWSER_CDP=9222 timeout 120 ../target/release/rembrowser 'https://en.wikipedia.org/wiki/Vim_(text_editor)' >vimtest.log 2>&1 & )
sleep 5
niri msg action focus-window --id "$(niri msg -j windows | jq -r '.[]|select(.app_id=="rembrowser")|.id')"; sleep 0.4

check "listeners installed" "$(ev "window.__kd=0;window.__kp=0;addEventListener('keydown',e=>{if(!/^(Control|Shift|Alt|Meta)$/.test(e.key))__kd++},true);addEventListener('keypress',()=>__kp++,true);1")" 1
k j; k j; k j; sleep 0.6
truthy "j scrolls"            "$(ev 'scrollY>100')"
check  "page never saw j"     "$(ev '__kd+__kp')" 0
k G; sleep 1
truthy "G = bottom"           "$(ev 'scrollY+innerHeight>=document.documentElement.scrollHeight-5')"
k gg; sleep 1
check  "gg = top"             "$(ev 'scrollY')" 0
k 5j; sleep 1
truthy "5j scrolls 5 steps"   "$(ev 'scrollY>=250')"
k -M ctrl d -m ctrl; sleep 1
truthy "C-d half page"        "$(ev 'scrollY>=250+innerHeight*0.4')"
k -M ctrl z -m ctrl; k j; sleep 0.4
check  "C-z pass: page gets j" "$(ev '__kd')" 1
k -k Escape

nav https://example.com
k f; sleep 0.5; k a; sleep 2.5
check  "f a follows link"     "$(url)" "https://www.iana.org/help/example-domains"

k o; k example.org; k -k Return; sleep 2.5
check  "o example.org"        "$(url)" "https://example.org/"
k :; k 'open aw niri'; k -k Return; sleep 3
check  ":open aw niri"        "$(url | cut -c1-26)" "https://wiki.archlinux.org"

nav 'data:text/html,<input id=i><p style="height:3000px">x'
k gi; k hello; sleep 0.3
check  "gi + typing inserts"  "$(ev 'i.value')" hello
k -k Escape; k j; sleep 0.6
check  "Esc leaves insert"    "$(ev 'i.value')" hello
truthy "j scrolls after Esc"  "$(ev 'scrollY>0')"

nav https://example.com
k y; sleep 0.3
check  "y yanks URL"          "$(wl-paste -n)" "https://example.com/"
k t; k example.net; k -k Return; sleep 2.5
check  "t opens tab"          "$(ntabs)" 2
k d; sleep 1
check  "d (user map :q) closes" "$(ntabs)" 1
k u; sleep 2.5
check  "u reopens closed"     "$(url)" "https://example.net/"

pkill -x rembrowser
echo "--- $fails failure(s)"; rg -v 'Gtk-WARNING|^$|DevTools' vimtest.log | head -5
exit "$fails"
