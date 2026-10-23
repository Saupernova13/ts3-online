#!/usr/bin/env bash
# Copy the built loader into the game folder ($TS3_DIR or via Steam), or remove it.
#   tools/deploy_loader.sh [--remove]
# Installs it as ts3-online-setup does, as dinput8.dll and as steam_api64.dll: the game loads
# steam_api64.dll first, and the first copy to load runs the mod. Steam's own steam_api64.dll is
# kept as ts3-online/steam_api64_o.dll. Never overwrites or removes a dinput8.dll that is not ours.
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

# never half-written: a cut-off copy of ours would pass for Steam's own on the next deploy
put() {
  cp -- "$1" "$2.new"
  mv -f -- "$2.new" "$2"
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
steam="$dir/steam_api64.dll"
original="$dir/ts3-online/steam_api64_o.dll"

if [ "$mode" = remove ]; then
  if [ -e "$dest" ] && ! is_ours "$dest"; then
    echo "refusing: $dest is not the ts3online loader" >&2
    exit 1
  fi
  if [ -e "$steam" ] && is_ours "$steam" && [ ! -f "$original" ]; then
    echo "refusing: Steam's steam_api64.dll is not kept at $original; verify the game's files in Steam" >&2
    exit 1
  fi
  done=0
  if [ -e "$dest" ]; then
    rm -- "$dest"
    echo "removed $dest"
    done=1
  fi
  if [ -e "$steam" ] && is_ours "$steam"; then
    put "$original" "$steam"
    echo "put Steam's steam_api64.dll back: $steam"
    done=1
  fi
  [ "$done" = 1 ] || echo "nothing to remove in $dir"
  exit 0
fi

[ -f "$src" ] || { echo "build first: (cd loader && cargo build --release)" >&2; exit 1; }
if [ -e "$dest" ] && ! is_ours "$dest"; then
  echo "refusing: $dest exists and is not the ts3online loader" >&2
  exit 1
fi
if [ -e "$steam" ] && ! is_ours "$steam"; then
  # Steam's own, perhaps newer than a copy kept before
  mkdir -p -- "$(dirname "$original")"
  put "$steam" "$original"
elif [ ! -e "$steam" ] && [ ! -f "$original" ]; then
  echo "refusing: the game folder has no steam_api64.dll and none is kept at $original" >&2
  exit 1
fi
put "$src" "$dest"
put "$src" "$steam"
echo "$dest"
echo "$steam"
