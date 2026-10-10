#!/usr/bin/env bash
# An isolated test instance: own profile, config, data files, socket and CDP port, so
# it never touches (or kills) a rembrowser you are using.
#   iso.sh start [url] [args...]   HEADLESS=1 = no window on screen
#   iso.sh stop | pid | win (niri window ids) | shot OUT.png | run ARGS... (2nd launch)
# Config: $ISO/config/rembrowser/config.yaml (created empty). CDP: 127.0.0.1:$CDP_PORT.
ISO=${ISO:-${TMPDIR:-/tmp}/rembrowser-iso}
export CDP_PORT=${CDP_PORT:-9333}
BIN=${BIN:-$(dirname "$(realpath "$0")")/../target/release/rembrowser}
# The socket lives here: a unix socket path must stay under 108 bytes.
RUN=${XDG_RUNTIME_DIR:-/tmp}/rembrowser-iso
# Chromium finds the compositor through an absolute WAYLAND_DISPLAY once
# XDG_RUNTIME_DIR points elsewhere.
wl=$WAYLAND_DISPLAY; [ "${wl#/}" = "$wl" ] && wl="$XDG_RUNTIME_DIR/$wl"
iso=(env XDG_DATA_HOME="$ISO/data" XDG_CONFIG_HOME="$ISO/config" XDG_RUNTIME_DIR="$RUN"
  WAYLAND_DISPLAY="$wl" REMBROWSER_CDP="$CDP_PORT" LD_LIBRARY_PATH=/usr/lib/cef "$BIN")
case $1 in
  start)
    shift
    mkdir -p "$ISO/data/rembrowser" "$ISO/config/rembrowser" "$RUN"; chmod 700 "$RUN"
    [ -e "$ISO/config/rembrowser/config.yaml" ] || echo 'settings: {}' >"$ISO/config/rembrowser/config.yaml"
    cdm=${XDG_DATA_HOME:-$HOME/.local/share}/rembrowser/WidevineCdm
    [ -d "$cdm" ] && [ ! -e "$ISO/data/rembrowser/WidevineCdm" ] && ln -s "$cdm" "$ISO/data/rembrowser/WidevineCdm"
    head=(); [ -n "$HEADLESS" ] && head=(--ozone-platform=headless)
    "${iso[@]}" "${head[@]}" "$@" >"$ISO/log" 2>&1 &
    echo $! >"$ISO/pid"
    for _ in $(seq 1 40); do curl -s "127.0.0.1:$CDP_PORT/json/version" >/dev/null && break; sleep 0.25; done
    ;;
  run) shift; "${iso[@]}" "$@" ;;
  stop) [ -e "$ISO/pid" ] && { kill "$(cat "$ISO/pid")" 2>/dev/null; sleep 0.7; kill -9 "$(cat "$ISO/pid")" 2>/dev/null; rm -f "$ISO/pid"; } ;;
  pid) cat "$ISO/pid" ;;
  win) niri msg -j windows | jq -r --argjson p "$(cat "$ISO/pid")" '[.[]|select(.pid==$p)|.id]|join(" ")' ;;
  shot) niri msg action screenshot-window --id "$("$0" win | cut -d' ' -f1)" --path "$(realpath "$2")"; sleep 0.5 ;;
  *) sed -n 2,6p "$0" ;;
esac
