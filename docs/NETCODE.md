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

Principle: online play is the game's own local co-op with the partner's controller on the other end
of the network. Remote players are real co-op partners in slots 1-3 (pick up, throw, ride, swap),
never puppets with interactions turned off.

| Phase | Work | Done when |
|---|---|---|
| H0 | Two copies on one PC: mutex and Steam relaunch bypass, per-copy pad mapping, per-copy log | done (T7) |
| H1 | UDP host and client, hello and roster, heartbeat with round-trip time | done (T9) |
| H2 | Each copy publishes slot 0's transform; the others show an avatar placed every frame | done (T9) |
| H3 | Co-op partners: the owner's pad state is sent every frame and fed to a virtual XInput pad 1-3; the remote avatar is bound to a player slot through the join path, so it moves, animates and interacts through the game's own code; the owner's position corrects drift; the `online` patch keeps one full-screen view of slot 0 | partners run, jump, pick each other up and throw on both screens |
| H4 | Connect: `net=auto` finds a host among the online tailnet peers (`tailscale status`) or becomes the host; two hosts that meet merge; explicit `net=client:<tailnet name>:27777` still works | two PCs on one tailnet connect with the same ini and no addresses |
| H5 | One world: the host sends its save at connect and the client plays it from a session folder without saving back, as local P2 plays P1's save; the client follows the host into its level (`SwitchLevel`, `0x14012e800`) | done (T11, T12) |
| H6 | Shared world state, system by system: gold and pickups, enemies and NPCs owned by the host, missions, shops, rides and vehicles | both players see the same enemies, pickups and mission state |
| H7 | Release: install/uninstall script, sample ini, README for players, 2-PC tailnet test | a friend installs it and plays |

**Level follow (H5b).** The host sends `Level { name }` (its current level, empty outside one) to
every client when it changes, once a second, and on welcome. A client whose level hash differs from the
host's State level for 1.5 s, with both in a level and the host's name matching that hash, posts
`SwitchLevel` with the host's name; it does so at most once per 15 s since its last switch. When a
copy's own game posts `SwitchLevel`, a client sends `LevelRequest { name }` to the host, which follows
if it is in another level. Our own posts bypass the detour, so following never echoes. Menus
(`gui_*`, `frontend`) are not levels. A partner whose owner leaves or is in another level stays in
its slot, idle ("parked"), until the owner returns or the level ends: removing an avatar mid-level
can crash the game (FINDINGS 10). A newcomer takes over a parked avatar of a player who left.

`net_sim=delay,jitter,loss` adds artificial latency and loss for testing on one PC.

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
