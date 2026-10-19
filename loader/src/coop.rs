//! Online co-op: every remote player is a real local partner in slots 1-3, driven by a virtual
//! XInput pad fed from the network, so the game's own co-op code runs unchanged (FINDINGS 9).
use crate::game::*;
use crate::hooks::bind_joining_avatar;
use crate::{hooks, log};
use crate::net::proto::{level_hash, Pad, State, CLASS_UNKNOWN};
use crate::{net, pad};
use std::collections::HashMap;
use std::ffi::{c_char, CStr};
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

const RETRY_US: u64 = 2_000_000;
const LOG_US: u64 = 1_000_000;
/// Farther than this from the owner's position, a partner is placed back.
const SNAP_DIST: f32 = 4.0;
/// Seconds of owner velocity the steering target leads by.
const LEAD: f32 = 0.10;
/// Inside this error (m) the partner copies the owner motion instead of chasing the position.
const NEAR_ERR: f32 = 0.25;
/// Error (m) at which chasing reaches full stick.
const FULL_ERR: f32 = 1.5;
const MOVING_SPEED: f32 = 0.5;
const IDLE_STICK: f32 = 0.15;
/// Owner updates older than this (us) count as standing still.
const STALE_US: u64 = 250_000;
/// Buttons that start an interaction: A, B, X, Y, LB, RB (XINPUT bits) and the triggers past half.
const ACTION_BUTTONS: u32 = 0xf300;
const LT_BIT: u32 = 1 << 16;
const RT_BIT: u32 = 1 << 17;
const TRIGGER_PRESSED: u8 = 128;
const PRESS_LOG_US: u64 = 500_000;
const FACE_US: u64 = 100_000;
const FACE_TOLERANCE: f32 = 10.0 * std::f32::consts::PI / 180.0;
/// `State.flags`: the local avatar carries someone / is carried.
const FLAG_CARRYING: u32 = 1;
const FLAG_CARRIED: u32 = 2;
const CARRY_DISAGREE_US: u64 = 500_000;
/// Farther than this, a placement fires `PrepareForPlacing` first.
const PREPARE_DIST: f32 = 2.0;
/// Local slots for remote players.
const SLOTS: [usize; 3] = [1, 2, 3];
/// Avatar byte that says whether controls are enabled.
const CONTROLS_ON: u64 = 0x428;
/// A client follows the host's level once the two have differed this long.
const FOLLOW_AFTER: Duration = Duration::from_millis(1500);
/// No level follow this soon after a level switch of this copy.
const FOLLOW_COOLDOWN: Duration = Duration::from_secs(15);

struct Class {
    /// NUL-terminated class name, as stored in the avatar's properties.
    class: &'static [u8],
    spec: &'static [u8],
    name: &'static [u8],
    hash: u32,
}

/// Indexed by the `class` byte of a state message.
const CLASSES: [Class; 3] = [
    Class { class: b"buzz\0", spec: b"class=buzz; model_name=characters/BuzzLightYear/BuzzLightYear\0", name: b"Buzz2\0", hash: 0xcc },
    Class { class: b"woody\0", spec: b"class=woody; model_name=characters/Woody/Woody\0", name: b"Woody2\0", hash: 0xd4 },
    Class { class: b"jessie\0", spec: b"class=jessie; model_name=characters/Jessie/Jessie\0", name: b"Jessie2\0", hash: 0xcf },
];

/// A remote player and the local slot that stands for it.
struct Partner {
    slot: usize,
    /// 0 while unbound
    avatar: u64,
    actor_id: u64,
    class: usize,
    next_try_us: u64,
    snaps: u32,
    last_log_us: u64,
    last_drift_us: u64,
    /// owner position history from received states: seq of the latest, its position and receive time
    last_seq: Option<u32>,
    last_pos: [f32; 3],
    last_at: u64,
    /// owner velocity on x and z (m/s)
    vel: [f32; 2],
    /// owner pressed-button set of the previous state
    prev_pressed: Option<u32>,
    last_press_log_us: u64,
    last_face_us: u64,
    carry: CarryWatch,
    /// owner gone or elsewhere: the avatar stays, idle, until the owner returns or the level ends
    parked: bool,
}

/// Notices when a partner carry state disagrees with its owner for a while.
#[derive(Default)]
struct CarryWatch {
    key: Option<(u32, u32)>,
    since: u64,
    logged: bool,
}

impl CarryWatch {
    /// True once per disagreement that has lasted `CARRY_DISAGREE_US`.
    fn check(&mut self, local: u32, owner: u32, now: u64) -> bool {
        if local == owner {
            self.key = None;
            return false;
        }
        if self.key != Some((local, owner)) {
            self.key = Some((local, owner));
            self.since = now;
            self.logged = false;
        }
        if !self.logged && now.saturating_sub(self.since) >= CARRY_DISAGREE_US {
            self.logged = true;
            return true;
        }
        false
    }
}

impl Partner {
    fn new(slot: usize) -> Partner {
        Partner {
            slot,
            parked: false,
            avatar: 0,
            actor_id: 0,
            class: 0,
            next_try_us: 0,
            snaps: 0,
            last_log_us: 0,
            last_drift_us: 0,
            last_seq: None,
            last_pos: [0.0; 3],
            last_at: 0,
            vel: [0.0; 2],
            prev_pressed: None,
            last_press_log_us: 0,
            last_face_us: 0,
            carry: CarryWatch::default(),
        }
    }

