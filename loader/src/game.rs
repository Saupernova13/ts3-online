//! Access to the game's own memory and functions. All addresses are VAs of the analysed exe
//! (image base 0x140000000) and are rebased with the runtime module base.
use std::ffi::c_char;
use std::sync::atomic::{AtomicU64, Ordering};

pub const IMAGE_BASE: u64 = 0x1_4000_0000;

static MODULE_BASE: AtomicU64 = AtomicU64::new(0);

/// Must be called before patches are applied.
pub fn set_module_base(base: u64) {
    MODULE_BASE.store(base, Ordering::Release);
}

pub fn va(v: u64) -> u64 {
    MODULE_BASE.load(Ordering::Acquire) + (v - IMAGE_BASE)
}

pub const MGR_PTR: u64 = 0x1_40b2_ef50;
pub const INPUT_PTR: u64 = 0x1_40ac_5380;
pub const EVENTS: u64 = 0x1_40b2_9a70;
pub const FLAGS: u64 = 0x1_40b2_ac90;
pub const RIDES: u64 = 0x1_4099_b8b0;
pub const STREAMING: u64 = 0x1_40b2_ef20;
pub const UI_PTR: u64 = 0x1_40b2_ef10;

pub const SLOT_AVATAR_BASE: u64 = 0x1a0;
pub const SLOT_STRIDE: u64 = 0x58;

pub unsafe fn rd_u64(addr: u64) -> u64 {
    std::ptr::read_unaligned(addr as *const u64)
}

pub unsafe fn rd_u32(addr: u64) -> u32 {
    std::ptr::read_unaligned(addr as *const u32)
}

pub unsafe fn rd_i32(addr: u64) -> i32 {
    std::ptr::read_unaligned(addr as *const i32)
}

/// Value of a game global (a pointer stored at the given VA).
pub unsafe fn global(v: u64) -> u64 {
    rd_u64(va(v))
}

/// Avatar pointer in player slot `i` of the manager (0 if none).
pub unsafe fn slot_avatar(mgr: u64, i: u64) -> u64 {
    rd_u64(mgr + SLOT_AVATAR_BASE + i * SLOT_STRIDE)
}

macro_rules! game_fn {
    ($name:ident, $va:expr, fn($($a:ident: $t:ty),*) -> $r:ty) => {
        pub unsafe fn $name($($a: $t),*) -> $r {
            let f: extern "system" fn($($t),*) -> $r = std::mem::transmute(va($va) as usize);
            f($($a),*)
        }
    };
}

game_fn!(assign_avatar, 0x1_405f_1030, fn(mgr: u64, idx: u32, avatar: u64) -> ());
game_fn!(ev5, 0x1_4049_e0b0, fn(events: u64, name: u64, a: u64, b: u64, id: u64) -> ());
game_fn!(ev4, 0x1_4049_e160, fn(events: u64, name: u64, id: u64, b: u64) -> ());
game_fn!(hash, 0x1_404f_dbb0, fn(v: u32) -> u64);
game_fn!(flagtest, 0x1_404f_ac20, fn(flags: u64, hb: u64, h: u64) -> u8);
game_fn!(lvltest, 0x1_4050_2bb0, fn(flags: u64, level_info: u64, h: u64) -> u8);
game_fn!(autodrive, 0x1_4019_b110, fn(rides: u64, idx: u32, avatar: u64, start: u64) -> ());
game_fn!(streaming, 0x1_405d_6ae0, fn(s: u64, idx: u32, level_info: u64, flag: u8) -> ());
game_fn!(join_cancel_tail, 0x1_400e_7450, fn(screen: u64) -> u64);
game_fn!(can_join, 0x1_405c_e0e0, fn(mgr: u64) -> u8);
game_fn!(post_event, 0x1_4047_8530, fn(name: u64, arg: u64) -> ());
game_fn!(level_name, 0x1_405b_b6a0, fn() -> u64);
game_fn!(get_screen, 0x1_404b_26c0, fn(ui: u64, name: u64) -> u64);
game_fn!(push_screen, 0x1_405d_6590, fn(screen: u64, idx: u32) -> u8);
game_fn!(registry_get, 0x1_4055_8400, fn(entry: u64) -> u64);
game_fn!(prop_string, 0x1_404f_dff0, fn(obj: u64, name: u64, flag: u8) -> u64);

/// Calls `this->vtable[off](this)` returning AL.
pub unsafe fn vt_u8(this: u64, off: u64) -> u8 {
    let f: extern "system" fn(u64) -> u8 = std::mem::transmute(rd_u64(rd_u64(this) + off) as usize);
    f(this)
}

/// Calls `this->vtable[off](this)` returning EAX.
pub unsafe fn vt_u32(this: u64, off: u64) -> u32 {
    let f: extern "system" fn(u64) -> u32 = std::mem::transmute(rd_u64(rd_u64(this) + off) as usize);
    f(this)
}

/// Input driver vtable +0x80: assign(this, player, device).
pub unsafe fn input_assign(input: u64, player: u32, device: u32) {
    let f: extern "system" fn(u64, u32, u32) = std::mem::transmute(rd_u64(rd_u64(input) + 0x80) as usize);
    f(input, player, device)
}

/// Input driver vtable +0x88: release(this, player).
pub unsafe fn input_release(input: u64, player: u32) {
    let vt = rd_u64(input);
    let f: extern "system" fn(u64, u32) = std::mem::transmute(rd_u64(vt + 0x88) as usize);
    f(input, player)
}

/// Input driver vtable +0x90: is_assigned(this, player) -> AL.
pub unsafe fn input_is_assigned(input: u64, player: u32) -> u8 {
    let vt = rd_u64(input);
    let f: extern "system" fn(u64, u32) -> u8 = std::mem::transmute(rd_u64(vt + 0x90) as usize);
    f(input, player)
}

/// The game's imported `_stricmp` (pointer stored in its IAT at VA 0x140759a00).
pub unsafe fn stricmp(a: *const c_char, b: *const c_char) -> i32 {
    let f: extern "C" fn(*const c_char, *const c_char) -> i32 = std::mem::transmute(rd_u64(va(0x1_4075_9a00)) as usize);
    f(a, b)
}
