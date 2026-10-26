# Findings

Evidence ledger for *Disney/Pixar Toy Story 3 Complete Edition* (Steam app 4049180).
Every address is a virtual address in this build only:

| | |
|---|---|
| File | `Toy Story 3 Complete Edition.exe` (unpatched original, 10,712,576 bytes) |
| SHA-256 | `f87941b418a56b935a5e8372c11720304410d204a8cabd1d9970becfceff9722` |
| Arch | x86-64 PE, image base `0x140000000`, D3D12, FMOD Studio, Bink 2, Lua 5.1.4 |

Tags: **[obs]** seen in code or data, **[inf]** inferred from what was seen, **[unk]** still open.
`FUN_` names are Ghidra auto-names; the raw query output lives in `work/out/` (not committed).

## 1. No network play exists

- **[obs]** `-host`, `-join` and `-nonetwork` are registered as `Kernel::CommandLineArgument`s
  (registrars `0x14001b290`, `0x14001b2d0`, `0x14001baa0`; objects `0x140b2ddc0`, `0x140b2de20`,
  `0x140b2e420`), but nothing reads those objects. Live flags (`skipTut`, `dxresolution`,
  `world`, `invert*`) are read through their objects. Script: `ghidra_scripts/FlagUse.java`.
- **[obs]** The session error strings ("Failed to join the gathering", "No Session name was
  supplied...") form a table at `0x1409500c0` with no code references.
- **[obs]** No Winsock, WinHTTP or WinINet import. `steam_api64.dll` imports only init, callbacks,
  shutdown and `FindOrCreateUserInterface`.
- **[obs]** The only Steam interfaces fetched: `SteamUser023` (`0x140282910`), `SteamFriends018`
  (`0x1402828e0`), `STEAMUSERSTATS_INTERFACE_VERSION013` (`0x140282940`), `SteamUtils010`
  (`0x1402839d0`). The networking, matchmaking and Remote Play version strings are part of
  Steam's init string blob and are never referenced.
- **[obs]** `NetworkBuffer*` strings belong to the animation graph (`Animation::SubnetworkManager`),
  not to netcode.
- **[inf]** The 2010 network layer (Quazal-style "gathering/session/agent" wording) was compiled
  out; only strings survived. Native online play would mean writing a netcode layer from scratch.

## 2. Split screen supports 4 viewports already

- **[obs]** `0x1405f4700 SetSplitScreenMode(int mode)` builds viewport rects from the back buffer
  size (`w`, `h`) and applies them with `0x1401f7bd0(viewports, count)`:

  | mode | viewports | layout |
  |---|---|---|
  | 0 / other | 1 | full screen |
  | 1 | 2 | top/bottom (`w x h/2`) |
  | 2 | 2 | side by side (`w/2 x h`) |
  | 3 | 3 | two top quadrants + bottom full width |
  | 4 | 4 | quadrants |

  Globals: mode `0x140b2c5f8`, viewport count `0x1409581a8`. Fires the `SplitScreenChanging`
  event with the mode.
- **[obs]** Join and leave logic already maps player count to mode: 2 -> mode 1 or 2 (by the
  vertical flag), 3 -> mode 3, 4 -> mode 4. Seen in `0x1405f1030` (assign avatar to player slot)
  and `0x1405f2200` (`SharedScreenModeChanged` handler).
- **[obs]** The vertical/horizontal choice comes from the world setting `SplitScreenVertical`
  (read in `0x14021e830`, `0x140190750`), the Lua binding `Players_SetSplitScreenVertical`
  (`0x1405ec690`) and the choreography node `MNode::SplitScreenMode` (`0x1406bfa50`).
  `0x1405f2440` re-applies the mode when the flag flips with exactly 2 players.
- **[obs]** `0x1405f4700` loads the mode with `movsxd rdi, ecx` (`0x1405f4718`) before it fires
  `SplitScreenChanging`, stores the mode global and dispatches, so replacing that one instruction
  with `push 4; pop rdi` forces quadrants everywhere (`queries/72`). Opt-in loader group
  `quadrants` (PLAN, rig design).
- **[obs] Full-screen moments bypass it.** Shared-screen mode (`0x1405f2200`,
  `SharedScreenModeChanged`) and a player leaving (`0x1405f1030`) set mode 0 and one viewport
  inline, without `0x1405f4700`. So forced quadrants still drop to one full-screen viewport there,
  visible as viewport count `0x1409581a8 == 1` (`queries/72`). Shared mode also switches only
  slots 0/1's `CameraState`.
- **[obs]** The pre-level avatar select (`ScreenBGAvatarSelect2`, `queries/73`) is built for 2:
  `P1ToLeft`, `Scn_AvatarSelect_Player1/2`, owner index `< 2`, and it releases pad 1. Because the
  teardown hook releases the P3/P4 pads, it only involves P1/P2; P3/P4 re-join in the level.

## 3. Player manager is compiled for 2 players

- **[obs]** The player manager is a static object at `0x1409582b0` (in `.data`); the global
  pointer `0x140b2ef50` is set to it in `0x1405a6620` and cleared in `0x1405ba110`. 758
  references to the pointer across 429 functions.
- **[obs]** Known layout:

  | offset | meaning |
  |---|---|
  | `+0x04` | int active player count (`Players_GetPlayerCount`, `0x1405ec4c0`) |
  | `+0x08` | int, decremented when a player leaves |
  | `+0x10 + i*8` | avatar pointers indexed by the actor's avatar index (`actor+0x480`) |
  | `+0xd0` | non-null = shared-screen mode (no split) |
  | `+0xd8` | pointer to dynamic array `{int count; T* data}` of per-player objects |
  | `+0xe0` | member torn down at shutdown |
  | `+0xf0..+0x18f` | event handlers: SetupLaunch, StartTimer, StopTimer, FullScreenChoreographyPending |
  | `+0x198 + i*0x58` | **player slot i, only 2 slots**; `slot+0x08` (`+0x1a0`) = avatar actor |
  | `+0x248` | bool SplitScreenVertical |
  | `+0x250` | actor pending removal |

  Actor fields: `+0x480` avatar index, `+0x484` player index (`-1` = not a player).
- **[obs]** Player-index bounds are a literal `< 2`: `0x1405bb090` GetPlayerObject(idx) (34
  callers), `0x1405f1030` assign, `0x1405fc7d0` remove pending, `0x1405ec210`
  `Players_GetAvatarFromPlayerId`.
- **[inf]** The engine source has `MAX_PLAYERS` as a compile-time constant set to 2 for this game,
  while the shared engine code keeps its 3/4-player branches.

### 3a. Patch surface for 4 slots (`queries/06`, `SlotBounds.java`, `tools/classify_bounds.py`)

- **[obs] Object size ~0x258.** The static constructor `0x140019d70` (a CRT dynamic initializer)
  writes members up to `+0x248` and constructs the two slots at `+0x198`; unrelated statics
  start at about `+0x27c`. **The object cannot grow in place; it must be relocated.**
- **[obs] Absolute references are few.** Only 3 functions address the object directly: the static
  ctor `0x140019d70`, the atexit dtor `0x140757bc0` (also destroys the slots at `+0x198`) and setup
  `0x1405a6620`. Everything else goes through the pointer `0x140b2ef50`.
- **[obs] Indexed slot accesses survive relocation unchanged:** 122 sites in 95 functions compute
  `idx * 0x58 + 0x1a0 + mgr` (`IMUL r, idx, 0x58`), so slots 2/3 at `+0x248`/`+0x2a0` work if the
  slots stay contiguous at `+0x198`.
- **[obs] Fields after the slots that must move:** `+0x248` SplitScreenVertical (8 instruction
  sites: `0x1401b8920`, `0x1405ec6a0`, `0x1405ec6ac`, `0x1405f10d1`, `0x1405f2305`, `0x1405f2440`,
  `0x1405f244c` + the ctor write `0x140019e40`) and `+0x250` pending removal (`0x14005185f`,
  `0x1405fc7d6`, `0x1405fc8b8`, `0x1405fca20`).
- **[obs] Bounds checks:** the compiled `idx < 2` is `CMP idx, 1 ; JA skip` right before the IMUL.
  Auto-classified candidates:

  | class | sites | functions | patch |
  |---|---|---|---|
  | guard (same register) | 89 | 70 | imm `1 -> 3` |
  | guard (index moved to another register) | 17 | 15 | imm `1 -> 3` |
  | guard (`CMP idx, 2 ; JGE`) | 3 | 3 | imm `2 -> 4` |
  | loop over players (`CMP i, 2 ; JC back`) | 1 | 1 | imm `2 -> 4` |
  | review by hand | 39 | 27 | - |
  | slot access with no nearby guard | 13 | 10 | check caller |

  The list is `data/bounds-candidates.json` (unreviewed).
- **[obs] Player-count tests:** 9 `count == 2` tests in 9 functions (become `>= 2`, case by case)
  and 34 `count > 1` style tests in 29 functions (already fine for 3-4).
- **[obs]** 30 functions hardcode player 2's avatar (`+0x1f8`). These are game logic that may need
  to consider players 3/4 too.
- **[unk]** Per-player arrays outside this object (HUD, cameras, control mappers, save data); the
  slot ctor/dtor element count (expected `2` in the vector ctor at `0x140019e34` and dtor at
  `0x140757bd6`).

### 3b. Relocation details (`queries/07`, `queries/08`)

- **[obs]** The slots are built by `_eh_vector_constructor_iterator_(mgr+0x198, 0x58, 2, ctor
  0x14059aac0, dtor 0x14059d200)` in the static ctor (`MOV R8D, 2` at `0x140019e2e`) and torn down by
  the matching destructor iterator in the atexit dtor (`0x140757bd0`).
- **[obs]** 66 RIP-relative instructions address the object (static ctor, atexit dtor, setup);
  `queries/08` lists them and `tools/gen_patches.py` turns them into `rip_reloc` patches.
- **[obs]** Exe identity for the loader: TimeDateStamp `0x6abf29b6`, SizeOfImage `0xbdc000`,
  DllCharacteristics `0x8160` (ASLR + high-entropy VA, no CFG, no SteamStub section).
- **[obs]** `FUN_1402632b0` / `FUN_140274a10` use a 0x58 stride on an unrelated dynamic array; not
  player code.

## 4. Input handles 4 pads

- **[obs]** `XInputInputDriver` (vftable `0x1407bccf8`, ctor `0x140279780(pad index)`, stored at
  `driver+0x2d0`). Device enumeration in `0x14027ea20` and `0x14027b500` loops pad indices
  `0..3` (`< 4`) and caps devices at 11 (`< 0xb`). Poll: `0x14027c000` (`XInputGetState`).
- **[obs]** `DAT_140ac5380` is a `WindowsSystemInputDriver` (0x508 bytes, ctor `0x140279430`,
  created in `0x14027e490`). Player -> device map at `+0x4b8` with **11** entries, count at `+0x4e4`.
  Vtable methods (all loop to 11, no 2-player limit):

  | slot | fn | does |
  |---|---|---|
  | `+0x70` | - | controller object for player i |
  | `+0x78` | `0x140282e50` | clear all assignments |
  | `+0x80` | `0x14027fec0` | assign(player, device); special case for player 1 at controller `+0x270` |
  | `+0x88` | `0x140282ea0` | release(player) |
  | `+0x90` | `0x14027d1c0` | is player assigned |
  | `+0xa0` | `0x140282f60` | is device assigned |
  | `+0xa8` | `0x14027dde0` | device of player |
  | `+0xd0` | `0x14027de10` | assigned count |
  | `+0xd8` | `0x14027ddb0` | first free player index |
  | `+0xe0` | `0x14027de70` | player of device |

## 5. Game layer assumes 2 players

- **[obs]** Join-in UI (`ScreenJoinIn`) offers Woody, Buzz and Jessie (`Scn_JoinIn_*`, used in
  `0x1401278d0`, `0x140104190`, `0x140104710`, `0x14011b4d0`, `0x140120740`); class names include
  `ForcePlayer2Woody`, `Scn_AvatarSelect_Player2`.
- **[obs]** Mission flow events: `PlayerJoinsMission`, `PlayerLeavesMission`,
  `AllPlayersJoinedOrDeclined` (`0x140190750`); co-op toggled by the `disableCoOp` world setting
  (`0x140060ca0`, sets `CoOpActive`).
- **[obs]** The game's plain-text Lua in `assets/*.zip` never references split screen, player
  count or networking: split screen is entirely engine-side.
- **[unk]** Whether story missions or Toy Box scripts break with more than 2 players.

### 5a. Join flow (`queries/09`-`queries/19`)

1. **[obs]** An event callback `0x14021fdb0(_, pad)` calls the join handler only while
   `player_count < 2`.
2. **[obs]** Join handler `0x1400eb6f0(pad)`: refuses if P1's avatar is in a no-join state or the
   manager says no (`0x1405ce0e0`), bails if `input->count() >= 2` (`CMP EAX,2 ; JNC` at
   `0x1400eb782`), then `input->assign(1, pad)` with a **hardcoded player index 1**
   (`MOV EDX,1` at `0x1400eb798`). It opens `ScreenJoinIn`, or `ScreenVerifyRestartLevel` in 15
   story levels, and releases player 1's pad if the screen fails to open.
3. **[obs]** `ScreenJoinIn` (vftable `0x140780330`): `0x14011b4d0` builds the menu and hides P1's
   character. The pick handler `0x140104190` maps Woody/Buzz/Jessie to an avatar named
   `Woody2`/`Buzz2`/`Jessie2`. It reuses an existing avatar of that class if one exists
   (`0x1405bae40`, which scans the 16-entry avatar registry at `mgr+0x10` **without checking
   whether it already belongs to a player**). Otherwise it spawns one (`0x1405af7e0` sync /
   `0x1405af9e0` async with callback `0x1400dbfe0`) at P1's position or the `StartPosition_P2_<level>`
   anchor.
4. **[obs]** Bind `0x1400e2490(avatar, level_info, start)`: `assign_avatar(mgr, 1, avatar)`
   (`0x1405f1030`), a `JoiningPlayer1Avatar` special case that swaps in `mgr+0x1f8`,
   `0x14019b110(DAT_14099b8b0, 1, ...)` (auto-drive onto a vehicle) and
   `0x1405d6ae0(DAT_140b2ef20, 1, level_info, 0)`. **Player index 1 is hardcoded throughout.**
5. **[obs]** `DAT_140b2ef20` is the world-streaming manager (set in `0x14021d460` /
   `0x14021e350`). It holds **two** region slots (`[9]`/`[10]`) with per-player "in region" bytes at
   `slot+8+player`, and swaps slots by player count (`0x1405d6ae0`).
6. **[obs]** Join-screen cancel `0x1400fe910` releases player 1's device (hardcoded), then runs the
   same tail as `0x1400e7450`. The join handler's failure path (`0x1400eb923`) also releases
   player 1.
7. **[obs]** Vehicle auto-drive `0x14019b110(rides, idx, avatar, start)` picks the *other*
   player's avatar as `idx == 0 ? slot1 : slot0`. For players 3/4 that is player 1, so it is safe as is.
8. **[obs]** Avatar-by-class lookup `0x1405bae40` has 3 callers: the pick handler, a paratrooper
   lookup that already skips player-owned avatars (`0x1400c6270`), and a generic class lookup
   (`0x140168730`).
- **[inf]** For P3/P4 the join chain needs a variable player index, not more byte patches: replace
  `0x1400eb6f0` and `0x1400e2490` with reimplementations; keep the avatar registry from handing a
  player-owned avatar to another player; give duplicate characters unique names (`Woody3`).
- **[unk]** How the world-streaming manager behaves with 3-4 players (two region slots), which
  viewport each player's camera renders to, and the per-player HUD.

### 5b. First 3-player run (2026-10-16, `queries/37`-`41`, `tools/dump_stack.py`)

- **[obs]** Toy Box with virtual pads: P2 dropped in through the hooks exactly like vanilla
  (`bind idx=1`, vertical split). P3's Start was accepted (`input_dev2` set, first free index 2),
  `JOIN IN` opened, and the bind ran with `idx=2`. `player_count`, `split_mode` and
  `viewport_count` all went to 3.
- **[obs]** The `JOIN IN` menu only reacted to pad 2. The handler opens it with
  `push_screen(screen, 1)` (`MOV EDX,1` at `0x1400eb912`), and that 1 is the owning player index.
- **[obs]** The game then raised `EXCEPTION_BREAKPOINT` in `DebugBreak`. Chain:
  `assign_avatar 0x1405f1030` -> slot init `0x1405f0cf0` -> pool grow `0x140040bf0` (node 0x18) ->
  heap alloc `0x140242630`. The allocator breaks when `FUN_140245790` finds no free block and the
  heap has flag `0x2000`.
- **[obs]** The heap object (`0x1409bb8a0`, the allocator's RSI) is named inline `"System"`
  (`+0x28`). Its size is `0x20000000` (512 MB, `+0x48`) with `0xe4a6` bytes free (`+0x120`). The size
  comes from the immediate at `0x1402440ec` in the heap setup `0x140244030`. `HeapSizes.lua` defines
  no level heaps, so the System heap carries Toy Box.
- **[obs]** The per-player object array at `mgr+0xd8` holds **2** entries in Toy Box, so
  `GetPlayerObject(2|3)` returns null.
- **[obs] Correction, the real cause (run 2 with a 1 GB cap and a free-space watch):** the System
  heap had ~865 MB free at the title screen, and a 1 GB cap was exhausted just like 512 MB.
  It is a runaway loop, not a budget. Slot init `0x1405f0cf0(slot, avatar)` drains the
  control stack of `slot->index` (`DAT_140ac61c0`; per-player stacks at `+0x58 + i*8`; top
  `0x14027d390`, pop `0x140280930`), allocating a node per item. Level setup `0x1405ae570` writes
  only `slot0.index = 0` (`mgr+0x198`) and `slot1.index = 1` (`mgr+0x1f0`). Slots 2/3 keep the slot
  ctor's `-1` (`0x14059aac0`), so the loop runs on "stack -1" forever.
  Fix: `bind_joining_avatar` writes `slot[idx].index = idx` when it is still -1. The 1 GB cap stays
  as headroom.
- **[obs]** Level setup `0x1405ae570` (callers `0x1405bec50`, `0x1405d81a0`) gathers up to 16
  scene objects named `Meridian_Player_%d` as the per-player objects. Toy Box has two, which is
  why `mgr+0xd8` has 2 entries. It spawns the avatars, then runs per-player setup
  `0x1405b0420(mgr, 0|1)` for slots 0 and 1 only.
- **[unk]** What players 3/4 lose without a `Meridian_Player_2/3` object and `0x1405b0420` setup
  (camera rig?).

### 5c. Per-player views and cameras (`queries/49`-`53`)

- **[obs]** `Meridian_Player_N` scene objects are character definitions (Toy Box: 0 Buzz,
  1 Woody, 2 Jessie, 8/9 paratroopers), not per-player rigs.
- **[obs]** The per-player objects at `mgr+0xd8` are views. Level setup calls
  `FUN_1404ceed0(arr, 2)` (`MOV EDX,2` at `0x1405ae5a7`), which allocates a 5-entry array and creates
  `n` views (`FUN_1403fb570`). Per-player setup `0x1405b0420(mgr, i)` gives view i a Path and a General
  camera, then `0x14021cd70` adds Tether (follows the avatar), SpaceFlight, ToyCamera and ShowDown.
  Level setup runs it for slots 0/1 only.
- **[obs] Fixed and verified in game:** create 4 views, raise the 10 inline view-array guards,
  and on a slot's first join run `slot index = idx`, `slot_init(slot, 0)` and
  `player_setup(mgr, idx)`, exactly what level setup does for an absent player 2. Viewports 3/4
  then follow players 3/4.

### 5d. Input for players 3/4 (`queries/58`-`66`)

- **[obs] Pad to mapper.** The input dispatch `0x140280b90` loops over 11 players: if player i has
  a pad (driver `vt+0x90`), it binds the pad's controller (`vt+0x70`) to the top of control stack
  i (mapper `vt+0x48` stores it at `+0x8`) and updates it (`vt+0x10`). No player-count limit.
- **[obs] Slot init pushes the avatar's own mapper.** `0x1405f0cf0(slot, avatar)` gets the
  avatar's `ControlMapper` component (`0x140559010`), registers it (`0x14027a480`), pushes it on
  stack `slot.index` (`0x140280d40`), then calls `avatar vt+0x130(avatar, index)`. For Buzz the
  mapper class is `BuzzControlMapper` (vtable `0x1407aff60`, ctor `0x1401f98e0`, size 0x180).
- **[obs] Mapper gates (all per object):** axis reads (`0x14027da20`, `vt+0x18`) need a bound
  controller (`+0x8`), the enabled flag (`+0x10`) and the stack-top flag (`+0x154`, set on push,
  cleared on the entry below). Values are `controller[0x64 + code*4] * scale[+0x60 + axis*4]`.
- **[obs] Locomotion.** `PlayerDriver` (vtable `0x1407608c0`) update `vt+0x80` (`0x140050240`)
  returns early unless its `vt+0x38` (`0x14009fa60`: component `+0x14` set, `+0x16` clear) passes,
  then reads axes 0/1 of the avatar's mapper into the anim variables `stickX`/`stickY`.
- **[obs] Controls on/off.** `avatar vt+0x148(avatar, enabled)` (`0x1401d7e50` ->
  `0x1405565c0`) sets `avatar+0x428` and mapper `+0x10`; when off, it activates the
  `KinematicDriver` and `ViewDriver` instead. Slot init and the global refreshers
  (`0x1405d3200`, `0x1405f8a70`) pass `dl = (slot+0x50 empty && 0x1409582a8 empty)`.
- **[obs] Why P3/P4 did not move.** They join while the join menu has controls disabled, so slot
  init turns their controls off. The global refreshers only turn slots 0/1 back on (unrolled).
  The first `refresh_player_controls` hook called `vt+0x148` **without the `enabled` argument**
  (garbage DL). The log shows it: "slot 1 controls -> true" repeats on every refresh because the
  flag never changed. Fixed by passing the flag (`vcall_u8`); the log now also prints the flag
  after the call.
- **[obs]** `0x1405bb170(settings, player, i)` returns per-player keyboard bindings from
  `0x140b2eaa8` (2 x 12 ints) for players 0/1 only, -1 otherwise. Only applied when the
  controller's `+0x28c` is set (keyboard path in `0x1402150b0`), so pads are unaffected.
- **[obs] Verified in game (2026-10-17):** with the argument passed, the log shows
  `slot 2/3 controls -> true (now 1)` and players 3 and 4 run with their sticks, alongside P2
  (`work/evidence/2026-10-17-*`).

### 5e. Other 2-player limits (`IndexChecks.java`, `queries/67`-`68`)

- **[obs]** `0x1405bcd10(mgr, actor_id)` returns an actor's player index (113 callers).
  `IndexChecks.java` follows that result (and reads of `actor+0x484`) to small compares. The
  `idx <= 1` guards it found that were unpatched are now raised to `<= 3`: ForceMountNode
  `0x14018d846` (per-slot disable add/remove, both bounds-patched) and three view refreshes after
  placing a player, `0x1406a21b4`, `0x1406a262b`, `0x1406aa78f` (`0x1405bb040` is view-guarded).
- **[obs]** Cosmetic, left alone: `0x14008ac70` (does something only for player 1),
  `0x1405fcf30` (a per-player value for players 0/1 only).
- **[obs] Level teardown `0x1405b0ea0`** (called from level unpublish `0x1405f8ed0` and two
  other handlers) resets slots 0/1 only, unrolled: pops each avatar's mapper off its control stack,
  detaches its `CameraState`, removes the view's "Avatar" camera, sets index -1 and avatar 0, and
  clears the slot's disable list (`slot+0x18`). Slots 2/3 kept a dangling avatar after a level
  change. The Rust hook `reset_player_slots` replaces the slot-1 block (`0x1405b0fd6`-`0x1405b10fb`,
  no jumps into it from outside; RBX = mgr and R14 = 0 stay live) and resets slots 1..3. Slots 2/3
  also release their pad, because level setup only restores players 0/1, so P3/P4 re-join with
  Start. **Verified in game (2026-10-17):** "Exit to Game Board" with 4 players logs both slot
  resets, the board loads, Toy Box loads again, and pads 2-4 re-join and move.
- **[obs] How level load brings players back** (`0x1405ae570`, `queries/49`, `71`): the class
  for player 0 is the level's `DefaultAvatar` (or a fixed one for Buzz/train/Bonnie levels); for
  player 1 it is `DefaultAvatar2`, **only when the input driver has exactly 2 assigned players**
  (`0x14021fa80`, `vt+0xd0 == 2`). The same `== 2` sets `mgr+0` (1 or 2). Picks made in the join
  menu are not remembered. So with P3/P4 pads still assigned, P2 would not come back either; the
  teardown hook's pad release keeps vanilla P2 behaviour.
- **[inf]** Bringing P3/P4 back automatically needs: remember their class and pad at teardown,
  then after level setup assign the pad and run the join bind with a spawned copy (no
  `DefaultAvatar3/4` exists). Deferred until the basics are tested in game.
- **[obs]** `0x1401b5bc0` disables controls for players `< 2` from a list; its guard
  (`0x1401b5bec`) was already patched by the bounds pass.
- **[obs] P3/P4 had no buttons (2026-10-22, `queries/197`, in game).** Slot init (`0x1405f0cf0`)
  calls `0x140281140(mapper, index)` for a pad, which writes the per-player binding
  `0x1405bb170(settings, player, action)` into every control of each rebindable action; the lookup
  answers -1 for players 2 and 3 (`cmp edx,2; jge`, a 2 x 12 table at `0x140b2eaa8`). Players 3 and
  4 kept the sticks (read directly) and lost jump, tool and throw: their device saw the A press, the
  avatar did not jump. Patch `bound_1405bb170`: `and edx,1`, so players 3 and 4 use players 1 and
  2's bindings; after it, P3 jumps and P4 throws on every copy.
- **[obs] A level switch with 3 or 4 players crashed (2026-10-22, `queries/198`, dump).** The level
  switch handler `0x1405d7760` (posted `SwitchLevel`) loops over the views and releases each slot
  avatar's `ControlMapper`, but reads the avatar only for index `<= 1` (`cmp rbx,1; ja` at
  `0x1405d77a0`) and passes null for 2/3 to `0x14055a920`, which reads `+0x3b0`. Patch
  `bound_1405d77a0`: `<= 3`; Toy Box to Train Rescue and back with 4 players then ran clean. Exit to
  Game Board (T6) takes another path.

### 5f. Conversation camera crash on views 2/3 (`queries/87`)

- **[obs] Crash.** Talking to a Toy Box character (Ham) crashed with an access violation writing
  `0x194` at `0x1403f81d7` in `CameraGroup::SetState` (RTTI `.?AVCameraGroup@@`, vtable
  `0x1407d3f60`, slot 1): the group's current state (`group+0x50`) was null. Four dumps, same fault,
  single player, all after the 4-view patch.
- **[obs] Path.** The ShowDown (conversation) camera `0x1406a8f40` walks every view
  (`[0x140b274c8]+0x38`, next at `view+0x2f0`) and pushes a camera state on each view's camera
  stack (`view+0x258`) through `0x1403ff250`, which ends in `SetState`.
- **[obs] Cause.** The `views` patch makes level setup create 4 views, but per-player view setup
  `0x1405b0420` (Avatar/General/Tether/SpaceFlight/ToyCamera/ShowDown cameras and the first state)
  ran for views 0/1 only. Views 2/3 had camera groups with no state until a P3/P4 join set them up.
- **[obs] Setup is safe without a player.** `0x1405b0420(mgr, i)` reads slot i's avatar (0 when
  absent) and calls `0x140400990(view, i < player_count)` to mark the view inactive; vanilla already
  runs it for an absent player 2. Joining activates the view through `assign_avatar`
  (`0x1405f1030` -> `0x140400990(view, 1)`).