    /// Records a received state (once per sequence number) and updates the owner velocity.
    /// True when the state is new.
    fn track(&mut self, seq: u32, pos: [f32; 3], now: u64) -> bool {
        if self.last_seq == Some(seq) {
            return false;
        }
        if self.last_seq.is_some() {
            if let Some(v) = velocity_xz(self.last_pos, self.last_at, pos, now) {
                self.vel = v;
            }
        }
        self.last_seq = Some(seq);
        self.last_pos = pos;
        self.last_at = now;
        true
    }

    /// Owner velocity, zero when the owner has gone quiet.
    fn velocity(&self, now: u64) -> [f32; 2] {
        if now.saturating_sub(self.last_at) > STALE_US {
            [0.0; 2]
        } else {
            self.vel
        }
    }
}

static PARTNERS: Mutex<Option<HashMap<u8, Partner>>> = Mutex::new(None);
static START: OnceLock<Instant> = OnceLock::new();
/// Last raw `ActorCarriedBy` / `ActorCarried` values of the local avatar, for change logging.
static LAST_CARRY: Mutex<(u64, u64)> = Mutex::new((0, 0));
/// `net_stick_sign`: multiplies the camera delta applied to partners' sticks.
static STICK_SIGN: AtomicI32 = AtomicI32::new(1);
static STEER: AtomicBool = AtomicBool::new(true);
static STEER_X: AtomicI32 = AtomicI32::new(1);
static STEER_Y: AtomicI32 = AtomicI32::new(1);
/// Since when the client's level differs from the host's (both in a level).
static MISMATCH_SINCE: Mutex<Option<Instant>> = Mutex::new(None);
/// When this copy last switched level, by its own game or by following.
static LAST_SWITCH: Mutex<Option<Instant>> = Mutex::new(None);
/// Hash of the level seen last frame, to log level changes.
static LAST_LEVEL: AtomicU32 = AtomicU32::new(0);

/// `net_steer`, `net_steer_x`, `net_steer_y`.
pub fn set_steer(on: bool, x: i32, y: i32) {
    STEER.store(on, Ordering::Relaxed);
    STEER_X.store(x, Ordering::Relaxed);
    STEER_Y.store(y, Ordering::Relaxed);
}

pub fn set_stick_sign(sign: i32) {
    STICK_SIGN.store(sign, Ordering::Relaxed);
}

/// Yaw (radians) of a pure-yaw quaternion `(0, y, 0, w)`.
pub fn yaw_from_quat(qy: f32, qw: f32) -> f32 {
    -2.0 * qy.atan2(qw)
}

/// Quaternion `(x, y, z, w)` for a rotation of `yaw` about the vertical axis.
pub fn quat_from_yaw(yaw: f32) -> [f32; 4] {
    let h = yaw * 0.5;
    [0.0, -h.sin(), 0.0, h.cos()]
}

/// Class index for a partner whose class is unknown: not the local avatar's, not another partner's.
pub fn choose_class(own: Option<usize>, used: &[usize], count: usize) -> usize {
    (0..count)
        .find(|c| Some(*c) != own && !used.contains(c))
        .or_else(|| (0..count).find(|c| Some(*c) != own))
        .unwrap_or(0)
}

/// Lowest free local slot, given the slots in use.
pub fn free_slot(taken: &[usize]) -> Option<usize> {
    SLOTS.iter().copied().find(|s| !taken.contains(s))
}

/// Interaction buttons held in a pad: XINPUT bits plus `LT_BIT` / `RT_BIT` for triggers past half.
pub fn pressed_bits(p: &Pad) -> u32 {
    let mut b = p.buttons as u32 & ACTION_BUTTONS;
    if p.lt >= TRIGGER_PRESSED {
        b |= LT_BIT;
    }
    if p.rt >= TRIGGER_PRESSED {
        b |= RT_BIT;
    }
    b
}

/// Buttons newly pressed between two pressed sets.
pub fn rising(prev: u32, cur: u32) -> u32 {
    cur & !prev
}

/// Absolute difference of two angles, wrapped to 0..=pi.
pub fn yaw_diff(a: f32, b: f32) -> f32 {
    let d = (a - b).rem_euclid(2.0 * std::f32::consts::PI);
    d.min(2.0 * std::f32::consts::PI - d)
}

/// Length of the left stick, 0..1.
fn owner_mag(p: &Pad) -> f32 {
    len2([p.lx as f32 / 32767.0, p.ly as f32 / 32767.0]).min(1.0)
}

/// Velocity on x and z between two received positions; None when they are less than 1 ms apart.
pub fn velocity_xz(p0: [f32; 3], t0: u64, p1: [f32; 3], t1: u64) -> Option<[f32; 2]> {
    let dt = t1.saturating_sub(t0);
    if dt < 1000 {
        return None;
    }
    let s = dt as f32 / 1e6;
    Some([(p1[0] - p0[0]) / s, (p1[2] - p0[2]) / s])
}

fn len2(v: [f32; 2]) -> f32 {
    (v[0] * v[0] + v[1] * v[1]).sqrt()
}

fn normalize2(v: [f32; 2]) -> Option<[f32; 2]> {
    let l = len2(v);
    (l.is_finite() && l > 1e-4).then(|| [v[0] / l, v[1] / l])
}

/// What a partner left stick should do.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Steer {
    /// walk in a world direction (unit) at a stick magnitude 0..1
    Dir([f32; 2], f32),
    /// the owner raw left stick, rotated into this view
    Raw,
    /// stand still
    Idle,
}

