#!/usr/bin/env bash
# Editor e2e: insert-mode Ctrl-T and ;e through the real foot+nvim popup (editor-command).
cd "$(dirname "$0")" || exit 1
fails=0
ev()   { NO_NAV=1 WAIT=300 node cdp.mjs x "$1"; }
k()    { wtype "$@"; sleep 0.35; }
check() { if [ "$2" = "$3" ]; then echo "PASS $1"; else echo "FAIL $1: got [$2] want [$3]"; fails=$((fails+1)); fi; }
win()  { niri msg -j windows | jq -r ".[]|select(.app_id==\"$1\")|.id" | head -1; }
# In the nvim popup: replace the whole buffer with $1 (\n = new line), save, quit.
nvim_replace() {
  for _ in $(seq 20); do [ -n "$(win vimb-editor)" ] && break; sleep 0.2; done
  [ -z "$(win vimb-editor)" ] && { echo "  (editor window never opened)"; return; }
  sleep 0.6; k -k Escape; k ggdG; k i; wtype "$1"; sleep 0.2; k -k Escape; k ':wq'; k -k Return; sleep 1
}

page='data:text/html,<input id=i value=old><textarea id=t>a%0Ab</textarea><div id=c contenteditable>rich</div><script>window.n=0;addEventListener("input",()=>n++,true)</script>'
pkill -x rembrowser; sleep 1
( LD_LIBRARY_PATH=/usr/lib/cef REMBROWSER_CDP=9222 timeout 90 ../target/release/rembrowser "$page" >edittest.log 2>&1 & )
sleep 4; niri msg action focus-window --id "$(win rembrowser)"; sleep 0.4

k gi; k -M ctrl t -m ctrl
nvim_replace 'Hello nvim'
check "C-t edits <input>"        "$(ev 'i.value')" "Hello nvim"
truthy=$(ev 'n>0'); check "page saw input events" "$truthy" true
check "focus back in the page"   "$(ev 'document.activeElement.id')" i

k -k Escape; k ';e'; k s
nvim_replace 'x
y'
check ";e edits <textarea> (multi-line)" "$(ev 'JSON.stringify(t.value)')" '"x\ny"'

k -k Escape; k ';e'; k d
nvim_replace 'bold move'
check ";e edits contenteditable" "$(ev 'c.innerText.trim()')" "bold move"

k -k Escape; k gi; k -M ctrl t -m ctrl
for _ in $(seq 20); do [ -n "$(win vimb-editor)" ] && break; sleep 0.2; done
sleep 0.6; k -k Escape; k ':q!'; k -k Return; sleep 1
check "quit without saving = unchanged" "$(ev 'i.value')" "Hello nvim"

k -k Escape; k ':'; wtype 'open example.org'; sleep 0.3; k -M ctrl t -m ctrl
for _ in $(seq 20); do [ -n "$(win vimb-cmdedit)" ] && break; sleep 0.2; done
check "popup C-t opens cmdline editor" "$([ -n "$(win vimb-cmdedit)" ] && echo yes)" yes
sleep 0.6; k -k Return; sleep 3
check "<CR> in nvim runs the line" "$(NO_NAV=1 WAIT=300 node cdp.mjs x location.href)" "https://example.org/"

rm -f ~/Downloads/example.org*
k ';s'; k a; sleep 3
check ";s saves link to ~/Downloads" "$(fd -1 -g 'example*' ~/Downloads --changed-within 1min | wc -l)" 1
fd -g 'example*' ~/Downloads --changed-within 1min -X rm

pkill -x rembrowser
echo "--- $fails failure(s)"
