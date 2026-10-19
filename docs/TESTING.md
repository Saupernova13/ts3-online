# Testing

In-game checks for the loader, in order. Every run writes `ts3online.log` next to the game exe.
The watcher logs manager, input and split-screen values whenever they change.

## Setup

```sh
(cd loader && cargo build --release)
tools/deploy_loader.sh            # tools/deploy_loader.sh --remove to uninstall
```

Launch from Steam as usual. Pads: real controllers, or virtual ones from `tools/pad.py` (ViGEmBus).
Virtual pads are system-wide, so don't run them while another game on the PC is using controllers.

## T1: loader boots (passed 2026-10-16)

Expect `identity OK`, `applied N patches`, `mgr relocated: OK`, and a clean exit from the menu.

## Driving it without a person at the PC

```sh
work/venv/Scripts/python -u tools/pad.py --pads 3 --follow work/out/pad_cmds.txt &   # virtual pads stay connected
echo "press 1 START" >> work/out/pad_cmds.txt                                         # one command per line
powershell -File tools/shot.ps1 <out.png> 3                                            # look at the game window
echo quit >> work/out/pad_cmds.txt
```

Route used on 2026-10-16: title `START` -> Game Select `A` (Start Game) -> `A` (Load Game) -> `A`
(save slot) -> board: left stick moves the pencil cursor; Toy Box is top right (`stick 1 L 1 0 450`
from the start position) -> `A` -> character select (P1 `A`) -> load. In Toy Box, `START` on another
pad opens JOIN IN. Back up `%APPDATA%\Disney\Toy Story 3 Complete Edition` first, because Toy Box
autosaves.

Crash triage: `tools/dump_stack.py` on the WER dump in `%LOCALAPPDATA%\CrashDumps` gives the real
fault context and the game return addresses on the stack (VA of the analysed exe).

## T2: two players, vanilla regression (passed 2026-10-16)

1. Start Toy Box (or any level allowing drop-in) with pad 1.
2. Press Start on pad 2 and pick a character.

Expect `join ... joining_index 1`, `bind_joining_avatar idx=1`, `player_count 1 -> 2`,
`viewport_count 1 -> 2`, `split_mode 0 -> 2` (vertical) or `1` (horizontal). Play for a few minutes,
then have P2 leave. Anything different from the unmodded game is a regression.

## T3: third player (passed 2026-10-17)

With two players in, press Start on pad 3 and pick a character.

Expect `input_dev2` set, `joining_index 2`, `bind_joining_avatar idx=2`, `player_count -> 3`,
`viewport_count -> 3`, `split_mode -> 3` (two top quadrants plus a wide bottom view).
Check whose view the bottom viewport shows, and whether pad 3 moves the new avatar.

## T4: fourth player (passed 2026-10-17)

Same with pad 4: expect `split_mode -> 4` (quadrants).

## T5: leave and rejoin

Have P3 leave, then rejoin on the same pad. Expect the free index to be reused and no stale avatar.
Then cancel the join screen once and check that `join_cancel` releases the joining index, not 1.

## T6: level change with 4 players (passed 2026-10-17)

With 4 players in Toy Box: pad 1 `START` -> Exit to Game Board -> Yes (saves). Expect
`reset_player_slots slot 2/3 ... pad released`, all slot indices -1, `input_count -> 2`, the board,
no crash. Enter Toy Box again (the 2-player avatar select appears; P1 `A`), then pads 2-4 `START`
+ `A`: each joins and moves.

Board note: the pencil cursor keeps its last position; from Toy Box's spot `A` alone enters it.

## T7: two copies on one PC (H0, passed 2026-10-17 except input focus)

Config in `%LOCALAPPDATA%\ts3online\ts3online.ini`: `enable=instances`, then `[copy1]` with `pad=1` and
`window=0,0,960,540`, `[copy2]` with `pad=2` and `window=960,0,960,540`. Start 2 virtual pads, launch
copy 1 from Steam, then `tools/launch_copy.ps1 -CopySave`. Expect `copy 1 of 4` / `copy 2 of 4` in
`ts3online.log` / `ts3online.copy2.log`, both pad hooks, and the saves redirect in copy 2's log. Use
`tools/focus.ps1 -ProcessId` and `tools/shot.ps1 -ProcessId` to pick a copy. Known gap: only the
focused copy reacts to its pad. To see where: press a button on the unfocused copy's pad and check
its log for `pad: result=0 buttons=...`. No line, or `foreground=false` with `buttons=0x0000` while
held, means XInput hands it neutral input; retry with Steam Input off for the game to confirm.

## T8: talk to Ham (conversation camera)

