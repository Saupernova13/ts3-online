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
- **[unk]** Whether P3/P4 move in game with the fix (needs a test window).

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
  Start. Not run in game yet.
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
