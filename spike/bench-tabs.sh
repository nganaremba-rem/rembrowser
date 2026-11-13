#!/usr/bin/env bash
# bench-tabs.sh [tabs=30] [seconds=20]: open N heavy tabs (spike/stress.html) in an
# isolated, headless browser and report how long the UI thread was stuck and the RAM used.
#   UI stalls come from the REMBROWSER_TIMING heartbeat (ui.rs stall_watch).
#   Runs on an invisible compositor (offscreen.sh); SHOW=1 = on your desktop instead.
set -u
here=$(dirname "$(realpath "$0")")
n=${1:-30}; secs=${2:-20}; port=8765
export ISO=${ISO:-${TMPDIR:-/tmp}/rembrowser-bench} CDP_PORT=${CDP_PORT:-9334} REMBROWSER_TIMING=1
iso="$here/iso.sh"
if [ -z "${SHOW:-}" ]; then
  WAYLAND_DISPLAY=$("$here/offscreen.sh" start) || exit 1
  export WAYLAND_DISPLAY
fi

"$iso" stop 2>/dev/null
rm -rf "$ISO"; mkdir -p "$ISO/config/rembrowser"
# Tab bar pinned (the costly case); nothing restored; no sleep during the run.
# EXTRA="  adblock: false\n" adds settings lines (to see what a feature costs).
printf 'settings:\n  tab-bar: true\n  restore-session: off\n  confirm-quit: false\n  sleep-after: 0\n%b' "${EXTRA:-}" >"$ISO/config/rembrowser/config.yaml"

python3 -m http.server "$port" --bind 127.0.0.1 -d "$here" >/dev/null 2>&1 &
srv=$!
trap 'kill $srv 2>/dev/null; "$iso" stop; [ -n "${SHOW:-}" ] || "$here/offscreen.sh" stop' EXIT
for _ in $(seq 1 40); do curl -sf "127.0.0.1:$port/stress.html" >/dev/null && break; sleep 0.25; done

"$iso" start "http://127.0.0.1:$port/stress.html?n=0${Q:-}"
sleep 2
t0=$(date +%s.%N)
for i in $(seq 1 $((n - 1))); do "$iso" run "http://127.0.0.1:$port/stress.html?n=$i${Q:-}" 2>/dev/null; done
opened=$(echo "$(date +%s.%N) - $t0" | bc)
sleep "$secs"
# Tab switching under load: 20 switches.
for i in $(seq 1 20); do "$iso" run ":b $((i % n + 1))" 2>/dev/null; done
sleep 2

loaded=$(curl -s "127.0.0.1:$CDP_PORT/json/list" | jq '[.[] | select(.title | startswith("tab "))] | length')
pid=$("$iso" pid)
pss=0; procs=0
for p in $pid $(pgrep -P "$pid") $(pgrep -x rembrowser); do
  # Only this instance: its processes carry our ISO data dir in their environment.
  tr '\0' '\n' <"/proc/$p/environ" 2>/dev/null | rg -qF "XDG_DATA_HOME=$ISO/data" || continue
  echo "$p"
done | sort -u >"$ISO/pids"
while read -r p; do
  k=$(awk '/^Pss:/{s+=$2}END{print s+0}' "/proc/$p/smaps_rollup" 2>/dev/null)
  pss=$((pss + ${k:-0})); procs=$((procs + 1))
done <"$ISO/pids"
cpu=$(ps -o cputime= -p "$pid" | tr -d ' ')
allcpu=$(xargs -a "$ISO/pids" ps -o cputimes= -p 2>/dev/null | awk '{s+=$1}END{print s+0}')

rg -o 'UI stall (\d+) ms' -r '$1' "$ISO/log" | sort -n >"$ISO/stalls"
echo "tabs loaded     $loaded/$n"
echo "open $((n - 1)) tabs    ${opened}s"
echo "UI stalls       $(wc -l <"$ISO/stalls") (over 30 ms)"
echo "worst stall     $(tail -1 "$ISO/stalls" 2>/dev/null || echo 0) ms"
echo "total stalled   $(awk '{s+=$1}END{print s+0}' "$ISO/stalls") ms"
echo "browser CPU     $cpu (UI process, whole run)"
echo "all CPU         ${allcpu}s (every process, whole run)"
echo "memory (PSS)    $((pss / 1024)) MB in $procs processes"