- **[obs] Fix.** Call hook `level_player_setup` at `0x1405aee38` (the slot-1 view setup and the
  view-array finish `0x1404ce950([rbp+0x398])`): view setup for 1..3, then the finish. The join hook
  no longer runs view setup, so cameras are not added twice.

## 6. Rendering and window

- **[obs]** `-dxresolution WxH` is parsed with `strtok` (`0x1402a5930`); the window is created
  with style `0xca0000` (caption, system menu, minimise; not resizable).
- **[obs] The renderer is built for 4 views** (`queries/20`-`22`):
  - `0x1401f7bd0(rects, n)` copies split-mode rects into the viewport table at `0x1409aac40`
    (0x20 per entry, exactly 4 entries before other fields at `+0x80`) and sets the count at
    `0x14094e294`.
  - Init `0x1401f82e0` (called from `0x14021d460`) creates four cameras, `camera`, `camera 1`,
    `camera 2`, `camera 3`, at `entry+0x18`.
  - The frame setup `0x1401f7f20` turns each viewport into a render pass (`0x14099bd00`, 0x40
    stride, room for 8) using that viewport's camera. Scene render `0x1401e9270` walks four
    views (`param_1[0..3]`).
- **[unk]** How a player's `CameraState` component maps to camera N; whether each viewport's
  camera uses its own aspect ratio. Static search was inconclusive (`queries/34`-`36`); settle it
  at runtime.

