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

**Next:** run the loader in the game and read its log (phase 3 check). Then hand-review the 39 + 13 sites and look for per-player arrays outside the manager (HUD,
cameras, control mappers).
