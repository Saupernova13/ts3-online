# ts3-online

Research and patches toward **4-player online co-op** for *Disney/Pixar Toy Story 3 Complete
Edition* (PC, Steam app 4049180), each player on a full screen of their own. The game ships with
2-player local split screen and no network play.

No game files are included. Everything here works against your own install.

## Status

Feasibility research is done and the player manager's 2-player limits are mapped down to the
instruction. The next step is the runtime patch loader. See [docs/PROGRESS.md](docs/PROGRESS.md).

| Doc | What |
|---|---|
| [docs/FINDINGS.md](docs/FINDINGS.md) | what the exe does, with addresses and evidence |
| [docs/PLAN.md](docs/PLAN.md) | approach, phases, risks |
| [docs/PROGRESS.md](docs/PROGRESS.md) | dated work log |

## Reproducing the analysis

Needs Ghidra 12.x, a JDK 21+, Python 3 with `pefile`, and Git Bash on Windows.

```sh
export GHIDRA_INSTALL_DIR='C:\utils\ghidra'
export JAVA_HOME='C:\Program Files\Java\jdk-21.0.12.1'

# one-off: import and analyse the unpatched exe (10-20 minutes)
tools/ghidra.sh import "/path/to/Toy Story 3 Complete Edition.exe"

# run a query file; output goes to work/out/<name>.txt
tools/ghidra.sh query queries/01-input-and-split-mode.cmd

# list command-line flags and whether the game reads them
tools/ghidra.sh script FlagUse.java "$(cygpath -w work/out/flags.txt)"

# player-manager map and MAX_PLAYERS bound sites
tools/ghidra.sh script PlayerMgrDump.java "$(cygpath -w work/out/pm_dump.c)"
python -I tools/scan_player_mgr.py work/out/pm_dump.c
tools/ghidra.sh script SlotBounds.java "$(cygpath -w work/out/slot_bounds.txt)"
python -I tools/classify_bounds.py work/out/slot_bounds.txt --json data/bounds-candidates.json

# static helpers
python -I tools/pe_info.py imports "/path/to/exe"
python -I tools/pe_info.py diff original.exe patched.exe
```

Query file commands are listed at the top of [ghidra_scripts/Multi.java](ghidra_scripts/Multi.java).
Addresses in the docs match the exe hash listed in FINDINGS.md.

## Layout

```
docs/            findings, plan, progress log
data/            generated research output worth keeping (patch-site lists)
ghidra_scripts/  headless Ghidra scripts (Java)
queries/         Multi.java query files, numbered in the order they were run
tools/           ghidra.sh runner, pe_info.py, player-manager scan and bound classifier
work/            Ghidra project and query output (gitignored)
```
