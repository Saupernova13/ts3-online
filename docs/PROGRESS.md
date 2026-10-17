# Progress log

Oldest first. One entry per working session: what was learned, what changed, what is next.

## 2026-10-16

- Located the install: `steamapps\common\Toy Story 3 Complete Edition` in a Steam library
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
- Automated testing paused: virtual-pad presses reached the desktop when the game lost
  focus. Further in-game runs wait for an agreed test window.

- Supervised test run: the slot-index fix works. 3 and 4 players join, with 3-way and 4-way
  split, no crash, memory stable. Then: 4 per-player views plus per-player setup for slots 2/3
  give viewports 3/4 working follow cameras (FINDINGS 5c). The controls-refresh hook re-enables
  slots 2/3 (5d), but players 3/4 still do not move. Evidence: `work/evidence/` (screenshots + logs).
- Test tooling: `tools/focus.ps1` (bring the game to front), `tools/padsend.sh` (sends pad
  commands only while the game is foreground), `tools/shot.ps1` prints `foreground=`.

- Static trace of P3/P4 input (FINDINGS 5d): pad -> mapper -> `PlayerDriver` has no player
  limit. The real bug was ours: `refresh_player_controls` called the avatar's controls setter
  without its `enabled` argument, so P3/P4 stayed controls-disabled from the join menu. Fixed.
- New `IndexChecks.java` sweep of player-index compares: four more `idx <= 1` guards raised
  (FINDINGS 5e). Level teardown reset slots 0/1 only (stale P3/P4 avatars on level change): new
  `reset_player_slots` call hook resets slots 1..3 and releases the P3/P4 pads. 219 patches.
  `tools/pe_info.py qwords` reads vtables without Ghidra.
- Streaming groundwork: rig design in PLAN (Apollo instances on persistent virtual displays;
  Apollo's per-stream virtual display only works for one instance). Loader options, off by
  default: `enable=quadrants` (optional patch groups) and `window=x,y,w,h` (borderless placement).
  Full-screen moments (shared screen, a player leaving) still use one viewport.
- Deployed (220 patches with the optional group); not run in the game yet.

## 2026-10-17

- Supervised test run: all four players move (T3, T4); exiting Toy Box with 4 players and
  re-entering works, and pads 2-4 re-join and move (T6). The game's own regrouping of players
  ran without trouble. Evidence: `work/evidence/2026-10-17-*`. Not tested: `enable=quadrants` and
  `window=` (they need a `ts3online.ini` in the game folder).

- Documentation pass: `docs/PATCHES.md` lists all 220 patches and 7 hooks with group, status and
  notes; `tools/gen_patches.py` regenerates it with the patch set. README status and tool list
  brought up to date.

- Native online design (`docs/NETCODE.md`): one player per copy, full screen, one copy hosts and
  the others connect; remote players are proxy avatars in slots 1-3, driven by the owner's input
  and corrected by the owner's position; the host owns the world. A lockstep variant was drafted
  first and kept as an alternative; its determinism survey is FINDINGS §8.
- Side-by-side copies (FINDINGS §9): the game blocks a second instance with the `OctaneApp` mutex
  and relaunches through Steam when started directly; both can be bypassed in the loader. It
  does not pause on focus loss.

- H0 built and run: optional `instances` group (single-instance checks and Steam relaunch), copy
  numbers through named mutexes, `[copyN]` config sections with a `%LOCALAPPDATA%` fallback, per-copy
  logs, `pad=N` through an XInput import hook, per-copy saves through an `SHGetFolderPathA` hook,
  `tools/launch_copy.ps1`. In game: two copies run side by side with isolated pads and saves (T7).
  Gap: an unfocused copy ignores its pad.

**Next:** find the input focus gate so both local copies can be played at once, then H1 (connect)
and H2 (see each other move).
