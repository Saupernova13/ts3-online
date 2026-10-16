# ts3-online

Reverse engineering *Toy Story 3 Complete Edition* toward 4-player online co-op.

## Rules

- **Document as you go.** Each discovery goes into `docs/FINDINGS.md` with its address and an
  [obs]/[inf]/[unk] tag, and each session gets an entry in `docs/PROGRESS.md`. Update
  `docs/PLAN.md` when the approach or phase status changes. Commit after each step.
- **Patches go through the generator.** Add hand-reviewed patches and hooks to
  `data/patches-manual.json`, then run `tools/gen_patches.py`; it rewrites `loader/patches.json`
  and `docs/PATCHES.md`. Update a group's status in `tools/patch_catalog.py` after testing it.
- **No game data in git.** No exe, DLLs, assets, Ghidra project or full decompiler dumps; those
  live in `work/` (gitignored). Short decompiled excerpts in docs are fine.
- **Never modify the game install.** Patches are applied at runtime by our loader or to a copy.
  The live exe already carries someone else's 7-byte patch; the analysed program is the
  original (`.exe.orig` in the install folder).
- Addresses are for the exe whose SHA-256 is in `docs/FINDINGS.md`. A Steam update invalidates
  them; patch code should locate code by byte signature, not hardcoded addresses.

## Analysis

- Ghidra project: `work/ghidra/ts3.gpr` (create it once with `tools/ghidra.sh import`). Needs
  Ghidra 12.x (`GHIDRA_INSTALL_DIR`) and a JDK 21+ (`JAVA_HOME`).
- Run queries with `tools/ghidra.sh query queries/NN-name.cmd`; add a new numbered query file
  per investigation so the evidence can be reproduced.
- Each headless run costs ~1 minute of startup: batch related commands into one query file.
