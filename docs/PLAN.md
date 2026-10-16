# Plan

**Goal:** up to 4 players online (a tailnet is enough), each player on their own full screen.

## Approach

Native netcode is out (see FINDINGS §1). Instead:

1. **4-player local first.** Raise the engine's compiled `MAX_PLAYERS` from 2 to 4. The renderer
   and the join logic already handle 3 and 4 viewports (FINDINGS §2); the player manager and
   the game layer do not (§3, §5).
2. **Online by streaming.** The host PC runs the game; each remote player gets one viewport
   streamed to them, and their gamepad comes back as a virtual XInput pad (XInput's 4-pad limit
   matches 4 players).

## Phases

| # | Phase | Output | Status |
|---|---|---|---|
| 0 | Feasibility RE | FINDINGS.md | done |
| 1 | Map every `MAX_PLAYERS` dependency | player manager size, slot accesses, bound sites (FINDINGS §3a) | manager layer done; review list + other per-player arrays open |
| 2 | Patch loader | Rust `cdylib` posing as `dinput8.dll` (the exe imports DINPUT8; Rust + MSVC target and VS 2022 Build Tools are installed), applying patches in memory by byte signature before the CRT static initializers run; the exe stays untouched | next |
| 3 | Relocate player slots + raise bounds | players 3/4 can be assigned; mode 3/4 split appears | done: manager relocated, 3-way split reached in game |
| 4 | Join flow, cameras, HUD, pad binding | P3/P4 join from their pads, own camera and HUD | in progress: join + pad binding work; slot-index fix awaiting a run |
| 5 | Game content | Toy Box first (open world, likely least scripted), then story missions | - |
| 6 | Streaming rig | see below | - |

## Phase 3 recipe (from FINDINGS §3a)

1. Allocate a ~0x320-byte buffer within +-2 GB of the module (RIP-relative reach).
2. Rewrite the RIP-relative operands in the 3 functions that address the object directly
   (static ctor, atexit dtor, setup) to point at the buffer at the same offsets.
3. Construct/destroy 4 slots instead of 2 (vector ctor/dtor count).
4. Move `+0x248` -> `+0x2f8` and `+0x250` -> `+0x300` at their 12 instruction sites.
5. Apply the 110 mechanical bound patches; hand-review the 39 + 13 others and the 9 `count == 2` tests.

## Phase 4 approach: replace functions, not bytes

Game logic with a hardcoded player index (join handler `0x1400eb6f0`, avatar bind `0x1400e2490`)
is reimplemented in Rust from the decompiled code, with the player index as a variable. The
loader overwrites the original entry with `mov rax, imm64; jmp rax`; the replacement calls back
into game functions through typed pointers at `module_base + rva`. Call sites that need a
different callee (e.g. the avatar registry lookup `0x1405bae40` in the pick handler) get their
rel32 retargeted to a stub allocated near the module.

Loader features this needs: `hook` (entry jump), `call_redirect` (rel32 retarget via near stub).

## Streaming options (phase 6)

| Option | How | Cost |
|---|---|---|
| A. Spanning window | Game renders a 2x2 grid at 3840x2160 across one real + three virtual 1920x1080 displays (Virtual Display Driver); one Sunshine/Apollo instance per virtual display; Moonlight on each remote PC over Tailscale | no rendering hook; relies on quadrant mode 4 and a window spanning displays |
| B. Window per viewport | Hook D3D12 so each viewport renders full size into its own window/swapchain; stream each window | cleaner, full resolution per player, weeks of hook work |

Audio is the host's single mix for everyone in both options.

## Risks

- `< 2` literals that are not `MAX_PLAYERS` (patching the wrong ones breaks unrelated logic).
- Per-player state outside the player manager (HUD screens, camera rigs, save/profile slots).
- Story missions scripted around exactly two avatars.
- Only three join-in characters (Woody, Buzz, Jessie): a 4th player needs duplicates or another avatar.
