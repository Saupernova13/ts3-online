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

## Security

- **Trust model.** The host is authoritative and a client plays the host's save file, so only play
  with people you trust. Tailscale already encrypts and authenticates the traffic between tailnet
  devices. `net_key` (a shared password in `ts3online.ini`) gates who can join and authenticates
  every datagram, but it does not encrypt: payloads are readable by anyone who can see the link.
  Without a `net_key`, anyone who can reach the host's UDP port on the tailnet or LAN can join.
- **Tag.** Protocol version 8. Every datagram ends with a 16-byte tag, the first 16 bytes of
  HMAC-SHA256 over all bytes before it, keyed with `net_key` (empty when none is set, so
  passwordless sessions work). Checked in constant time before anything is parsed. `Probe`,
  `ProbeReply` and `Refuse` always use the empty key, so a host can be found and can tell a wrong
  key apart (`Refuse` reason 4; `ProbeReply.locked` says the host has a key). A bad tag is dropped,
  except a `Hello` with a bad tag, which is answered with that refusal.
- **Replays.** The tag does not stop a recorded datagram being sent again. Replays are bounded by
  the sequence numbers: states older than the newest seen and spends already applied are ignored.
- **Amplification.** A `Probe` is padded to be at least as long as the largest `ProbeReply`, so a
  spoofed probe cannot make a copy send more than it received.
- **Validation of peers' input.**
  - A `State` with a non-finite position, yaw or camera, or a position component beyond 1e6, is
    dropped (not stored, not relayed).
  - Level names must be empty or 1-64 bytes of `[A-Za-z0-9_]`: a client ignores an invalid `Level`,
    the host ignores an empty or invalid `LevelRequest`.
  - Control characters in player names become `?`, so a name cannot inject lines into the log.
  - The host rejects a spend below -1,000,000 (still acknowledged, never applied), on top of the
    existing rejection of positive deltas and unknown wallets.
  - The host answers a client's `SaveRequest` at most once per 200 ms.
  - World events: the event must be on the whitelist (`world::SYNCED`) and the reference name 1-96
    bytes of `[A-Za-z0-9_:/.-]`; both are checked on encode and decode, a report is accepted only for
    the host's current level, and a list holds at most 512 entries.

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
| H6 | Shared world state, system by system: gold and pickups, enemies and NPCs owned by the host, missions, shops, rides and vehicles; wallets (gold, imagination points) are built (H6a), and so are world events by reference name (H6b: crates breaking) | both players see the same enemies, pickups and mission state |
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

**Wallets (H6a).** The host owns the two HUD counters, `Stat_GOLD_COLLECTED` and
`Stat_IMAGINATION_POINTS_COLLECTED` (`wallet::WALLETS`). It sends `Wallet { values, acked }` to each
client when a value changes and once a second; the client writes the values into its stats table (the
HUD counter animates to them). Pickups are not sent: the partner avatar follows its owner, so both
worlds see the same pickups and the host's value wins. Spending (shops) happens only in the world
of the local player, so a client that sees a counter drop records a spend (seq, wallet index, delta)
and sends every unacknowledged one in `Spend { spends }` (about 10 Hz) until the host's `acked`
(the highest seq it applied from that client) covers it. The host applies each seq once through the
game's own stat add (`0x140632130`), so resends are harmless and a lost datagram only delays. While
a spend is unacknowledged the client shows the host value plus its pending spends, so the counter
does not flicker back. Only spending is forwarded; a host that spends needs no message.

**World events (H6b).** Actor ids differ between copies, so a one-off event on an actor is carried
by the actor's reference name (`actor+0x150`, unique and equal in both copies for most actors;
FINDINGS 10). Only events in `world::SYNCED` (now `ExploderDestroy`, a crate breaking) posted with
a=0 and b=0 are synced; more are added to that list once verified. `ExploderDestroy` is only the notice
of a break, so a replay breaks the actor's `MExploder` component itself (FINDINGS 10), and waits
until the local player has been in the level for 3 s (a replay during the load is lost). The targeted-post detour
(`0x14049e0b0`) resolves the actor, reads its reference name safely and queues `(event, name)` for
the game thread. The host keeps the authoritative list for its play level: `(seq, event, name)`,
seq from 1, no duplicates, cleared when its level changes. It sends each client
`World { level, entries }` (at most 8 per datagram) when something is new and every 200 ms while
unacknowledged; the client answers `WorldAck { level, seq }` with the highest contiguous seq (entries
are taken in order; a late joiner starts at 0 and receives everything). A client whose own game
produced a synced event sends `WorldReport { level, event, name }` every 200 ms until the entry shows
up in the host's list; the host accepts it only for its current level with a whitelisted event and a
valid name, applies it in its own game and lists it. Applying looks the actor up by reference name
(`0x140557f60`) and posts the event with `call_post_actor`, which bypasses the detour, so a replay is
never captured again. An actor that does not exist yet is retried each frame for 10 s, then given
up with one log line. Events the local game produced count as applied. Each copy forgets its applied
state when its level changes. Wire layouts (little-endian, after the common header): `World` = level
u32, count u8, then count x (seq u32, event len u8 + bytes, name len u8 + bytes); `WorldAck` =
level u32, seq u32; `WorldReport` = level u32, event len u8 + bytes, name len u8 + bytes.

`net_sim=delay,jitter,loss` adds artificial latency and loss for testing on one PC.

## Performance

Every copy logs two `perf:` lines every 30 s: the game's frame rate with the loader's own work per
frame (`coop::update`, timed in the frame hook), and the session thread's loop passes, datagrams
and bytes each way, and how long it held the session lock. The session thread receives and sends
outside that lock, so the game thread never waits on a socket call.

Measured 2026-10-19, two copies on one PC in Toy Box, host and one client:

| | Host | Client |
|---|---|---|
| Frames | 59.8/s (gap max 33 ms) | 60.0/s (gap max 19 ms) |
| Loader work per frame | avg 4 us, max 172 us | avg 5 us, max 157 us |
| Session thread | 186 passes/s | 185 passes/s |
| Datagrams | 59 in, 61 out per s | 62 in, 60 out per s |
| Bandwidth | 4.0 kB/s in, 4.1 kB/s out | 4.1 kB/s in, 4.1 kB/s out |
| Session lock held | avg 2 us, max 95 us | avg 2 us, max 96 us |

The loader's work is under 0.1 % of a 16.7 ms frame and its thread uses about 0.3 % of a core;
both games' own main threads run at a full core whether or not the loader is there. The session
thread's 5 ms sleep adds at most 5 ms before a state goes out or is read. Each player's state is a
stream of about 4 kB/s, and a host sends every client the other players' streams, so with N players
it uploads (N-1)^2 streams: about 36 kB/s for a full game of 4; a client uploads one.

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
