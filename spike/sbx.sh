#!/usr/bin/env bash
# sbx.sh [url]: sandboxed rembrowser (own data/runtime dir, CDP on 9333), 10 min.
SB=$HOME/.cache/rb-sandbox; mkdir -p -m700 $SB/run; ln -sf $XDG_RUNTIME_DIR/$WAYLAND_DISPLAY $SB/run/$WAYLAND_DISPLAY
pkill -f "^target/release/rembrowser"; sleep 1
cd "$(dirname "$0")/.."
( XDG_RUNTIME_DIR=$SB/run XDG_DATA_HOME=$SB REMBROWSER_CDP=9333 timeout 600 target/release/rembrowser "${1:-http://127.0.0.1:8765/}" >$SB/run.log 2>&1 & )
sleep 8; curl -s localhost:9333/json/list | jq -r '.[]|[.type,.url[0:80]]|@tsv'
