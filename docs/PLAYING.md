# Playing ts3-online

Online co-op for *Toy Story 3 Complete Edition* (Steam). Each player runs the game on their own
PC. Your partner shows up in your game as a real co-op partner, the same as local split screen,
including picking each other up and throwing each other.

## Quick start

1. **Both players:** install [Tailscale](https://tailscale.com/download), sign in, and get both PCs
   onto one tailnet ([how](#set-up-tailscale-once)). Skip this when both PCs are on the same LAN.
2. **Both players:** download `ts3-online.zip` from the
   [latest release](https://github.com/Saupernova13/ts3-online/releases/latest), unzip it, and
   double-click **`Setup.cmd`**. Say yes to the firewall question.
3. **Host:** start the game from Steam. In the "Toy Story 3 Online" window, click **Host**, then
   load your save and enter Toy Box.
4. **Friend:** start the game. The host's game appears in the list: select it and click **Join**.
   Pick a different character and enter Toy Box.

The window remembers your name, password and last game, so next time it is two clicks.

## What you need

- The Steam version of *Toy Story 3 Complete Edition* on every PC (the current Steam build; the
  loader leaves any other build untouched).
- A controller or the keyboard on every PC. With the keyboard, Q (the game's BACK) and menu keys stay on your own screen.
- A way for the PCs to reach each other: the same LAN, or [Tailscale](https://tailscale.com)
  (free for personal use) for PCs in different places.
- The same ts3-online release on every PC ([Update](#update)).

## Set up Tailscale (once)

1. Every player installs Tailscale from <https://tailscale.com/download> and signs in.
2. Put the PCs on one tailnet: either everyone signs in to the same Tailscale account, or the
   host shares their PC with the other players (Tailscale admin console, **Machines**, the PC's
   menu, **Share**) and they accept the invite.
3. Check: `tailscale status` on each PC lists the other PC as online. Setup.cmd also tells you how
   many other devices it sees.

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
`install.ps1` also takes `-Name`, `-Password`, `-Port`, `-Firewall`, `-Update` and `-Uninstall` on
their own.

## Play

1. Join the tailnet on both PCs, or use the same LAN.
2. The **host** starts the game first. The "Toy Story 3 Online" window opens: check your name, then
   click **Host** (closing the window also hosts). Load your save into Toy Box.
3. The **client** starts the game afterwards. The window lists the games it finds on your tailnet
   and LAN, with the host's name, the players (`1/4`) and the address. Select the game and click
   **Join** (Enter or a double-click does the same). Join first checks that the host answers and
   that your password matches, and says what is wrong if not ([Troubleshooting](#troubleshooting)).
   The loader then downloads the host's save and plays from it (the client's own save is not
   touched or written). Pick a character and enter Toy Box.
4. Each player now sees the other as their co-op partner. When the host changes level, the client
   follows a few seconds later; when the client goes through a level door, the host follows.

**A game that is not listed:** the **This PC** line at the bottom of the host's window shows the
addresses other players can use, for example `100.64.0.7 (Tailscale)`. Send that to your
friend; they type it in the Address box and click Join.

**Password:** games marked `[password]` need the same password in your Password field. To host
with a password, type it before you click Host. Without one, anyone who can reach your PC on the
tailnet or LAN can join.

The game does not continue loading while the window is open. A host takes up to 4 players (tested
with 2). With `net=auto` in `ts3online.ini` there is no window: the first copy to start hosts and
the others find it on their own.

## Update

Every player needs the same ts3-online release. Double-click **`Update.cmd`** (with the game
closed): it checks GitHub for the latest release, installs it, and keeps your settings and saves.
The connect window's title shows the release you have, and it tells you when the host runs a
different one. (Release 1.0.0 had no Update.cmd: download the latest zip and run Setup.cmd once.)

## Troubleshooting

| What you see | What to do |
|---|---|
| The list stays empty | The host must have clicked **Host** first. Check `tailscale status` shows the other PC online on both PCs. Or type the address from the host's **This PC** line and click Join. |
| "No answer from ..." | The host's game is not open, or the firewall blocks it: on the host, run Setup.cmd again and say yes to the firewall (or click Allow when Windows asks). Both PCs must be online in Tailscale. Clicking Join again joins anyway and keeps trying in the background. |
| "Wrong password" or "needs a password" | Type the host's password, exactly, in the Password box. |
| "[other ts3-online version]" or "has an older ts3-online" | Both players run Update.cmd. |
| "running the game but not hosting" | That player clicked Join or is still in the window: they click Host. |
| "This game is full" | A host takes 4 players. |
| Joined, but no partner in the game | Both players must be in the same level (Toy Box). Check `ts3online.log` next to the game exe for `net: connected`. |

The loader writes `ts3online.log` next to the game exe; attach it to bug reports.

## Two copies on one PC (testing)

With two controllers you can try online play alone: double-click **`LocalCoop.cmd`**. It starts the
game twice side by side, each window half the screen. Copy 1 hosts with your save and controller 1;
copy 2 joins it, downloads that save and uses controller 2. On each, choose Start Game and Load Game
and enter Toy Box (copy 1 first), with different characters. While they run, `ts3online.ini` in
the game folder holds a two-copy setup; keep the script's window open and it puts your own file
back once both games are closed.

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

`net_key` is optional. Every player sets the same value, in the connect window or with
`install.ps1 -Password <text>`; a player with a different or missing password is refused, and the
connect window says so before joining. The password is never written to the log. It authenticates
the traffic but does not encrypt it (Tailscale encrypts it).

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
