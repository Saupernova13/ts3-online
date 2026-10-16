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

**Next:** phase 1. Measure the player manager's real size, list every access to its slot
array and every `MAX_PLAYERS` bound, and find other per-player arrays (HUD, cameras, control
mappers).
