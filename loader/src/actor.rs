//! A game actor's memory layout (FINDINGS 9, 10) and the reads and writes every sync module shares.
use crate::game::*;
use crate::math::{quat_from_yaw, yaw_from_quat};
use std::ffi::{c_char, CStr};

/// Actor id, the key of the registry (`registry_get`).
pub const ID: u64 = 0x38;
/// Scene node; its rotation setter and refresh live on the node.
pub const NODE: u64 = 0x50;
/// Position as three f32.
pub const POS: u64 = 0x80;
/// Rotation quaternion `(x, y, z, w)`.
pub const ROT: u64 = 0x90;
/// Reference name pointer, unique and equal across copies for most placed actors.
pub const REF: u64 = 0x150;
/// Bit 0 set once the actor is activated.
pub const ACTIVE_FLAGS: u64 = 0x17c;
/// Property bag (`prop_string` reads `class` from it).
pub const PROPS: u64 = 0x2e0;
/// Avatar: flag set checked by the join flow.
pub const AVATAR_FLAGS: u64 = 0x408;
/// Avatar: nonzero while controls are enabled.
pub const CONTROLS_ON: u64 = 0x428;
/// Avatar: owning player index, -1 for none.
pub const PLAYER_INDEX: u64 = 0x484;

/// 0 for a null actor.
pub unsafe fn id(actor: u64) -> u64 {
    if actor == 0 {
        0
    } else {
        rd_u64(actor + ID)
    }
}

pub unsafe fn pos(actor: u64) -> [f32; 3] {
    [rd_f32(actor + POS), rd_f32(actor + POS + 4), rd_f32(actor + POS + 8)]
}

pub unsafe fn yaw(actor: u64) -> f32 {
    yaw_from_quat(rd_f32(actor + ROT + 4), rd_f32(actor + ROT + 12))
}

pub unsafe fn pose(actor: u64) -> ([f32; 3], f32) {
    (pos(actor), yaw(actor))
}

/// None when the actor has no reference name.
pub unsafe fn refname(actor: u64) -> Option<String> {
    let p = rd_u64(actor + REF);
    (p != 0).then(|| CStr::from_ptr(p as *const c_char).to_string_lossy().into_owned())
}

pub unsafe fn controls_enabled(avatar: u64) -> bool {
    rd_u8(avatar + CONTROLS_ON) != 0
}

/// Rotation only: node refresh, matrix from the yaw quaternion, setter, node `vt+0x8`.
pub unsafe fn set_rotation(actor: u64, yaw: f32) {
    let node = actor + NODE;
    node_dirty(node);
    let quat = quat_from_yaw(yaw);
    let mut mat = [0f32; 12];
    quat_to_mat(mat.as_mut_ptr(), quat.as_ptr());
    node_set_rot(node, mat.as_ptr());
    vt_call(node, 0x8);
}

/// Writes the position directly and sets the rotation; skips `PrepareForPlacing`, which may reset
/// carry state.
pub unsafe fn place_pose(actor: u64, pos: [f32; 3], yaw: f32) {
    for (i, v) in pos.iter().enumerate() {
        std::ptr::write_unaligned((actor + POS + i as u64 * 4) as *mut f32, *v);
    }
    set_rotation(actor, yaw);
}
