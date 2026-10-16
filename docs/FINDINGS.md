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
  while the shared engine code keeps its 3/4-player branches. Slots 2 and 3 would land on
  `+0x248` (the vertical flag) and beyond, past the end of the object, so the slot array has to
  be relocated, not just unbounded.
- **[unk]** Total object size and what follows `+0x258`; how many `< 2` checks are really
  `MAX_PLAYERS`; per-player arrays outside this object (HUD, cameras, control mappers, save data).

## 4. Input handles 4 pads

- **[obs]** `XInputInputDriver` (vftable `0x1407bccf8`, ctor `0x140279780(pad index)`, stored at
  `driver+0x2d0`). Device enumeration in `0x14027ea20` and `0x14027b500` loops pad indices
  `0..3` (`< 4`) and caps devices at 11 (`< 0xb`). Poll: `0x14027c000` (`XInputGetState`).
- **[obs]** `DAT_140ac5380` vtable `+0x88` is called with a player index when a player leaves
  (releases that player's controller, **[inf]**).
- **[unk]** How pads are bound to players 0/1 at join time (`ControlMapper` component, join-in screen).

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

## 6. Rendering and window

- **[obs]** `-dxresolution WxH` is parsed with `strtok` (`0x1402a5930`); the window is created
  with style `0xca0000` (caption, system menu, minimise; not resizable).
- **[unk]** Whether each viewport's camera uses its own viewport aspect ratio.

## 7. Existing patch on the live exe

- **[obs]** The live exe differs from the original in 7 bytes at file offset `0x2b6400`
  (VA `0x1402b7000`): `movss xmm0, [rip+const]` became `movaps xmm0, xmm3` + `nop`. It sits right
  after a `divss xmm3, xmm1` and before a `comiss`.
- **[inf]** Removes a clamp on a computed ratio (probably aspect ratio or FOV). Not made in this
  repo; purpose unverified.
