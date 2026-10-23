# Playing ts3-online

Online co-op for *Toy Story 3 Complete Edition* (Steam). Each player runs the game on their own
PC. Your partner shows up in your game as a real co-op partner, the same as local split screen,
including picking each other up and throwing each other.

## Quick start

1. **Everyone:** install [Tailscale](https://tailscale.com/download), sign in, and get every PC onto
   one tailnet ([how](#set-up-tailscale-once)). Skip this when you all share one home network.
2. **Everyone:** download `ts3-online.zip` from the
   [latest release](https://github.com/Saupernova13/ts3-online/releases/latest), unzip it, and run
   the setup: **`ts3-online-setup.exe`** on Windows, **`ts3-online-setup`** on Linux or Steam Deck
   (Desktop Mode).
3. The setup asks one question, **Host a game** or **Join a friend**, then the details: your name,
   an optional password, and for a friend, their game (found by itself on your tailnet or network,
   or typed). Click **Install**.
4. Start the game from Steam. The host's game hosts by itself; a friend's game joins it by itself.
   Pick a character and enter Toy Box.

To change anything later (host instead of join, a new address, your name or password), run the
same setup program again: it opens on your settings.

## What you need

- The Steam version of *Toy Story 3 Complete Edition* on every PC (the current Steam build; the
  loader leaves any other build untouched). Windows, or Linux and Steam Deck through Proton.
- On Linux, a 64-bit system with glibc 2.35 or newer (SteamOS 3, Ubuntu 22.04 or newer).
- A controller or the keyboard on every PC. With the keyboard, Q (the game's BACK) and menu keys stay on your own screen.
- A way for the PCs to reach each other: the same LAN, or [Tailscale](https://tailscale.com)
  (free for personal use) for PCs in different places.
- The same ts3-online release on every PC ([Update](#update)).

## Set up Tailscale (once)

1. Every player installs Tailscale from <https://tailscale.com/download> and signs in.
2. Put the PCs on one tailnet: either everyone signs in to the same Tailscale account, or the
   host shares their PC with the other players (Tailscale admin console, **Machines**, the PC's
   menu, **Share**) and they accept the invite.
3. The setup's first page says whether Tailscale is connected and how many other devices it sees.

## The setup program

`ts3-online-setup` is the same program on Windows and Linux. The first time, it is a three-step
wizard:

1. **Your game.** It finds Toy Story 3 in your Steam libraries (or you paste the folder) and checks
   Tailscale.
2. **How do you want to play?**
   - **Host a game:** your game hosts every time it starts, with no window; you play your own save.
   - **Join a friend:** your game joins your friend's game every time it starts. You play the
     host's save; yours is not touched or written.
   - **Choose each time:** the "Toy Story 3 Online" window opens at every start: Host, or pick a
     game and Join.
3. **Details.** Your name and an optional password (every player uses the same one). A host sees
   the addresses friends can use, with Copy buttons. The first time you host, Windows asks whether
   to let the game through its firewall: click Allow (in full screen, Alt+Tab to see it). A friend
   picks the host's game from the **Games found** list, or types its address, and can **Test** it:
   the setup asks the host's game whether it would take you, with the same checks the game makes.

**Install** then backs up your saves into `ts3-online/save backups/<date>` in the game folder, puts
the mod there (`dinput8.dll`, and `steam_api64.dll` in place of Steam's so Proton loads it without a
launch option; Steam's own is kept as `ts3-online/steam_api64_o.dll`), copies this setup into
`ts3-online` so you can run it from there later, and saves your settings. Everything it writes stays
in the game folder, so that folder can be moved to another PC or drive. Your own copy's save stays
in the game's usual save folder (`%APPDATA%\Disney\...` on Windows, the game's Proton prefix on
Linux); the backup in `ts3-online/save backups` moves with the folder.

Run it again at any time for the **settings** page: switch between host and join, pick or type a
new host address, change your name, password or port, forget games you joined before, check for
updates, open the game or settings folder, or uninstall.

Games you joined are remembered, and the in-game window lists them whenever they answer. If the
saved host's game does not answer when you start, the window opens instead of joining, so you can
pick another game or join anyway and keep trying; a game you pick there becomes the saved one.

Without a desktop (for example over SSH on a Steam Deck) the same program takes command-line
options: `ts3-online-setup --apply --role join --join 100.64.0.7 --name Alice`, `--role host`,
`--update`, `--uninstall`; `--help` lists them.

## Play

1. The host starts the game; with **Host a game** it hosts by itself. Load your save into Toy Box.
2. Friends start the game; with **Join a friend** it joins by itself, downloads the host's save and
   plays from it. Pick a different character and enter Toy Box.
3. Each player now sees the others as co-op partners. When the host changes level, the others
   follow a few seconds later; when someone goes through a level door, the host follows.

A host takes up to 4 players.

## Update

Every player needs the same ts3-online release. Run the setup, open **Updates, help and
uninstall**, click **Check for updates**, then **Update** (with the game closed). The in-game
window's title shows the release you have, and it tells you when the host runs a different one. A
new release also comes with a new setup program: download the zip again to get it.

## Troubleshooting

| What you see | What to do |
|---|---|
| The setup's Games found list stays empty | The host's game must be running and hosting. Check that Tailscale shows the other PC online on both PCs, or type the host's address (shown in the host's setup) and click Test. |
| "No answer from ..." | The host's game is not open, or a firewall blocks it: on a Windows host, allow the game in Windows Defender Firewall (Windows asks the first time you host: click Allow). Both PCs must be online in Tailscale. Joining again joins anyway and keeps trying in the background. |
| "Wrong password" or "needs a password" | Everyone uses the host's password exactly: change it in the setup. |
| "[other ts3-online version]" or "has an older ts3-online" | Everyone updates to the latest release. |
| "running the game but not hosting" | That player chose Join or Choose each time: they switch to Host in the setup. |
| "This game is full" | A host takes 4 players. |
| Linux: the game starts without the mod | Run the setup again with the game closed: Steam's file check may have put its own `steam_api64.dll` back, and the setup reinstalls the mod. |
| Joined, but no partner in the game | Both players must be in the same level (Toy Box). Check `ts3-online/ts3online.log` in the game folder for `net: connected`. |

The loader writes `ts3online.log` in the game folder's `ts3-online` folder (the setup's **Open the
game folder** button goes there); attach it to bug reports.

## Two copies on one PC (testing)

With two controllers you can try online play alone: double-click **`LocalCoop.cmd`**. It starts the
game twice side by side, each window half the screen. Copy 1 hosts with your save and controller 1;
copy 2 joins it, downloads that save and uses controller 2. On each, choose Start Game and Load Game
and enter Toy Box (copy 1 first), with different characters. While they run, a two-copy
`ts3online.ini` in the game folder takes precedence over your settings; keep the script's window
open and it removes it again once both games are closed.

## Settings file

The setup keeps your settings in `ts3-online/settings.ini` in the game folder, so they move with
it. A `ts3online.ini` next to the game exe is read first (the two-copy test script writes one while
it runs); settings from releases before 1.7 (`%LOCALAPPDATA%\ts3online\ts3online.ini` on Windows)
are read last, and the setup removes them once it has written the new file. You should not need to
edit the file; these are its keys:

| Key | Default | Meaning |
|---|---|---|
| `net` | (off) | `host` hosts at start; `menu` shows the in-game window (with `join`, only when that game does not answer); `auto` finds a host or becomes one without a window; `client:ADDRESS` joins directly |
| `join` | (none) | with `net=menu`: the game to join at every start |
| `recent_hosts` | (none) | games joined before, newest first; the window lists them while they answer |
| `menu_last` | (none) | the game the window joined last |
| `name` | `player` | the name other players see |
| `net_port` | `27777` | UDP port |
| `net_peers` | (Tailscale) | addresses to look for games at besides the tailnet (the setup writes your tailnet here on Linux, where the game cannot ask Tailscale) |
| `net_key` | (none) | shared password, up to 64 bytes; every player must use the same one |
| `pad` | `1` | which of this PC's controllers plays |
| `enabled` | `1` | `0` turns the loader off |

### Password

The password is optional. Every player sets the same one in the setup; a player with a different or
missing password is refused, and the setup's Test and the in-game window say so before joining. The
password is never written to the log. It authenticates the traffic but does not encrypt it
(Tailscale encrypts it).

## Known limitations

- Each PC runs its own world, kept in step by the host: players, what they carry and throw,
  townspeople, broken crates, gold, missions, unlocks and shop purchases, and the buildings on Toy
  Box's lots (a client that joins later builds the host's on arrival).
- Enemies are run by the host. On the client they move where the host's enemies are and do not
  play their attacks; their hits arrive from the host, so a hit can land a moment late.
- One player's pause pauses everyone, with the menu on that player's screen only. When every
  player is down, everyone restarts at the checkpoint together.
- Tested in Toy Box and the start of Train Rescue, with two and with four players (four copies on
  one PC). If the host leaves, one of the others takes over hosting and the rest rejoin it.
