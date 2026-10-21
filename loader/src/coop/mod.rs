//! Online co-op: every remote player is a real local partner in slots 1-3, driven by a virtual
//! XInput pad fed from the network, so the game's own co-op code runs unchanged (FINDINGS 9).
mod level;
mod partner;
mod ride;
mod steer;
mod view;

pub use level::{is_play_level, on_local_switch};

use crate::actor;
use crate::game::*;
use crate::hooks::bind_joining_avatar;
use crate::math::{dist, dist2, yaw_diff};
use crate::net::proto::{flags, level_hash, Pad, State, CLASS_UNKNOWN};
use crate::projectile;
use crate::target::{self, Target};
use crate::{log, net, pad};
use level::{follow_levels, level_loaded, local_level_name};
use partner::{adopt_parked, choose_class, class_name, free_slot, Class, Partner, CLASSES, SLOTS};
use steer::{is_idle, mirror_input, pressed_bits, rising, rotate_pad, steer_pad, stick_delta};
use std::collections::HashMap;
use std::ffi::c_char;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Instant;
use view::{view_axes, view_heading};

const RETRY_US: u64 = 2_000_000;
const LOG_US: u64 = 1_000_000;
/// Partner farther than this (m) from its owner is placed back.
const SNAP_DIST: f32 = 4.0;
/// Seconds of owner velocity the steering target leads by.
const LEAD: f32 = 0.10;
const PRESS_LOG_US: u64 = 500_000;
const FACE_US: u64 = 100_000;
const FACE_TOLERANCE: f32 = 10.0 * std::f32::consts::PI / 180.0;
/// Farther than this, a placement fires `PrepareForPlacing` first.
const PREPARE_DIST: f32 = 2.0;

static PARTNERS: Mutex<Option<HashMap<u8, Partner>>> = Mutex::new(None);
static START: OnceLock<Instant> = OnceLock::new();
/// Last raw local `ActorCarriedBy` / `ActorCarried`, to log changes.
static LAST_CARRY: Mutex<(u64, u64)> = Mutex::new((0, 0));
static LOCAL_TARGET: Mutex<Target> = Mutex::new(Target::None);
/// `net_stick_sign`: sign of the camera delta applied to partner sticks.
static STICK_SIGN: AtomicI32 = AtomicI32::new(1);
static STEER: AtomicBool = AtomicBool::new(true);
static STEER_X: AtomicI32 = AtomicI32::new(1);
static STEER_Y: AtomicI32 = AtomicI32::new(1);
static LAST_LEVEL: AtomicU32 = AtomicU32::new(0);
/// `LevelLoadComplete` was posted since the last level switch.
static LOAD_COMPLETE: AtomicBool = AtomicBool::new(false);
/// When slot 0 last went from empty to an avatar; cleared by a level switch.
static AVATAR_SINCE: Mutex<Option<Instant>> = Mutex::new(None);
static LOADED: AtomicBool = AtomicBool::new(false);
static LOCAL_RIDING: AtomicBool = AtomicBool::new(false);
static AUTO_RIDE: AtomicBool = AtomicBool::new(true);

/// `net_steer`, `net_steer_x`, `net_steer_y`.
pub fn set_steer(on: bool, x: i32, y: i32) {
    STEER.store(on, Ordering::Relaxed);
    STEER_X.store(x, Ordering::Relaxed);
    STEER_Y.store(y, Ordering::Relaxed);
}

/// Dev command `autoride`: whether partners follow their owner's riding.
pub fn set_auto_ride(on: bool) {
    AUTO_RIDE.store(on, Ordering::Relaxed);
}

/// Dev command `horse`: spawns a Bullseye at a slot's avatar and mounts it; the horse's id or 0.
pub unsafe fn dev_horse(slot: u64) -> u64 {
    let avatar = slot_avatar(global(MGR_PTR), slot);
    if avatar == 0 {
        return 0;
    }
    let horse = ride::summon(avatar);
    if horse != 0 {
        ride::mount(avatar, horse);
    }
    horse
}

