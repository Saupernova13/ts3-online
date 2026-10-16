#!/usr/bin/env bash
# Copy the built loader (dinput8.dll) into the game folder, or remove it again.
#
#   tools/deploy_loader.sh [--remove]
#
# Game folder: $TS3_DIR if set, else found through Steam's libraryfolders.vdf.
# Only dinput8.dll is touched, and only if it is ours (export name + ts3online marker).
set -euo pipefail

GAME="Toy Story 3 Complete Edition"
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
src="$here/../loader/target/release/dinput8.dll"

to_unix() {
  if command -v cygpath >/dev/null 2>&1; then cygpath -u "$1"; else printf '%s\n' "$1"; fi
}

find_game_dir() {
  if [ -n "${TS3_DIR:-}" ]; then
    to_unix "$TS3_DIR"
    return 0
  fi
  local steam vdf lib
  steam="$(MSYS2_ARG_CONV_EXCL='*' reg query 'HKCU\Software\Valve\Steam' /v SteamPath 2>/dev/null \
    | tr -d '\r' | sed -n 's/^.*SteamPath[[:space:]]*REG_SZ[[:space:]]*//p' | head -n1)"
  [ -n "$steam" ] || { echo "cannot find Steam (set TS3_DIR)" >&2; return 1; }
  steam="$(to_unix "$steam")"
  vdf="$steam/steamapps/libraryfolders.vdf"
  [ -f "$vdf" ] || { echo "missing $vdf (set TS3_DIR)" >&2; return 1; }
  {
    printf '%s\n' "$steam"
    sed -n 's/^[[:space:]]*"path"[[:space:]]*"\(.*\)".*$/\1/p' "$vdf" | sed 's/\\\\/\\/g' | tr -d '\r'
  } | while IFS= read -r lib; do
    lib="$(to_unix "$lib")"
    if [ -d "$lib/steamapps/common/$GAME" ]; then
      printf '%s\n' "$lib/steamapps/common/$GAME"
      break
    fi
  done
}

is_ours() {
  grep -aq 'DirectInput8Create' "$1" && grep -aq 'ts3online' "$1"
}

mode=deploy
case "${1:-}" in
  "") ;;
  --remove) mode=remove ;;
  *) echo "usage: $0 [--remove]" >&2; exit 2 ;;
esac

dir="$(find_game_dir || true)"
[ -n "$dir" ] && [ -d "$dir" ] || { echo "game folder not found (set TS3_DIR)" >&2; exit 1; }
dest="$dir/dinput8.dll"

if [ "$mode" = remove ]; then
  if [ ! -e "$dest" ]; then
    echo "nothing to remove: $dest"
    exit 0
  fi
  is_ours "$dest" || { echo "refusing: $dest is not the ts3online loader" >&2; exit 1; }
  rm -- "$dest"
  echo "removed $dest"
  exit 0
fi

[ -f "$src" ] || { echo "build first: (cd loader && cargo build --release)" >&2; exit 1; }
if [ -e "$dest" ] && ! is_ours "$dest"; then
  echo "refusing: $dest exists and is not the ts3online loader" >&2
  exit 1
fi
cp -- "$src" "$dest"
echo "$dest"
