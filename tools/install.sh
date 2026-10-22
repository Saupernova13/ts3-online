#!/usr/bin/env bash
# ts3-online for Linux and SteamOS (Proton): installs, updates or removes the loader (dinput8.dll)
# in the Steam copy of Toy Story 3, writes a starter ts3online.ini and sets the launch option
# Proton needs to load it.
#
#   bash install.sh                 set up (back up saves, install, ini, launch option, Tailscale)
#   bash install.sh --update        install the latest GitHub release
#   bash install.sh --uninstall     remove the loader and the launch option
#   bash install.sh --peers         refresh net_peers from Tailscale (after new players join the tailnet)
#
# Options: --name NAME  --password TEXT  --port N  --game-dir DIR (or TS3_DIR)  --steam-dir DIR
set -euo pipefail

GAME='Toy Story 3 Complete Edition'
APPID=4049180
REPO='Saupernova13/ts3-online'
OVERRIDE='WINEDLLOVERRIDES="dinput8=n,b"'
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

mode=setup name="${USER:-player}" password='' port=27777 game_dir="${TS3_DIR:-}" steam_dir="${TS3_STEAM_DIR:-}"
while [ $# -gt 0 ]; do
    case "$1" in
        --update) mode=update ;;
        --uninstall) mode=uninstall ;;
        --peers) mode=peers ;;
        --name) name="$2"; shift ;;
        --password) password="$2"; shift ;;
        --port) port="$2"; shift ;;
        --game-dir) game_dir="$2"; shift ;;
        --steam-dir) steam_dir="$2"; shift ;;
        -h|--help) sed -n '2,11p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
        *) echo "Unknown option: $1 (see --help)"; exit 2 ;;
    esac
    shift
done

die() { echo "Error: $*" >&2; exit 1; }
PY=''
for c in python3 python; do "$c" -c '' >/dev/null 2>&1 && { PY=$c; break; }; done
[ -n "$PY" ] || die 'python3 is needed (SteamOS and most desktops have it).'

# Steam roots: native, Debian's ~/.steam link, Flatpak, Snap
steam_roots() {
    [ -n "$steam_dir" ] && { echo "$steam_dir"; return; }
    for d in "$HOME/.local/share/Steam" "$HOME/.steam/steam" "$HOME/.steam/debian-installation" \
             "$HOME/.var/app/com.valvesoftware.Steam/.local/share/Steam" "$HOME/snap/steam/common/.local/share/Steam"; do
        [ -d "$d/steamapps" ] && readlink -f "$d"
    done | awk '!seen[$0]++'
}

libraries() {
    for root in $(steam_roots); do
        echo "$root"
        vdf="$root/steamapps/libraryfolders.vdf"
        [ -f "$vdf" ] && sed -n 's/^[[:space:]]*"path"[[:space:]]*"\(.*\)"/\1/p' "$vdf" | sed 's/\\\\/\\/g'
    done | awk '!seen[$0]++'
}

find_game() {
    while IFS= read -r lib; do
        [ -f "$lib/steamapps/common/$GAME/$GAME.exe" ] && { echo "$lib/steamapps/common/$GAME"; return; }
    done < <(libraries)
    return 1
}

if [ -z "$game_dir" ]; then
    game_dir=$(find_game) || die "Cannot find $GAME in your Steam libraries. Pass --game-dir \"<game folder>\"."
fi
[ -f "$game_dir/$GAME.exe" ] || die "No $GAME.exe in $game_dir."
library=$(cd "$game_dir/../../.." && pwd)
dll="$game_dir/dinput8.dll"
ini="$game_dir/ts3online.ini"
saves="$library/steamapps/compatdata/$APPID/pfx/drive_c/users/steamuser/AppData/Roaming/Disney/$GAME"
echo "Game folder: $game_dir"