pub fn set_stick_sign(sign: i32) {
    STICK_SIGN.store(sign, Ordering::Relaxed);
}

/// Per-frame facts every partner is driven with.
struct Frame<'a> {
    mgr: u64,
    /// play level hash, 0 in a menu
    level: u32,
    /// us since the first frame
    now: u64,
    own_class: Option<usize>,
    /// (avatar actor id, player id) of every player on this copy
    players: &'a [(u64, u8)],
}

/// Per frame, on the game thread.
pub unsafe fn update() {
    let (name, level, switched) = current_level();
    follow_levels(level);
    crate::wallet::update(level);
    crate::world::update(level);
    crate::npc::update(level);
    let Ok(mut guard) = PARTNERS.lock() else { return };
    let partners = guard.get_or_insert_with(HashMap::new);
    if switched && !partners.is_empty() {
        // the switch is only posted: leave the avatars to the teardown, freeing them first crashes (FINDINGS 10)
        log!("coop: level switch: {} partner(s) left to the level teardown", partners.len());
        partners.clear();
        forget_pads();
    }
    let mgr = global(MGR_PTR);
    let p0 = if mgr != 0 { slot_avatar(mgr, 0) } else { 0 };
    let loading = level != 0 && !track_load(switched, p0, &name);
    crate::hold::update(if loading || p0 == 0 { "" } else { &name });
    let loading_flag = if loading { flags::LOADING } else { 0 };

    let held = pad::take_local_pad();
    if p0 == 0 {
        net::set_local_state(0, CLASS_UNKNOWN, [0.0; 3], 0.0, 0.0, Pad::default(), loading_flag);
        // the level teardown already reset slots 1-3
        partners.clear();
        forget_pads();
        return;
    }
    let own_class = class_of(p0);
    let players = publish_local(mgr, p0, level, own_class, held, loading_flag, partners);
    if loading {
        // slot 0 still holds the old level's avatar: a partner spawned now dies with that level
        return;
    }
    crate::respawn::update(mgr);
    crate::health::update(level, &players);
    let now = START.get_or_init(Instant::now).elapsed().as_micros() as u64;
    let frame = Frame { mgr, level, now, own_class, players: &players };

    let remote = net::remote_states();
    for (id, p) in partners.iter_mut() {
        if !remote.iter().any(|(r, _)| r == id) {
            park(*id, p, "left");
        }
    }
    for (id, _) in &remote {
        if !partners.contains_key(id) {
            adopt_parked(partners, *id, &remote);
        }
    }
    for (id, st) in remote {
        let taken: Vec<usize> = partners.values().map(|p| p.slot).collect();
        let used: Vec<usize> = partners.iter().filter(|(i, p)| **i != id && p.avatar != 0).map(|(_, p)| p.class).collect();
        let p = match partners.get_mut(&id) {
            Some(p) => p,
            None => {
                let Some(slot) = free_slot(&taken) else { continue };
                partners.entry(id).or_insert(Partner::new(slot))
            }
        };
        if st.level != level || level == 0 || st.flags & flags::LOADING != 0 {
            park(id, p, "is in another level");
            continue;
        }
        drive(&frame, id, p, st, &used);
    }
}

/// The detour saw `LevelLoadComplete`.
pub fn on_level_loaded() {
    LOAD_COMPLETE.store(true, Ordering::Relaxed);
}

/// Whether the current level has finished loading (and its intro movie); logs the moment it has.
fn track_load(switched: bool, p0: u64, name: &str) -> bool {
    if switched {
        LOAD_COMPLETE.store(false, Ordering::Relaxed);
        LOADED.store(false, Ordering::Relaxed);
    }
    let Ok(mut since) = AVATAR_SINCE.lock() else { return true };
    if switched || p0 == 0 {
        *since = None;
    } else if since.is_none() {
        *since = Some(Instant::now());
    }
    let complete = LOAD_COMPLETE.load(Ordering::Relaxed);
    let loaded = level_loaded(complete, since.map(|t| t.elapsed()));
    if loaded && !LOADED.swap(true, Ordering::Relaxed) && !name.is_empty() {
        let how = if complete { "" } else { " (no LevelLoadComplete; assumed after 60 s)" };
        log!("coop: level {name} loaded{how}");
    }
    loaded
}

