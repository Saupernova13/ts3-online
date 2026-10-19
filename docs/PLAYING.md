# Playing ts3-online

Online co-op for *Toy Story 3 Complete Edition* (Steam). Each player runs the game on their own
PC. Your partner shows up in your game as a real co-op partner, the same as local split screen,
including picking each other up and throwing each other.

## What you need

- The Steam version of *Toy Story 3 Complete Edition* on every PC (the current Steam build; the
  loader leaves any other build untouched).
- An Xbox-style controller on every PC. Keyboard input is not sent to the other players yet.
- Every PC on the same [Tailscale](https://tailscale.com) tailnet (free for personal use), or on
  the same LAN.

## Install (each PC)

1. Back up your saves: `%APPDATA%\Disney\Toy Story 3 Complete Edition`.
2. Unzip `ts3-online.zip` anywhere and run, in that folder:

   ```
   powershell -ExecutionPolicy Bypass -File install.ps1 -Name YourName
   ```

   This copies `dinput8.dll` into the game folder and writes `ts3online.ini` there with
   `net=auto`. Add `-Firewall` from an administrator PowerShell to allow the game through the
   Windows Firewall up front; otherwise allow it when Windows asks the first time.
3. Uninstall with `install.ps1 -Uninstall`. It only removes the DLL if it is ours.

## Play

1. Join the tailnet on both PCs (`tailscale status` lists the other PC as online).
2. The **host** starts the game first and loads their save into Toy Box.
3. The **client** starts the game afterwards. The loader finds the host on the tailnet, downloads
   the host's save and plays from it (the client's own save is not touched or written). Pick a
   character and enter Toy Box.
4. Each player now sees the other as their co-op partner. When the host changes level, the client
   follows a few seconds later; when the client goes through a level door, the host follows.

The copy that starts first hosts. A host takes up to 4 players (tested with 2).

## Settings

`ts3online.ini` next to the game exe:

| Key | Default | Meaning |
|---|---|---|
| `net` | (off) | `auto` finds a host or becomes one; `host`; `client:ADDRESS` joins a host directly |
| `name` | `player` | the name other players see |
| `net_port` | `27777` | UDP port |
| `net_peers` | (Tailscale) | comma-separated addresses to look for a host at, instead of the tailnet |
| `net_key` | (none) | shared password, up to 64 bytes; every player must use the same one |
| `pad` | `1` | which of this PC's controllers plays |
| `enabled` | `1` | `0` turns the loader off |

### Password

`net_key` is optional. Every player sets the same value (or runs `install.ps1 -Password <text>`);
a player with a different or missing password is refused, and the log says so. Without it, anyone
who can reach your PC on the tailnet or LAN can join. The password is never written to the log.
It authenticates the traffic but does not encrypt it.

The loader writes `ts3online.log` next to the game exe; attach it to bug reports.

## Known limitations

- Each PC runs its own world. Players and everything they do with their own buttons match on both
  screens; enemies, gold and missions are simulated on each PC and can drift apart.
- Only Toy Box has been tested.
