# Native online play (design)

Goal: each person runs their own copy of the game with one local player, full screen, no split
screen. One copy hosts; the others connect to it. In game, everyone sees everyone else and plays
in the same world. First target: two copies on one PC; the same code then works across a tailnet.
No screen streaming. The game has no networking code at all (FINDINGS §1, §8), so all of it lives
in our loader.

## Direction: the host's world is the only world (long-term goal)

Playtesting (2026-10-19) confirmed what the model below implies: every copy runs its own world and
only the players are shared, so enemies, physics objects and NPCs differ between screens and
partners collide with things that are not there for their owner. That is not true multiplayer.
The long-term goal is host authority over the whole world: the host simulates everything, clients
stop simulating what the host owns and show the host's version (positions, animation, state), and
whatever a client does to the world (hits, pickups, interactions) is decided on the host.

Until then the owner-driven model stays, refined step by step toward that goal: each system that
can come from the host does (wallets, broken objects so far), and new work is judged by how much
of the world it moves under the host. Order of work:

1. Partner fidelity: the owner's moves (jumps, carries, throws) look the same on every screen.
2. NPCs and enemies: the host streams their transforms and state by reference name; a client
   stops their AI and movement and shows the host's.
3. Objects and pickups: breakables (done), collectables, doors, runtime spawns with network ids.
4. Interactions: a client's hits and pickups become requests the host decides.
5. Missions and progress follow the host.

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
     (blend when close, snap when far), so small differences never build up;
  3. while the owner's controls are off (a tutorial, a shop screen, a menu) its pad is flagged, and a
     copy passes it on only while the proxy's controls are off there too, so the same screen on both
     copies gets the same buttons and a menu of the owner's own leaves the proxy still.
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
- **Tag.** Since protocol version 7. Every datagram ends with a 16-byte tag, the first 16 bytes of
  HMAC-SHA256 over all bytes before it, keyed with `net_key` (empty when none is set, so
  passwordless sessions work). Checked in constant time before anything is parsed. `Probe`,
  `ProbeReply` and `Refuse` always use the empty key, so a host can be found and can tell a wrong
  key apart (`Refuse` reason 4; `ProbeReply.locked` says the host has a key). A bad tag is dropped,
  except a `Hello` or `Check` with a bad tag, which is answered with that refusal.
- **Join check.** Protocol 12. Before leaving the connect window, Join sends a `Probe` and a
  `Check { nonce }` tagged with the typed password; a host answers `CheckReply` with the nonce under
  its key, which proves the passwords match without taking a player slot. Both are 8-byte bodies,
  so neither amplifies.
- **Other versions.** A host answers a datagram of another protocol version with `Refuse` reason 3
  in its own version, but never answers a refusal (two hosts of different versions would otherwise
  refuse each other forever). Magic, version, type and reason sit at the same offsets in every
  version, so a client or connect window reads such a refusal as "the host runs version N".
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
  - `Npcs`: at most 48 entries, finite numbers, coordinates within 1e6, only for the client's current level.
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
| H6 | Shared world state, system by system: gold and pickups, enemies and NPCs owned by the host, missions, shops, rides and vehicles; wallets (gold, imagination points) are built (H6a), world events by reference name (H6b: crates breaking), NPC movement (H6c) and enemy health (H6d) | both players see the same enemies, pickups and mission state |
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