/// Play level name ("" in a menu), its hash and whether the level changed since the last frame.
unsafe fn current_level() -> (String, u32, bool) {
    let raw = local_level_name();
    let level = level_hash(&raw);
    let switched = LAST_LEVEL.swap(level, Ordering::Relaxed) != level;
    if switched {
        log!("coop: level {raw:?}");
    }
    let name = if is_play_level(&raw) { raw } else { String::new() };
    net::set_local_level_name(&name);
    let hash = level_hash(&name);
    (name, hash, switched)
}

/// Sends this copy's state, target and projectile; returns the players here for naming pick-up targets.
unsafe fn publish_local(mgr: u64, p0: u64, level: u32, own_class: Option<usize>, held: Pad, extra_flags: u32, partners: &HashMap<u8, Partner>) -> Vec<(u64, u8)> {
    let (pos, yaw) = actor::pose(p0);
    let controls = actor::controls_enabled(p0);
    let cam = view_heading(mgr, 0).unwrap_or(0.0);
    let riding = ride::riding(p0);
    if LOCAL_RIDING.swap(riding, Ordering::Relaxed) != riding {
        log!("coop: local player riding {riding}");
    }
    let state_flags = carry_flags(p0, true) | extra_flags | if controls { 0 } else { flags::CONTROLS_OFF } | if riding { flags::RIDING } else { 0 };
    let mut players: Vec<(u64, u8)> = partners.iter().filter(|(_, p)| p.avatar != 0).map(|(id, p)| (p.actor_id, *id)).collect();
    if let Some(me) = net::local_id() {
        players.push((actor::id(p0), me));
    }
    let target = target::local(p0, &players, level);
    if let Ok(mut prev) = LOCAL_TARGET.lock() {
        target::log_change(&mut prev, target);
    }
    let (state_flags, target_value) = target.encode(state_flags);
    net::set_local_target(target_value);
    let partner_ids: Vec<(u64, u8)> = players.iter().copied().filter(|(_, id)| Some(*id) != net::local_id()).collect();
    let slot_ids: Vec<u64> = (0..4).map(|i| slot_avatar(mgr, i)).filter(|a| *a != 0).map(|a| actor::id(a)).collect();
    net::set_local_proj(projectile::update(actor::id(p0), &partner_ids, &slot_ids));
    net::set_local_state(level, own_class.map_or(CLASS_UNKNOWN, |c| c as u8), pos, yaw, cam, held, state_flags);
    players
}

