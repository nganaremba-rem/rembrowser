#!/usr/bin/env bash
# ext-probe.sh: what does an extension see of our tabs and windows? Loads spike/ext-probe
# (a test extension asking what Bitwarden asks) in an isolated, invisible browser.
#   NOSLEEP=1 skips the 40 s "extension worker went to sleep" step.
here=$(dirname "$(realpath "$0")")
export ISO=${ISO:-${TMPDIR:-/tmp}/rembrowser-extprobe} CDP_PORT=${CDP_PORT:-9335}
iso="$here/iso.sh"; A=127.0.0.1:8767; B=localhost:8767
WAYLAND_DISPLAY=$("$here/offscreen.sh" start) || exit 1; export WAYLAND_DISPLAY
read -r swaypid _ <"${XDG_RUNTIME_DIR:-/tmp}/rembrowser-offscreen"
python3 -m http.server 8767 --bind 127.0.0.1 -d "$here/ext-probe" >/dev/null 2>&1 & srv=$!
# An idle virtual keyboard: without a keyboard no page ever has keyboard focus here.
wtype -s 3600000 -k Shift_L & kbd=$!
trap 'kill $srv $kbd 2>/dev/null; "$iso" stop; "$here/offscreen.sh" stop' EXIT
for _ in $(seq 1 40); do curl -sf "$A/a.html" >/dev/null && break; sleep 0.25; done

fresh() { # a new profile with the probe installed, then: iso.sh start ARGS
  "$iso" stop; rm -rf "$ISO"
  mkdir -p "$ISO/config/rembrowser" "$ISO/data/rembrowser/extensions"
  cp -r "$here/ext-probe" "$ISO/data/rembrowser/extensions/abcdefghijklmnopabcdefghijklmnop"
  printf 'settings:\n  extensions: true\n  restore-session: off\n  confirm-quit: false\n' >"$ISO/config/rembrowser/config.yaml"
  "$iso" start "$@"; sleep 2
}
ask() { # ask URL-PART COMMAND -> the worker's answer (JSON)
  NO_NAV=1 TARGET=$1 WAIT=0 node "$here/cdp.mjs" x "new Promise(r=>{document.addEventListener('probe-done',()=>r(document.documentElement.dataset.cmd),{once:true});document.dispatchEvent(new CustomEvent('probe-cmd',{detail:'{\"probe\":\"$2\"}'}));setTimeout(()=>r('\"timeout\"'),8000)})" 2>&1
}
popup() { curl -s "127.0.0.1:$CDP_PORT/json/list" | jq -r '[.[] | select(.title | startswith("POPUP")) | .title] | "\(length) open: \(.[0] // "")"' | sd -F '&quot;' '"'; }
focused() { swaymsg -s "${XDG_RUNTIME_DIR:-/tmp}/sway-ipc.$(id -u).$swaypid.sock" -t get_tree | jq -r '[.. | select(.focused? == true) | .name] | first'; }
show() { printf '%-34s %s\n' "$1" "$2"; }

echo "== 1. two tabs: site a (tab 1, active) and site b (tab 2, in the background)"
fresh "http://$A/a.html"; "$iso" run "http://$B/b.html"; sleep 1.5; "$iso" run ":b 1"; sleep 1.5
r=$(ask "$B" bg)
show "background tab asks: active is" "$(jq -c .active <<<"$r")   (want: site a everywhere)"
show "tab calls (get/send/exec)" "$(jq -c '[.get,.send,.exec]' <<<"$r")"
show "tab events seen by worker" "$(jq -c .events <<<"$r")"
show "windows.get(-1) / getCurrent" "$(jq -c '[.winMinus1,.winCurrent]' <<<"$r")"
boot=$(jq .boot <<<"$r")
show "open popout (Unlock now)" "$(ask "$A" win)"; sleep 2
show "popup page asks: active is" "$(popup)"
swaymsg -s "${XDG_RUNTIME_DIR:-/tmp}/sway-ipc.$(id -u).$swaypid.sock" '[app_id="^rembrowser$"] focus' >/dev/null; sleep 0.5
show "focus before 2nd click" "$(focused)"
show "2nd click (focus the popout)" "$(ask "$A" focus)"; sleep 2
show "focus after 2nd click" "$(focused)   (want: the popup)"
show "popouts now" "$(popup | cut -c1-8)"
# Bitwarden drops tab events from other windows once a real window (the popout) had
# focus: its list then stays on the previous site.
swaymsg -s "${XDG_RUNTIME_DIR:-/tmp}/sway-ipc.$(id -u).$swaypid.sock" '[app_id="^rembrowser$"] focus' >/dev/null; sleep 0.5
"$iso" run ":open http://$A/b.html"; sleep 2
r=$(ask "$A/b.html" bg)
show "same tab to site b: Bitwarden list" "$(jq -r .bitwarden.lastUrl <<<"$r")   ignored $(jq .bitwarden.ignored <<<"$r")   (want: b.html, 0)"
"$iso" run ":open http://$A/a.html"; sleep 2
if [ -z "${NOSLEEP:-}" ]; then
  sleep 40; r=$(ask "$B" bg)
  show "after 40 s idle: worker restarted" "$([ "$(jq .boot <<<"$r")" != "$boot" ] && echo yes || echo no)"
  show "  active is" "$(jq -c .active.cur <<<"$r")   (want: site a)"
fi
"$iso" run ":q"; sleep 1.5; r=$(ask "$B" bg)
show "tab 1 closed: active is" "$(jq -c .active.cur <<<"$r")   (want: site b)"

echo "== 2. only a web-app window (no browser window)"
fresh "--app=http://$A/a.html"; r=$(ask "$A" bg)
show "windows.get(-1) / getCurrent" "$(jq -c '[.winMinus1,.winCurrent]' <<<"$r")"
show "open popout (Unlock now)" "$(ask "$A" win)"; sleep 1.5; show "popouts now" "$(popup | cut -c1-8)"

echo "== 3. browser window closed, web app still open"
fresh "http://$B/b.html"; "$iso" run "--app=http://$A/a.html"; sleep 2
swaymsg -s "${XDG_RUNTIME_DIR:-/tmp}/sway-ipc.$(id -u).$swaypid.sock" '[app_id="^rembrowser$"] focus' >/dev/null; sleep 0.5
"$iso" run ":q" 2>/dev/null; sleep 2; r=$(ask "$A" bg)
show "windows.get(-1) / getCurrent" "$(jq -c '[.winMinus1,.winCurrent]' <<<"$r")"
show "open popout (Unlock now)" "$(ask "$A" win)"; sleep 1.5; show "popouts now" "$(popup | cut -c1-8)"
if kill -0 "$("$iso" pid)" 2>/dev/null; then show "browser" "alive"; else show "browser" "DIED"; fi
if rg -q 'panicked|Check failed|FATAL' "$ISO/log"; then show "log" "has errors: $ISO/log"; else show "log" "clean"; fi