/// Closed-loop steering: `err` is target minus the partner position on x/z, `vel` the owner
/// velocity, `owner_mag` the owner left-stick length (0..1).
pub fn decide_steer(err: [f32; 2], vel: [f32; 2], owner_mag: f32) -> Steer {
    let e = len2(err);
    if !e.is_finite() {
        return Steer::Idle;
    }
    if e < NEAR_ERR {
        if let Some(d) = normalize2(vel).filter(|_| len2(vel) > MOVING_SPEED) {
            return Steer::Dir(d, owner_mag.clamp(0.0, 1.0));
        }
        return if owner_mag < IDLE_STICK { Steer::Idle } else { Steer::Raw };
    }
    Steer::Dir([err[0] / e, err[1] / e], owner_mag.max(e / FULL_ERR).clamp(0.0, 1.0))
}

/// Left-stick axes for a world direction under a view whose `right` and `look` are unit vectors on
/// x/z; `sx` and `sy` are the axis signs (`net_steer_x`, `net_steer_y`).
pub fn world_to_stick(dir: [f32; 2], mag: f32, right: [f32; 2], look: [f32; 2], sx: i32, sy: i32) -> (i16, i16) {
    let to_i16 = |v: f32| (v * 32767.0).round().clamp(-32767.0, 32767.0) as i16;
    let dot = |a: [f32; 2], b: [f32; 2]| a[0] * b[0] + a[1] * b[1];
    (to_i16(mag * dot(dir, right) * sx as f32), to_i16(mag * dot(dir, look) * sy as f32))
}

/// The pad a bound partner gets: buttons and triggers from the owner, left stick steered, right stick centred.
#[allow(clippy::too_many_arguments)]
pub fn steer_pad(owner: Pad, err: [f32; 2], vel: [f32; 2], right: [f32; 2], look: [f32; 2], delta: f32, sx: i32, sy: i32) -> (Pad, f32) {
    let owner_mag = owner_mag(&owner);
    let (lx, ly, mag) = match decide_steer(err, vel, owner_mag) {
        Steer::Dir(d, m) => {
            let (x, y) = world_to_stick(d, m, right, look, sx, sy);
            (x, y, m)
        }
        Steer::Raw => {
            let (x, y) = rotate_stick(owner.lx, owner.ly, delta);
            (x, y, owner_mag)
        }
        Steer::Idle => (0, 0, 0.0),
    };
    (Pad { lx, ly, rx: 0, ry: 0, ..owner }, mag)
}

/// Rotates a stick (i16 axes, normalised to -1..1) by `d` radians, counter-clockwise.
pub fn rotate_stick(x: i16, y: i16, d: f32) -> (i16, i16) {
    let (fx, fy) = (x as f32 / 32767.0, y as f32 / 32767.0);
    let (s, c) = d.sin_cos();
    let to_i16 = |v: f32| (v * 32767.0).round().clamp(-32768.0, 32767.0) as i16;
    (to_i16(fx * c - fy * s), to_i16(fx * s + fy * c))
}

/// `pad` with both thumbsticks rotated by `d` radians.
pub fn rotate_pad(pad: Pad, d: f32) -> Pad {
    if d == 0.0 || !d.is_finite() {
        return pad;
    }
    let (lx, ly) = rotate_stick(pad.lx, pad.ly, d);
    let (rx, ry) = rotate_stick(pad.rx, pad.ry, d);
    Pad { lx, ly, rx, ry, ..pad }
}

unsafe fn view_ptr(mgr: u64, i: u64) -> Option<u64> {
    let views = rd_u64(mgr + 0xd8);
    if views == 0 || rd_i32(views) <= i as i32 {
        return None;
    }
    let view = rd_u64(rd_u64(views + 8) + i * 8);
    (view != 0).then_some(view)
}

/// Unit `right` and `look` vectors on x/z of the view for slot `i`, from its view matrix
/// (`right = (m00, m20)`, `look = (-m02, -m22)`).
unsafe fn view_axes(mgr: u64, i: u64) -> Option<([f32; 2], [f32; 2])> {
    let view = view_ptr(mgr, i)?;
    let right = normalize2([rd_f32(view + 0x20), rd_f32(view + 0x40)])?;
    let look = normalize2([-rd_f32(view + 0x28), -rd_f32(view + 0x48)])?;
    Some((right, look))
}

/// Camera heading of the view for player slot `i`, from its view matrix (`view+0x20`, row major):
/// `atan2(m[0][2], m[2][2])`. None when the view is missing or the values are not finite.
unsafe fn view_heading(mgr: u64, i: u64) -> Option<f32> {
    let view = view_ptr(mgr, i)?;
    let h = rd_f32(view + 0x28).atan2(rd_f32(view + 0x48));
    h.is_finite().then_some(h)
}

/// Rotation that turns a stick held relative to the owner's camera (`owner_cam`) into the same
/// world direction under this copy's camera for the slot (`local_cam`).
pub fn stick_delta(local_cam: Option<f32>, owner_cam: f32, sign: i32) -> f32 {
    match local_cam {
        Some(l) if owner_cam.is_finite() => sign as f32 * (l - owner_cam),
        _ => 0.0,
    }
}

/// Index in `CLASSES` of an avatar's class property, if known.
unsafe fn class_of(avatar: u64) -> Option<usize> {
    let cls = prop_string(avatar + 0x2e0, b"class\0".as_ptr() as u64, 0);
    if cls == 0 {
        return None;
    }
    CLASSES.iter().position(|c| stricmp(cls as *const c_char, c.class.as_ptr() as *const c_char) == 0)
}

/// Name of the current level, empty outside a level.
unsafe fn local_level_name() -> String {
    let name = level_name();
    if name == 0 {
        return String::new();
    }
    CStr::from_ptr(name as *const c_char).to_string_lossy().into_owned()
}

/// A level players play in; the front end and the story game board (`gui_*`) are menus.
pub fn is_play_level(name: &str) -> bool {
    !name.is_empty() && !name.get(..4).is_some_and(|p| p.eq_ignore_ascii_case("gui_")) && !name.eq_ignore_ascii_case("frontend")
}

