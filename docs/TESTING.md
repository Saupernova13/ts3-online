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

## T3: third player (run 1 on 2026-10-16: bind OK, then out of System heap; fix pending)

With two players in, press Start on pad 3 and pick a character.

Expect `input_dev2` set, `joining_index 2`, `bind_joining_avatar idx=2`, `player_count -> 3`,
`viewport_count -> 3`, `split_mode -> 3` (two top quadrants plus a wide bottom view).
Check whose view the bottom viewport shows, and whether pad 3 moves the new avatar.

## T4: fourth player

Same with pad 4: expect `split_mode -> 4` (quadrants).

## T5: leave and rejoin

Have P3 leave, then rejoin on the same pad. Expect the free index to be reused and no stale avatar.
Then cancel the join screen once and check that `join_cancel` releases the joining index, not 1.

## Reporting

Copy the log plus a note of what was on screen (or a screenshot) into an issue or the progress log.