### 6a. HUD (`queries/32`-`33`)

- **[obs]** HUD screens form an enum-indexed table at `0x14094e700` (16-byte `{name, flags}`
  entries): `IPCounter7`, `P1HealthBarHud`, `P2HealthBarHud`, `P1MissionCompassHud`,
  `P2MissionCompassHud`, `ControllerLayoutHUD`, `NegativeUIScreen`, `P1AmmoMeterHud`, ...
- **[obs]** Per-player HUD code addresses P1 and P2 explicitly, by name and by fixed slot (e.g.
  `0x1402091a0` binds the mission compasses to `mgr+0x1a0` and `mgr+0x1f8`), not by player index.
- **[inf]** Players 3/4 get no HUD rather than a crash. Giving them one means new screen assets
  (`UI/screens/*.sc`) plus table entries; that is polish for later.

## 7. Existing patch on the live exe

- **[obs]** The live exe differs from the original in 7 bytes at file offset `0x2b6400`
  (VA `0x1402b7000`): `movss xmm0, [rip+const]` became `movaps xmm0, xmm3` + `nop`. It sits right
  after a `divss xmm3, xmm1` and before a `comiss`.
- **[inf]** Removes a clamp on a computed ratio (probably aspect ratio or FOV). Not made in this
  repo; purpose unverified.