/// The level name a client should switch to: both sides are in a level, the levels differ for
/// `FOLLOW_AFTER`, the host's name matches the hash its state carries, and this copy has not
/// switched within `FOLLOW_COOLDOWN`.
pub fn decide_follow(local_hash: u32, host_hash: u32, host_name: Option<&str>, mismatch_for: Duration, since_switch: Duration) -> Option<String> {
    if local_hash == 0 || host_hash == 0 || local_hash == host_hash {
        return None;
    }
    let name = host_name.filter(|n| level_hash(n) == host_hash)?;
    (mismatch_for >= FOLLOW_AFTER && since_switch >= FOLLOW_COOLDOWN).then(|| name.to_string())
}

fn note_switch() {
    if let Ok(mut t) = LAST_SWITCH.lock() {
        *t = Some(Instant::now());
    }
}

/// A switch the game itself posted (runs on every `SwitchLevel` post, before the game handles it).
pub fn on_local_switch(name: &str) {
    note_switch();
    if net::is_client() && is_play_level(name) {
        net::request_level(name);
    }
}

unsafe fn post_switch_level(name: &str) {
    hooks::call_broadcast(global(EVENTS), b"SwitchLevel\0".as_ptr() as u64, leak_cstr(name), 0);
}

/// Client: how long the local and host levels have differed (zero when they match or either is unset).
fn mismatch_duration(local: u32, host: u32, now: Instant) -> Duration {
    let Ok(mut since) = MISMATCH_SINCE.lock() else { return Duration::ZERO };
    if local == 0 || host == 0 || local == host {
        *since = None;
        return Duration::ZERO;
    }
    now.saturating_duration_since(*since.get_or_insert(now))
}

/// Level sync: a client follows the host's level, the host follows a client's switch.
unsafe fn follow_levels(local: u32) {
    let now = Instant::now();
    if net::is_client() {
        let host = net::remote_states().into_iter().find(|(id, _)| *id == 0).map_or(0, |(_, s)| s.level);
        let mismatch = mismatch_duration(local, host, now);
        let since_switch = LAST_SWITCH.lock().ok().and_then(|t| *t).map_or(Duration::MAX, |t| now.saturating_duration_since(t));
        let host_name = net::host_level_name();
        if let Some(name) = decide_follow(local, host, host_name.as_deref(), mismatch, since_switch) {
            log!("coop: following the host to level {name}");
            note_switch();
            mismatch_duration(0, 0, now);
            post_switch_level(&name);
        }
        return;
    }
    for name in net::take_level_requests() {
        if local != 0 && is_play_level(&name) && local != level_hash(&name) {
            log!("coop: a client changed level, following to {name}");
            note_switch();
            post_switch_level(&name);
        }
    }
}

fn forget_pads() {
    for s in SLOTS {
        pad::set_remote_pad(s, None);
    }
}

