#!/usr/bin/env bash
# Send pad commands to a running `tools/pad.py --follow` only while the game window is in front.
#   tools/padsend.sh <cmdfile> 'press 1 A' 'wait 500' ...
# Exits 3 without sending anything when the game is not the foreground window.
set -euo pipefail
cmdfile="$1"; shift
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
tmp="$(mktemp -u).png"
fg="$(powershell -NoProfile -File "$(cygpath -w "$here/shot.ps1")" "$(cygpath -w "$tmp")" 8 2>/dev/null | grep -o 'foreground=[A-Za-z]*' || true)"
rm -f "$tmp"
if [ "$fg" != "foreground=True" ]; then
  echo "not sent: game is not in front ($fg)" >&2
  exit 3
fi
printf '%s\n' "$@" >> "$cmdfile"