## 8. Determinism surface (for native netcode, `queries/74`-`77`)

- **[obs] No networking code**: no Winsock or other socket imports (also §1). Steam is linked
  (`steam_api64.dll`, 7 imports) but only for callbacks/achievements.
- **[obs] One game clock.** `0x14025a5e0` returns milliseconds from `QueryPerformanceCounter`; the
  per-frame update `0x14025a470` averages the last 15 frame times from the same source, clamps to
  0.1 s, and writes the frame delta (`0x14094ecf0`) and a frame counter (`0x1409c2928`).
- **[obs] A built-in clock override.** Both functions return a stored time (`0x1409c2918`) instead of
  reading the counter when the byte `0x1409c27b9` is set. Nothing in the exe writes either, so it is
  a dormant debug or capture mode the loader can turn on and drive frame by frame.
- **[obs] The override pauses the game (2026-10-20, `queries/126`, `129`, in game).** The stored time
  is a u32 of milliseconds; `0x1409c291c` holds the last real reading. With the stored time set to
  that reading and the byte set, frames keep running at 60/s (counter `0x1409c2928`) with a frame
  delta (`0x14094ecf0`) of 0; clearing the byte gives one frame of the 0.1 s clamp. The real clock
  is `(QPC - [0x1409c2dc0]) * 1000 / [0x1409c2db8]` (i64 base and frequency, set once by
  `0x14025a540`). [inf] Adding the time held to the base makes the clock carry on from the stored
  time. Gameplay does not stop with it (§10, the game's pause), so the loader does not use it.
- **[obs] RNG.** The CRT `rand` has 6 callers (`0x1400c4660` town-person spawn, `0x140189bd0`,
  `0x140268470`, `0x14046a660`, `0x140522f70`, `0x1407127a0`) and `srand` is never called, so it runs
  from the CRT's default seed. The UCRT keeps `rand` state per thread. One more generator: an LCG
  with multiplier `0x19660d` in `0x14070b840`. Engine-level random nodes exist (`RandomIntNode`,
  `RandomFloatNode`, `Navigate::RandomPointSearch`); their source is not traced yet.
- **[obs] Concurrency.** Threads come from 2 creators (`0x140250010`, `0x140257bc0`) and fibers from 2
  (`0x14025a8b0`, `0x14025ae20`): a job system. Asynchronous work exists: `AsyncActorCreator`
  (`AsyncActorTask`), `ChoreographyNode::AsyncChoreographyTask`, `Navigate::DeferredWorker`, and the
  world streaming manager.
- **[obs] Physics is Bullet** (`btCollisionWorld`, `CBulletVehicle`, `CRigidBody`), which repeats
  exactly given the same step size, inputs and object order on one thread.
- **[obs]** The game reads FMOD Studio timeline positions (`getTimelinePosition`), so some timing may
  follow audio playback.
- **[unk]** Whether the job system or async tasks finish in an order that changes gameplay; whether
  any gameplay depends on what is rendered (culling, LOD); where the random nodes get numbers;
  whether CPU-specific CRT math paths differ between machines. Only the lockstep alternative in
  NETCODE.md depends on these.

## 9. Running copies side by side (for host/client play, `queries/78`-`80`)

- **[obs] Single instance.** `0x14027e130` creates the named mutex `OctaneApp`; if it already exists
  (`ERROR_ALREADY_EXISTS`) or access is denied, the game shows "Cannot start Octane Application
  because another instance is already running." and exits.
- **[obs] Steam relaunch.** Startup `0x1402839d0` calls `SteamAPI_RestartAppIfNecessary(4049180)`
  (`0x3dc91c`) at `0x140283a78`, then `SteamInternal_SteamAPI_Init`. A copy started outside Steam
  is relaunched through Steam.
- **[obs] Focus.** The window class (registered in `0x1402a5930`, WndProc `0x1402b6d70` ->
  `0x1402b08f0`) handles `WM_ACTIVATE` by releasing or restoring the cursor clip and re-acquiring
  input (driver `vt+0x100`); it sets no pause flag. `WM_SIZE` minimise sets flags at `+0x681`-`+0x684`.
- **[obs] Transforms.** An actor embeds its scene node at `actor+0x50`. The placing handler
  (`0x1406a2010`, `queries/68`) teleports an actor like this: fire `PrepareForPlacing` with the actor
  id, write the position to `actor+0x80/+0x84/+0x88` (x, y, z), mark the node dirty
  (`0x14056dab0(node)`), build a yaw quaternion and set it with `0x140568b50(node, quat)` (rotation
  at node `+0x40` = `actor+0x90`), then call the node's `vt+0x8`. Reading those fields gives the
  local player's transform; repeating the sequence places a proxy.
- **[obs] Spawning an avatar without the menu** (pick handler `0x140104190`, `queries/16`): build a
  property set (`0x140288540`) from a string such as
  `class=woody; model_name=characters/Woody/Woody` (`0x14050d490`), spawn with
  `0x1405af7e0(mgr, props, &pos, yaw, 0)` (the spawner level setup uses; an async variant
  `0x1405af9e0` is used when `0x14094cce2` is set), name it (`0x1400e7550`), free the property set
  (`0x1404f8cb0`), activate it if `avatar+0x17c` bit 0 is clear (`0x1405503e0(world, avatar)`,
  `0x14055b850(avatar, 0)`), then bind it (`0x1400e2490`, hooked as `bind_joining_avatar`). The
  spawn point is next to player 1 (`p1+0x1d0`) or the level's `StartPosition_P2_<region>`.
- **[inf]** With controls off, an avatar's `KinematicDriver` is active (`0x1405565c0`, §5d), which
  suits a proxy placed from network data.
- **[obs] A once-per-frame hook site on the game thread** (`queries/82`): the input dispatch
  `0x140280b90` runs once per frame and, at `0x140280c18`-`0x140280c2a` (18 bytes), makes three calls
  with no arguments, each followed by a nop: `0x140227920`, `0x1402292a0`, `0x140229660`. A `call`
  hook can replace the block with one Rust function that makes the same three calls and then does
  per-frame network work (read slot 0, place proxies). The stack is 16-byte aligned there.
- **[obs]** The `PrepareForPlacing` event name is at `0x140771420`.
- **[obs] Proxy spawn details (`queries/88`).** The property set is 3 qwords (`0x140288540` zeroes
  it); `0x14050d490` parses `key=value;` pairs into it. `0x1405af7e0(mgr, props, &pos, yaw_radians,
  0)` returns the avatar. Naming `0x1400e7550(avatar, name, hash)` uses `Woody2`/`hash(0xd4)`,
  `Buzz2`/`hash(0xcc)`, `Jessie2`/`hash(0xcf)` and fires `RespawnAvatar`. The menu takes the async
  spawner `0x1405af9e0` because the byte `0x14094cce2` is 1 in the image (read once, never written);
  level setup uses the sync one. A leaving player's avatar is removed with `0x140556230(avatar)`
  (player leave `0x1405fc7d0`).
- **[obs] Rotation.** The node keeps a quaternion `(x, y, z, w)` at node `+0x40` (`actor+0x90`).
  Euler to quaternion `0x14024b9c0(&euler_radians[3], &quat)` (half-angle factor `0.5` at
  `0x140776e28`), quaternion to 3x3 matrix `0x140256d00(&mat, &quat)`, and the setter
  `0x140568b50(node, &mat)` converts back, rotates node `+0x80` along, and marks the node dirty.
  A pure yaw `(0, yaw, 0)` gives `(0, -sin(yaw/2), 0, cos(yaw/2))`, so yaw = `-2 atan2(qy, qw)`.
- **[obs] Actor lookup by id.** `0x140558400(id)` returns the actor for an id (`actor+0x38`) or 0;
  the placing handler resolves its target this way, so a stored id survives a level change safely.
- **[obs] Verified in game (2026-10-17):** with `enable=instances`, copy 1 started from Steam and copy 2
  started through Explorer run side by side, each with its own number, log, pad mapping and window
  rect; copy 2's saves go to `%LOCALAPPDATA%\ts3online\copy2`. Pad isolation holds: a copy ignores
  the other copy's pad.
- **[obs] Only the focused copy reads its pad.** An unfocused copy keeps running (attract demo at
  about 15 fps) but ignores its own pad. `GetFocus` is only used for the cursor, and the XInput
  read `0x14027c000` (the controller's update, via the thunk `0x140717a60`) has no focus check.
- **[obs] The exe does not gate pads on focus (`queries/85`-`86`).** The driver update `0x14027b500`
  calls every controller's `vt+0xd8` each frame (XInput: `0x14027c000`), gated only by a device
  re-enumeration timer. Focus flag `driver+0x501` (set on `WM_ACTIVATE`) only zeroes keyboard and
  mouse state. The exe never calls `XInputEnable` (only ordinals 2-4 are imported).
- **[inf]** So `XInputGetState` itself returns neutral input to the unfocused copy, most likely
  Steam Input's in-process hook (the test pads are seen by Steam). The pad hook now logs each
  button change with `foreground=` to confirm. Real online play is unaffected (one focused copy per
  PC); this only matters for two copies on one PC.
- **[obs]** On deactivation the window handler sends the window to the bottom of the z-order
  (`SetWindowPos(HWND_BOTTOM)` in the `WM_ACTIVATE` branch of `0x1402b08f0`), so only the focused copy
  stays visible; `tools/shot.ps1` uses `PrintWindow` to capture a covered copy.
- **[obs] Pads** are read through `XINPUT1_4` ordinals 2, 3 and 4 (`XInputGetState`,
  `XInputSetState`, `XInputGetCapabilities`).

## 10. Online co-op partners (H3, `queries/89`-`94`)

- **[obs] Partners bind like local P2 (2026-10-19, in game).** With a virtual XInput pad at index 1
  (`input_assign(input, 1, 2)`: XInput pad k is input device k+1, device 0 being the keyboard), a spawned
  avatar bound through `bind_joining_avatar` lands in slot 1, the player count goes to 2, and the
  `online` patch keeps one full-screen viewport. The partner's pad state drives it through the game's
  own `PlayerDriver` (running animation, co-op prompts).
- **[obs] Movement input is camera-relative.** A partner's avatar on the receiving copy uses the
  camera of its own view (`views[slot]`, not rendered), whose heading differs from the owner's camera,
  so replayed stick input sends it a different way until the position snap catches it.
- **[obs] View camera matrix (runtime memory diff).** `view + 0x20` holds a row-major 4x4 view matrix
  (rows at `+0x20/+0x30/+0x40/+0x50`). Column 0 is the camera's right vector (y = 0), column 2 its
  forward axis, so the camera heading is `atan2(view[+0x28], view[+0x48])`. Views are `0x300` bytes
  apart in one block; `views[i]` belongs to slot i.
- **[obs] The focus gate (corrects §9).** The input driver update `0x14027b500` reads the keyboard
  and mouse through DirectInput first; while the window is unfocused those reads fail, it re-acquires
  them and jumps straight to its exit (`0x14027b6b4`, `0x14027b910`), so no controller is updated at
  all. The `online` group retargets both jumps to the controller loop (`0x14027b9a2`). Steam Input was
  not the cause. With it, partner input works on a copy without focus.
- **[obs] Closed-loop steering tracks within 0.1-0.4 m** (T10): the partner's left stick is computed
  from the owner's position and velocity in the partner view's camera frame; buttons and triggers are
  replayed as they are.
- **[obs] Carry and throw replicate through button replay** once the partner's pose is matched to the
  owner's at the moment of an action press (pose error 0.00-0.11 m, under 1 degree). Buzz picking up
  Woody and Woody picking up Buzz both showed on both copies; after the throw the thrown player landed
  within 0.3 m on both.
- **[obs] The pick-up target is the `PlayerPerception` candidate (2026-10-19, `queries/97`-`99`,
  `121`, in game).** The avatar's `PlayerPerception` component holds the nearest thing it could pick
  up at `+0x280` while its state `+0x278` is 0 or 1; on the X press the state goes to 24 and `+0x208`
  becomes the carried actor (`PlayerPerception_GetCarryActor`, `0x1400ba380`, reads `+0x208`, else
  `+0x280` in states 0-1). The game posts `CARRY_START a=0 b=<carrier id> actor=<target id>`, then on
  RT `StartThrowAction`, `StartThrow`, `CARRY_END`, and `CarriedThrowLand` on the thrown actor.
  Button replay picked whatever was nearest on the receiving copy: one test grabbed a townsperson
  that stood elsewhere on the owner's copy. Writing the partner avatar's `+0x280` each frame to the
  owner's target holds (the game did not rewrite it in testing), and the replayed X then picked the
  right target even with a 110 degree pose error. `PlayerPerception_CarryBegin` (`0x140056b30`) only
  posts `StartPickup` (a=0, b=carrier) to a non-AI target: posting that alone put the target in its
  "being lifted" pose with no carrier, so a carry has to start from the carrier's own action.
- **[obs] A thrown Pixar ball is a new physics actor (2026-10-20, in game).** With the ball tool out
  (B, `TogglePixarBalls`), RT posts `StartThrow a=0 b=<thrower id> actor=<new id>`; the new actor is
  `Avatar_Tools/Pixar_Ball_Physics` (no reference name) and despawns a few seconds after it lands.
  Its `actor+0x80` stays at the spawn point; `+0xec` and `+0x100` hold per-frame copies of the
  physics position and `+0xe0` of the velocity, and writes to them are undone the next frame. The
  physics state is `[[actor+0x20]+0x78]`: origin at `+0x60` (and `+0xa0`), velocity at `+0x160` (and
  `+0xb0`); raising the origin by 4 m mid-flight moved the ball and the simulation carried on from
  there. Without sync the two copies' balls matched within 0.5 m for 2 s, then bounced apart (1.5 m
  by 3 s).
- **[obs] Townspeople have the same reference names on every copy (2026-10-19).** Spawned
  townspeople, cactus toys and help capsules are named after their spawner (`TownpersonSpawn
  Actor165_167`, `ENV_CactusToySpawn Actor105_164`) and those names matched on both copies; static
  ones stood at the same places, but townspeople had walked to different places on each.
- **[obs] Context prompts are per view (`queries/110`-`114`).** The context prompts ("X Pick up") are
  `HudControllerLayout` (screen `ControllerLayoutHUD`), with two panels: P1's at `+0x260` ("LeftIn")
  and P2's at `+0x2f0` ("RightIn"). `0x140218770(hud, text, id, player)` adds a prompt to
  `panel[player]` only when `player < min(view count, 2)` (`[0x140b2ef50]+4`; the bound at
  `0x1402187e6` is raised to 4, but the view-count test still holds players 2-3 out), and to both
  panels when `player >= 4` (prompts for every player). With the `online` patch's single view, the
  partner's own prompts are dropped. [inf] The "X Pick up" next to the partner is then the local
  player's own prompt to pick the partner up. (This corrects an earlier note that said partner prompts showed on
  the local screen.)