/// Per-frame work on the game thread: publish the local player, bind, feed and correct partners.
pub unsafe fn update() {
    let raw = local_level_name();
    let level = level_hash(&raw);
    let switched = LAST_LEVEL.swap(level, Ordering::Relaxed) != level;
    if switched {
        log!("coop: level {raw:?}");
    }
    let name = if is_play_level(&raw) { raw } else { String::new() };
    let level = level_hash(&name);
    net::set_local_level_name(&name);
    follow_levels(level);
    crate::wallet::update(level);
    let Ok(mut guard) = PARTNERS.lock() else { return };
    let partners = guard.get_or_insert_with(HashMap::new);
    if switched && !partners.is_empty() {
        // the name changes when the switch is posted; the coming teardown frees the partner
        // avatars; removing them here first was followed by a crash (FINDINGS 10)
        log!("coop: level switch: {} partner(s) left to the level teardown", partners.len());
        partners.clear();
        forget_pads();
    }
    let mgr = global(MGR_PTR);
    let p0 = if mgr != 0 { slot_avatar(mgr, 0) } else { 0 };

    let held = pad::take_local_pad();
    if p0 == 0 {
        net::set_local_state(0, CLASS_UNKNOWN, [0.0; 3], 0.0, 0.0, Pad::default(), 0);
        // the level teardown already reset slots 1-3
        partners.clear();
        forget_pads();
        return;
    }
    let own_class = class_of(p0);
    let pos = [rd_f32(p0 + 0x80), rd_f32(p0 + 0x84), rd_f32(p0 + 0x88)];
    let yaw = yaw_from_quat(rd_f32(p0 + 0x94), rd_f32(p0 + 0x9c));
    let controls = rd_u8(p0 + CONTROLS_ON) != 0;
    let pad_out = if controls { held } else { Pad::default() };
    let cam = view_heading(mgr, 0).unwrap_or(0.0);
    let flags = carry_flags(p0, true);
    net::set_local_state(level, own_class.map_or(CLASS_UNKNOWN, |c| c as u8), pos, yaw, cam, pad_out, flags);

    let now = START.get_or_init(Instant::now).elapsed().as_micros() as u64;
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
        if st.level != level || level == 0 {
            park(id, p, "is in another level");
            continue;
        }
        if p.parked {
            p.parked = false;
            log!("coop: player {id} is back: slot {} avatar {:#x}", p.slot, p.avatar);
        }
        let fresh = p.track(st.seq, st.pos, now);
        if p.avatar != 0 && slot_avatar(mgr, p.slot as u64) != p.avatar {
            log!("coop: player {id} lost slot {} (avatar {:#x}); will rebind", p.slot, p.avatar);
            p.avatar = 0;
        }
        let pressed = pressed_bits(&st.pad);
        let edges = match (fresh, p.prev_pressed) {
            (true, Some(prev)) => rising(prev, pressed),
            _ => 0,
        };
        if fresh {
            p.prev_pressed = Some(pressed);
        }
        if p.avatar != 0 && edges != 0 {
            // interactions depend on exact geometry: make the pose match the owner before the press lands
            let cur = [rd_f32(p.avatar + 0x80), rd_f32(p.avatar + 0x84), rd_f32(p.avatar + 0x88)];
            let yaw_err = yaw_diff(avatar_yaw(p.avatar), st.yaw);
            place_pose(p.avatar, st.pos, st.yaw);
            if now.saturating_sub(p.last_press_log_us) >= PRESS_LOG_US {
                p.last_press_log_us = now;
                log!("coop: player {id} press {edges:#x}, pose error {:.2} m / {:.1} deg", dist2(cur, st.pos).sqrt(), yaw_err.to_degrees());
            }
        }
        let delta = stick_delta(view_heading(mgr, p.slot as u64), st.cam, STICK_SIGN.load(Ordering::Relaxed));
        let mut out = rotate_pad(st.pad, delta);
        let mut mag = f32::NAN;
        let mut idle = false;
        if p.avatar != 0 && STEER.load(Ordering::Relaxed) {
            if let Some((right, look)) = view_axes(mgr, p.slot as u64) {
                let v = p.velocity(now);
                let target = [st.pos[0] + v[0] * LEAD, st.pos[2] + v[1] * LEAD];
                let err = [target[0] - rd_f32(p.avatar + 0x80), target[1] - rd_f32(p.avatar + 0x88)];
                let (sx, sy) = (STEER_X.load(Ordering::Relaxed), STEER_Y.load(Ordering::Relaxed));
                (out, mag) = steer_pad(st.pad, err, v, right, look, delta, sx, sy);
                idle = decide_steer(err, v, owner_mag(&st.pad)) == Steer::Idle && len2(v) < MOVING_SPEED;
            }
        }
        if p.avatar != 0 {
            if idle && edges == 0 && now.saturating_sub(p.last_face_us) >= FACE_US && yaw_diff(avatar_yaw(p.avatar), st.yaw) > FACE_TOLERANCE {
                p.last_face_us = now;
                set_rotation(p.avatar, st.yaw);
            }
            let local = carry_flags(p.avatar, false);
            if p.carry.check(local, st.flags & (FLAG_CARRYING | FLAG_CARRIED), now) {
                log!("coop: player {id} carry state local={local} owner={}", st.flags & (FLAG_CARRYING | FLAG_CARRIED));
            }
        }
        pad::set_remote_pad(p.slot, Some(out));
        if p.avatar == 0 {
            if now >= p.next_try_us {
                p.next_try_us = now + RETRY_US;
                bind(mgr, id, p, &st, own_class, &used);
            }
        } else {
            correct(p, id, &st, now, delta, mag);
        }
    }
}

/// Spawns the remote's character and binds it to the next free local player slot.
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
    bind_joining_avatar(avatar, 0, 0);
    match SLOTS.iter().copied().find(|s| slot_avatar(mgr, *s as u64) == avatar) {
        Some(actual) => {
            if actual != slot {
                log!("coop: player {id} landed in slot {actual}, expected {slot}");
                pad::set_remote_pad(slot, None);
                p.slot = actual;
            }
            p.avatar = avatar;
            p.actor_id = rd_u64(avatar + 0x38);
            log!("coop: player {id} bound: slot {} avatar {avatar:#x} actor {:#x} class {}", p.slot, p.actor_id, class_name(p.class));
        }
        None => {
            log!("coop: bind of player {id} (avatar {avatar:#x}) reached no slot; removing it");
            remove_avatar(avatar);
            input_release(input, slot as u32);
        }
    }
}

fn class_name(c: usize) -> String {
    CLASSES.get(c).map_or(String::new(), |c| String::from_utf8_lossy(&c.class[..c.class.len() - 1]).into_owned())
}

/// A newcomer takes over the avatar of a player who left, instead of a fresh slot next to it.
fn adopt_parked(partners: &mut HashMap<u8, Partner>, id: u8, remote: &[(u8, State)]) {
    let old = partners.iter().find(|(i, p)| p.parked && p.avatar != 0 && !remote.iter().any(|(r, _)| r == *i)).map(|(i, _)| *i);
    let Some(old) = old.and_then(|o| partners.remove(&o).map(|p| (o, p))) else { return };
    let (old_id, p) = old;
    let mut q = Partner::new(p.slot);
    (q.avatar, q.actor_id, q.class, q.parked) = (p.avatar, p.actor_id, p.class, true);
    log!("coop: player {id} takes over slot {} (avatar {:#x}) from player {old_id}", q.slot, q.avatar);
    partners.insert(id, q);
}

/// Leaves a partner idle in its slot. Removing an avatar mid-level drops the player count, and in
/// wt_stuntpark that crashed the game a few seconds later (FINDINGS 10); the level teardown frees it.
fn park(id: u8, p: &mut Partner, why: &str) {
    pad::set_remote_pad(p.slot, None);
    if !p.parked {
        p.parked = true;
        if p.avatar != 0 {
            log!("coop: player {id} {why}: slot {} parked (avatar {:#x})", p.slot, p.avatar);
        }
    }
}

