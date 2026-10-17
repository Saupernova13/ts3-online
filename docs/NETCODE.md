# Native online play (design)

Goal: each person runs their own copy of the game with one local player, full screen, no split
screen. One copy hosts; the others connect to it. In game, everyone sees everyone else and plays
in the same world. First target: two copies on one PC; the same code then works across a tailnet.
No screen streaming. The game has no networking code at all (FINDINGS §1, §8), so all of it lives
in our loader.

## Model: host session, owner-driven avatars

```
 copy A (host)                          copy B (client)
 +-------------------------+   UDP    +-------------------------+
 | slot 0: A's player      | <------> | slot 0: B's player      |
 | slot 1: proxy for B     |          | slot 1: proxy for A     |
 | world: authoritative    |          | world: follows the host |
 +-------------------------+          +-------------------------+
```

- **Each copy simulates its own player.** Slot 0 is the local player, driven by the local pad as in
  single player, with the normal full-screen camera.
- **Remote players are proxy avatars in slots 1-3.** The 4-player work (phases 3-5) already gives
  real player avatars in those slots, with animation, collision and co-op abilities. On each copy
  they are driven by network data instead of a local pad:
  1. the owner's pad input for that frame goes into the proxy's control mapper, so the proxy runs,
     jumps and attacks through the game's own `PlayerDriver` and animation;
  2. the owner's position, facing and velocity, sent 30-60 times a second, correct the proxy
     (blend when close, snap when far), so small differences never build up.
- **The host owns the world.** Enemies, pickups, doors, missions and level changes are decided on
  the host and sent to clients as events and corrections; a client that hits an enemy or grabs a
  pickup sends a request. This part grows system by system.
- **One view per copy.** The split-screen mode is forced to one viewport showing slot 0, and the
  2-player behaviours that assume a shared screen are turned off for networked sessions:
  shared-screen mode (`0x1405f2200`) and regrouping a player who strays too far (the placing
  handlers, FINDINGS 5e).

Why proxies in player slots rather than plain NPC puppets: a puppet needs its animation state set
directly, which means reverse engineering the animation graph. A player-slot proxy animates itself
from input, and the game already treats it as a player for co-op moves and interactions.

## What the exe needs (FINDINGS §9)

| Need | Finding | Plan |
|---|---|---|
| Two copies on one PC | a named mutex `OctaneApp` refuses a second instance (`0x14027e130`) | the loader gives the mutex a per-process name |
| Launch outside Steam's single launch | `SteamAPI_RestartAppIfNecessary(4049180)` relaunches through Steam (`0x1402839d0`) | the loader makes it return false; both copies then init the Steam API themselves |
| Keep running unfocused | the window handler does not pause on deactivation, it only releases the mouse clip (`0x1402b08f0`) | verify in game |
| Separate pads per copy | the game reads pads through `XInputGetState` (`XINPUT1_4` #2) | per-copy pad mapping in `ts3online.ini` (copy A reads pad 1, copy B pad 2); proxy slots read network state |
| Separate files per copy | both copies would share the save folder and `ts3online.log` | per-copy log name; a client never writes saves |
| Remote player avatars | the join flow can spawn and bind avatars for slots 1-3 (`join_request`, pick handler, `bind_joining_avatar`) | bind a proxy without a menu: spawn the class, assign the slot and a virtual pad device |
| Read and set positions | actor transforms: a set-rotation at `0x140568b50` (96 callers) used by the placing handler; position fields to confirm at runtime | read slot 0's transform every frame; write the proxy's |
| Single viewport | `SetSplitScreenMode` (`0x1405f4700`), the mode patch point from the quadrants work | force mode 0 in networked sessions |

## Network

- **Transport.** UDP in a loader thread. `ts3online.ini`: `net=host:27777` or
  `net=client:127.0.0.1:27777` (a tailnet address works the same way), plus `name=` and the
  character class.
- **Messages.**
  - Client to host: hello (name, class), per-frame player state (pad input, position, facing,
    velocity, slot-0 action flags), requests (hit, pickup, interact).
  - Host to clients: roster (who is in which slot), other players' states relayed, world events,
    level changes.
- **Timing.** Each copy runs at its own frame rate and pace. Remote states are buffered for about
  100 ms and interpolated; nothing waits on the network, so a slow link makes proxies lag, never
  the local game.

## Phases

| Phase | Work | Done when |
|---|---|---|
| H0 | Two copies on one PC: mutex and Steam relaunch bypass, per-copy pad mapping, per-copy log, no client saves | both copies run Toy Box side by side, each moved by its own pad |
| H1 | UDP host and client, hello and roster, heartbeat with round-trip time | the logs show both copies connected |
| H2 | Ghosts: each copy sends slot 0's transform; the other spawns a proxy in slot 1 and places it every frame | each copy shows the other player moving, in one viewport |
| H3 | Live proxies: drive the proxy from the owner's input, with position correction | proxies run, jump and attack like the owner |
| H4 | Networked-session rules: no split screen, no shared-screen mode, no regrouping; both copies follow the host's level | two players play a Toy Box session without either world breaking |
| H5 | Shared world, system by system: pickups and gold, enemies and NPCs owned by the host, missions, rides and vehicles | both players see the same enemies, pickups and mission state |
| H6 | Tailnet test between two PCs, then up to 4 players, joining and leaving mid-session | a 4-player session across PCs |

H0-H3 reuse what already exists (join and bind hooks, slots 1-3, the controls fix). H5 is the
long tail: every world system the players can touch needs its own sync rule.

## Risks

- **World consistency (H5).** Each copy simulates its own enemies and NPCs until a system is
  synced, so early builds will show different enemies on each screen. Toy Box free roam
  tolerates this; story missions do not.
- **2-player assumptions.** Logic that reacts to slots 1-3 (camera framing, regrouping, HUD,
  respawn) was written for one shared screen and must be found and gated for networked sessions.
- **Interactions between players.** Co-op moves (throwing, carrying) happen on two copies with
  slightly different positions; they need owner rules (whoever starts the move decides it).
- **Same build everywhere.** Every copy needs the identical game build; the hello message carries
  the build ID and a mismatch is refused.

## Alternative kept on file

Deterministic lockstep (every copy runs every player and only pad inputs are exchanged) would give
a perfectly shared world without per-system sync, but every copy has to simulate all players in
step, wait for the slowest PC, and share pause and menus. The evidence for it is in FINDINGS §8;
it stays an option for world sync if H5 turns out larger than expected.