- **[obs] Level changes are events.** The app registers `SwitchLevel`, `ProcessSwitchLevel` and
  `LoadLevel` (3 arguments) with the event system (`0x1405cc7d0`). Leaving the avatar select for Toy
  Box posts `SwitchLevel` with the string `"WesternTown"` (`0x14012e800`, also sets `0x140992e77`);
  other posters: `0x140129600`, `0x1406a67a0` (choreography), `0x1401278d0`.
- **[obs] `SwitchLevel` can be posted with a level name.** Posting it from the dev command file with
  the level name as argument reloads that copy's level in about 5 s, and the online partner re-binds
  afterwards (level teardown resets the slots; `level_player_setup` and `bind_joining_avatar` run
  again). Entering Toy Box from the game board posts `SetupLaunch` (level props `filename`), not
  `SwitchLevel`.
- **[obs] Save size.** The whole save is one ~20 KB file (`savedata\Saved Game`) plus
  `newsettings.bin`, small enough to send at connect.
- **[obs] Level names (2026-10-19, `coop: level` log).** `level_name()` (`0x1405bb6a0`) reads
  `gui_storymode_boardgame` on the story game board, `westerntown` in Toy Box and `wt_stuntpark` in the
  stunt park; a fresh board launch can pass through `st_train`. The name changes the moment
  `SwitchLevel` is posted, before the old level is torn down. Follow treats `gui_*` and `frontend`
  as menus.
- **[obs] Level load completion and intro movies (2026-10-20, `queries/130`-`131`, in game).** The
  level update `0x1405dd410` posts `LevelLoadComplete` (broadcast, no arguments) when streaming is
  done, then, when the level's `postLoadMovie` property is set (flag `+0x2a9`, set by
  `0x1405d81a0` after `LevelPublishDone`), plays that movie in the same call through the Bink
  player `0x14047f390` (`FMVPlayer`; `BinkOpen`/`BinkWait`/`BinkClose`), which blocks the game loop
  until the movie ends. The avatar is up before either: Train Rescue showed `LevelPublishDone` and
  the avatar, `LevelLoadComplete` 0.05-7 s later, then a single 62 s frame (its intro movie).
  Toy Box after a fresh start took 26 s from `LevelPublishDone` to `LevelLoadComplete`.
- **[obs] A story level from Toy Box loads slowly (2026-10-20).** On one PC with two copies, the host
  loaded Train Rescue from the story board in 3 s; the client, following from Toy Box, needed 65-75 s
  to its avatar. [unk] How much of that is the second copy sharing the disk.
- **[obs] The game's pause (2026-10-20, `queries/132`-`135`, in game).** The pause menu posts
  `Pause` with a pointer to `{i32 kind @0, char* screen @8, u8 pause_audio @0x10, u8 @0x11,
  i32 result @0x14, i32 player @0x18}`; listeners dereference it (`0x1405beba0` reads `kind`), so a
  null argument crashes the game (c0000005 at `0x1405beba0`, seen). The handler `0x1405d6360` sets
  the game timer `0x140954820` to speed 0 (`0x140479c00(timer, speed, 0)`; speed 9 is normal, and
  the timer also sets the audio timer rate), posts `SyncTimer`, pauses audio when `pause_audio`,
  locks controls (`GameCommonLoopLock`), opens `screen` when given, sets `0x140b2c5a6` and posts
  `PauseChanged 1`. `Unpause` (handler `0x1405f8d60`) posts `RequestUnpauseSpeed` (9 unless a
  listener changes it), restores the speed and audio, unlocks controls and posts `PauseChanged 0`.
  `0x1405bf660` posts kind 4 with no screen and `0x101` in the audio bytes; posted by the loader
  in Toy Box, that changed nothing (the `Pause` listener `0x1405beba0` routes kind 3 and kind 4 to
  different calls of the front end, so kind 4 is a request). Calling the handler's own steps
  pauses: `0x140479c00(timer, 0, 0)` put the timer in mode 0 (rate 0) as the pause menu does, and
  2 s of stick moved the player 0 m (13 m after speed 9). Scripted sequences run on this timer,
  not on the raw clock: with the clock override of FINDINGS 8 holding the frame delta at 0, Train
  Rescue's opening still shot the idle player down.
- **[obs] A player down alone in a story level waits (2026-10-20, `queries/124`-`128`).** The host's
  idle Woody, shot down in Train Rescue while the client was still loading, sat on a black screen
  with its health bar for minutes; the pause menu did not open. `Respawn` (component, handlers
  `ManDown` `0x140657ce0`, `RespawnAvatar` `0x140658f10`) sets its down flag and a
  `RespawnControlLock` on `ManDown`; a menu screen named `SCN_multiplayerRespawn_wait` exists.