ours() { [ -f "$1" ] && grep -aq DirectInput8Create "$1" && grep -aq ts3online "$1"; }
version_of() { [ -f "$1" ] && grep -ao 'ts3online-version=[0-9.]*;' "$1" | head -1 | sed 's/.*=\(.*\);/\1/' || true; }
steam_running() { pgrep -x steam >/dev/null 2>&1 || pgrep -x steamwebhelper >/dev/null 2>&1; }

# Adds (or with "remove", takes out) the dinput8 override in every Steam user's launch options.
launch_option() {
    local action=$1 changed=0 files=()
    for root in $(steam_roots); do
        for f in "$root"/userdata/*/config/localconfig.vdf; do [ -f "$f" ] && files+=("$f"); done
    done
    [ ${#files[@]} -gt 0 ] || { echo "Launch option: no Steam user config found."; return 1; }
    if steam_running; then
        return 1
    fi
    for f in "${files[@]}"; do
        if out=$("$PY" - "$f" "$APPID" "$action" "$OVERRIDE" <<'PY'
import re, shutil, sys
path, appid, action, override = sys.argv[1], sys.argv[2], sys.argv[3], sys.argv[4]
text = open(path, encoding='utf-8', errors='surrogateescape').read()
tok = re.compile(r'"((?:[^"\\]|\\.)*)"|([{}])')
# walk the key/value tree to the app's block inside an "apps" block
stack, last, start, end = [], None, None, None
for m in tok.finditer(text):
    if m.group(2) == '{':
        stack.append(last); last = None
        if len(stack) >= 2 and stack[-1] == appid and stack[-2].lower() == 'apps' and start is None:
            start = m.end()
    elif m.group(2) == '}':
        if start is not None and end is None and len(stack) >= 2 and stack[-1] == appid and stack[-2].lower() == 'apps':
            end = m.start()
        stack.pop(); last = None
    else:
        last = None if last is not None else m.group(1)
if start is None:
    print('no-app'); sys.exit(0)
block = text[start:end]
lo = re.search(r'(\n[ \t]*)"LaunchOptions"([ \t]+)"((?:[^"\\]|\\.)*)"', block)
cur = lo.group(3).replace('\\"', '"').replace('\\\\', '\\') if lo else ''
if action == 'add':
    if 'dinput8' in cur:
        print('kept'); sys.exit(0)
    new = f'{override} {cur}' if '%command%' in cur else f'{override} %command% {cur}'.rstrip()
else:
    new = re.sub(r'WINEDLLOVERRIDES="dinput8=n,b"\s*', '', cur)
    if new.strip() == '%command%':
        new = ''
    if new == cur:
        print('kept'); sys.exit(0)
esc = new.replace('\\', '\\\\').replace('"', '\\"')
if lo:
    block = block[:lo.start()] + f'{lo.group(1)}"LaunchOptions"{lo.group(2)}"{esc}"' + block[lo.end():]
else:
    indent = re.search(r'\n([ \t]*)"', block)
    pad = indent.group(1) if indent else '\t\t\t\t\t\t'
    close = block.rstrip(' \t')
    block = close + f'{pad}"LaunchOptions"\t\t"{esc}"\n' + block[len(close):]
shutil.copy2(path, path + '.ts3-online.bak')
open(path, 'w', encoding='utf-8', errors='surrogateescape', newline='').write(text[:start] + block + text[end:])
print('set: ' + new)
PY
); then
            case "$out" in set:*) changed=1; echo "Launch option ($f): ${out#set: }";; esac
        fi
    done
    return 0
}

manual_launch_option() {
    echo "Launch option: Steam is running, so it was not changed (Steam rewrites its settings when it exits)."
    echo "  In Steam: Toy Story 3 > Properties > General > Launch Options, enter:"
    echo "    $OVERRIDE %command%"
    echo "  (or close Steam completely and run this again)."
}

# Online tailnet peers' first IPv4, comma-separated; empty without Tailscale.
tailnet_peers() {
    command -v tailscale >/dev/null 2>&1 || return 0
    tailscale status --json 2>/dev/null | "$PY" -c '
import json, sys
try: s = json.load(sys.stdin)
except Exception: sys.exit(0)
ips = [next((a for a in p.get("TailscaleIPs") or [] if "." in a), None) for p in (s.get("Peer") or {}).values() if p.get("Online")]
print(",".join(i for i in ips if i))' || true
}

show_tailscale() {
    if ! command -v tailscale >/dev/null 2>&1; then
        echo 'Tailscale: not installed. Install it (https://tailscale.com/download/linux; on a Steam Deck see the README)'
        echo '  and sign in with the same account as your friends, unless you are on the same LAN.'
        return
    fi
    tailscale status --json 2>/dev/null | "$PY" -c '
import json, sys
try: s = json.load(sys.stdin)
except Exception: print("Tailscale: installed but not answering. Is tailscaled running?"); sys.exit(0)
if s.get("BackendState") != "Running": print("Tailscale: installed but not connected. Run: sudo tailscale up"); sys.exit(0)
me = s.get("Self") or {}
peers = [p for p in (s.get("Peer") or {}).values() if p.get("Online")]
print("Tailscale: connected as %s (%s), %d other device(s) online." % (me.get("HostName"), (me.get("TailscaleIPs") or ["?"])[0], len(peers)))
if not peers: print("  No other device is online: your friends must join your tailnet, or share their device with you.")' || true
}

# Writes (or refreshes) the net_peers line: the game cannot ask Tailscale itself under Proton.
write_peers() {
    local peers; peers=$(tailnet_peers)
    [ -f "$ini" ] || return 0
    if [ -z "$peers" ]; then
        echo "net_peers: no online Tailscale peers found; players on your LAN are still found, others can be typed in the Address box."
        return 0
    fi
    "$PY" - "$ini" "$peers" <<'PY'
import sys
path, peers = sys.argv[1], sys.argv[2]
lines = open(path, encoding='utf-8').read().splitlines()
lines = [l for l in lines if not l.strip().startswith('net_peers')]
lines.append('net_peers=' + peers)
open(path, 'w', encoding='utf-8', newline='\r\n').write('\n'.join(lines) + '\n')
PY
    echo "net_peers: $peers (run 'bash install.sh --peers' again when someone new joins your tailnet)"
}

backup_saves() {
    if [ ! -d "$saves" ]; then echo 'No saves yet; nothing to back up.'; return; fi
    local base="${XDG_DOCUMENTS_DIR:-$HOME/Documents}"; [ -d "$base" ] || base="$HOME"
    local dst="$base/ts3-online save backup/$(date +%F)"
    if [ -d "$dst" ]; then echo "Saves already backed up today: $dst"; return; fi
    mkdir -p "$dst" && cp -a "$saves" "$dst/"
    echo "Backed up your saves to $dst"
}

update_loader() {
    if [ -f "$dll" ] && ! ours "$dll"; then die "$dll is not ours (another mod?). Move it away first."; fi
    local installed latest url tmp
    installed=$(version_of "$dll")
    tmp=$(mktemp -d); trap 'rm -rf "$tmp"' RETURN
    curl -fsSL -H 'User-Agent: ts3-online-update' "https://api.github.com/repos/$REPO/releases/latest" -o "$tmp/rel.json" \
        || die 'Cannot reach GitHub.'
    read -r latest url < <("$PY" -c '
import json, sys
r = json.load(open(sys.argv[1]))
a = next((a for a in r.get("assets", []) if a.get("name") == "ts3-online.zip"), {})
print(r.get("tag_name", "").lstrip("v"), a.get("browser_download_url", ""))' "$tmp/rel.json")
    echo "Installed: ${installed:-$( [ -f "$dll" ] && echo 'an older release' || echo nothing)}. Latest release: $latest."
    if [ -n "$installed" ] && [ "$(printf '%s\n%s\n' "$latest" "$installed" | sort -V | tail -1)" = "$installed" ]; then
        echo 'Already up to date.'; return
    fi
    if pgrep -f "$GAME.exe" >/dev/null 2>&1; then die 'Close Toy Story 3 first, then run the update again.'; fi
    [ -n "$url" ] || die "Release $latest has no ts3-online.zip."
    echo "Downloading $url"
    curl -fsSL "$url" -o "$tmp/ts3-online.zip" || die 'Download failed.'
    "$PY" -c 'import sys, zipfile; zipfile.ZipFile(sys.argv[1]).extractall(sys.argv[2])' "$tmp/ts3-online.zip" "$tmp"
    ours "$tmp/ts3-online/dinput8.dll" || die 'The download does not contain the ts3-online dinput8.dll.'
    cp "$tmp/ts3-online/dinput8.dll" "$dll"
    echo "Installed ts3-online $latest. Your ts3online.ini and saves are unchanged."
    # a source checkout keeps its own scripts
    if [ ! -f "$HERE/../loader/Cargo.toml" ]; then
        cp -r "$tmp/ts3-online/." "$HERE/" && echo "Updated the files in $HERE."
    fi
}

case "$mode" in
update)
    update_loader
    [ -f "$ini" ] || echo "No ts3online.ini yet: run 'bash install.sh' to finish setting up."
    exit 0 ;;
peers)
    [ -f "$ini" ] || die "No ts3online.ini yet: run 'bash install.sh' first."
    write_peers; exit 0 ;;
uninstall)
    if ours "$dll"; then rm -f "$dll"; echo 'Removed dinput8.dll.'
    elif [ -f "$dll" ]; then echo 'dinput8.dll there is not ours; left alone.'
    else echo 'Not installed.'; fi
    if [ -f "$ini" ]; then echo "Kept $ini (delete it by hand if you want)."; fi
    if launch_option remove; then :; else
        echo "Launch option: Steam is running. Remove $OVERRIDE from Toy Story 3's Launch Options by hand, or close Steam and run this again."
    fi
    exit 0 ;;
esac

# setup
backup_saves
src=''
for p in "$HERE/dinput8.dll" "$HERE/../loader/target/x86_64-pc-windows-gnu/release/dinput8.dll" "$HERE/../loader/target/release/dinput8.dll"; do
    [ -f "$p" ] && { src="$p"; break; }
done
if [ -f "$dll" ] && ! ours "$dll"; then die "$dll exists and is not ours (another mod?). Move it away first."; fi
if [ -n "$src" ]; then
    cp "$src" "$dll"; echo "Installed dinput8.dll ($(version_of "$dll"))."
else
    echo 'No dinput8.dll next to this script: downloading the latest release.'
    update_loader
fi

if [ -f "$ini" ]; then
    echo "Kept the existing $ini."
    if [ -n "$password" ]; then echo 'The password was not written: add net_key=<text> to that file by hand.'; fi
else
    clean=$(printf '%s' "$name" | tr -cd '[:alnum:]_ -' | sed 's/^ *//;s/ *$//'); [ -n "$clean" ] || clean=player
    {
        echo '# ts3-online: every player runs their own copy. net=menu shows a window at start that lists the'
        echo '# games on your Tailscale tailnet or LAN: pick one and Join, or Host. (net=auto skips the window.)'
        echo 'net=menu'
        echo "name=$clean"
        if [ "$port" != 27777 ]; then echo "net_port=$port"; fi
        if [ -n "$password" ]; then echo "net_key=$password"; fi
    } | sed 's/$/\r/' > "$ini"
    echo "Wrote $ini (net=menu, name=$clean)."
fi

launch_option add || manual_launch_option
write_peers
show_tailscale
if command -v ufw >/dev/null 2>&1 && ufw status 2>/dev/null | grep -q 'Status: active'; then
    echo "Firewall: ufw is active. To host, allow the game's port: sudo ufw allow $port/udp"
elif command -v firewall-cmd >/dev/null 2>&1 && firewall-cmd --state >/dev/null 2>&1; then
    echo "Firewall: firewalld is running. To host: sudo firewall-cmd --add-port=$port/udp --permanent && sudo firewall-cmd --reload"
fi
echo 'Done. Start Toy Story 3 from Steam: the "Toy Story 3 Online" window opens before the game.'
