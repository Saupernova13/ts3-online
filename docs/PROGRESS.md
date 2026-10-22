# Progress log

Oldest first. One entry per working session: what was learned, what changed, what is next.

## 2026-10-16

- Located the install: `steamapps\common\Toy Story 3 Complete Edition` in a Steam library
  (Steam app 4049180). The live exe already carries a 7-byte patch; `.exe.orig` is the original
  and is what Ghidra analysed (FINDINGS §7).
- REA's own Ghidra provider is not configured on this machine (`GHIDRA_INSTALL_DIR` unset, JDK 17
  default), so analysis runs through `tools/ghidra.sh` with Ghidra 12.1.3 and JDK 21.
- Ruled out native networking (FINDINGS §1).
- Found that the renderer and join logic already handle 3 and 4 viewports and that the player
  manager is the 2-player bottleneck (FINDINGS §2-§4).
- Input enumerates 4 XInput pads already.
- Set up this repo; queries 01-05 in `queries/` reproduce the evidence.

- Phase 1, player-manager layer: dumped 510 manager-touching functions (`PlayerMgrDump.java`),
  mapped offsets (`tools/scan_player_mgr.py`), measured the object (~0x258 bytes, static, 3
  functions address it absolutely), and listed instruction-level bound sites
  (`SlotBounds.java` -> `tools/classify_bounds.py`): 110 mechanical patches, 39 to review, 13
  unguarded accesses, 12 tail-field sites. FINDINGS §3a has the details; PLAN.md has the
  phase 3 recipe.

- Phase 2, loader built (`loader/`, Rust `dinput8.dll` proxy): verifies the 188 patches against the
  exe bytes, relocates the player manager into a buffer within +/-2 GiB of the module, applies
  the set, and runs a watcher thread logging manager fields to `ts3online.log`. `cargo test`
  passes; not yet run inside the game.

- Phase 4 start: loader gained `hook` patches (`entry`, `entry_caller` via an R8 thunk, `call`)
  and Rust hooks for the join bind, join cancel, join failure release and avatar-by-class
  lookup (`hooks.rs`, `game.rs`); gen_patches.py emits them from `data/patches-manual.json` and
  the watch list now includes the input driver and the player-object count. Unit-tested
  encodings only; not run in the game.

- Join handler `0x1400eb6f0` is now the Rust hook `join_request` (first free player index for
  assign, screen owner and release); the `release_joining` call hook and the three join byte
  patches are gone, and a 1 GB "System" heap patch was added. Not run in the game yet.

- In-game runs (virtual pads, Toy Box): loader boots and exits cleanly (T1); 2-player drop-in
  through the Rust hooks matches vanilla (T2). Three players: P3 joins with its own pad, binds
  to slot 2, and the engine switches to the 3-way split, then dies in a runaway loop in slot init.
  Root cause: slots 2/3 keep player index -1 (FINDINGS 5b). Fixed in `bind_joining_avatar`; not
  re-run yet.
- Automated testing paused: virtual-pad presses reached the desktop when the game lost
  focus. Further in-game runs wait for an agreed test window.

- Supervised test run: the slot-index fix works. 3 and 4 players join, with 3-way and 4-way
  split, no crash, memory stable. Then: 4 per-player views plus per-player setup for slots 2/3
  give viewports 3/4 working follow cameras (FINDINGS 5c). The controls-refresh hook re-enables
  slots 2/3 (5d), but players 3/4 still do not move. Evidence: `work/evidence/` (screenshots + logs).
- Test tooling: `tools/focus.ps1` (bring the game to front), `tools/padsend.sh` (sends pad
  commands only while the game is foreground), `tools/shot.ps1` prints `foreground=`.

- Static trace of P3/P4 input (FINDINGS 5d): pad -> mapper -> `PlayerDriver` has no player
  limit. The real bug was ours: `refresh_player_controls` called the avatar's controls setter
  without its `enabled` argument, so P3/P4 stayed controls-disabled from the join menu. Fixed.
