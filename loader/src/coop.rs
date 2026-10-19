//! Online co-op: every remote player is a real local partner in slots 1-3, driven by a virtual
//! XInput pad fed from the network, so the game's own co-op code runs unchanged (FINDINGS 9).
use crate::game::*;
use crate::hooks::bind_joining_avatar;
use crate::log;
use crate::net::proto::{level_hash, Pad, State, CLASS_UNKNOWN};
use crate::{net, pad};
use std::collections::HashMap;
use std::ffi::{c_char, CStr};
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

const RETRY_US: u64 = 2_000_000;
const LOG_US: u64 = 1_000_000;
/// Farther than this from the owner's position, a partner is placed back.
const SNAP_DIST: f32 = 3.0;
/// Farther than this, a placement fires `PrepareForPlacing` first.
const PREPARE_DIST: f32 = 2.0;
/// Local slots for remote players.
const SLOTS: [usize; 3] = [1, 2, 3];
/// Avatar byte that says whether controls are enabled.
const CONTROLS_ON: u64 = 0x428;

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
}

static PARTNERS: Mutex<Option<HashMap<u8, Partner>>> = Mutex::new(None);
static START: OnceLock<Instant> = OnceLock::new();

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

/// Index in `CLASSES` of an avatar's class property, if known.
unsafe fn class_of(avatar: u64) -> Option<usize> {
    let cls = prop_string(avatar + 0x2e0, b"class\0".as_ptr() as u64, 0);
    if cls == 0 {
        return None;
    }
    CLASSES.iter().position(|c| stricmp(cls as *const c_char, c.class.as_ptr() as *const c_char) == 0)
}

/// Hash of the current level's name, 0 outside a level.
unsafe fn local_level() -> u32 {
    let name = level_name();
    if name == 0 {
        return 0;
    }
    level_hash(&CStr::from_ptr(name as *const c_char).to_string_lossy())
}

fn forget_pads() {
    for s in SLOTS {
        pad::set_remote_pad(s, None);
    }
}

/// Per-frame work on the game thread: publish the local player, bind, feed and correct partners.
pub unsafe fn update() {
    let Ok(mut guard) = PARTNERS.lock() else { return };
    let partners = guard.get_or_insert_with(HashMap::new);
    let mgr = global(MGR_PTR);
    let p0 = if mgr != 0 { slot_avatar(mgr, 0) } else { 0 };

    let held = pad::take_local_pad();
    if p0 == 0 {
        net::set_local_state(0, CLASS_UNKNOWN, [0.0; 3], 0.0, Pad::default(), 0);
        // the level teardown already reset slots 1-3
        partners.clear();
        forget_pads();
        return;
    }
    let level = local_level();
    let own_class = class_of(p0);
    let pos = [rd_f32(p0 + 0x80), rd_f32(p0 + 0x84), rd_f32(p0 + 0x88)];
    let yaw = yaw_from_quat(rd_f32(p0 + 0x94), rd_f32(p0 + 0x9c));
    let controls = rd_u8(p0 + CONTROLS_ON) != 0;
    let pad_out = if controls { held } else { Pad::default() };
    net::set_local_state(level, own_class.map_or(CLASS_UNKNOWN, |c| c as u8), pos, yaw, pad_out, 0);

    let now = START.get_or_init(Instant::now).elapsed().as_micros() as u64;
    let remote = net::remote_states();
    let gone: Vec<u8> = partners.keys().copied().filter(|id| !remote.iter().any(|(r, _)| r == id)).collect();
    for id in gone {
        if let Some(p) = partners.remove(&id) {
            unbind(mgr, id, &p, "left");
        }
    }
    for (id, st) in remote {
        let taken: Vec<usize> = partners.values().map(|p| p.slot).collect();
        let used: Vec<usize> = partners.iter().filter(|(i, p)| **i != id && p.avatar != 0).map(|(_, p)| p.class).collect();
        let p = match partners.get_mut(&id) {
            Some(p) => p,
            None => {
                let Some(slot) = free_slot(&taken) else { continue };
                partners.entry(id).or_insert(Partner { slot, avatar: 0, actor_id: 0, class: 0, next_try_us: 0, snaps: 0, last_log_us: 0 })
            }
        };
        if st.level != level || level == 0 {
            if p.avatar != 0 {
                unbind(mgr, id, p, "is in another level");
                p.avatar = 0;
            }
            pad::set_remote_pad(p.slot, None);
            continue;
        }
        pad::set_remote_pad(p.slot, Some(st.pad));
        if p.avatar == 0 {
            if now >= p.next_try_us {
                p.next_try_us = now + RETRY_US;
                bind(mgr, id, p, &st, own_class, &used);
            }
        } else if slot_avatar(mgr, p.slot as u64) != p.avatar {
            log!("coop: player {id} lost slot {} (avatar {:#x}); will rebind", p.slot, p.avatar);
            p.avatar = 0;
        } else {
            correct(p, id, &st, now);
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

/// Leaves the slot the way the game does for a leaving player, then removes the avatar.
unsafe fn unbind(mgr: u64, id: u8, p: &Partner, why: &str) {
    pad::set_remote_pad(p.slot, None);
    if p.avatar == 0 {
        return;
    }
    if slot_avatar(mgr, p.slot as u64) == p.avatar {
        assign_avatar(mgr, p.slot as u32, 0);
        if registry_get(p.actor_id) == p.avatar {
            remove_avatar(p.avatar);
        }
        log!("coop: player {id} {why}: slot {} released, avatar {:#x} removed", p.slot, p.avatar);
    } else {
        log!("coop: player {id} {why}: slot {} no longer holds avatar {:#x}", p.slot, p.avatar);
    }
}

/// Places a bound partner back when it drifted from the owner's position.
unsafe fn correct(p: &mut Partner, id: u8, st: &State, now: u64) {
    let cur = [rd_f32(p.avatar + 0x80), rd_f32(p.avatar + 0x84), rd_f32(p.avatar + 0x88)];
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

/// The placing handler's sequence (0x1406a2010): PrepareForPlacing when far, position, rotation, node refresh.
unsafe fn place(actor: u64, actor_id: u64, pos: [f32; 3], yaw: f32) {
    let cur = [rd_f32(actor + 0x80), rd_f32(actor + 0x84), rd_f32(actor + 0x88)];
    if dist2(cur, pos) > PREPARE_DIST * PREPARE_DIST {
        ev5(global(EVENTS), va(0x1_4077_1420), 0, 0, actor_id); // "PrepareForPlacing"
    }
    for (i, v) in pos.iter().enumerate() {
        std::ptr::write_unaligned((actor + 0x80 + i as u64 * 4) as *mut f32, *v);
    }
    let node = actor + 0x50;
    node_dirty(node);
    let quat = quat_from_yaw(yaw);
    let mut mat = [0f32; 12];
    quat_to_mat(mat.as_mut_ptr(), quat.as_ptr());
    node_set_rot(node, mat.as_ptr());
    vt_call(node, 0x8);
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
    fn distances() {
        assert_eq!(dist2([0.0; 3], [1.0, 2.0, 2.0]), 9.0);
    }
}
