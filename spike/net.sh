#!/usr/bin/env bash
# Usage: net.sh [secs] [chromium flags...]  -> hosts/URLs contacted while idle on about:blank
secs=$1; shift
log="$HOME/coding/rembrowser/spike/netlog-$$.json"
pkill -x rembrowser; sleep 1
LD_LIBRARY_PATH=/usr/lib/cef timeout "$secs" "$HOME/coding/rembrowser/target/release/rembrowser" --log-net-log="$log" "$@" about:blank >/dev/null 2>&1
echo "urls:";  rg -o '"url":"https?://[^"?]+' "$log" | sed 's/"url":"//' | sort | uniq -c | sort -rn | head -15
echo "hosts:"; rg -o '"host":"[^"]+"' "$log" | sed 's/"host":"//;s/"$//;s#https://##;s/:443$//' | sort -u | tr '\n' ' '; echo
rm -f "$log"