- **[obs] What ends the wait (2026-10-20, `queries/137`-`140`).** The `Respawn` component
  (`0x14064a820`, `0x188` bytes) runs a state machine in its update `0x140645280`, state at `+0x18`:
  `ManDown` sets 1 and the down flag `+0x20`; 1 goes to 3; 3 waits `+0x2c` ms of game time, then
  goes to 4 (the player has a pad: the `Respawn` screen fades in) or 6; 4 waits for the fade, then 6.
  State 6 sets `+0x1e` (waiting) and, when the two-player respawn is on (`[0x140b2fdf8]+8`) and two
  players are in, looks at the other slot's `Respawn`: if that one waits too, both go to 5 (respawn
  at the last checkpoint start, `+0x30`, posting `StartingRespawn`); else, once the point stored for
  this player is valid and the partner is not down (`+0x20`), it goes to 7: respawn at that point
  (`RespawningActorDied`, `PlayerSpawning`, `FinishedRespawn`, back to 0). Otherwise it waits
  forever. With one player, or the two-player respawn off, 6 goes straight to 5.
- **[obs] Respawn points (`queries/139`-`140`).** `[0x140b2fdf8]` holds one point per player: valid
  flag `+9`, position `+0xc`, 3x3 rotation `+0x1c` for the point player 0 set, and `+0x40`, `+0x44`,
  `+0x54` for player 1's. A player respawns at the point its partner set: `0x140632fe0(points, 0)`
  reads `+0x40` and `0x1406176e0` its position. The setter `0x14060fc10(points, player, pos, rot,
  pull, distance)` is called by a checkpoint script node (`0x14016bec0`, cleared by `0x14016c7a0`);
  with `pull` set and the other player farther than `distance` from the new point, it respawns the
  other player there (`0x14066b060`). `0x140632ac0` clears both at a level start.
- **[obs] A remote partner reaches no checkpoint (2026-10-20, in game).** On a copy, the stand-in of
  a remote partner is placed from network data and never triggered a checkpoint node, so the point
  for the local player stayed invalid and a downed local player waited in state 6 with
  `ShowRespawnTip` and no `FinishedRespawn` (90 s in the run of T20). Filling player 0's point
  (`+0x40` from the `+9` block, or the partner's position with an identity rotation) after 2 s of
  state 6 respawned the player within a frame (`FinishedRespawn`), twice in two runs.
- **[obs] Actor interfaces by type id (`queries/150`).** An actor's `vt+0x48` (TS3Avatar
  `0x1405598e0`) takes a type id in `edx` and returns its interface of that type, from the
  component set at `actor+0x368` or `actor+0x370`. Type ids are numbered on first use from the
  counter `0x140b2a448` and kept in a global per type (the decompiler hides the argument: it shows
  the inline `if (id == 0) id = ++counter` before the call). The state machine is type
  `0x140b2a44c`: `vt+0x8(name, 0)` requests a state, `vt+0x60(name)` and `vt+0x18(name)` test one.
- **[obs] Riding Bullseye (2026-10-20, `queries/141`-`149`, in game).** A rider is in the state
  `Riding` (`vt+0x60` true), both on a summoned horse in Toy Box and on Train Rescue's horse, which
  runs by itself (`BullseyeAutoRide`, true only under `vt+0x18`). The game's summon (`0x140220db0`,
  case 6) spawns a Bullseye from the property string `0x14078f520` with `0x140554a80(props, &pos,
  yaw, 0, 0)`, sets the rider's blackboard `mountActor` (`0x140561ae0(actor, 0)`, `0x14054c420`) to
  its id and requests `BullseyeForceRide`; done for a partner, it sat on the horse in `Riding`, in
  Toy Box and in Train Rescue. That horse did not move with the rider's stick, and placing it
  (`+0x80`) did not carry the rider, so both have to be placed. The script binding
  `CreateJessieRidingBullseye` (`0x1400b48d0`), the game's own second player on a horse, spawns
  `Bullseye2` (props `0x140774390`), sets `myBullseye` and `bullseyeAutoRideMode` and requests
  `BullseyeAutoRide`; done for a partner in Train Rescue, the horse followed the local player as a
  companion and the partner stayed on foot.
- **[obs] Auto-drive and the join's level region (`queries/141`-`143`).** The join menu's pick
  handler passes the bind `*0x1405bc790(streaming, 0)` (player 1's level region; its first field is
  the level info, region name at `+0x10`) and a game string `StartPosition_P2_<region>`
  (`0x140023fb0(&out, cstr, allocator)`). The bind auto-drives the joining player
  (`0x14019b110`) only in levels with flag `0x1f1`, `0x1f6` or `0x1f8`: it takes player 1's ride
  from its `RideController` (`+0x30`, the ride's id), the matching ride of the other group in
  `[0x14099b8b0]` (`+0x188` groups of three ids, `+0x190` count), places it at the start locator
  and posts `ForceRide(ride, 0, avatar)`. Train Rescue has no region for player 0 and no ride
  groups: its horse is the NPC `BullseyeNPC`, mounted by the level at its start.
- **[obs] Removing an avatar mid-level can crash later (2026-10-19, in game).** In `wt_stuntpark`, with
  2 players, removing the partner (`assign_avatar(slot, 0)` + `remove_avatar`) dropped the player
  count to 1 and the copy crashed about 4 s later (c0000005 at `0x1401bdc5e`). The faulting function
  `0x1401bdb60` (called each frame from `0x14021ed70`) checks two player entries for shared vehicle
  seats ("Occupied") and, with one view (`[0x140b2ef50]+4 == 1`), dereferences
  `0x1405bc790([0x140b2ef20], 0)`, which returns null when neither per-player flag at `+0x48`/`+0x50`
  is set for player 0 (`queries/104`, `105`). One copy alone in `wt_stuntpark` runs fine, and leaving the
  partner avatar to the level teardown, or parking it idle when its owner leaves, avoids the crash.
  [unk] Why the flag for player 0 is cleared by the removal.
- **[obs] Stats table (`queries/100`-`102`).** `[0x140b2fdb8]` holds the game stats: entries of 0x60
  bytes at `+8`, count at `+0x10`; name inline at `+4`, type at `+0x3c` (0 add, 1 max, 2 min, 3 mean,
  4 average, 5 capped add with the cap at `+0x44`), value at `+0x4c` (int or float), times changed at
  `+0x48`. `0x140632130(stats, idx, int)` and `0x1406322a0(stats, idx, float)` apply a value by type,
  `0x140617bc0(stats, idx)` reads one. Toy Box gold shows as `Stat_GOLD_COLLECTED` (index 47).
- **[obs] Wallets (H6a, `queries/106`).** `Stat_GOLD_COLLECTED` is the gold wallet the Toy Box HUD shows
  (writing its value makes the counter animate to it; `GoldCoins` pickups add 100 via `0x140632130`).
  `Stat_IMAGINATION_POINTS_COLLECTED` is the second counter. `0x140617c40(stats, name)` finds a stat
  index (-1 when absent).
- **[obs] Actor ids differ between copies (2026-10-19, `queries/115`).** The actor registry behind
  `0x140558400(id)` is a 256-bucket hash table at `0x140b2bb90` (bucket `id & 0xff`; node `[0]` actor,
  `[1]` id, `[3]` next). Two copies in Toy Box from the same save both held 943 actors (RTTI: 818
  `Npc`, 86 `CLineEntity`, 20 `CStageEntity`, 14 `CSphereEntity`, 4 `TS3Avatar`), but only 887 shared
  an id and class, and 163 `Npc` with the same id stood at different places: the same set of
  positions under other ids, from a different spawn order. Ids therefore cannot match NPCs or enemies
  across copies; a world sync has to match them another way (name, spawner, start position).
- **[obs] Actor names and reference names (2026-10-19, `queries/116`-`118`, in game).** Every actor has
  an actor name `char*` at `actor+0x148` and a reference name `char*` at `actor+0x150`. The actor manager
  is `[0x140b2bb70]`. `0x140557e10(mgr, name)` finds an actor by actor name and `0x140557f60(mgr, name)`
  by reference name: they walk three lists at `mgr+0x100`, `+0x1c0` and `+0x1a8` (node `+8` actor, `+0x30`
  next), compare with `_stricmp` and return the actor or 0. The script bindings `FindActorByActorName`
  and `FindActorByReferenceName` (registered at `0x14056b02e` / `0x14056b062`) point at them. The actor
  id is the u64 at `actor+0x38`; `0x140558400(id)` returns the actor for an id. In Toy Box 756 of 822
  actors had a reference name that was unique and identical in both copies; the ambiguous rest were
  runtime spawns (50 `PICKUP` coins, 8 `CowAmmo`, a few vehicles) and actors with an empty reference
  name. So a reference name identifies most placed actors across copies where ids do not.
- **[obs] Crate break event sequence and cross-copy replay (2026-10-19, in game).** Breaking a crate on
  the host posted, through the targeted post `0x14049e0b0` (events, name, a, b, actor id),
  `ExploderDestroy a=0 b=0 actor=0x33e` (the crate, reference name
  `EXP_CrateSmallREDUCEDEXP_CrateSmallREDUCED_40`), followed by the broadcasts `SpawnGoldPickup` and
  `SmashCrate` with b=0x33e and `TriggerPickup` on the new coin actors. On the client the same crate
  had id 0x34b, and it broke there too, but by the client's copy of the host's player hitting it.
- **[obs] `ExploderDestroy` is a notice, the break is `MExploder` (2026-10-19, `queries/119`-`120`, in
  game).** `0x140652ac0(exploder)` does the break (debris, `Slave` and kinematic controller off, sets
  `exploder+0xdc` bit 0) and posts `ExploderDestroy` to the owner (`exploder+8`) afterwards. It runs
  when `exploder+0xdc & 3 == 2` (bit 1 requested, bit 0 done) and the vcall at `+0x38` agrees; its only
  caller is the handler `0x140656710` of the `Explode@MExploder` message. The component is
  `MExploder`, found with `0x140558fb0(actor, "MExploder")` (component lookup by name through
  `actor+0x370`; a script getter `0x1405ea4b0` reads bit 0 as "exploded"). Posting `ExploderDestroy`
  to a crate a late-joining client had intact changed nothing; setting bit 1 and calling
  `0x140652ac0` broke it with its coins (screenshot), and `actor+0x3f8` became 0 as on a crate broken
  by hand.
- **[obs] Screens a partner opens on the other copy (2026-10-20).** Carrying a townsperson puts the
  carrier in `StrafeAndThrowAlien` with its controls off (`avatar+0x428` = 0) and, the first time,
  shows the throw tutorial (`ShowThrowTut`, a push on that slot's controller stack:
  `cstack<slot>_count` 1 -> 2). The copy where the carrier is a partner shows the same tutorial for the
  partner's slot and waits for the partner's RT. Throwing into a shop (`ActorEnteredBuilding a=<lot>
  b=<townsperson>`) opens the shop's customization screen (`SharedScreenModeChanged a=1`), which also
  reads the carrier's pad on both copies. A choice ends with `BlockheadCustomized actor=<townsperson>`,
  `ActorCustomizationTaskSuccess`, `CHALLENGE_PASS_MESSAGE`, `CHALLENGE_AVAILABLE_MESSAGE`, then the
  reward coins (`TriggerPrimedRewards`, `TriggerPickup`). With the owner's pad mirrored to a partner
  whose controls are off too, both copies ran the mission "Give an Alien a Hat" to the same hat and
  the same completion.
- **[obs] Toy Box lots (`queries/122`-`123`).** Each building site is an `Npc` actor with reference
  name `LOT_<site>` (`LOT_TownHall`, `LOT_TownMedium2`, `LOT_HatShop`, ...) and actor name
  `BL_<building>`, `BL_EmptyLot` while nothing is built. 15 lots in Toy Box town on this save. The
  purchase flow is named by strings: Al's Toy Barn catalog (`ScreenAlsToyBarn`, `Catalog_ItemPurchased`,
  `PurchaseBuilding`), a lot selector (`BuildingLotSelector`, `BldgLotSlctEnter`), then
  `BuildingContructionComplete`; script bindings include `BuildingData_SetBuildingStateForBuildingLot`,
  `BuildingData_SetBuildingBuilt` and `BuildingLotData_InitializeLot`. [unk] Not exercised: the test
  save has not opened the catalog yet.