**Level start hold.** Story levels are scripted from their first frame (intro movie, then a
scripted opening with enemies), and copies load at different speeds: in testing the host loaded
Train Rescue in 3 s and the client, following from Toy Box, in 65-75 s, so the host's opening ran
and its idle player was shot down before the client arrived. A copy sets `flags::LOADING` in its
state from the moment it switches to a play level until the level has loaded: the game posted
`LevelLoadComplete` and, since the level's intro movie plays in that same update and blocks the
game loop (FINDINGS 10), the loader runs a frame after it with an avatar in slot 0 (or the avatar
has been up 60 s without that event). A copy whose level has loaded and that sees a partner still
on the way (loading, or in another play level it will follow from) pauses the way the pause menu
does, without the menu (FINDINGS 10): the game timer goes to speed 0, which stops gameplay and the
scripted sequences, and the sound pauses, while frames and the network keep running. It resumes
(speed 9, sound back) when no partner is on the way any more, or after 120 s. A partner in a menu does not
follow and is not waited for. A level is held at most once, when it has just loaded. A client
follows a loading host to the level it loads (the host's `Level` name), so both load at once,
and a partner whose owner is loading is parked rather than spawned at the old level's
coordinates. Two other ways to pause failed in testing: the raw clock override (FINDINGS 8) zeroes
the frame delta but the scripted opening kept running and shot the held player down, and a posted
`Pause` of kind 4 changed nothing.

**Respawn with a remote partner.** In two-player story mode a downed player respawns at the
checkpoint its partner last reached, and waits until there is one (FINDINGS 10). A remote
partner's stand-in is placed from network data and may never trigger a checkpoint on this copy, so
the local player could wait for the rest of the level. When the local player has waited 2 s for a
point that is not there, `respawn.rs` writes one: the checkpoint the local player itself last
reached, else where the partner stands. The game then respawns the player as it would have.

**Riding.** The owner sends `flags::RIDING` while its avatar is in the game's `Riding` state (a
summoned horse, or a story level's horse that runs by itself). A story level mounts each copy's own
player at its start, before partners are bound, so a partner whose owner rides gets a Bullseye of
its own, made the way the game's summon makes one (FINDINGS 10). That horse does not answer its
rider's stick, so while owner and partner ride, the horse and the partner are placed on the owner's
position and heading every frame (`coop::ride`). A partner is also bound with player 1's level
region and P2 start locator, as a local join is, so vehicle levels can auto-drive it.

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

**Pick-up targets.** Each state carries what its player would pick up or is carrying: nothing, a
player (by id), a uniquely named actor (the `level_hash` of its reference name) or something else
(a runtime spawn no other copy can name). The receiving copy points the partner avatar's
`PlayerPerception` candidate at the same player or named actor, or at nothing, every frame while the
partner is only looking (FINDINGS 10). A replayed X press then picks up the owner's target, and
never a different object that only stands near the partner on this copy.

**Thrown objects.** The thrower's copy owns what its player throws. `StartThrow` (thrower, thrown
actor) is seen by the targeted-post detour on every copy: on the thrower's copy the object gets a
seq and, for 6 s or until it despawns, its rigid-body position and velocity ride in the player's
`State.proj`. On the other copies the partner's replayed throw spawns their own object; the first
report of a new seq is paired with the partner's latest throw there (within 1.5 s), and from then
on that object's rigid body follows the owner's every frame. The report is carried forward by the
state's age (half the round trip per hop, a client seeing another client through the host, plus the
time since it arrived; at most 200 ms) to where the owner's object is now, and the body gets the
owner's velocity plus whatever closes the gap in 0.1 s, so the game's collisions still stop it; it
is placed outright only when more than 3 m off. A body is only written when its origin is within
2 m of the actor's own copy of it. Thrown players are left out (their owner already drives them).

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
are taken in order; a late joiner starts at 0 and receives everything). The two sides re-converge
after either restarts: a client entering a level acks its position (0), and a lower ack rewinds the
host's send cursor for that client, as does a repeated `Hello` from a known address (the client lost
the host and reconnected); a change of the host's `Level` name tells a client the host's list
started over, so it takes the list from seq 1 again (the game-thread applier skips entries it
already applied). A client whose own game
produced a synced event sends `WorldReport { level, event, name }` every 200 ms until the entry shows
up in the host's list; the host accepts it only for its current level with a whitelisted event and a
valid name, applies it in its own game and lists it. Applying looks the actor up by reference name
(`0x140557f60`) and posts the event with `call_post_actor`, which bypasses the detour, so a replay is
never captured again. An actor that does not exist yet is retried each frame for 10 s, then given
up with one log line. Events the local game produced count as applied. Each copy forgets its applied
state when its level changes. Wire layouts (little-endian, after the common header): `World` = level
u32, count u8, then count x (seq u32, event len u8 + bytes, name len u8 + bytes); `WorldAck` =
level u32, seq u32; `WorldReport` = level u32, event len u8 + bytes, name len u8 + bytes.

**NPCs (H6c).** First step of direction item 2: the host owns the movement of named actors. Townspeople
and other movers have the same reference names on every copy (FINDINGS 10), so the host sends
`Npcs { level, entries }` to every client, unreliable (latest wins, no ack), entries being (hash of
the reference name u32, position 3 x f32, yaw f32), at most 48 per datagram (20 bytes each, under
1000 bytes); a host with more movers sends several datagrams. Decode refuses a count above 48,
non-finite floats and coordinates beyond 1e6. The host samples all uniquely named actors at 15 Hz
(`npc::Tracker`; names that repeat on the host and the four player avatars are skipped). An actor
becomes a mover once it is more than 5 cm from where it was first seen in the level. A mover is sent
when it moved more than 2 cm or turned more than 2 degrees since it was last sent, and in any case
once a second even when still, so a late joiner and a lost datagram converge. The ones that moved
most go first; at most 480 per sample, the rest wait for the next. The tracker resets on a level
change and forgets actors that disappear. A client keeps the latest transform per hash for its own
level (messages for another level are dropped) and every frame writes it to the actor the hash
resolves to (`target::named`, rebuilt at most once a second): a step of 0.3 toward the host's
position and yaw, or the host's position itself beyond 3 m. Skipped: the player avatars (local and
partners') and actors being carried (`ActorCarried+0x18` not 0). The client's own AI still runs
underneath and is overridden each frame; stopping it is a later step. Wire layout (after the common
header): `Npcs` = level u32, count u8, then count x (hash u32, x f32, y f32, z f32, yaw f32).

