#!/usr/bin/env bash
# safe-run.sh SCRIPT [ARGS...]: run an end-to-end test (vimtest.sh, tabtest.sh, p4test.sh,
# edittest.sh) without touching the browser and desktop you are using. Those scripts
# `pkill -x rembrowser`, type with wtype and ask niri; here they get:
#   - an invisible compositor (offscreen.sh) and a throwaway profile, config copy, socket
#   - `pkill` / `pgrep` that only see processes started in this sandbox
#   - `niri msg` answered by that compositor
here=$(dirname "$(realpath "$0")")
[ -n "$1" ] || { sed -n 2,7p "$0"; exit 1; }
rt=${XDG_RUNTIME_DIR:-/tmp}
sand=$(mktemp -d "${TMPDIR:-/tmp}/rembrowser-safe.XXXX")
name=$("$here/offscreen.sh" start) || exit 1
read -r swaypid _ <"$rt/rembrowser-offscreen"
mkdir -p "$sand/bin" "$sand/run" "$sand/config/rembrowser" "$sand/data"; chmod 700 "$sand/run"
# CONFIG=file: use that config instead of a copy of yours.
cp "${CONFIG:-${XDG_CONFIG_HOME:-$HOME/.config}/rembrowser/config.yaml}" "$sand/config/rembrowser/config.yaml" 2>/dev/null

# The editor popup's own nvim config (one file, no plugin manager).
ln -s "${XDG_CONFIG_HOME:-$HOME/.config}/nvim-rembrowser" "$sand/config/nvim-rembrowser"

# pgrep [-x][-c][-a] NAME, limited to this sandbox; pkill = the same list, killed.
cat >"$sand/bin/pgrep" <<'EOS'
#!/usr/bin/env bash
name=${*: -1}; flags=${*:1:$#-1}; x=(); [[ $flags == *x* ]] && x=(-x)
mine=()
for p in $(/usr/bin/pgrep "${x[@]}" "$name"); do
  { tr '\0' '\n' <"/proc/$p/environ"; } 2>/dev/null | rg -qxF "REMBROWSER_SANDBOX=$REMBROWSER_SANDBOX" && mine+=("$p")
done
case $flags in
  *c*) echo "${#mine[@]}" ;;
  *a*) for p in "${mine[@]}"; do echo "$p $(tr '\0' ' ' <"/proc/$p/cmdline")"; done ;;
  *) printf '%s\n' "${mine[@]}" ;;
esac
EOS
cat >"$sand/bin/pkill" <<'EOS'
#!/usr/bin/env bash
for p in $(pgrep "$@"); do kill "$p"; done
exit 0
EOS
cat >"$sand/bin/niri" <<'EOS'
#!/usr/bin/env bash
case "$*" in
  "msg -j windows") swaymsg -s "$SWAYSOCK" -t get_tree |
    jq -c '[.. | select((.type? == "con" or .type? == "floating_con") and .pid? != null) | {id, app_id, title: .name}]' ;;
  "msg action focus-window --id "*) swaymsg -s "$SWAYSOCK" "[con_id=${*: -1}] focus" >/dev/null ;;
  *) echo "niri stand-in: not supported: $*" >&2 ;;
esac
EOS
chmod +x "$sand/bin/pkill" "$sand/bin/pgrep" "$sand/bin/niri"

script=$(realpath "$1"); shift
cd "$(dirname "$script")" || exit 1
box=(env -u DISPLAY -u NIRI_SOCKET REMBROWSER_SANDBOX="$sand" PATH="$sand/bin:$PATH"
  XDG_DATA_HOME="$sand/data" XDG_CONFIG_HOME="$sand/config" XDG_RUNTIME_DIR="$sand/run"
  WAYLAND_DISPLAY="$rt/$name" SWAYSOCK="$rt/sway-ipc.$(id -u).$swaypid.sock")
# A keyboard that stays plugged in (it sleeps an hour before its one key): without one
# no window has keyboard focus, and the first key of every wtype call is lost.
"${box[@]}" wtype -s 3600000 -k Shift_L &
"${box[@]}" "$script" "$@"
code=$?
"${box[@]}" pkill -x wtype
"${box[@]}" pkill -x rembrowser
"${box[@]}" pkill -x mpv
"$here/offscreen.sh" stop; rm -rf "$sand"
exit $code