- **[obs] Health and damage (`queries/151`-`154`).** An actor's health is the `HealthTracker`
  component (`0x158` bytes, vtable `0x1407d94f8`; `TS3HealthTracker` for avatars); its health
  interface sits at `+0x18` (type slot `0x140b2a464` for the actor's `vt+0x48`) with the owner actor
  pointer at interface `-0x10`, max health at `+0x8` and health at `+0x14` (f32; 100 by default).
  Interface `vt+0x48` reads health, `vt+0x50` max, `vt+0x18(value)` sets it, and `vt+0x80(amount,
  source actor id, flag, kind)` takes damage: `0x140441f80`, shared by both trackers
  (`TS3HealthTracker` first posts `ManDownZeroHealth` when the hit is lethal). Unless `kind` is 3 it
  returns when the owner is inactive (`vt+0x38`), invulnerable (`+0x24`) or a friend of the source
  (`FriendlyFilter`), and scales the amount by `+0x50` and the per-kind factor `+0x54 + kind*4`; kind
  3 skips all of that. It then sets health (`0x14045f350`), posts `DamageNotification`, and at 0
  health posts `NotifyOfDeath` (flag `+0x64`) or calls `vt+0x90(source)`. Every hit reaches it:
  `HealthTracker`'s `MunitionImpact` handler (`0x140449a80`) passes the munition's damage
  (`impact+0x4c`) and kind (fourcc `expl` 1, `spec` 2), the script binding `DoDamage` (`0x1405e77f0`)
  passes the script owner as the source.
- **[obs] Damage in Train Rescue (2026-10-21, in game).** The opening's obstacles (`TripperPrism1`..`10`)
  hit the horse for 1, and the horse passes 20 to its rider, source the horse; four hits downed an
  idle Woody. On the copy where a partner rides its summoned horse, the obstacles hit that horse and
  the stand-in as well. The level's enemies are not out at its start: the UFOs (`UFO`..`UFO9`),
  `Dr_P` and the train's `TrainStub:*` actors are placed with unique reference names. The level's
  Jessie companion (`Jessie2` on `Bullseye2`) uses `TS3HealthTracker` like the players (interface
  vtable `0x1407692b0`; the plain tracker's is `0x1407d9660`); on one copy it went to 0 health and
  `Bullseye2` fell out of the world, while on the other both stayed at 100, and raw damage to the
  companion did not keep it down. Toy Box's chickens (`Created_5`..`10`, reference names
  `Created_<n>Spawn Actor<m>_<k>`, equal on both copies, ids not) have plain trackers, take 10 from a
  punch, flee once hit and stay in the world at 0 health.
- **[obs] Controller devices and keyboard input (#9, `queries/155`-`161`).** The input driver keeps up to
  11 devices at `driver+0x460` (`+0x8c` qwords; `XInputInputDriver` per pad, `KeyControllerInputDriver`
  for the keyboard), `vt+0x70(player)` returns a player's device and a device's `vt+0x10` its kind
  (`XInput` for pads). Every device shares one layout: code k's state is a byte pair at `+0x10 + 2k`
  (previous) and `+0x11 + 2k` (current), the left stick is f32 at `+0x68`/`+0x6c`, and `+0xb0` points
  at the u32 code of each control id. The XInput poll (`0x14027c000`) fills codes 1-16 (dpad left,
  down, right, up, X, A, B, Y, LB, RB, LT, RT, START, BACK, LS, RS); the keyboard device's update
  (`0x14027ad90`) fills codes `0x13`-`0x25` from the keys bound to them (`0x140282570`), and the
  driver update reads the keys through DirectInput. `0x1402150b0` sets a device's control table: on a
  pad control 5 is A, 0/2/6 X, 1/7/9 B, 3/0x11 Y and the throw controls RT; on the keyboard several
  controls share a code (attack and throw are one key), so a key cannot be replayed as one pad button
  without context. `0x140281140` applies the per-player key rebinding (`0x140b2eaa8`) to the table.
  [obs] In game (2026-10-21) the keyboard device is updated every frame while player 0 reads a pad,
  but its control table is all zero (only an assigned device gets one): Space sets codes `0x15` and
  `0x25`, E `0x14` and `0x16`, Q `0x19`, Z `0x1f`. Input driver `vt+0x80(player, 0)` did not move
  player 0 off its pad.
- **[obs] On-screen text for a waiting message (`queries/162`-`165`).** The script binding
  `ShowTextMessage` (`0x1405ee6b0`) posts `HUDShowMessage` with `{char* text, u32 time, u8, u8}` and
  `HideTextMessage` posts it with no argument, but `GameHUD`'s handler for it (`GameHUD::ShowMessage`,
  vtable `0x1407f6778`, slot 3) is an empty stub in this build. `HelpText_Show` posts `ShowHelpText`;
  `PrototypeHUD_ShowMessage` calls `0x140218770(hud, text, mode, player)`, which writes into the
  `ControllerLayoutHUD` screen's per-player text only while no `TutorialTipScreen` is up and the
  player stands still. [unk] Whether either draws literal text or needs a string-table key; the level
  start hold shows no message yet.
- **[obs] Missions are challenges (2026-10-21, `queries/166`-`170`, in game).** The `ChallengeManager`
  `[0x140b2fdc8]` holds the challenge pointers at `+0x10`, count (i32) `+0x18`; `0x140614570(mgr,
  &name)` finds one by name (`strcmp` on `challenge+0x80`, a `char*`). A challenge's state (i32) is
  at `+0x94`: 0 and 1 not yet available, 2 available, 3 active, 4 passed, 5 failed, 6 presenting
  (`0x140617e30` returns the field; the script getters `ChallengeManager_IsChallengeActive`/`Available`
  test 3/2). `0x1406362f0(challenge, state)`, slot 19 of the challenge vtables, sets 2, 3, 4 or 5 the
  way the game does (`CHALLENGE_AVAILABLE/ACTIVATE/PASS/FAIL_MESSAGE`, rewards for a pass, then
  `ChallengeStateChanged`); any other value only writes `+0x1c0`. Toy Box on the test save has 291
  challenges (104 in state 3, many of them always-on achievements); a story level has none. Passing
  `DeliverCows1` this way on a client ran the pass message, the unlock that follows
  (`FindCowBells` 0 -> 2, `BL_Barn` 0 -> 1) and the reward effects, as on the host.
- **[obs] Unlocks (2026-10-21, `queries/171`-`172`, in game).** The unlock manager `[0x140b2fde0]` holds
  728 locks (catalog packs, hats, building parts, buildings `BL_*`, cars, Toy Box cards), pointers at
  `+0x10`, count in `[0x140b2c838]`; a lock's name `char*` is at `+0x10` (`0x1402aba50`), its state
  (i32) at `+0xa0`: 0 locked, `+0x9c` (1, or 5) once unlocked, 2 for sale, 5 bought.
  `0x1405f2480(lock, state)` sets it and posts `LockUnlocked` when it leaves 0; `0x140632cd0(mgr,
  name, state)` does that only for a locked one. A catalog purchase (`0x1400eddf0`) adds the price to
  stat `0xcc`, takes it off gold (`0x2f`), counts `0xcb`, posts `FirstSale` and sets the lock to 5.
  The table is level-scoped: Toy Box loads the Toy Box purchases (`LockUnlocked` x 12 at its load)
  and Train Rescue shows them at 0.
- **[obs] Building a bought building (2026-10-21, `queries/173`-`174`).** The lot selector's OK
  (`0x140114440`, `ScreenBuildingSelection::OnButtonOK`) posts `WorldEvent` to the lot actor with an
  event id from `0x140615440(&0x140b2ff60, name)`, which allocates a new id per call and stores the
  event's properties (`EventName=PurchaseBuilding`) under it, so the id differs between copies.
  `FrontEndShowScreen` (`0x1404c53e0([0x140b2ef10], name, -1)`) opens `Catalog2` (the Toy Store) from
  the loader, but it takes no input opened that way; the in-world trigger is in the level scripts.
  [unk] The trigger.
- **[obs] Lots and buildings (2026-10-21, `queries/183`-`188`, in game).** The lot table
  `[0x14099b878]` holds lot pointers at `+0x110`, count (i32) `+0x118`; a lot's reference name
  (`LOT_<site>`, `char*`) is at `+8`, its actor's id at `+0x10`, its building at `+0x38` (an entry of
  the building table, null while empty). The building table `[0x14099b880]` holds 21 kinds at
  `+0x48`, count `+0x50`: index (i32) `+0`, short name `+8` (`BarberShop`), key `+0x18`
  (`BL_BarberShop`), built (u8) `+0x70`. Toy Box on the test save has 22 lots, 6 built. The lot
  selector's OK (`0x140114440`) reads only its screen's vtable slot `+0x1e0` (close) and three fields:
  `+0x260` the building's key, `+0x268` the buying avatar (must be set), `+0x270` the lot actor's
  id. It allocates an event id with `EventName=PurchaseBuilding` and `BuildingName`, puts the
  building on the lot (`0x1401b3790`), sets built, posts `WorldEvent` to the lot and frees the id.
  Called with a stand-in screen it builds as a purchase does: barriers and a builder, then
  `BuildingContructionComplete` and the new townsperson's arrival, on whichever copy calls it.
