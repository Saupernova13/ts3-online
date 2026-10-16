# ts3-online

**4-player co-op for *Disney/Pixar Toy Story 3* on PC, working toward real online play with each
player on their own PC and screen.**

The PC release (*Toy Story 3 Complete Edition*, Steam app 4049180) has 2-player local split
screen and no network play. This project raises the player limit to 4 with a runtime patch loader,
and is building native online play on top of it: lockstep netcode in the loader, with only pad
inputs crossing the network. No game files are included or modified; everything is applied in memory to
your own install.

## Status

| | |
|---|---|
| 4 players join from their own pads (drop-in) | working |
| 4-way split screen with a follow camera per player | working |
| All 4 players move and play | working |
| Leaving and re-entering a level with 4 players | working (players 3/4 press Start again) |
| Force 4 quarters for streaming (`enable=quadrants`) | built, not yet tested in game |
| Borderless window placement (`window=`) | built, not yet tested in game |
| HUD for players 3/4 (health, ammo, compass) | missing |
| Native online play (lockstep netcode, no streaming) | designed ([NETCODE.md](docs/NETCODE.md)) |
| Streaming each quarter to a remote player (fallback) | designed, not built ([PLAN, phase 6](docs/PLAN.md)) |

Tested in Toy Box with four virtual Xbox 360 pads. Story levels have not been played with 4
players yet; their scripts assume at most two avatars, so expect rough edges there.

## How it works

1. **Loader.** `dinput8.dll` is a proxy DLL: the game loads it from its own folder, it patches the
   game in memory before the game starts, then forwards to the real system DLL. It checks the exe's
   build and every original byte first, and patches nothing if anything differs.
2. **Player limit.** The game's player manager is compiled for 2 players. The loader moves it to a
   larger buffer with room for 4, raises about 120 `idx < 2` checks to `< 4`, and replaces join, bind,
   controls and level-teardown code that hardcodes player 1 with Rust functions that take the
   player index. The renderer and input already handle 4 views and 4 pads. Every change is listed
   in [docs/PATCHES.md](docs/PATCHES.md).
3. **Online (planned).** Every player runs the game on their own PC. The loader keeps all copies
   in lockstep: each frame it sends the local pad state to the others and feeds all four players'
   pads into every copy, with the game clock locked to the frame count, so the copies never
   diverge and only inputs cross the network. See [docs/NETCODE.md](docs/NETCODE.md). Streaming
   each player's quarter of one host's screen ([docs/PLAN.md](docs/PLAN.md), phase 6) is the
   fallback.

## Requirements

- Windows 10/11 x64 with the Steam version of *Toy Story 3 Complete Edition*. The loader only
  patches the build it was made for (exe TimeDateStamp `0x6abf29b6`) and leaves any other build
  untouched; a Steam update means the addresses have to be found again.
- Up to 4 XInput controllers (Xbox pads, or virtual ones).
- To build: a Rust toolchain with the `x86_64-pc-windows-msvc` target, and Git Bash for the
  helper scripts.

## Install

```sh
git clone https://github.com/Saupernova13/ts3-online
cd ts3-online/loader && cargo build --release && cd ..
tools/deploy_loader.sh            # copies loader/target/release/dinput8.dll into the game folder
```

`deploy_loader.sh` finds the game through Steam's library list, or set `TS3_DIR` to the game
folder. Launch the game from Steam as usual. `tools/deploy_loader.sh --remove` deletes the DLL again
(only if it is ours).

Toy Box autosaves. Back up `%APPDATA%\Disney\Toy Story 3 Complete Edition` before trying it.

## Playing with 4 players

1. Start a level with player 1, for example Toy Box.
2. On each extra pad, press **Start** and pick a character in the JOIN IN menu. Taken characters
   are spawned again, so two players can both be Woody.
3. The screen splits into 2, 3 or 4 views as players join.

After a level change, players 3 and 4 press Start again; the game itself only brings back
players 1 and 2. The character select before a level also only knows players 1 and 2.

## Configuration

Optional `ts3online.ini` next to the game exe, `key=value` lines:

| Key | Default | Meaning |
|---|---|---|
| `enabled` | `1` | `0` turns the loader off entirely |
| `watch` | `1` | log player-manager, input and split-screen state changes |
| `watch_interval_ms` | `500` | how often the watcher samples |
| `enable` | (none) | optional patch groups, comma-separated. `quadrants` always splits into 4 equal quarters |
| `window` | (none) | `x,y,w,h`: make the game window borderless at that rect and keep it there |

The game window size comes from the game's own `-dxresolution WxH` launch option.

The loader writes `ts3online.log` next to the exe on every start. A `ts3online.patches.json` next
to the exe replaces the built-in patch set.

