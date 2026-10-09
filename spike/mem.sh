#!/usr/bin/env bash
# Usage: mem.sh [url]  -> PSS grouped by Chromium process type, after 6s idle
url=${1:-https://example.com}
pkill -x rembrowser; sleep 1
( LD_LIBRARY_PATH=/usr/lib/cef REMBROWSER_CDP=9222 timeout 40 "$HOME/coding/rembrowser/target/release/rembrowser" "$url" >/dev/null 2>&1 & )
sleep 6
declare -A typ
while read -r pid t; do typ[$pid]=$t; done < <(node "$HOME/coding/rembrowser/spike/procs.mjs")
total=0
for p in $(pgrep -x rembrowser); do
  k=$(( $(awk '/^Pss:/{s+=$2}END{print s+0}' "/proc/$p/smaps_rollup" 2>/dev/null) / 1024 ))
  total=$((total + k)); echo "${typ[$p]:-zygote} $k"
done | sort | awk '{n[$1]++; s[$1]+=$2} END {for (k in s) printf "  %-10s x%d %4d MB\n", k, n[k], s[k]}'
for p in $(pgrep -x rembrowser); do awk '/^Pss:/{s+=$2}END{print s+0}' "/proc/$p/smaps_rollup" 2>/dev/null; done | awk '{s+=$1} END {printf "  TOTAL %d MB\n", s/1024}'
pkill -x rembrowser; exit 0