Single player in Toy Box: walk to Ham and talk to him. Expect `hook level_player_setup` in the log at
level load and no crash when the conversation camera starts. Repeat with 3-4 players in, so the
conversation camera also runs on views 2/3 with avatars.

## T9: two copies see each other (H1, H2, passed 2026-10-19)

Same setup as T7, plus `net=host:27777` in `[copy1]` and `net=client:127.0.0.1:27777` in `[copy2]`
(and `name=` per copy). Expect in the logs: `net: host on 0.0.0.0:27777`, `net: connected as player
1` (copy 2), roster lines with both names and `net: rtt` lines. Enter Toy Box in both copies.
Expect `proxy: spawned` in each log and the other player's avatar standing where the other copy's
player is. Move the focused copy's player: the proxy follows in the other copy (use
`tools/shot.ps1 -ProcessId` to capture the covered copy). Exit Toy Box in one copy: no crash in
either; the proxy comes back after re-entering.

## T10: co-op partners online (H3, passed 2026-10-19)

T9's setup. Host (Woody) and client (Buzz) in Toy Box: each copy shows one full-screen view; the
other player joins as player 2 (`coop: player N bound: slot 1`, `player_count -> 2`). The client runs
a square: on the host, drift stays under 0.5 m with no snaps. Bring them together; the client presses
X facing Woody: Buzz carries Woody on both copies; RT throws him, and both copies agree on where he
lands. Then the same with the host carrying and throwing Buzz. `work/scratch/pos.py` and `walkto.py`
read positions from both processes and steer a player to a spot.

## T11: find each other and share the host's save (H4, H5a, passed 2026-10-19)

Both copies with `net=auto` (no addresses). Copy 1 probes the online tailnet peers for 2.5 s, finds
no host and hosts; copy 2 finds the port taken and joins the local host. Expect in copy 2's log
`net: save: received N bytes (crc ok)` before `saves: SHGetFolderPathA(APPDATA) -> ...\ts3online\session`,
and in copy 1's `net: save: sent N bytes to player 1`. Then T10's partner checks pass unchanged. The
first time a copy hosts, Windows Firewall asks about the game: on a tailnet the host must allow it.

## T12: follow each other between levels (H5b, passed 2026-10-19)

Start as for T11. Then:

1. Host: `post SwitchLevel wt_stuntpark` in `cmd.copy1.txt` (needs `dev_cmds=1`). Expect in the
   client's log `coop: following the host to level wt_stuntpark` about 1.5 s after the host's
   `coop: level "wt_stuntpark"`, then both rebind their partner.
2. Client: `post SwitchLevel wt_stuntpark` (`post` goes through the game's own post entry, so the
   loader sees it like a door). Expect in the host's log `coop: a client changed level, following to ...`.
3. Close the client, wait, start it again (`work/scratch/rejoin.sh`): the host parks the partner
   (`parked`), and wakes it when the player is back (`is back`). Close the host: the client parks
   its partner, takes over hosting and keeps running.
4. Both copies keep running for a minute in the new level with drift under 0.5 m.

A client on the game board does not follow (menus are not levels); it enters Toy Box through its own
menus and follows from there.

## T13: shared wallets (H6a, passed 2026-10-19)

Start as for T11. `work/scratch/setstat.py <pid> Stat_GOLD_COLLECTED <value>` writes a counter in a
running copy, standing in for a purchase or a pickup.

1. Client: lower its gold by 500 (a purchase). Expect in the host's log
   `wallet: player 1 spent -500 Stat_GOLD_COLLECTED` and both copies at the lower value, with no
   `wallet:` write on the client back to the old value (no refund flicker).
2. Host: raise its gold by 100 (a pickup). Expect the client's log `wallet: Stat_GOLD_COLLECTED a -> b`
   within a frame and its HUD counter animating up.
3. Put the host's value back afterwards: the host's save is real.

## T14: world events (H6b, passed 2026-10-19)

Start as for T11 (`log_events=1` helps). Break crates and barrels (`EXP_*` actors) near the Toy Box
start; Buzz breaks them with Y up close.

1. Host breaks a crate. Expect the host's `world: host list +ExploderDestroy <reference name>` and
   the client's `world: applied ExploderDestroy to <reference name>`, and the crate gone on both.
2. Client breaks a barrel. The host's copy of the client's player usually breaks it there as well;
   either way both copies end with it broken (`work/scratch/broken.py <pid> <reference names>`
   reads `actor+0x3f8`, which is 0 once broken).
3. Close the client and start it again (`work/scratch/rejoin.sh`). About 3 s after it appears in
   Toy Box, expect `world: applied ...` for every earlier break and those crates gone on screen.

## Reporting

Copy the log plus a note of what was on screen (or a screenshot) into an issue or the progress log.
