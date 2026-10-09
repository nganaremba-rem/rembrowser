#!/usr/bin/env bash
# Usage: bench.sh [runs] [url] [title]  -> launch→loaded + PSS per process type (needs a built target/release/rembrowser)
runs=${1:-3}; url=${2:-https://example.com}; title=${3:-Example Domain}
bin="$HOME/coding/rembrowser/target/release/rembrowser"
pss() { awk '/^Pss:/{s+=$2}END{print s+0}' "/proc/$1/smaps_rollup" 2>/dev/null; }
for run in $(seq 1 "$runs"); do
  pkill -x rembrowser; sleep 1.5
  s=$(date +%s.%N)
  ( LD_LIBRARY_PATH=/usr/lib/cef timeout 40 "$bin" "$url" >/dev/null 2>&1 & )
  until niri msg -j windows | jq -e --arg t "$title" '.[]|select(.title==$t)' >/dev/null; do sleep 0.02; done
  e=$(date +%s.%N); sleep 4
  total=0; breakdown=""
  root=$(pgrep -xo rembrowser)
  for p in $(pgrep -x rembrowser) $(pgrep -f '^/proc/self/exe --type'); do
    [ "$p" = "$root" ] && t=browser || t=$(tr '\0' '\n' <"/proc/$p/cmdline" | sed -n 's/^--type=//p;s/^--utility-sub-type=//p' | tail -1)
    k=$(( $(pss "$p") / 1024 )); total=$((total + k)); breakdown+="$t:$k "
  done
  printf 'run%d launch→loaded=%.2fs PSS=%sMB procs=%s [%s]\n' "$run" "$(echo "$e - $s" | bc)" "$total" "$(wc -w <<<"$breakdown")" "$breakdown"
done
pkill -x rembrowser; exit 0