- **[obs] The AI's tick (2026-10-21, `queries/180`, `189`-`196`, in game).** `AiBehaviorControl`
  (vtable `0x140804818`, the actor at `+8`): `+0x28` starts the behaviour tree named by the
  actor's `BehaviorTreeName`, `+0x30` stops it, `+0x38` tests it is running, `+0x80`
  (`0x140644700`) ticks it each frame (the tree at `+0x38`, a second runner at `+0x40`, both with
  the frame time `[0x14095483c]`), `+0x90` (`0x1406432b0`) only runs timers and the death frames.
  Skipping `+0x80` on a client for Train Rescue's UFO and Toy Box's stagecoach left both where the
  host's stream put them, and the level ran on in step. Telling an enemy from a friendly AI: the
  actors' `FriendlyFilter` sets (take-damage's friendly fire, `0x1404fcb50` and `0x140502bb0`) and
  `AiPerception`'s `AipIsEnemy` (`0x140662800`) both called Bullseye, Hamm and the UFO alike;
  [inf] neither is set up for the player in these levels. Bullseye and the UFO both use the plain
  `HealthTracker`.
- **[obs] Checkpoints (2026-10-21, `queries/171`, `175`-`177`, in game).** `CheckpointSystem`
  `[0x140b2bb78]`: up to 32 entries of 16 bytes from `+8` (interned name `char*`, started byte,
  finished byte), the current index (i32, 32 for none) at `+0x224`. `StartCheckpoint`'s worker
  `0x14056bb40(system, name, finish_previous, clear_finished)` marks one started and posts
  `CheckpointStarted`. Train Rescue: `General_Reset`, `FlatCar`, `Check Point Down Hill`, reached
  the same way on both copies while both ride. Respawn state 6 (`0x140645280`) sets `+0x1e`
  (waiting); a waiting player whose partner's `Respawn` has `+0x1e` set goes to 5 (both restart at
  the checkpoint, `StartingRespawn` with the player's id), and goes to 7 (respawn at the partner's
  point) only while the partner's `+0x20` (down) is clear: a player is held while its partner is
  down. The story pause menu has no restart entry.
- **[obs] The pause flag (`queries/178`).** `0x140b2c5a6` (u8) is set by the `Pause` handler
  (`0x1405d6360`) and cleared by `Unpause` (`0x1405f8d60`); the timer pause of the level start hold
  leaves it alone. A partner's mirrored START opened the pause menu on the other copy as well (the
  game's local co-op lets any player pause), and its B closed it.
- **[obs] Respawn with 3 or 4 players (2026-10-22, in game).** The two-player rule pairs slots 0 and
  1 only, so slot 1's stand-in now shows every remote partner: down while all are down, waiting
  while all wait. Two timing facts shaped it. A waiting player leaves the wait the frame it sees the
  others wait, often within 150 ms, and a copy whose frames stall (background copies here) missed it
  entirely, so the owner keeps sending its wait 1.5 s after a restart (state 5), and drops it at
  once when it respawns at a partner (state 7) instead; held longer, partners going down just after
  restarted without it. And the point the loader gives a player with no checkpoint stayed valid:
  every later down respawned at once where a partner had stood the first time, so it is cleared
  once the player is up.
- **[obs] Four copies on one PC (2026-10-22).** The copy in front runs at 60 fps, the others at about
  20 with stalls of 100 ms to 1 s, whichever copy hosts; the loader's work stays about 0.1-0.3 ms a
  frame. A background copy's level intro movie barely advances (minutes), and finishes within
  seconds once in front. A copy closed with `taskkill` keeps its UDP port and copy mutex for seconds
  to minutes while it exits.
- **[obs] Buzz's game crashed every copy after a partner's bind (2026-10-22, `queries/199`-`203`,
  in game).** A story level brings player 2 back as local co-op does (the partner's virtual pad
  counts as a second assigned player), and the bind used to spawn a new avatar and swap it in. In
  `st_buzzgame` both copies then faulted within 8 s in `0x1403fbfe0` (find child by name, null list)
  from the flight controller `0x1400cf480`, and after a null check there, in `0x140044261`: the
  swapped-out ship's controller still ticked, its actor no longer a player (`0x1405bcd10` -1), so
  `0x1405bccd0` gave no view and no SpaceFlight camera. Every view had the full camera set
  (`work/scratch/views.py`). Binding the partner to the level's own player-2 avatar when it has the
  owner's class (always, in a level that forces the class) removed the crash.
- **[obs] Buzz's rail flight (`queries/204`, in game).** `BuzzFlightController` places its ship every
  frame (update `0x1400431e0`, case at `0x140044202`): the point at distance `+0xac` on rail `+0x88`
  (`0x14052ef90` on the rails at `+0x68`), plus `+0x90` along the view's SpaceFlight camera right axis
  (`+0x1b4`) and `+0x8c` along its up axis (`+0x1c0`), plus a fixed height step for player 2
  (`[0x140b2ef50]+8 > 1`). Placing the actor is overwritten the next frame: a partner placed every
  frame still drifted to 2 km, as the owners respawned at different checkpoints. `+0xbc` is the
  controller's mode.
- **[obs] A story level resets the wallet counters while it loads (2026-10-22).** A client following
  the host from Toy Box saw gold 1080 -> 0 during the load, reported it as a spend of 1080 (and 95
  imagination points), and the host deducted it: both copies showed -1080 in the level.
- **[obs] A player back as another character (2026-10-23, in game, T34).** Two copies in Toy Box
  as Woody; copy 2 closed, started again and picked Jessie. It rejoined under the same player id,
  so the host's partner for that id was the parked Woody, and before the fix the host showed that
  Woody driven by Jessie's input. Setting the Woody aside under a spare key and binding a new
  avatar in the next free slot showed Jessie on the host within 4 s (`player 1 plays jessie but
  slot 1 holds woody; parking it`, then `bound: slot 2 ... class jessie`), with no crash; the old
  Woody stands idle until the level ends, as removing an avatar mid-level can crash (above). With
  no free slot (four players) the player keeps driving the avatar it has.
- **[obs] The character switch (2026-10-26, `queries/209`, in game, #65).** `LB` (control `0x20`)
  in the avatar input handler `0x14004ee90` calls `0x14027d4e0(mapper)`, then the swap
  `0x1400c7620(controller)` (`controller+8` is the avatar): it tells the avatar's `0x140b2a468`
  interface that it is left (`vt+0x90`), takes the next avatar from `0x1401db630` and gives it to the
  same player with `assign_avatar` (which posts `PlayerSwappedAvatars` with the old avatar index and
  the new id), then requests `SwapAvatar` on the new avatar's state machine unless `vt+0x18("ANY")`
  holds. `0x1401db630` walks avatar indices 0-3 (`mgr+0x10`, registry entries) from the current
  one and takes the first whose `vt+0x140` (`0x1401e2240`) passes: no player owns it (`+0x484` is
  -1) and `+0x478` is 0. Spawned avatars get higher indices (`0xb` seen), so the swap never takes
  one. A stand-in fed its owner's `LB` swapped on its own copy, where the free avatars differ, and
  the mod then set it aside and spawned again on every switch (seven avatars for one partner in the
  2026-10-23 playtest). The mod now follows an owner's class change with the swap's own steps onto
  a free level avatar of that class, or onto an avatar it spawned earlier and switched away from;
  only with neither does it set aside and spawn. Ten switches of a partner in Andy's House (host
  Woody): Jessie, Buzz and the one spare Woody in turn, no new avatar; on the client copy the host's
  stand-in followed four switches of the host the same way. The replayed `LB` itself never reached
  the stand-in's handler: the owner's swap changes its class in the same frame, so the stand-in is
  already in `SwapAvatar` when the press lands. The `LB` block is still hooked (`swap_pressed`) so a
  press without a class change cannot swap a stand-in [inf: not seen].
- **[obs] Enemy motions (2026-10-23, `queries/205`-`206`, in game, T35).** An actor's animation
  component `+0x360` holds its motion system at `+0x18`, and `0x140340b40(system, layer)` finds a
  layer; the by-name player `0x14056bda0(actor, name, start, flags, blend_ms)` stops layers 0 and 1
  together. A layer keeps the index of the motion it plays at `+0xc0` (`0x14033fa40`) and its
  animation set at `+0xb0`, whose name table `0x14033f170` looks names up in: buckets at `+0x70`,
  bucket count at `+0x80`, entries {next, hash, name, index} at `+0`, `+8`, `+0x10`, `+0x18`. The
  hash is `h * 33 + byte` over the name (signed bytes; `WALK` is `0x30d38f`). The by-name player
  plays through `0x140350ca0(layer, index, flags, start_s, blend_s, force)` and posts `StartMotion`.
  Haunted Bakery's Muffins ran `WALK`, `START_CHARGE`, `CHARGE`, `QUICKWINDUP`, `REACT_WOBBLE` and
  `RECOVER_1`-`3` on layer 0 and `LEAN` on layer 1, and died in `REACT_BLASTAWAY`; the cannons and
  cow barrels have no layers. Indexes matched on both copies ([inf] the same set loads), but only
  names are guaranteed. A client that skips the behaviour tree plays none of these; calling
  `0x140350ca0` with flags 0, start 0 and a 0.2 s blend on each change of the host's motion made a
  client's Muffins run the host's sequence, the death included.