/// Drives one partner from its owner's state: presses, pad, pick-up target, carry, then binding or drift.
unsafe fn drive(f: &Frame, id: u8, p: &mut Partner, st: State, used: &[usize]) {
    let (mgr, now) = (f.mgr, f.now);
    if p.parked {
        p.parked = false;
        log!("coop: player {id} is back: slot {} avatar {:#x}", p.slot, p.avatar);
    }
    let fresh = p.track(st.seq, st.pos, now);
    if p.avatar != 0 && slot_avatar(mgr, p.slot as u64) != p.avatar {
        log!("coop: player {id} lost slot {} (avatar {:#x}); will rebind", p.slot, p.avatar);
        p.avatar = 0;
    }
    let partner_off = p.avatar != 0 && !actor::controls_enabled(p.avatar);
    let st = &mirror_input(st, partner_off);
    let pressed = pressed_bits(&st.pad);
    let edges = match (fresh, p.prev_pressed) {
        (true, Some(prev)) => rising(prev, pressed),
        _ => 0,
    };
    if fresh {
        p.prev_pressed = Some(pressed);
    }
    if p.avatar != 0 && edges != 0 {
        // interactions depend on exact geometry: match the owner's pose before the press lands
        let (cur, cur_yaw) = actor::pose(p.avatar);
        let yaw_err = yaw_diff(cur_yaw, st.yaw);
        actor::place_pose(p.avatar, st.pos, st.yaw);
        if now.saturating_sub(p.last_press_log_us) >= PRESS_LOG_US {
            p.last_press_log_us = now;
            log!("coop: player {id} press {edges:#x}, pose error {:.2} m / {:.1} deg", dist(cur, st.pos), yaw_err.to_degrees());
        }
    }
    let (out, delta, mag, idle) = partner_pad(mgr, p, st, now);
    if p.avatar != 0 {
        projectile::follow(id, st.proj);
        let want = Target::decode(st.flags, st.target);
        if want != p.target {
            p.target = want;
            log!("coop: player {id} target {want:?}");
        }
        if !target::steer(p.avatar, want, f.players, f.level, &mut p.target_written) {
            p.target_lost += 1;
            if now.saturating_sub(p.last_target_log_us) >= LOG_US {
                p.last_target_log_us = now;
                log!("coop: player {id} pick-up candidate rewritten by the game ({} frames)", p.target_lost);
            }
        }
        if idle && edges == 0 && now.saturating_sub(p.last_face_us) >= FACE_US && yaw_diff(actor::yaw(p.avatar), st.yaw) > FACE_TOLERANCE {
            p.last_face_us = now;
            actor::set_rotation(p.avatar, st.yaw);
        }
        if AUTO_RIDE.load(Ordering::Relaxed) {
            follow_ride(id, p, st.flags & flags::RIDING != 0, now);
        }
        let local = carry_flags(p.avatar, false);
        let owner = st.flags & (flags::CARRYING | flags::CARRIED);
        if p.carry.check(local, owner, now) {
            log!("coop: player {id} carry state local={local} owner={owner}");
        }
    }
    pad::set_remote_pad(p.slot, Some(out));
    if p.avatar == 0 {
        if now >= p.next_try_us {
            p.next_try_us = now + RETRY_US;
            bind(mgr, id, p, st, f.own_class, used);
        }
    } else {
        correct(p, id, st, now, delta, mag);
    }
}

/// Mounts the partner when its owner rides and it does not.
unsafe fn follow_ride(id: u8, p: &mut Partner, owner_riding: bool, now: u64) {
    let alive = p.ride.horse != 0 && registry_get(p.ride.horse) != 0;
    match p.ride.step(owner_riding, ride::riding(p.avatar), alive, now) {
        ride::Step::Idle => {}
        ride::Step::Mount => {
            log!("coop: player {id} rides; mounting its partner on horse {:#x} again", p.ride.horse);
            ride::mount(p.avatar, p.ride.horse);
        }
        ride::Step::Summon => {
            p.ride.horse = ride::summon(p.avatar);
            if p.ride.horse == 0 {
                log!("coop: player {id} rides; spawning a horse for its partner failed");
                return;
            }
            log!("coop: player {id} rides; spawned horse {:#x} for its partner", p.ride.horse);
            ride::mount(p.avatar, p.ride.horse);
        }
        ride::Step::GiveUp => log!("coop: player {id} rides; its partner could not be mounted, it stays on foot"),
    }
}

/// The partner's pad for this frame, the camera delta applied, the stick magnitude (NaN when not
/// steering) and whether the partner stands still.
unsafe fn partner_pad(mgr: u64, p: &Partner, st: &State, now: u64) -> (Pad, f32, f32, bool) {
    let delta = stick_delta(view_heading(mgr, p.slot as u64), st.cam, STICK_SIGN.load(Ordering::Relaxed));
    if p.avatar == 0 || !STEER.load(Ordering::Relaxed) {
        return (rotate_pad(st.pad, delta), delta, f32::NAN, false);
    }
    let Some((right, look)) = view_axes(mgr, p.slot as u64) else { return (rotate_pad(st.pad, delta), delta, f32::NAN, false) };
    let v = p.velocity(now);
    let target = [st.pos[0] + v[0] * LEAD, st.pos[2] + v[1] * LEAD];
    let cur = actor::pos(p.avatar);
    let err = [target[0] - cur[0], target[1] - cur[2]];
    let (sx, sy) = (STEER_X.load(Ordering::Relaxed), STEER_Y.load(Ordering::Relaxed));
    let (out, mag) = steer_pad(st.pad, err, v, right, look, delta, sx, sy);
    (out, delta, mag, is_idle(err, v, &st.pad))
}