## Known limitations

- No HUD for players 3/4. The HUD screens define player 1 and 2 instances by name in the game's
  screen assets.
- Players 3/4 have to re-join after every level change.
- Shared-screen moments (players close together, scripted shots) drop to one full-screen view,
  as in the original game.
- Only tested in Toy Box so far.

## Building and testing

```sh
cd loader
cargo build --release     # -> target/release/dinput8.dll
cargo test                # patch document parsing, encodings, config
```

In-game tests can be driven with virtual pads (`tools/pad.py`, ViGEmBus + `vgamepad`); the route
and checks are in [docs/TESTING.md](docs/TESTING.md). The loader's watcher log shows each step.

## Reproducing the analysis

Everything in [docs/FINDINGS.md](docs/FINDINGS.md) can be reproduced with headless Ghidra. Needs
Ghidra 12.x, a JDK 21+, Python 3 with `pefile`, and Git Bash on Windows.

```sh
export GHIDRA_INSTALL_DIR=/path/to/ghidra_12.x
export JAVA_HOME=/path/to/jdk-21

# one-off: import and analyse the unpatched exe (10-20 minutes)
tools/ghidra.sh import "/path/to/Toy Story 3 Complete Edition.exe"

# run a query file; output goes to work/out/<name>.txt
tools/ghidra.sh query queries/01-input-and-split-mode.cmd

# player-manager map and MAX_PLAYERS bound sites
tools/ghidra.sh script PlayerMgrDump.java "$(cygpath -w work/out/pm_dump.c)"
python -I tools/scan_player_mgr.py work/out/pm_dump.c
tools/ghidra.sh script SlotBounds.java "$(cygpath -w work/out/slot_bounds.txt)"
python -I tools/classify_bounds.py work/out/slot_bounds.txt --json data/bounds-candidates.json

# small compares on a player index from a getter (here 0x1405bcd10) or actor+0x484
tools/ghidra.sh script IndexChecks.java "$(cygpath -w work/out/index_checks.txt)" 1405bcd10

# regenerate loader/patches.json and docs/PATCHES.md from the analysis output
python -I tools/gen_patches.py "/path/to/Toy Story 3 Complete Edition.exe.orig"

# static helpers
python -I tools/pe_info.py imports "/path/to/exe"
python -I tools/pe_info.py qwords "/path/to/exe" 1407aff60 16    # e.g. vtable slots
```

Query files are numbered in the order they were run; their commands are listed at the top of
[ghidra_scripts/Multi.java](ghidra_scripts/Multi.java).

## Documentation

| Doc | What |
|---|---|
| [docs/FINDINGS.md](docs/FINDINGS.md) | what the exe does, with addresses and evidence tags |
| [docs/PATCHES.md](docs/PATCHES.md) | every runtime patch and hook (generated) |
| [docs/NETCODE.md](docs/NETCODE.md) | native online play: lockstep design, evidence, phases |
| [docs/PLAN.md](docs/PLAN.md) | approach, phases, streaming rig design, risks |
| [docs/PROGRESS.md](docs/PROGRESS.md) | dated work log |
| [docs/TESTING.md](docs/TESTING.md) | in-game test route, virtual pads, crash triage |

## Repository layout

```
loader/          Rust dinput8.dll patch loader; patches.json is its generated patch set
data/            hand-reviewed patches and classified patch sites
docs/            findings, patch catalogue, plan, progress, testing
ghidra_scripts/  headless Ghidra scripts (Java)
queries/         query files for ghidra_scripts/Multi.java, numbered in the order they were run
tools/           Ghidra runner, PE helpers, classifiers, patch generator and catalogue, deploy and
                 test helpers (pad.py, padsend.sh, shot.ps1, focus.ps1, dump_stack.py)
work/            Ghidra project, query output, test evidence (gitignored)
```

## Contributing

Issues and pull requests are welcome, especially determinism findings for the netcode, HUD
screens for players 3/4, and story-level testing.

- Record each discovery in `docs/FINDINGS.md` with its address and an `[obs]`/`[inf]`/`[unk]` tag,
  and add a numbered query file that reproduces it.
- Add patches and hooks to `data/patches-manual.json` and run `tools/gen_patches.py`; it rewrites
  `loader/patches.json` and `docs/PATCHES.md`.
- Never commit game files (exe, DLLs, assets) or full decompiler output.

## Legal

This is an unofficial fan project, not affiliated with or endorsed by Disney, Pixar, Avalanche
Software or Digital Eclipse. *Toy Story* is a trademark of Disney/Pixar. You need your own copy of
the game; this repository contains no game code or assets beyond the short byte sequences the
loader checks before patching, and short decompiled excerpts in the docs used for documentation.

## License

[MIT](LICENSE)
