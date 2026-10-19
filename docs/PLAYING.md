# Playing ts3-online

Online co-op for *Toy Story 3 Complete Edition* (Steam). Each player runs the game on their own
PC. Your partner shows up in your game as a real co-op partner, the same as local split screen,
including picking each other up and throwing each other.

## What you need

- The Steam version of *Toy Story 3 Complete Edition* on every PC (the current Steam build; the
  loader leaves any other build untouched).
- An Xbox-style controller on every PC. Keyboard input is not sent to the other players yet.
- A way for the PCs to reach each other: the same LAN, or [Tailscale](https://tailscale.com)
  (free for personal use) for PCs in different places.

## Set up Tailscale (PCs in different places, once)

1. Every player installs Tailscale from <https://tailscale.com/download> and signs in.
2. Put the PCs on one tailnet: either everyone signs in to the same Tailscale account, or the
   host shares their PC with the other players (Tailscale admin console, **Machines**, the PC's
   menu, **Share**) and they accept the invite.
3. Check: `tailscale status` on each PC lists the other PC as online.

## Install (each PC)

1. Download `ts3-online.zip` from the
   [latest release](https://github.com/Saupernova13/ts3-online/releases/latest) and unzip it
   anywhere.
2. Double-click **`Setup.cmd`**. It:
   - backs up your saves to `Documents\ts3-online save backup\<date>`;
   - finds the game through Steam and copies `dinput8.dll` into its folder, next to a starter
     `ts3online.ini`;
   - says whether Tailscale is installed and connected;
   - asks whether to let other players through the Windows Firewall (Windows asks for
     administrator rights for that step only; say no and Windows asks again the first time you
     host instead).
3. To remove it, double-click **`Uninstall.cmd`**. It only deletes the DLL when it is ours.

If Setup cannot find the game (a non-Steam library layout), run
`powershell -ExecutionPolicy Bypass -File install.ps1 -Setup -GameDir "<game folder>"`.
`install.ps1` also takes `-Name`, `-Password`, `-Port`, `-Firewall` and `-Uninstall` on their own.

## Play

1. Join the tailnet on both PCs (`tailscale status` lists the other PC as online), or use the
   same LAN.
2. The **host** starts the game first. A small window, "Toy Story 3 Online", opens: check your
   name, then click **Host** (or close the window, which also hosts). Load your save into Toy Box.
3. The **client** starts the game afterwards. The same window lists the games it finds on your
   tailnet and LAN, with the host's name, the players (`1/4`) and the address. Select the game and
   click **Join** (Enter or a double-click on the entry does the same). The loader downloads the
   host's save and plays from it (the client's own save is not touched or written). Pick a
   character and enter Toy Box.
4. Each player now sees the other as their co-op partner. When the host changes level, the client
   follows a few seconds later; when the client goes through a level door, the host follows.

The window also has fields for your name and the password, and an Address box for a game that is
not listed (type `100.64.0.7` or `100.64.0.7:27777`). Games marked `[password]` need the same
password in your Password field, so type it before you click Join. Your name, password and the last game
you joined are saved in `ts3online.ini`, so the next start is prefilled. The game does not
continue loading while the window is open.

A host takes up to 4 players (tested with 2). With `net=auto` there is no window: the first copy
to start hosts and the others find it on their own.

## Settings

`ts3online.ini` next to the game exe:

| Key | Default | Meaning |
|---|---|---|
| `net` | (off) | `menu` shows the connect window at start; `auto` finds a host or becomes one without a window; `host`; `client:ADDRESS` joins a host directly |
| `menu_last` | (none) | the game the connect window joined last (it is saved here and prefilled in the Address box) |
| `name` | `player` | the name other players see |
| `net_port` | `27777` | UDP port |
| `net_peers` | (Tailscale) | comma-separated addresses to look for a host at, instead of the tailnet |
| `net_key` | (none) | shared password, up to 64 bytes; every player must use the same one |
| `pad` | `1` | which of this PC's controllers plays |
| `enabled` | `1` | `0` turns the loader off |

### Password

`net_key` is optional. Every player sets the same value (or runs `install.ps1 -Password <text>`);
a player with a different or missing password is refused (the connect window warns before you join), and the log says so. Without it, anyone
who can reach your PC on the tailnet or LAN can join. The password is never written to the log.
It authenticates the traffic but does not encrypt it.

The loader writes `ts3online.log` next to the game exe; attach it to bug reports.

## Known limitations

- Each PC runs its own world. Players and everything they do with their own buttons match on both
  screens; enemies, gold and missions are simulated on each PC and can drift apart.
- Only Toy Box has been tested.