unsafe fn class_of(avatar: u64) -> Option<usize> {
    let cls = prop_string(avatar + actor::PROPS, b"class\0".as_ptr() as u64, 0);
    if cls == 0 {
        return None;
    }
    CLASSES.iter().position(|c| stricmp(cls as *const c_char, c.class.as_ptr() as *const c_char) == 0)
}

fn forget_pads() {
    for s in SLOTS {
        pad::set_remote_pad(s, None);
    }
}

/// Spawns the remote's character and binds it to the next free player slot.
unsafe fn bind(mgr: u64, id: u8, p: &mut Partner, st: &State, own_class: Option<usize>, used: &[usize]) {
    let input = global(INPUT_PTR);
    if input == 0 {
        return;
    }
    let slot = p.slot;
    if input_is_assigned(input, slot as u32) == 0 {
        input_assign(input, slot as u32, slot as u32 + 1);
    }
    p.class = match st.class {
        c if (c as usize) < CLASSES.len() => c as usize,
        _ => choose_class(own_class, used, CLASSES.len()),
    };
    let avatar = spawn(mgr, &CLASSES[p.class], st.pos, st.yaw);
    if avatar == 0 {
        input_release(input, slot as u32);
        log!("coop: spawn for player {id} failed");
        return;
    }
    let (level_info, start) = join_start();
    bind_joining_avatar(avatar, level_info, start);
    match SLOTS.iter().copied().find(|s| slot_avatar(mgr, *s as u64) == avatar) {
        Some(actual) => {
            if actual != slot {
                log!("coop: player {id} landed in slot {actual}, expected {slot}");
                pad::set_remote_pad(slot, None);
                p.slot = actual;
            }
            p.avatar = avatar;
            p.actor_id = actor::id(avatar);
            log!("coop: player {id} bound: slot {} avatar {avatar:#x} actor {:#x} class {}", p.slot, p.actor_id, class_name(p.class));
        }
        None => {
            log!("coop: bind of player {id} (avatar {avatar:#x}) reached no slot; removing it");
            remove_avatar(avatar);
            input_release(input, slot as u32);
        }
    }
}

/// What the join menu hands the bind (`0x140104190`): player 1's level region and the name of the
/// `StartPosition_P2_<region>` locator, so a ride level puts the partner on its mount (auto-drive).
/// The game string is leaked: the bind keeps no reference, and binds are rare.
unsafe fn join_start() -> (u64, u64) {
    let region = player_region(global(STREAMING), 0);
    let info = if region == 0 { 0 } else { rd_u64(region) };
    let name = if info == 0 { 0 } else { rd_u64(info + LEVEL_INFO_NAME) };
    if name == 0 {
        log!("coop: no level region for player 1; the partner is bound without the level's start (no auto-drive)");
        return (0, 0);
    }
    let region_name = std::ffi::CStr::from_ptr(name as *const c_char).to_string_lossy();
    let Ok(locator) = std::ffi::CString::new(format!("StartPosition_P2_{region_name}")) else { return (info, 0) };
    let alloc = match global(STRING_ALLOC) {
        0 => string_alloc_create(),
        a => a,
    };
    let start: &'static mut u64 = Box::leak(Box::new(0));
    string_new(start, locator.as_ptr(), alloc);
    (info, start as *mut u64 as u64)
}

/// Leaves the avatar idle: removing one mid-level drops the player count and can crash (FINDINGS 10).
fn park(id: u8, p: &mut Partner, why: &str) {
    pad::set_remote_pad(p.slot, None);
    if !p.parked {
        p.parked = true;
        if p.avatar != 0 {
            log!("coop: player {id} {why}: slot {} parked (avatar {:#x})", p.slot, p.avatar);
        }
    }
}