/// Places a bound partner back when it drifted from the owner's position.
unsafe fn correct(p: &mut Partner, id: u8, st: &State, now: u64, delta: f32, mag: f32) {
    let cur = [rd_f32(p.avatar + 0x80), rd_f32(p.avatar + 0x84), rd_f32(p.avatar + 0x88)];
    if now.saturating_sub(p.last_drift_us) >= LOG_US {
        p.last_drift_us = now;
        log!("coop: player {id} slot {} drift {:.1} m, cam delta {:.1} deg, steer {:.2}", p.slot, dist2(cur, st.pos).sqrt(), delta.to_degrees(), mag);
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

fn dist2(a: [f32; 3], b: [f32; 3]) -> f32 {
    (0..3).map(|i| (a[i] - b[i]) * (a[i] - b[i])).sum()
}

/// Spawns an avatar with the synchronous level-setup spawner; returns it (0 on failure).
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
    if avatar != 0 && rd_u32(avatar + 0x17c) & 1 == 0 {
        activate(global(0x1_40b2_bb70), avatar);
        wake(avatar, 0);
    }
    avatar
}

/// Carry state of an avatar as `FLAG_*` bits. `ActorCarriedBy` and `ActorCarried` each keep a
/// pointer at `+0x18` (script bindings 0x1400b0020 / 0x1400b0120); which one means which
/// direction is unconfirmed, so the raw values are logged when the local ones change.
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
    (if by != 0 { FLAG_CARRYING } else { 0 }) | (if carried != 0 { FLAG_CARRIED } else { 0 })
}

/// Yaw of an avatar from its rotation quaternion (`+0x90`).
unsafe fn avatar_yaw(avatar: u64) -> f32 {
    yaw_from_quat(rd_f32(avatar + 0x94), rd_f32(avatar + 0x9c))
}

/// Sets only the rotation: node refresh, matrix from the yaw quaternion, setter, node `vt+0x8`.
unsafe fn set_rotation(actor: u64, yaw: f32) {
    let node = actor + 0x50;
    node_dirty(node);
    let quat = quat_from_yaw(yaw);
    let mut mat = [0f32; 12];
    quat_to_mat(mat.as_mut_ptr(), quat.as_ptr());
    node_set_rot(node, mat.as_ptr());
    vt_call(node, 0x8);
}

/// Position and rotation, without `PrepareForPlacing` (that event may reset carry state).
unsafe fn place_pose(actor: u64, pos: [f32; 3], yaw: f32) {
    for (i, v) in pos.iter().enumerate() {
        std::ptr::write_unaligned((actor + 0x80 + i as u64 * 4) as *mut f32, *v);
    }
    set_rotation(actor, yaw);
}

