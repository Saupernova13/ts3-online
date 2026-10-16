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
| 4 | Join flow, cameras, HUD, pad binding | P3/P4 join from their pads, own camera and HUD | mostly done: join, split, cameras, movement and level change work for 4 players; HUD for P3/P4 open |
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

## Native online play

Real netcode instead of streaming: each person runs their own copy with one local player and full
screen, one copy hosts, and remote players appear as proxy avatars in slots 1-3 driven by network
data. The host owns the world. Design and phases H0-H6 are in [NETCODE.md](NETCODE.md); the first
target is two copies on one PC. It reuses the 4-player work from phases 3-5. The streaming rig
below stays on file as the fallback.

## Streaming options (phase 6)

| Option | How | Cost |
|---|---|---|
| A. Spanning window | Game renders a 2x2 grid at 3840x2160 across one real + three virtual 1920x1080 displays (Virtual Display Driver); one Sunshine/Apollo instance per virtual display; Moonlight on each remote PC over Tailscale | no rendering hook; relies on quadrant mode 4 and a window spanning displays |
| B. Window per viewport | Hook D3D12 so each viewport renders full size into its own window/swapchain; stream each window | cleaner, full resolution per player, weeks of hook work |

Audio is the host's single mix for everyone in both options.

### Rig design (2026-10-16)

Host software: Apollo (a Sunshine fork, with the SudoMaker virtual display adapter), Tailscale,
ViGEm Bus and Moonlight. If Apollo is already used for everyday remote play, the rig runs separate
instances with their own config and ports and leaves that config alone. The host's own monitor is
assumed to be 1920x1080.

Game side (loader, opt-in through `ts3online.ini`):
- `enable = quadrants`: every split request builds 4 quadrants (patch `0x1405f4718`), so each
  player always has the same 16:9 quarter of the frame, whatever the player count. Without it, 2
  players get half-screens and player 3 gets a full-width strip (mode 3).
- `window = x,y,w,h`: borderless game window at that rect; with `-dxresolution 3840x2160` the
  quarters are 1920x1080 each.

| | A. Spanning | C. Crop mirror |
|---|---|---|
| Layout | the host monitor + 3 virtual 1920x1080 displays in a 2x2 grid; the game window spans them | game window on one 3840x2160 display; a mirror app shows quadrant k full screen on display k |
| Streams | one Apollo instance per virtual display (`output_name`, own port base) | same |
| Cost | no new code beyond the loader options | an extra GPU copy and about a frame of latency |
| Full-screen UI | menus, cutscenes and loading screens are split across the four players | the mirror shows the whole frame, scaled, when the game has one viewport or a full-screen menu (the loader can publish that state) |

Input: each Apollo instance creates a virtual X360 pad per client, and XInput order follows
connection order, so the connection order decides player numbers. The host's own pad is player 1.

Plan: prove the loop with A (two machines on the tailnet, two quadrants), then decide whether the
full-screen UI problem needs C.

Displays: Apollo creates one virtual display per stream, but with several instances only one
virtual display works at a time (Apollo issue #874, closed "not planned"; the maintainer's advice
is separate instances). So each rig instance must capture a persistent display picked with
`output_name`, with Apollo's per-stream virtual display off. That needs a driver that keeps N
virtual monitors (for example Virtual Display Driver, configured by XML). Installing it is a
deliberate host change, not something the scripts do on their own.

## Risks

- `< 2` literals that are not `MAX_PLAYERS` (patching the wrong ones breaks unrelated logic).
- Per-player state outside the player manager (HUD screens, camera rigs, save/profile slots).
- Story missions scripted around exactly two avatars.
- Only three join-in characters (Woody, Buzz, Jessie): a 4th player needs duplicates or another avatar.