**Enemy health (H6d).** Second step of direction item 2: the host decides how much health every
enemy has. Every hit goes through one function, the health interface's take-damage (`0x140441f80`,
FINDINGS 10), which the loader detours. On every copy in a networked session, a hit on a partner's
stand-in is dropped (its owner's copy decides that player's health), and so is a hit dealt by a
partner's stand-in to anything else (its owner's copy reports it). A client's own player hitting an
enemy is applied as usual and the health it took is added to a running total per enemy, which the
client sends to the host every 200 ms for 3 s after the last change (`Hits`, unreliable, repeated);
the host applies whatever part of each total it has not applied yet, with the client's stand-in as
the source and the raw damage kind (3: no multipliers or invulnerability), so a kill on the host
runs the game's own death. Only actors with the plain `HealthTracker` count as enemies; avatars and the level's companions (Train Rescue's `Jessie2`) use `TS3HealthTracker` and are left to each copy. The host watches every enemy a player has hit this level (a hit from a player's avatar, or a client's report), not actors only the level damages, and sends each
one's health when it changes and once a second (`Healths`, unreliable), a dead one every 250 ms for
5 s even after the game removes it. A client follows each figure: higher local health takes the
difference as raw damage, with the host's stand-in as the source (a kill plays the death), and lower
local health is raised to the host's unless the client's own hits on that enemy are still being
reported; a figure already applied to an enemy is applied again only after 1 s, since some actors refuse damage or come straight back. Enemies are named by a `Key`: the hash of the reference name when that is unique on the
sender, else the hash of the actor name and the position, matched to the nearest actor with that
name within 4 m. Wire layouts (after the common header): `Healths` = level u32, count u8 (at most
40), then count x (key, health f32); `Hits` = level u32, count u8 (at most 34), then count x (the
client's actor id u32, key, running total f32); key = reference-name hash u32, actor-name hash u32,
position 3 x f32. Decode refuses larger counts, non-finite numbers, health beyond 1e6 and positions
beyond 1e6. Each copy's enemy AI still runs, but only the host's counts against players: a client
drops a hit on its own player whose source is an enemy (an actor with a plain `HealthTracker`), and
the host forwards each such hit on a client's stand-in to that client (`Hurt`: level u32, count u8,
at most 32 x (seq u32, amount f32, damage kind u8), resent every 100 ms until `HurtAck`: level u32,
highest seq applied u32), which applies it through the game's damage with no source. Hits with no
enemy source (an obstacle's hit a horse passes on, falls) stay with each copy: the stand-in's
summoned horse barely meets Train Rescue's obstacles on the host.

**Missions and unlocks (H6e).** The game keeps both as named tables for the session (FINDINGS 10):
291 challenges in Toy Box and 728 locks. The host reads both every 250 ms and pages them to clients
(`Progress`: level u32, kind u8 (0 challenge, 1 lock), page u8, pages u8, count u8, then up to 200 x
(name hash u32, state u8)): a page goes out when its content changes, and one page in turn every
250 ms. A client takes the host's state where it is behind, once the difference has lasted 3 s
(both copies usually make the same change themselves, and a forced pass on top of the game's own
runs the pass twice), through the game's own setters (`0x1406362f0` for challenges, `0x1405f2480`
for locks), and retries every 2 s. Challenges only move forward to 2, 3 or 4; locks never go back to
0. A client's own changes are reported (`ProgressReport`: level u32, count u8, up to 100 x (kind u8,
hash u32, state u8)) until the host shows them, at most 10 s, and the host takes them the same way.
Both messages carry the level and everything resets on a level change: the lock table is
level-scoped, and copies change level at different times. Toy Box's lots are a third table
(kind 2): a lot's state is its building's index + 1, 0 while empty, and a copy only builds on an
empty lot, by calling the lot selector's OK with a stand-in screen and its own player as the buyer
(FINDINGS 10), so each copy runs its own construction. The `WorldEvent` the selector posts is not
replayed: its id is allocated per call.

**Enemy AI.** On an online client, the behaviour tree of each actor with a plain `HealthTracker`
(Bullseye excepted: a horse answers to its rider on the rider's copy) does not tick
(`AiBehaviorControl+0x80`, FINDINGS 10). Such an enemy only moves where the host's NPC stream
puts it, and its hits on players come from the host (above). The role is noted once per frame, so
the tick does not take the session lock; it lapses after 1 s without a note, and enemies think
again. `enemyai on` in a command file turns it off.

**Shared pause.** An owner sends `flags::PAUSED` while its game's own pause is on (`0x140b2c5a6`);
other copies in the same level stop their game timer the way the level start hold does, without a
menu, until no partner is paused, and pause again if their own menu closes meanwhile. START and BACK
never reach a stand-in, so menus open only on their owner's screen.

**Down together.** An owner sends `flags::DOWN` and `flags::WAITING` (its `Respawn` down flag, state
6). Each copy writes them to the slot-1 stand-in's `Respawn` (`+0x20`, `+0x1e`), the wait held for
1.5 s after it ends, so the game's own rule applies on every copy: a downed player is held while its
partner is down and both restart at the checkpoint once both wait; one player down alone respawns
at its partner as before.

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
