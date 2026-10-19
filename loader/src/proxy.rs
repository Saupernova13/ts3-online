//! Proxy avatars: other players' network state drives spare avatars placed every frame (FINDINGS 9).
use crate::game::*;
use crate::log;
use crate::net;
use std::collections::HashMap;
use std::ffi::c_char;
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

const RESPAWN_US: u64 = 2_000_000;
/// Farther than this from its target, a proxy is re-placed with `PrepareForPlacing` first.
const SNAP_DIST: f32 = 2.0;

struct Class {
    /// NUL-terminated class name, as stored in the avatar's properties.
    class: &'static [u8],
    spec: &'static [u8],
    name: &'static [u8],
    hash: u32,
}

const CLASSES: [Class; 3] = [
    Class { class: b"buzz\0", spec: b"class=buzz; model_name=characters/BuzzLightYear/BuzzLightYear\0", name: b"Buzz2\0", hash: 0xcc },
    Class { class: b"woody\0", spec: b"class=woody; model_name=characters/Woody/Woody\0", name: b"Woody2\0", hash: 0xd4 },
    Class { class: b"jessie\0", spec: b"class=jessie; model_name=characters/Jessie/Jessie\0", name: b"Jessie2\0", hash: 0xcf },
];

struct Proxy {
    actor_id: u64,
    next_spawn_us: u64,
    /// Not yet placed since the spawn: the first placement fires `PrepareForPlacing`.
    fresh: bool,
    class: usize,
}

static PROXIES: Mutex<Option<HashMap<u8, Proxy>>> = Mutex::new(None);
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

/// Class index for a new proxy: not the local avatar's, not another live proxy's.
pub fn choose_class(own: Option<usize>, used: &[usize], count: usize) -> usize {
    (0..count)
        .find(|c| Some(*c) != own && !used.contains(c))
        .or_else(|| (0..count).find(|c| Some(*c) != own))
        .unwrap_or(0)
}

/// Index in `CLASSES` of an avatar's class property, if known.
unsafe fn class_of(avatar: u64) -> Option<usize> {
    let cls = prop_string(avatar + 0x2e0, b"class\0".as_ptr() as u64, 0);
    if cls == 0 {
        return None;
    }
    CLASSES.iter().position(|c| stricmp(cls as *const c_char, c.class.as_ptr() as *const c_char) == 0)
}

/// Per-frame work on the game thread: publish slot 0's transform, spawn and place proxies.
pub unsafe fn update() {
    let Ok(mut guard) = PROXIES.lock() else { return };
    let proxies = guard.get_or_insert_with(HashMap::new);
    let mgr = global(MGR_PTR);
    let p0 = if mgr != 0 { slot_avatar(mgr, 0) } else { 0 };
    if p0 == 0 {
        proxies.clear(); // the actors went with the level
        return;
    }
    let pos = [rd_f32(p0 + 0x80), rd_f32(p0 + 0x84), rd_f32(p0 + 0x88)];
    net::set_local_state(pos, yaw_from_quat(rd_f32(p0 + 0x94), rd_f32(p0 + 0x9c)), 0);

    let now = START.get_or_init(Instant::now).elapsed().as_micros() as u64;
    let remote = net::remote_states();
    let gone: Vec<u8> = proxies.keys().copied().filter(|id| !remote.iter().any(|(r, _)| r == id)).collect();
    for id in gone {
        if let Some(p) = proxies.remove(&id) {
            let actor = if p.actor_id != 0 { registry_get(p.actor_id) } else { 0 };
            if actor != 0 {
                remove_avatar(actor);
            }
            log!("proxy: removed player {id} (actor {:#x})", p.actor_id);
        }
    }
    for (id, st) in remote {
        let used: Vec<usize> = proxies.values().filter(|p| p.actor_id != 0).map(|p| p.class).collect();
        let p = proxies.entry(id).or_insert(Proxy { actor_id: 0, next_spawn_us: 0, fresh: true, class: 0 });
        let mut actor = if p.actor_id != 0 { registry_get(p.actor_id) } else { 0 };
        if actor == 0 && now >= p.next_spawn_us {
            p.next_spawn_us = now + RESPAWN_US;
            p.class = choose_class(class_of(p0), &used, CLASSES.len());
            p.actor_id = spawn(mgr, &CLASSES[p.class], st.pos, st.yaw);
            p.fresh = true;
            actor = if p.actor_id != 0 { registry_get(p.actor_id) } else { 0 };
            log!("proxy: spawn player {id} as {} -> actor {:#x}", String::from_utf8_lossy(&CLASSES[p.class].class[..CLASSES[p.class].class.len() - 1]), p.actor_id);
        }
        if actor != 0 {
            place(actor, p.actor_id, p.fresh, st.pos, st.yaw);
            p.fresh = false;
        }
    }
}

/// Spawns an avatar with the synchronous level-setup spawner; returns its actor id (0 on failure).
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
    if avatar == 0 {
        return 0;
    }
    if rd_u32(avatar + 0x17c) & 1 == 0 {
        activate(global(0x1_40b2_bb70), avatar);
        wake(avatar, 0);
    }
    vcall_u8(avatar, 0x148, 0); // controls off: its KinematicDriver runs
    rd_u64(avatar + 0x38)
}

/// The placing handler's sequence (0x1406a2010): optional PrepareForPlacing, position, rotation, node refresh.
unsafe fn place(actor: u64, actor_id: u64, fresh: bool, pos: [f32; 3], yaw: f32) {
    let cur = [rd_f32(actor + 0x80), rd_f32(actor + 0x84), rd_f32(actor + 0x88)];
    let d2: f32 = (0..3).map(|i| (cur[i] - pos[i]) * (cur[i] - pos[i])).sum();
    if fresh || d2 > SNAP_DIST * SNAP_DIST {
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
    fn class_table_is_nul_terminated() {
        for c in &CLASSES {
            assert!(c.class.ends_with(b"\0") && c.spec.ends_with(b"\0") && c.name.ends_with(b"\0"));
        }
    }
}
