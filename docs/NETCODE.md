# Native online play (design)

Goal: real online co-op for up to 4 players, each on their own PC with their own copy of the game
and a full screen of their own. No screen streaming. The game has no networking code at all
(FINDINGS §1, §8), so the netcode lives in our loader.

## Options

| | A. Deterministic lockstep | B. Authoritative host, replicated state | C. Loose sync |
|---|---|---|---|
| What crosses the network | each player's pad state, once per frame | positions, animation, AI, physics, pickups, scripts, UI state of every relevant object | player avatars (transform + animation) and a list of key events |
| What each PC runs | the whole game, identically | the host simulates; clients draw what they are told | its own copy of the world; other players are puppets |
| Work | make the simulation repeatable, plus a small input layer | serialise and apply state for every gameplay system: years for a game this size | per-system event sync; worlds drift apart, and enemies, carried objects and missions disagree |
| Fits this project because | 4-player local play already works, so only the pad inputs differ between PCs | - | fallback for free roam (Toy Box) if A cannot be made repeatable |

Rollback netcode (GGPO-style) would need to save and restore the whole game state every frame;
the game has no save states and a heap of 1 GB, so it is out. Delay-based lockstep needs none of
that, at the cost of a few frames of input delay.

**Recommendation: A.** Every PC runs the same 4-player game from the same save, and the loader
feeds all four players' pads to every PC on the same frame. If the simulation is repeatable, the
PCs never diverge and no game state ever has to be sent.

## Why A is plausible here (evidence, FINDINGS §8)

| Source of divergence | What the exe does | Plan |
|---|---|---|
| Wall-clock time | one game clock (`0x14025a5e0`, `0x14025a470`) on `QueryPerformanceCounter`, with a dormant override (`0x1409c27b9` + `0x1409c2918`) | turn the override on and advance it by a fixed step per simulated frame: fixed delta time, frame-locked timers |
| Random numbers | CRT `rand` from 6 call sites, never seeded; an LCG in `0x14070b840`; engine random nodes | hook `rand` with one session PRNG seeded by the host (no per-thread state); trace the other generators |
| Threads and async work | a job system (2 thread and 2 fiber creators), async actor creation, async choreography, deferred pathfinding, world streaming | find which results gameplay consumes and make them land on a fixed frame (wait for completion at a frame boundary) |
| Physics | Bullet | repeatable with a fixed step and a fixed object order |
| Rendering | each PC would draw a different view | every PC draws all 4 views, exactly as local play does, so any culling or LOD-driven logic matches; each PC then shows only its own quarter, scaled to full screen |
| Loading | load and stream times differ per PC | no PC simulates past a barrier until every PC has finished loading |
| Audio and video | FMOD timeline reads; Bink cutscenes run on real time | pause the simulation during cutscenes and resume on a barrier; check what the timeline reads feed |
| Floating point | same exe, x64 SSE on every PC | expected to match; the CRT's CPU-specific math paths are a risk to measure |
| Menus, pause, joining | all driven by pad input | already repeatable once input is |

## Architecture

```
 PC 1 (host)                 PC 2..4
 +--------------+  inputs   +--------------+
 | game (4P)    | <-------> | game (4P)    |   same exe, same save, same level
 | loader:      |    UDP    | loader:      |
 |  clock lock  |           |  clock lock  |
 |  XInput hook |           |  XInput hook |
 |  frame gate  |           |  frame gate  |
 +--------------+           +--------------+
```

- **Input layer.** The game reads pads through `XInputGetState` (`XINPUT1_4` ordinal 2 in its
  import table). The loader replaces that import: on every PC, pad *k* reports player *k*'s state
  for the current frame, taken from the local pad on player *k*'s own PC and from the network on
  the others. The game's own join, menu and control code stays untouched.
- **Frame gate.** Once per frame (the input dispatch `0x140280b90` runs once per frame), the loader
  sends the local pad state for frame *N + D* and waits until it has every player's state for frame
  *N*. *D* is the input delay in frames: about 1-2 on a LAN or tailnet, 3-4 across a region.
- **Clock lock.** The clock override advances by exactly one frame per gated frame, so delta time,
  timers and animation steps are identical everywhere, and a slow PC slows everyone instead of
  diverging.
- **Desync check.** Every N frames each PC hashes a state digest (avatar positions, gold, slot
  states, RNG state) and sends it with its input. A mismatch stops the session and logs the frame,
  first for debugging, later for an automatic resync (everyone reloads the level from the host's
  save).
- **Session.** The host picks the save and the level; clients receive the save file, all load it,
  and play starts on a barrier. Joining mid-session is out of scope at first.
- **Transport.** UDP between tailnet addresses first (Tailscale handles NAT). Later, Steam
  networking through the game's own `steam_api64.dll` (`SteamInternal_FindOrCreateUserInterface` is
  already imported), which would add NAT traversal and friend invites without Tailscale.
- **Own screen.** Version 1 can show the familiar 4-way split on every PC. Version 2 presents only
  the local player's quarter, scaled to full screen, with the game still drawing all four (see the
  rendering row above).

## Phases

| Phase | Work | Done when |
|---|---|---|
| N0 | Replay harness in the loader: clock override on, record every pad state per frame, replay it from the same save, hash state every frame | two runs of 5 minutes in Toy Box on one PC match frame for frame, or the first divergence is found |
| N1 | Fix each divergence N0 finds (RNG hook, async completion points, streaming and cutscene barriers) | replays match, including level loads and a cutscene |
| N2 | Cross-PC replay: record on one PC, replay on another | replays match across two different CPUs |
| N3 | UDP transport and the frame gate, 2 PCs, 4-way split on both | two players finish a Toy Box session over the tailnet without a desync |
| N4 | 4 PCs; save hand-off and load barrier; own-quarter presentation; desync resync | four players, each full screen |
| N5 | Input-delay tuning, disconnect handling (a dropped player's avatar leaves), HUD for players 3/4, Steam transport | playable by people other than the author |

N0 and N1 decide everything: if the game cannot be made repeatable, option C is the fallback for
free roam.

## Risks

- Unknown nondeterminism in systems not yet read (AI, scripts, particles that touch gameplay).
- A slow PC slows all four (lockstep runs at the slowest PC's speed).
- Story levels with scripted two-avatar sequences are untested even locally.
- Every PC needs the identical game build; a Steam update on one PC breaks the session (the loader
  already refuses to patch an unknown build, and the session can compare build IDs at connect).