/// The placing handler's sequence (0x1406a2010): PrepareForPlacing when far, position, rotation, node refresh.
unsafe fn place(actor: u64, actor_id: u64, pos: [f32; 3], yaw: f32) {
    let cur = [rd_f32(actor + 0x80), rd_f32(actor + 0x84), rd_f32(actor + 0x88)];
    if dist2(cur, pos) > PREPARE_DIST * PREPARE_DIST {
        ev5(global(EVENTS), va(0x1_4077_1420), 0, 0, actor_id); // "PrepareForPlacing"
    }
    place_pose(actor, pos, yaw);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::PI;

    /// Smallest absolute difference between two angles.
    fn ang_diff(a: f32, b: f32) -> f32 {
        let d = (a - b).rem_euclid(2.0 * PI);
        d.min(2.0 * PI - d)
    }

    #[test]
    fn yaw_round_trip() {
        for yaw in [0.0, 0.5, -0.5, 1.5, -1.5, PI - 0.01, -PI + 0.01, PI, -PI, 3.0, -3.0] {
            let q = quat_from_yaw(yaw);
            let back = yaw_from_quat(q[1], q[3]);
            assert!(ang_diff(yaw, back) < 1e-5, "{yaw} -> {back}");
        }
        let q = quat_from_yaw(PI / 2.0);
        assert!((q[1] + (PI / 4.0).sin()).abs() < 1e-6 && (q[3] - (PI / 4.0).cos()).abs() < 1e-6);
    }

    #[test]
    fn class_choice() {
        assert_eq!(choose_class(Some(0), &[], 3), 1);
        assert_eq!(choose_class(Some(1), &[], 3), 0);
        assert_eq!(choose_class(Some(0), &[1], 3), 2);
        assert_eq!(choose_class(Some(0), &[1, 2], 3), 1);
        assert_eq!(choose_class(None, &[0], 3), 1);
        assert_eq!(choose_class(None, &[0, 1, 2], 3), 0);
        assert_eq!(choose_class(Some(0), &[], 1), 0);
    }

    #[test]
    fn slots_are_lowest_free() {
        assert_eq!(free_slot(&[]), Some(1));
        assert_eq!(free_slot(&[1]), Some(2));
        assert_eq!(free_slot(&[1, 3]), Some(2));
        assert_eq!(free_slot(&[2, 3]), Some(1));
        assert_eq!(free_slot(&[1, 2, 3]), None);
    }

    #[test]
    fn class_table_is_nul_terminated() {
        for c in &CLASSES {
            assert!(c.class.ends_with(b"\0") && c.spec.ends_with(b"\0") && c.name.ends_with(b"\0"));
        }
        assert_eq!(class_name(1), "woody");
    }

    #[test]
    fn stick_rotation() {
        let near = |a: (i16, i16), b: (i16, i16)| (a.0 as i32 - b.0 as i32).abs() <= 1 && (a.1 as i32 - b.1 as i32).abs() <= 1;
        assert!(near(rotate_stick(32767, 0, PI / 2.0), (0, 32767)));
        assert!(near(rotate_stick(0, 32767, PI / 2.0), (-32767, 0)));
        assert!(near(rotate_stick(32767, 0, PI), (-32767, 0)));
        assert!(near(rotate_stick(12345, -6789, 0.0), (12345, -6789)));
        assert!(near(rotate_stick(3000, 4000, -0.7), rotate_stick(3000, 4000, 2.0 * PI - 0.7)));
        // a full-deflection corner stays in range
        let (x, y) = rotate_stick(-32768, -32768, 0.5);
        assert!(x != 0 && y != 0);
        let (x, y) = rotate_stick(32767, 32767, PI / 4.0);
        assert!(x.abs() <= 1 && y == 32767); // clamped, not wrapped
    }

    #[test]
    fn pad_rotation_keeps_buttons_and_triggers() {
        let p = Pad { buttons: 0x1234, lt: 9, rt: 8, lx: 32767, ly: 0, rx: 0, ry: -32767 };
        let r = rotate_pad(p, PI / 2.0);
        assert_eq!((r.buttons, r.lt, r.rt), (0x1234, 9, 8));
        assert!(r.lx.abs() <= 1 && (r.ly - 32767).abs() <= 1);
        assert!((r.rx - 32767).abs() <= 1 && r.ry.abs() <= 1);
        assert_eq!(rotate_pad(p, 0.0), p);
        assert_eq!(rotate_pad(p, f32::NAN), p);
    }

    #[test]
    fn camera_delta_and_sign() {
        assert_eq!(stick_delta(Some(1.0), 0.25, 1), 0.75);
        assert_eq!(stick_delta(Some(1.0), 0.25, -1), -0.75);
        assert_eq!(stick_delta(None, 0.25, 1), 0.0);
        assert_eq!(stick_delta(Some(1.0), f32::NAN, 1), 0.0);
        assert_eq!(stick_delta(Some(1.0), f32::INFINITY, 1), 0.0);
    }

    fn near(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-4
    }

    #[test]
    fn velocity_from_two_states() {
        let v = velocity_xz([0.0, 5.0, 0.0], 1_000_000, [1.0, 9.0, -0.5], 1_100_000).unwrap_or_default();
        assert!(near(v[0], 10.0) && near(v[1], -5.0)); // y is ignored
        assert_eq!(velocity_xz([0.0; 3], 100, [1.0; 3], 600), None);
        assert_eq!(velocity_xz([0.0; 3], 500, [1.0; 3], 100), None);
    }

    #[test]
    fn partner_tracks_once_per_sequence_and_goes_stale() {
        let mut p = Partner::new(1);
        p.track(1, [0.0; 3], 0);
        assert_eq!(p.velocity(0), [0.0; 2]);
        p.track(2, [0.2, 0.0, 0.0], 20_000);
        p.track(2, [9.0, 0.0, 0.0], 30_000); // same seq: ignored
        assert!(near(p.velocity(30_000)[0], 10.0));
        assert_eq!(p.velocity(20_000 + STALE_US + 1), [0.0; 2]);
    }

    #[test]
    fn steering_decisions() {
        // far: chase the target, at least as fast as the owner stick
        match decide_steer([3.0, 0.0], [0.0; 2], 0.2) {
            Steer::Dir(d, m) => assert!(near(d[0], 1.0) && near(d[1], 0.0) && near(m, 1.0)),
            other => panic!("{other:?}"),
        }
        match decide_steer([0.0, -0.6], [0.0; 2], 0.1) {
            Steer::Dir(d, m) => assert!(near(d[1], -1.0) && near(m, 0.4)),
            other => panic!("{other:?}"),
        }
        match decide_steer([0.0, 1.5], [0.0; 2], 0.9) {
            Steer::Dir(_, m) => assert!(near(m, 1.0)),
            other => panic!("{other:?}"),
        }
        // near and the owner is moving: follow its velocity
        match decide_steer([0.1, 0.0], [0.0, 3.0], 0.8) {
            Steer::Dir(d, m) => assert!(near(d[1], 1.0) && near(m, 0.8)),
            other => panic!("{other:?}"),
        }
        // near and slow: idle or the raw stick
        assert_eq!(decide_steer([0.1, 0.0], [0.1, 0.0], 0.1), Steer::Idle);
        assert_eq!(decide_steer([0.1, 0.0], [0.1, 0.0], 0.5), Steer::Raw);
        assert_eq!(decide_steer([f32::NAN, 0.0], [0.0; 2], 0.5), Steer::Idle);
    }

    #[test]
    fn world_to_stick_mapping() {
        // a view looking along +z with right = -x (any orthonormal pair works)
        let (right, look) = ([-1.0, 0.0], [0.0, 1.0]);
        assert_eq!(world_to_stick([0.0, 1.0], 1.0, right, look, 1, 1), (0, 32767));
        assert_eq!(world_to_stick([-1.0, 0.0], 0.5, right, look, 1, 1), (16384, 0));
        assert_eq!(world_to_stick([0.0, 1.0], 1.0, right, look, 1, -1), (0, -32767));
        assert_eq!(world_to_stick([-1.0, 0.0], 1.0, right, look, -1, 1), (-32767, 0));
        // magnitude over 1 clamps, never wraps
        assert_eq!(world_to_stick([0.0, 1.0], 3.0, right, look, 1, 1), (0, 32767));
        let (x, y) = world_to_stick([0.6, 0.8], 1.0, [1.0, 0.0], [0.0, 1.0], 1, 1);
        assert!((x - 19660).abs() <= 1 && (y - 26214).abs() <= 1);
    }

    #[test]
    fn steered_pad_keeps_buttons_and_centres_the_right_stick() {
        let owner = Pad { buttons: 0x1000, lt: 4, rt: 5, lx: 0, ly: 0, rx: 1000, ry: -2000 };
        let (p, mag) = steer_pad(owner, [2.0, 0.0], [0.0; 2], [1.0, 0.0], [0.0, 1.0], 0.0, 1, 1);
        assert_eq!((p.buttons, p.lt, p.rt, p.rx, p.ry), (0x1000, 4, 5, 0, 0));
        assert_eq!((p.lx, p.ly), (32767, 0));
        assert!(near(mag, 1.0));
        let moving = Pad { lx: 0, ly: 32767, ..owner };
        let (p, _) = steer_pad(moving, [0.0, 0.1], [0.0; 2], [1.0, 0.0], [0.0, 1.0], std::f32::consts::FRAC_PI_2, 1, 1);
        assert!(p.lx < -32000 && p.ly.abs() <= 1); // raw stick, rotated by delta
        let (p, mag) = steer_pad(owner, [0.0, 0.1], [0.0; 2], [1.0, 0.0], [0.0, 1.0], 0.0, 1, 1);
        assert_eq!((p.lx, p.ly, mag), (0, 0, 0.0));
    }

    #[test]
    fn button_edges() {
        let pad = |buttons: u16, lt: u8, rt: u8| Pad { buttons, lt, rt, ..Pad::default() };
        let none = pressed_bits(&pad(0, 0, 0));
        let x = pressed_bits(&pad(0x4000, 0, 0));
        assert_eq!(x, 0x4000);
        assert_eq!(rising(none, x), 0x4000);
        assert_eq!(rising(x, x), 0); // held
        assert_eq!(rising(x, none), 0); // release is not a press
        assert_eq!(rising(x, pressed_bits(&pad(0x4000 | 0x1000, 0, 0))), 0x1000);
        // d-pad, start and thumb clicks are not interactions
        assert_eq!(pressed_bits(&pad(0x0001 | 0x0010 | 0x0040, 0, 0)), 0);
        // triggers count from 128
        assert_eq!(pressed_bits(&pad(0, 127, 127)), 0);
        assert_eq!(pressed_bits(&pad(0, 128, 0)), LT_BIT);
        assert_eq!(rising(none, pressed_bits(&pad(0, 0, 255))), RT_BIT);
        assert_eq!(pressed_bits(&pad(0x0100 | 0x0200, 0, 0)), 0x0300);
    }

    #[test]
    fn yaw_difference_wraps() {
        assert!(near(yaw_diff(0.1, -0.1), 0.2));
        assert!(near(yaw_diff(PI - 0.1, -PI + 0.1), 0.2));
        assert!(near(yaw_diff(-PI + 0.1, PI - 0.1), 0.2));
        assert!(near(yaw_diff(3.0 * PI, PI), 0.0));
        assert!(near(yaw_diff(0.0, PI), PI));
        assert!(yaw_diff(0.0, 0.1) < FACE_TOLERANCE && yaw_diff(0.0, 0.2) > FACE_TOLERANCE);
    }

    #[test]
    fn carry_watch_logs_once_after_half_a_second() {
        let mut w = CarryWatch::default();
        assert!(!w.check(0, 0, 0));
        assert!(!w.check(0, 1, 1_000));
        assert!(!w.check(0, 1, 400_000));
        assert!(w.check(0, 1, 501_000));
        assert!(!w.check(0, 1, 900_000)); // already logged
        assert!(!w.check(0, 2, 1_000_000)); // a new disagreement restarts the clock
        assert!(w.check(0, 2, 1_500_000));
        assert!(!w.check(2, 2, 1_600_000)); // agreement resets
        assert!(!w.check(0, 1, 1_700_000));
        assert!(w.check(0, 1, 2_200_000));
    }

    #[test]
    fn menus_are_not_play_levels() {
        assert!(is_play_level("westerntown"));
        assert!(is_play_level("st_train"));
        assert!(!is_play_level("gui_storymode_boardgame"));
        assert!(!is_play_level("GUI_ToyBarn"));
        assert!(!is_play_level("frontend"));
        assert!(!is_play_level(""));
        assert!(is_play_level("gu"));
    }

    #[test]
    fn follow_needs_a_lasting_mismatch_between_two_levels() {
        let (toybox, daycare) = (level_hash("ST_ToyBox"), level_hash("ST_Daycare"));
        let (long, never) = (Duration::from_secs(2), Duration::MAX);
        let go = |l, h, n: Option<&str>, m, s| decide_follow(l, h, n, m, s);
        assert_eq!(go(toybox, daycare, Some("ST_Daycare"), long, never).as_deref(), Some("ST_Daycare"));
        // too soon, or cooling down after a switch
        assert_eq!(go(toybox, daycare, Some("ST_Daycare"), Duration::from_millis(1499), never), None);
        assert_eq!(go(toybox, daycare, Some("ST_Daycare"), FOLLOW_AFTER, never).as_deref(), Some("ST_Daycare"));
        assert_eq!(go(toybox, daycare, Some("ST_Daycare"), long, Duration::from_millis(14_999)), None);
        assert!(go(toybox, daycare, Some("ST_Daycare"), long, FOLLOW_COOLDOWN).is_some());
        // front end or loading on either side, or the same level
        assert_eq!(go(0, daycare, Some("ST_Daycare"), long, never), None);
        assert_eq!(go(toybox, 0, Some(""), long, never), None);
        assert_eq!(go(toybox, toybox, Some("ST_ToyBox"), long, never), None);
        // the reported name must be the one the state hash belongs to
        assert_eq!(go(toybox, daycare, None, long, never), None);
        assert_eq!(go(toybox, daycare, Some("ST_Attic"), long, never), None);
        assert_eq!(go(toybox, daycare, Some("st_daycare"), long, never).as_deref(), Some("st_daycare"));
    }

    #[test]
    fn distances() {
        assert_eq!(dist2([0.0; 3], [1.0, 2.0, 2.0]), 9.0);
    }
}
