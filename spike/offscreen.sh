#!/usr/bin/env bash
# An invisible Wayland compositor (sway, headless backend) for tests that need real
# windows but must not pop up on the desktop. Chromium's own --ozone-platform=headless
# loads only the first tab, so multi-tab tests run here instead.
#   offscreen.sh start   -> prints the WAYLAND_DISPLAY to use (1920x1080 screen in memory)
#   offscreen.sh stop
dir=${XDG_RUNTIME_DIR:-/tmp}
state=$dir/rembrowser-offscreen
case $1 in
  start)
    "$0" stop
    cfg=$(mktemp); printf 'output HEADLESS-1 resolution 1920x1080\n' >"$cfg"
    before=$(cd "$dir" && echo wayland-*)
    # GPU drawing like a real desktop (WLR_RENDERER=pixman = software, much slower).
    env -u WAYLAND_DISPLAY -u DISPLAY WLR_BACKENDS=headless WLR_LIBINPUT_NO_DEVICES=1 WLR_RENDERER="${WLR_RENDERER:-gles2}" \
      sway -c "$cfg" >/dev/null 2>&1 &
    pid=$!
    for _ in $(seq 1 40); do
      for s in "$dir"/wayland-*; do
        case $s in *.lock) continue ;; esac
        name=${s##*/}
        case " $before " in *" $name "*) continue ;; esac
        [ -S "$s" ] && { echo "$pid $name" >"$state"; rm -f "$cfg"; echo "$name"; exit 0; }
      done
      sleep 0.1
    done
    kill "$pid" 2>/dev/null; rm -f "$cfg"; echo "offscreen: sway did not start" >&2; exit 1
    ;;
  stop)
    [ -e "$state" ] && { read -r pid _ <"$state"; kill "$pid" 2>/dev/null; rm -f "$state"; }
    ;;
  *) sed -n 2,6p "$0" ;;
esac
