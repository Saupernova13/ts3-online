#!/usr/bin/env bash
# Headless Ghidra runner for this repo. Works from Git Bash on Windows and from Linux.
#
#   tools/ghidra.sh import <path-to-exe>         create work/ghidra/ts3.gpr and run full analysis (10-20 min)
#   tools/ghidra.sh query <cmdfile> [outfile]    run ghidra_scripts/Multi.java against the analysed program
#   tools/ghidra.sh script <Script.java> [args]  run any script in ghidra_scripts/ (paths in args are passed through)
#
# Environment:
#   GHIDRA_INSTALL_DIR  extracted Ghidra 12.x release (required unless analyzeHeadless is on PATH)
#   JAVA_HOME           a JDK 21+ (Ghidra 12 refuses older ones)
set -euo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PROJ_DIR="$REPO/work/ghidra"
PROJ_NAME="ts3"
SCRIPTS="$REPO/ghidra_scripts"
OUT_DIR="$REPO/work/out"
mkdir -p "$PROJ_DIR" "$OUT_DIR"

is_windows() { case "$(uname -s)" in MINGW*|MSYS*|CYGWIN*) return 0 ;; *) return 1 ;; esac; }
native() { if is_windows; then cygpath -w "$1"; else printf '%s' "$1"; fi; }

find_headless() {
  local name="analyzeHeadless"
  is_windows && name="analyzeHeadless.bat"
  if [ -n "${GHIDRA_INSTALL_DIR:-}" ]; then
    local p
    if is_windows; then p="$(cygpath -u "$GHIDRA_INSTALL_DIR")/support/$name"; else p="$GHIDRA_INSTALL_DIR/support/$name"; fi
    [ -f "$p" ] && { printf '%s' "$p"; return; }
  fi
  command -v "$name" 2>/dev/null && return
  echo "error: set GHIDRA_INSTALL_DIR to a Ghidra 12.x install" >&2
  exit 1
}

check_java() {
  local java="java"
  if [ -n "${JAVA_HOME:-}" ]; then
    if is_windows; then java="$(cygpath -u "$JAVA_HOME")/bin/java"; else java="$JAVA_HOME/bin/java"; fi
  fi
  local v
  v="$("$java" -version 2>&1 | head -1 | sed -E 's/.*version "([0-9]+).*/\1/')"
  if ! [ "$v" -ge 21 ] 2>/dev/null; then
    echo "error: Ghidra 12 needs JDK 21+, found '$v' - point JAVA_HOME at a JDK 21+" >&2
    exit 1
  fi
}

run_headless() {
  local headless; headless="$(find_headless)"
  check_java
  if is_windows; then
    # cmd.exe splits on , ; = and pipes on |: keep those out of the args
    cmd //c "$(cygpath -w "$headless")" "$@"
  else
    "$headless" "$@"
  fi
}

cmd="${1:-}"; shift || true
case "$cmd" in
  import)
    [ $# -ge 1 ] || { echo "usage: $0 import <path-to-exe>" >&2; exit 2; }
    run_headless "$(native "$PROJ_DIR")" "$PROJ_NAME" -import "$(native "$1")" -max-cpu 8 \
      > "$OUT_DIR/import.log" 2>&1
    tail -3 "$OUT_DIR/import.log"
    ;;
  query)
    [ $# -ge 1 ] || { echo "usage: $0 query <cmdfile> [outfile]" >&2; exit 2; }
    in="$1"
    out="${2:-$OUT_DIR/$(basename "${in%.*}").txt}"
    run_headless "$(native "$PROJ_DIR")" "$PROJ_NAME" -process -noanalysis -readOnly \
      -scriptPath "$(native "$SCRIPTS")" -postScript Multi.java "$(native "$in")" "$(native "$out")" \
      > "$out.log" 2>&1
    grep -E 'ERROR|Exception' "$out.log" | head -5 || true
    echo "$out ($(wc -l < "$out") lines)"
    ;;
  script)
    [ $# -ge 1 ] || { echo "usage: $0 script <Script.java> [args]" >&2; exit 2; }
    s="$1"; shift
    log="$OUT_DIR/$(basename "${s%.*}").log"
    run_headless "$(native "$PROJ_DIR")" "$PROJ_NAME" -process -noanalysis -readOnly \
      -scriptPath "$(native "$SCRIPTS")" -postScript "$s" "$@" > "$log" 2>&1
    grep -E 'ERROR|Exception' "$log" | head -5 || true
    ;;
  *)
    sed -n '2,12p' "$0"
    exit 2
    ;;
esac