- New `IndexChecks.java` sweep of player-index compares: four more `idx <= 1` guards raised
  (FINDINGS 5e). Level teardown reset slots 0/1 only (stale P3/P4 avatars on level change): new
  `reset_player_slots` call hook resets slots 1..3 and releases the P3/P4 pads. 219 patches.
  `tools/pe_info.py qwords` reads vtables without Ghidra.
- Streaming groundwork: rig design in PLAN (Apollo instances on persistent virtual displays;
  Apollo's per-stream virtual display only works for one instance). Loader options, off by
  default: `enable=quadrants` (optional patch groups) and `window=x,y,w,h` (borderless placement).
  Full-screen moments (shared screen, a player leaving) still use one viewport.
- Deployed (220 patches with the optional group); not run in the game yet.

## 2026-10-17

- Supervised test run: all four players move (T3, T4); exiting Toy Box with 4 players and
  re-entering works, and pads 2-4 re-join and move (T6). The game's own regrouping of players
  ran without trouble. Evidence: `work/evidence/2026-10-17-*`. Not tested: `enable=quadrants` and
  `window=` (they need a `ts3online.ini` in the game folder).

- Documentation pass: `docs/PATCHES.md` lists all 220 patches and 7 hooks with group, status and
  notes; `tools/gen_patches.py` regenerates it with the patch set. README status and tool list
  brought up to date.

- Native online design (`docs/NETCODE.md`): one player per copy, full screen, one copy hosts and
  the others connect; remote players are proxy avatars in slots 1-3, driven by the owner's input
  and corrected by the owner's position; the host owns the world. A lockstep variant was drafted
  first and kept as an alternative; its determinism survey is FINDINGS §8.
- Side-by-side copies (FINDINGS §9): the game blocks a second instance with the `OctaneApp` mutex
  and relaunches through Steam when started directly; both can be bypassed in the loader. It
  does not pause on focus loss.

- H0 built and run: optional `instances` group (single-instance checks and Steam relaunch), copy
  numbers through named mutexes, `[copyN]` config sections with a `%LOCALAPPDATA%` fallback, per-copy
  logs, `pad=N` through an XInput import hook, per-copy saves through an `SHGetFolderPathA` hook,
  `tools/launch_copy.ps1`. In game: two copies run side by side with isolated pads and saves (T7).
  Gap: an unfocused copy ignores its pad.

**Next:** find the input focus gate so both local copies can be played at once, then H1 (connect)
and H2 (see each other move).

## 2026-10-19

- Ham crash (FINDINGS 5f): talking to a Toy Box character crashed in `CameraGroup::SetState`
  because views 2/3 were created without camera setup and the conversation camera pushes a state on
  every view. New `level_player_setup` hook sets up views 1..3 at level load; the join hook no longer
  does. Not run in the game yet (T8).
- H1 (`loader/src/net/`): UDP host/client session thread, `net=host:PORT` / `net=client:ADDR:PORT`
  and `name=` in `ts3online.ini`. Hello/Welcome/Refuse (full, build or protocol mismatch), roster,
  ping/pong RTT, 5 s timeouts with client reconnect, and per-player `State` relayed by the host.
  Tested over loopback in `cargo test` (host + 2 clients); not run in the game.
- H2 (`loader/src/proxy.rs`): a frame hook (`0x140280c18`) publishes slot 0's position and yaw and
  spawns one proxy avatar per remote player (sync spawner, a class different from the local one,
  controls off), placed every frame with the placing handler's sequence; removed when the player
  leaves, forgotten on level change. Spawn and rotation details: FINDINGS 9. Not run in the game
  yet (T9). 225 patches.

- T9 passed (recorded with OBS, `tools/obsctl.py`): host and client copies side by side. The client
  connected as player 1 within 10 ms of starting, round trip about 5 ms. In Toy Box each copy spawned
  a proxy of the other player (host Woody saw a Buzz proxy, client Buzz saw a Woody proxy), and each
  proxy followed its owner's moves in both directions. The client saved, exited to the board and
  re-entered: no crash in either copy, and the client respawned the host's proxy. Evidence:
  `work/evidence/2026-10-19-t9-*`.
- Seen in T9: the host shows the "X Pick up" prompt next to the Buzz proxy, so the game treats a
  proxy as a co-op partner that can be picked up and thrown; proxies slide in their idle pose.

- H3 (T10 passed, OBS recording): remote players are real co-op partners. Their owner's pad goes to a
  virtual XInput pad 1-3, the partner is bound to a player slot through the join path, and the
  `online` patch keeps one full-screen view. Found on the way: stick input is camera-relative (fixed
  with closed-loop steering in the partner view's camera frame, `view+0x20` matrix), and the game
  stops updating every controller while its window is unfocused (DirectInput keyboard read fails ->
  early return; patched). Action presses first match the partner's pose to the owner's, after which
  carry and throw replicate in both directions.

- H4/H5a (T11 passed, OBS recording): `net=auto` hosts or joins with no addresses (tailnet peers from
  `tailscale status --json`, or `net_peers=`; a taken port means a host on this PC; lone hosts merge).
  A client downloads the host's save at connect (CRC-checked) and plays it from
  `%LOCALAPPDATA%\ts3online\session`, never writing to its own saves. The save-folder hook waits up
  to 8 s at startup for that decision. The host's first run triggers a Windows Firewall prompt.

- Event tools (PR #8): detour hooks on the three event-post functions, `log_events=1` logs every game
  event, `dev_cmds=1` runs `event`/`post`/`log` lines from `cmd.copyN.txt`. Posting `SwitchLevel` with
  a level name reloads that copy in about 5 s.

- H5b (T12 passed, OBS recording `2026-10-19 15-07-49`): the host sends its level name; a client in
  another level for 1.5 s follows it, and a client's own level switch makes the host follow. Found on
  the way: removing a partner avatar mid-level crashed `wt_stuntpark` a few seconds later (FINDINGS
  10), so partners whose owner left or changed level now stay parked, idle, until the owner returns or
  the level ends; the level teardown frees them. Menus (`gui_*`) are not followed.
- Release prep: `tools/install.ps1` (find the game through Steam, copy the DLL, write a `net=auto` ini,
  optional firewall rule, uninstall), `docs/PLAYING.md`, and a CI workflow that tests, builds and
  attaches `ts3-online.zip` to `v*` tags. Keyboard input is not replicated yet (issue #9).

- H6a (T13 passed, OBS recording `2026-10-19 15-56-52`): shared wallets. `Stat_GOLD_COLLECTED` is the
  gold counter (writing it animates the HUD) and `Stat_IMAGINATION_POINTS_COLLECTED` the second one. The
  host's values win; a client's drops are sent as sequenced spends until acknowledged, and the host only
  acknowledges a spend with a value that includes it (a first build sent the ack early and the client's
  counter flicked back for a frame).

**Next:** world sync beyond wallets (H6): enemies, missions, shops and the purchases that change the
town.

- HUD prompts: `HudControllerLayout` adds a player's prompts only for players below the view count,
  so with one view the partner's prompts never show; the "X Pick up" next to the partner is the
  local player's own (FINDINGS 10). No change needed.

- 2026-10-19, netcode hardening (protocol 7): optional `net_key` password; every datagram ends with a
  16-byte HMAC-SHA256 tag (probes and refusals use the empty key; a wrong key gets `Refuse` reason 4),
  probes padded against amplification, and peers' input validated (finite in-range states, level name
  charset, control characters in names, spend cap, save serving limited to once per 200 ms).
  `install.ps1 -Password`. See NETCODE.md "Security".

- 2026-10-19, connect menu: `net=menu` opens a small Win32 window at game start (own thread, own message
  loop) that probes `127.0.0.1`, the LAN broadcast, `net_peers` and the online tailnet peers once a second,
  lists the hosts of our build (`name  players/4  ip:port`, `[password]`, `[full]`) and lets the player join,
  type an address or host. Nothing times out while it is open (`menu_open` in the net state: `wait_decision`
  starts its clock only after it closes). Name, password and the joined address (`menu_last`) are written
  back to the ini. `install.ps1` now writes `net=menu`. Logic in `menu_model.rs` (tested), window in `menu.rs`
  (exercised in game with two copies: the joining copy listed the host, joined, downloaded the save
  and bound its partner in Toy Box; `window=` placement now skips the menu window).

- 2026-10-19, H6b (protocol 8): world events by reference name. Actor ids differ between copies but the
  reference name (`actor+0x150`) is unique and equal for most placed actors (756 of 822 in Toy Box).
  `ExploderDestroy` turned out to be the notice after a break; the break is `0x140652ac0` on the
  actor's `MExploder` component (queries 119-120), which a replay now calls. The
  targeted-post detour now queues whitelisted events (`world::SYNCED`) with the actor's reference name;
  the host keeps a per-level list that clients acknowledge (`World`, `WorldAck`), clients report their
  own events (`WorldReport`), and each copy replays entries by reference name (`0x140557f60`), 3 s
  after its player appeared, retrying for 10 s. In game (two copies, T14): a crate and a barrel
  broken on the host were listed, and a client that joined later broke both on arrival. See
  NETCODE.md "World events".

- 2026-10-19, performance: `perf:` log lines every 30 s (frame rate, loader work per frame, session
  thread passes, datagrams, bytes and lock time); socket calls moved out of the session lock. In
  game: 60 frames/s, loader work avg 4-5 us per frame, 4 kB/s each way at 60 datagrams/s, lock held
  avg 2 us. Nothing needed optimizing; numbers in NETCODE.md "Performance".

- 2026-10-19, pick-up targets (protocol 9): playtesting showed partners picking up different things
  than their owners (a townsperson standing next to the partner on one copy only). `State` now
  carries the owner's `PlayerPerception` candidate as a player id, a reference-name hash or "other",
  and the receiving copy writes the partner's candidate to match. In game: Woody picking up Buzz
  showed `CARRY_START` on both copies with the right target (also at a 110 degree pose error), and
  the throw landed within 0.1 m. Jumps no longer snap the partner (PR 21). Townspeople turned out to
  share reference names across copies but walk differently, the starting point for NPC sync.

- 2026-10-19, NPC sync (protocol 10): the host streams the transforms of moving named actors
  (`Npcs`, unreliable, 15 Hz sampling, mover rule 5 cm / resend 2 cm, 2 degrees or 1 s) and clients
  override the same actors by reference-name hash, smoothed (lerp 0.3, snap beyond 3 m), skipping
  avatars and carried actors. In game (T16): 21 movers streamed; all 760 shared named actors within
  0.5 m across copies (townspeople 0.14-0.18 m, where they had been tens of metres apart); loader
  work per frame rose to 41-54 us and host upload by 3.6 kB/s. Also checked (T15): single and double
  jumps rise the same on both copies (1.47 / 1.47 m, 2.37 / 2.32 m), tool switches (B) replay one for
  one. Open: X next to a townsperson started a conversation rather than a pick-up on the host, so
  carrying townspeople is not verified; the client's AI still runs underneath the override. See
  NETCODE.md "NPCs (H6c)".

- 2026-10-20, thrown objects (protocol 11): a thrown Pixar ball is a new physics actor on every
  copy, and the copies' balls bounced apart. The thrower's copy now sends its object's rigid-body
  position and velocity in `State.proj`; other copies pair it with their partner's throw and write
  it into their own ball's rigid body. In game (T17): the host's throw matched on the client within
  a few centimetres through two bounces (1.5 m off by 3 s before); the client's throw was followed
  on the host along the same path, about 0.3 m behind at full speed.

- 2026-10-20, player access (release 1.1.0, protocol 12): Join in the connect window now checks the
  host before leaving the window (a `Probe` plus a password `Check` the host answers under its key)
  and names the problem: no answer, wrong password, another ts3-online version or game build, not
  hosting, full. Hosts of another version are listed as such (their protocol refusal is read across
  versions), and a host no longer answers another version's refusal, which looped between two
  hosts. The window shows this PC's Tailscale and LAN addresses and the release. `Update.cmd`
  (`install.ps1 -Update`) installs the latest GitHub release over our DLL, read from a new
  `ts3online-version=` marker. PLAYING.md now opens with a quick start and has Update and
  Troubleshooting sections. Verified in game (T18), including against a real 1.0.0 host.

- 2026-10-20, partner screens (protocol 13): a full Toy Box mission in two copies. The host picked
  up the alien and threw him into the hat shop; the client's copy of the host's Woody stayed frozen
  in the throw tutorial, because the owner sent a neutral pad whenever its controls were off. The
  owner now sends its real pad with a controls-off flag, and a copy applies it to a partner whose
  controls are off too (the same tutorial or shop screen) and keeps a free partner still otherwise.
  In game (T19): both copies threw, showed the same hat screen moving in step, gave the alien the
  same hat, passed the mission and ended with the same gold (1580); a menu open only on the host
  left its Woody still on the client. Townspeople pick-up confirmed on both copies. Toy Box
  buildings found as `LOT_*` actors named `BL_*`; the catalog flow is mapped by name only.

- 2026-10-20, review and cleanup (no game run): comments across the repo cut by a third. A
  six-agent review of `main` found no crash reachable from the network, and these open items:
  world events stop syncing for the rest of a level once one side resets (`session.rs`
  `WorldAck` uses `max`), a recorded `Hello`/`Spend` can be replayed past `net_key`, a failed
  patch run still starts net and the save redirect, the save path garbles non-ASCII user names
  (`saves::ansi_path`), `net=host`/`client` bind and resolve inside DllMain, and panics leave no
  log line. Structure pass (PR 28, behaviour unchanged): shared `actor`, `math`, `pe` and
  `proto::flags`; one `hooks::table`; `install.rs` for the patch pipeline; `net/` split into
  `role`, `runner`, `auto`, `world_sync`; `coop/` and `menu/` split into pure and Win32/game
  halves; `NetKey` carried through `net::Options`.
- 2026-10-20, fixes from the review (not run in game; unit-tested in CI): world events re-converge
  after a one-sided restart (a client acks its position on entering a level, a lower ack or a
  repeated `Hello` rewinds the host's cursor, a host `Level` change restarts the client's list);
  a failed patch run no longer starts pads, net or the connect menu, and a panic is written to the
  log before the abort; the save folder redirect converts the path to the ANSI code page and falls
  back to the 8.3 short name, and logs why when it cannot redirect instead of silently sharing the
  real folder. Code standards recorded in `CLAUDE.md`.

- 2026-10-20, story levels start together (protocol 14): first online run of a story level (Train
  Rescue, host from the story board, client following from Toy Box). The host loaded in 3 s, the
  client in 65-75 s, so the host's opening ran alone and its idle player was shot down; once both
  were in, each copy's partner stood where its owner's was. Now a copy sends `flags::LOADING` until
  its level has loaded (`LevelLoadComplete`, then the intro movie, which blocks the game loop in the
  same update), a client follows a loading host at once, and a copy whose level has loaded pauses
  the game timer (as the pause menu does, without the menu) until no partner is on the way, at most
  120 s (`hold.rs`). Two pauses that did not work are recorded: the raw clock override (the opening
  kept running) and a posted `Pause` of kind 4 (a request only). In game (T20): the host paused for
  64.8 s and resumed 28 ms after the client's `loaded` line; both copies then showed the same
  opening line by line. A posted `Pause` with no argument crashes the game (FINDINGS 10). Open: a
  partner does not ride (Bullseye) on the other copy; a player downed while its partner stands idle
  can wait on a black screen.

- 2026-10-20, respawn with a remote partner (#31): the game respawns a downed player in two-player
  story mode at the checkpoint its partner last reached and waits until there is one; a remote
  partner's stand-in reaches none on this copy, so the downed player waited for good. After 2 s of
  that wait `respawn.rs` fills the point from the player's own last checkpoint, else the partner's
  position. In game (T21, Train Rescue, two runs): down at 664.9 s, point given at 667.5 s,
  `FinishedRespawn` the same frame; before, no respawn in 90 s.

- 2026-10-20, partners ride Bullseye (protocol 15): in Train Rescue each copy's own player rides the
  level's horse, mounted at the level start before partners are bound, so a partner stood on foot
  and was snapped to its owner again and again. The owner now sends `flags::RIDING`; a partner whose
  owner rides gets a Bullseye made the way the game's summon makes one and is mounted on it, and
  while both ride the horse and the partner are placed on the owner every frame (the summoned horse
  ignores its rider's stick). The avatar's interface getter takes a type id the decompiler hid; the
  state machine calls failed silently until it was passed. The game's own second-player recipe
  (`CreateJessieRidingBullseye`) left the partner on foot. In game (T22): a horse spawned for the
  partner on both copies within 0.1 s of the bind, drift 0.0 m while riding, the partner shown on a
  horse in Train Rescue and in Toy Box. New dev commands: `states`, `request`, `horse`, `autoride`.

- 2026-10-21, enemy health from the host (protocol 16): every hit goes through one function, the
  health interface's take-damage (`0x140441f80`), now detoured. Each copy drops hits on and by a
  partner's stand-in; a client reports what its own player takes off enemies and the host applies it;
  the host streams the health of enemies a player hit and clients follow it down through the game's
  own damage, so kills play the death on every copy. The first run in Train Rescue showed the client
  hitting the level's Jessie companion every frame (the host's copy was at 0, the client's would not
  stay down): now only plain `HealthTracker` actors count, the host streams only what players hit,
  and a figure is retried at most once a second. In game (T23, Toy Box): a host punch took a chicken
  to 90 on both copies; two client hits (dev `hit`) took another to 40 on both, matched by reference
  name across different ids; a kill from each side left both chickens at 0 on both copies.

- 2026-10-21, keyboard players online (#9): the owner's buttons came only from the XInput hook. Every
  input device shares one controller layout (code byte pairs, a control-to-code table), the
  keyboard device is updated each frame even while player 0 reads a pad, and an unassigned keyboard
  has no control table, so the loader reads it with the game's own keyboard layout and turns each
  active code into the pad button of its first control (the throw trigger while carrying; BACK left
  out). In game (T24): Space on the host's keyboard made the host's avatar jump on the client's
  screen; E arrived as B+Y, as the game itself sets it.

- 2026-10-21, missions, unlocks, pause, restarts and player damage (protocol 17): the game keeps
  missions (challenges, 291 in Toy Box) and unlocks (728 locks: catalog packs, hats, buildings) as
  named tables with a state each; the host pages both to clients, a client takes the host's state
  where it is behind after a 3 s grace through the game's own setters, and reports its own changes,
  which the host takes in turn (T25: a lock and a challenge each way, and a forced pass that ran the
  pass message, the unlocks after it and the rewards on the client). The lock table turned out to be
  level-scoped (Toy Box purchases read 0 in Train Rescue), which a first build reported as a client
  change; pages and reports now carry the level. A partner's mirrored START had been opening the
  pause menu on every copy; START and BACK now stay local, and the other copies pause their timer
  while a partner's game is paused (T26). With remote partners a level never restarted at its
  checkpoint (a stand-in is never down on this copy); each copy now shows its partner's down and
  waiting status on the stand-in's `Respawn`, and the game restarts both copies together when both
  are down (T27, three cases). Enemy hits on players are decided on the host: a client drops its own
  enemies' hits, and the host forwards its enemies' hits on a client's stand-in (T28). Not done: what
  a purchase builds in town (the lot event's id is per copy; the catalog's in-world trigger was not
  found), and stopping a client's AI (its hits no longer count, so it only shows). Per-part timings
  joined the `perf:` log; Train Rescue ran at 58 fps with 80-105 us of loader work per frame.

- 2026-10-21, buildings (#39, protocol 18): the lot selector's OK handler turned out to need only a
  building key, a buyer and a lot, so the loader calls it with a stand-in screen. The lots joined
  the progress tables as a third kind (a lot's building index + 1); a copy builds the other's
  building only on an empty lot, and each copy runs its own construction. T29: a building from
  each side, and a client restarted mid-session built both on joining. The catalog's in-world
  trigger is still unknown; it is no longer needed for the sync.

- 2026-10-21, enemy AI on clients: a client's enemies now skip their behaviour tree
  (`AiBehaviorControl`'s tick, detoured) and only follow the host's stream; their hits already came
  from the host. Telling enemies from friendly AI took three tries: the game's friendly-fire sets
  and `AipIsEnemy` both took Bullseye for an enemy, so the rule is the damage sync's (a plain
  `HealthTracker`) minus Bullseye. T30 at Train Rescue's start: the UFO followed the host, the
  horses kept running and both copies reached the same checkpoint. A level with walking enemies
  is still to be played.

- 2026-10-22, thrown objects on time: with 50 ms of simulated delay each way, a ball the host threw
  ran 75 ms behind on the client, 3-5 m at throwing speed. Each copy now estimates how old a
  partner's state is from the round trips and carries the report forward by that age. Placing the
  ball there directly threw one out of the level at a bounce (it was put inside the wall), so the
  follower is steered by velocity instead and placed only when 3 m off. Measured both ways: 5-20 ms
  behind, 0.2-0.4 m in flight, centimetres after the bounces.
- 2026-10-22, four players, joining and leaving, shots: everything two players had was played
  with four copies on this PC (T31, T32). Four things only broke with more players: players 3 and 4
  had sticks but no buttons (their binding lookup answers -1; patched), a level switch with 3 or 4
  players crashed (a 2-player guard in the switch handler; patched), the down-together restart
  paired slots 0 and 1 only (slot 1 now stands for every partner, and three timing problems found
  on the way are fixed: the wait latch, a stale given point, a slow copy missing a short wait), and
  a copy started while another was still exiting ran as a second copy 4. Joining and leaving: a
  closed client is parked and re-adopted with everything caught up; when the host leaves, a client
  hosts, but it had served its own save and players could drive the wrong character after the
  ids were handed out again (both fixed). Shots: thrown objects are carried forward by the state's
  age (#43). A copy in the background runs at about 20 fps here and its intro
  movie stalls until it is in front, which only matters with several copies on one PC.
- 2026-10-22, every story level online (T33): a sweep through all ten story levels with two copies
  and 80 ms of simulated round trip. Three faults, all fixed: Buzz's game crashed both copies a
  few seconds in (the partner's bind swapped out the level's own player-2 ship, whose flight
  controller then found no view; the partner now takes the level's ship); in both flight levels
  the partner drifted kilometres from its owner, because the flight controller places the ship from
  its rail state (now synced, protocol 19: about 5 m apart at 80 m/s); and a client entering any
  story level reported the level's reset of the coin counters as a purchase, which the host
  deducted from the shared wallet (counters are now ignored while a level loads). The other eight
  levels loaded, held and bound on both copies.
