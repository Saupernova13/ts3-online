# Progress log

Newest first. One entry per working session: what was learned, what changed, what is next.

## 2026-10-16

- Located the install: `G:\SteamLibrary\steamapps\common\Toy Story 3 Complete Edition`
  (Steam app 4049180). The live exe already carries a 7-byte patch; `.exe.orig` is the original
  and is what Ghidra analysed (FINDINGS §7).
- REA's own Ghidra provider is not configured on this machine (`GHIDRA_INSTALL_DIR` unset, JDK 17
  default), so analysis runs through `tools/ghidra.sh` with Ghidra 12.1.3 and JDK 21.
- Ruled out native networking (FINDINGS §1).
- Found that the renderer and join logic already handle 3 and 4 viewports and that the player
  manager is the 2-player bottleneck (FINDINGS §2-§4).
- Input enumerates 4 XInput pads already.
- Set up this repo; queries 01-05 in `queries/` reproduce the evidence.

- Phase 1, player-manager layer: dumped 510 manager-touching functions (`PlayerMgrDump.java`),
  mapped offsets (`tools/scan_player_mgr.py`), measured the object (~0x258 bytes, static, 3
  functions address it absolutely), and listed instruction-level bound sites
  (`SlotBounds.java` -> `tools/classify_bounds.py`): 110 mechanical patches, 39 to review, 13
  unguarded accesses, 12 tail-field sites. FINDINGS §3a has the details; PLAN.md has the
  phase 3 recipe.

- Phase 2, loader built (`loader/`, Rust `dinput8.dll` proxy): verifies the 188 patches against the
  exe bytes, relocates the player manager into a buffer within +/-2 GiB of the module, applies
  the set, and runs a watcher thread logging manager fields to `ts3online.log`. `cargo test`
  passes; not yet run inside the game.

- Phase 4 start: loader gained `hook` patches (`entry`, `entry_caller` via an R8 thunk, `call`)
  and Rust hooks for the join bind, join cancel, join failure release and avatar-by-class
  lookup (`hooks.rs`, `game.rs`); gen_patches.py emits them from `data/patches-manual.json` and
  the watch list now includes the input driver and the player-object count. Unit-tested
  encodings only; not run in the game.

- Join handler `0x1400eb6f0` is now the Rust hook `join_request` (first free player index for
  assign, screen owner and release); the `release_joining` call hook and the three join byte
  patches are gone, and a 1 GB "System" heap patch was added. Not run in the game yet.

- In-game runs (virtual pads, Toy Box): loader boots and exits cleanly (T1); 2-player drop-in
  through the Rust hooks matches vanilla (T2). Three players: P3 joins with its own pad, binds
  to slot 2, and the engine switches to the 3-way split, then dies in a runaway loop in slot init.
  Root cause: slots 2/3 keep player index -1 (FINDINGS 5b). Fixed in `bind_joining_avatar`; not
  re-run yet.
- Automated testing paused: virtual-pad presses reached the user's desktop when the game lost
  focus. Further in-game runs wait for an agreed test window.

**Next:** in a test window, re-run T3/T4 (3 and 4 players in Toy Box) and check whose view each
viewport shows and whether P3/P4 can move. Then look at what P3/P4 lack without
`Meridian_Player_2/3` objects and per-player setup `0x1405b0420`.
