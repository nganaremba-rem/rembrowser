#!/usr/bin/env bash
# Usage: probe.sh [extra cefsimple args...]  -> prints the drm: title
B="$HOME/coding/rembrowser/.spike-bundle"
pkill -x cefsimple; sleep 1
( cd "$B" && LD_LIBRARY_PATH="$B" timeout 40 ./cefsimple "$@" --url="file://$HOME/coding/rembrowser/spike/drm.html" >"$B/../spike/last.log" 2>&1 & )
t=""
for _ in $(seq 1 25); do
  t=$(niri msg -j windows | jq -r '.[]|select(.title|startswith("drm:"))|.title')
  [ -n "$t" ] && [ "$t" != "drm:pending" ] && break
  sleep 1
done
echo "${t:-no-window}"
pkill -x cefsimple
exit 0