/// Snaps a partner that drifted too far back to its owner.
unsafe fn correct(p: &mut Partner, id: u8, st: &State, now: u64, delta: f32, mag: f32) {
    let cur = actor::pos(p.avatar);
    if now.saturating_sub(p.last_drift_us) >= LOG_US {
        p.last_drift_us = now;
        log!("coop: player {id} slot {} drift {:.1} m, cam delta {:.1} deg, steer {:.2}", p.slot, dist(cur, st.pos), delta.to_degrees(), mag);
    }
    // a summoned horse does not answer its rider's stick, nor carry its rider when placed: both
    // are placed on the owner every frame (a rider's position is its horse's)
    let owner_riding = st.flags & flags::RIDING != 0;
    let horse = if owner_riding && p.ride.horse != 0 && ride::riding(p.avatar) { registry_get(p.ride.horse) } else { 0 };
    if horse != 0 {
        actor::place_pose(horse, st.pos, st.yaw);
        actor::place_pose(p.avatar, st.pos, st.yaw);
        return;
    }
    if dist2(cur, st.pos) <= SNAP_DIST * SNAP_DIST {
        return;
    }
    place(p.avatar, p.actor_id, st.pos, st.yaw);
    p.snaps += 1;
    if now.saturating_sub(p.last_log_us) >= LOG_US {
        p.last_log_us = now;
        log!("coop: player {id} slot {} snapped to the owner ({} snaps)", p.slot, p.snaps);
    }
}

/// Uses the synchronous level-setup spawner; 0 on failure.
unsafe fn spawn(mgr: u64, class: &Class, pos: [f32; 3], yaw: f32) -> u64 {
    let mut props = [0u64; 3];
    props_init(props.as_mut_ptr());
    props_parse(props.as_mut_ptr(), class.spec.as_ptr() as *const c_char);
    let at = [pos[0], pos[1], pos[2], 0.0];
    let avatar = spawn_avatar(mgr, props.as_mut_ptr(), at.as_ptr(), yaw, 0);
    if avatar != 0 {
        name_avatar(avatar, class.name.as_ptr() as *const c_char, hash(class.hash));
    }
    props_free(props.as_mut_ptr());
    if avatar != 0 && rd_u32(avatar + actor::ACTIVE_FLAGS) & 1 == 0 {
        activate(global(ACTOR_MGR), avatar);
        wake(avatar, 0);
    }
    avatar
}

/// `ActorCarriedBy` / `ActorCarried` keep a pointer at `+0x18` (bindings 0x1400b0020 / 0x1400b0120).
/// Which one means which direction is unconfirmed, so the raw local values are logged.
unsafe fn carry_flags(avatar: u64, log_raw: bool) -> u32 {
    let raw = |name: &[u8]| {
        let comp = find_component(avatar, name.as_ptr() as u64);
        if comp == 0 {
            0
        } else {
            rd_u64(comp + 0x18)
        }
    };
    let (by, carried) = (raw(b"ActorCarriedBy\0"), raw(b"ActorCarried\0"));
    if log_raw {
        if let Ok(mut last) = LAST_CARRY.lock() {
            if *last != (by, carried) {
                *last = (by, carried);
                log!("coop: local carry raw ActorCarriedBy={by:#x} ActorCarried={carried:#x}");
            }
        }
    }
    (if by != 0 { flags::CARRYING } else { 0 }) | (if carried != 0 { flags::CARRIED } else { 0 })
}

/// Mirrors the game's placing handler (0x1406a2010).
unsafe fn place(actor: u64, actor_id: u64, pos: [f32; 3], yaw: f32) {
    if dist2(actor::pos(actor), pos) > PREPARE_DIST * PREPARE_DIST {
        ev5(global(EVENTS), va(0x1_4077_1420), 0, 0, actor_id); // "PrepareForPlacing"
    }
    actor::place_pose(actor, pos, yaw);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn facing_tolerance_is_ten_degrees() {
        assert!(yaw_diff(0.0, 0.1) < FACE_TOLERANCE && yaw_diff(0.0, 0.2) > FACE_TOLERANCE);
    }
}
