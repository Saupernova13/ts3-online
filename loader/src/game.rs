//! Access to the game's own memory and functions. All addresses are VAs of the analysed exe
//! (image base 0x140000000) and are rebased with the runtime module base.
use std::ffi::{c_char, CString};
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
/// Stats table: 0x60-byte entries at `+8`, count at `+0x10`.
pub const STATS_PTR: u64 = 0x1_40b2_fdb8;

pub const SLOT_AVATAR_BASE: u64 = 0x1a0;
pub const SLOT_STRIDE: u64 = 0x58;

/// Never freed, so the game may keep the pointer.
pub fn leak_cstr(s: &str) -> u64 {
    let c = CString::new(s.replace('\0', "")).unwrap_or_default();
    Box::leak(c.into_boxed_c_str()).as_ptr() as u64
}

pub unsafe fn rd_u64(addr: u64) -> u64 {
    std::ptr::read_unaligned(addr as *const u64)
}

pub unsafe fn rd_u32(addr: u64) -> u32 {
    std::ptr::read_unaligned(addr as *const u32)
}

pub unsafe fn rd_i32(addr: u64) -> i32 {
    std::ptr::read_unaligned(addr as *const i32)
}

pub unsafe fn rd_f32(addr: u64) -> f32 {
    std::ptr::read_unaligned(addr as *const f32)
}

pub unsafe fn rd_u8(addr: u64) -> u8 {
    std::ptr::read(addr as *const u8)
}

pub unsafe fn vcall_u8(this: u64, off: u64, a: u8) {
    let f: extern "system" fn(u64, u8) = std::mem::transmute(rd_u64(rd_u64(this) + off) as usize);
    f(this, a)
}

pub unsafe fn wr_u8(addr: u64, v: u8) {
    std::ptr::write(addr as *mut u8, v)
}

pub unsafe fn wr_i32(addr: u64, v: i32) {
    std::ptr::write_unaligned(addr as *mut i32, v)
}

pub unsafe fn wr_u64(addr: u64, v: u64) {
    std::ptr::write_unaligned(addr as *mut u64, v)
}

/// Player-index field at the start of each manager slot; level setup only fills slots 0 and 1.
pub const SLOT_INDEX_BASE: u64 = 0x198;

pub unsafe fn global(v: u64) -> u64 {
    rd_u64(va(v))
}

/// 0 if the slot has no avatar.
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
game_fn!(slot_init, 0x1_405f_0cf0, fn(slot: u64, avatar: u64) -> ());
game_fn!(player_setup, 0x1_405b_0420, fn(mgr: u64, idx: u32) -> ());
game_fn!(views_finish, 0x1_404c_e950, fn(views: u64) -> ());
game_fn!(control_mapper, 0x1_4055_9010, fn(actor: u64) -> u64);
game_fn!(cstack_pop, 0x1_4028_0930, fn(stacks: u64, entry: u64, player: u32) -> u64);
game_fn!(cstack_unregister, 0x1_4028_1470, fn(stacks: u64, entry: u64, player: u32, flag: u8) -> ());
game_fn!(component_ref, 0x1_4055_fd40, fn(actor: u64, name: u64) -> u64);
game_fn!(component_set, 0x1_4045_8040, fn(comp: u64, value: u64) -> ());
game_fn!(find_component, 0x1_4055_8fb0, fn(actor: u64, name: u64) -> u64);
game_fn!(camera_state_set_view, 0x1_4066_d8c0, fn(state: u64, view: u64) -> ());
game_fn!(view_find_camera, 0x1_403f_c110, fn(view: u64, name: u64) -> u64);
game_fn!(view_pop_camera, 0x1_403f_f140, fn(view: u64, camera: u64) -> u64);
game_fn!(view_remove_camera, 0x1_403f_ffc0, fn(view: u64, camera: u64) -> ());
game_fn!(list_clear, 0x1_403f_b1e0, fn(list: u64) -> ());
game_fn!(stat_find, 0x1_4061_7c40, fn(stats: u64, name: *const c_char) -> i32);
game_fn!(stat_add, 0x1_4063_2130, fn(stats: u64, idx: i32, delta: i32) -> ());
game_fn!(stat_get, 0x1_4061_7bc0, fn(stats: u64, idx: i32) -> i32);
game_fn!(find_actor_by_name, 0x1_4055_7e10, fn(mgr: u64, name: *const c_char) -> u64);
game_fn!(find_actor_by_ref, 0x1_4055_7f60, fn(mgr: u64, name: *const c_char) -> u64);
game_fn!(component, 0x1_4055_8fb0, fn(actor: u64, name: *const c_char) -> u64);
game_fn!(exploder_break, 0x1_4065_2ac0, fn(exploder: u64) -> ());

/// The game timer gameplay and scripted sequences run on (FINDINGS 10); an object, not a pointer.
pub const GAME_TIMER: u64 = 0x1_4095_4820;
/// `timer_set_speed` speeds: 0 stops the timer, 9 is normal.
pub const TIMER_STOPPED: u32 = 0;
pub const TIMER_NORMAL: u32 = 9;
/// The sound system the pause handlers pause and resume.
pub const AUDIO_PTR: u64 = 0x1_40ac_2ec0;
game_fn!(timer_set_speed, 0x1_4047_9c00, fn(timer: u64, speed: u32, flag: u8) -> ());
game_fn!(timer_sync, 0x1_4047_79d0, fn(timer: u64) -> ());
game_fn!(set_timer_scaled, 0x1_404c_7620, fn(on: u8) -> ());
game_fn!(audio_pause, 0x1_4027_0530, fn(audio: u64, all: u8) -> ());
game_fn!(audio_resume, 0x1_4027_5270, fn(audio: u64, all: u8) -> ());
// the second step of the handlers' sound pause and resume, on the object `audio_state` returns
game_fn!(audio_state, 0x1_4047_6800, fn() -> u64);
game_fn!(audio_state_pause, 0x1_4027_0640, fn(state: u64) -> ());
game_fn!(audio_state_resume, 0x1_4027_53a0, fn(state: u64) -> ());

// the level region a player (0 or 1) is streamed in, or 0; the level info is its first field and
// the region name a `char*` at `+0x10` of that
game_fn!(player_region, 0x1_405b_c790, fn(streaming: u64, player: u32) -> u64);
pub const LEVEL_INFO_NAME: u64 = 0x10;
/// The default allocator of game strings (`0x1409bb6d8`, created on first use by `0x1402215c0`).
pub const STRING_ALLOC: u64 = 0x1_409b_b6d8;
game_fn!(string_alloc_create, 0x1_4022_15c0, fn() -> u64);
// builds a reference-counted game string from a C string into `out`
game_fn!(string_new, 0x1_4002_3fb0, fn(out: *mut u64, s: *const c_char, alloc: u64) -> u64);

/// The property string the Bullseye summon spawns with (`0x140220db0`, also `0x1400ee9b0`).
pub const BULLSEYE_PROPS: u64 = 0x1_4078_f520;
// spawns any actor from a property set (the summon's spawner; `deferred` 0 instantiates now).
game_fn!(spawn_actor, 0x1_4055_4a80, fn(props: *mut u64, pos: *const f32, yaw: f32, deferred: u8, flag: u8) -> u64);
// an actor's blackboard `idx` (0: the one rider properties such as `mountActor` live on).
game_fn!(blackboard, 0x1_4056_1ae0, fn(actor: u64, idx: u32) -> u64);
game_fn!(blackboard_set, 0x1_4054_c420, fn(bb: u64, name: *const c_char, value: u64) -> ());
/// Interface type ids, numbered on first use from one counter (`DAT_140b2a448`); an actor's
/// `vt+0x48(id)` returns its interface of that type.
const TYPE_COUNTER: u64 = 0x1_40b2_a448;
/// The state machine interface (`ForceRide` trigger `0x14016c390`, summon `0x140220db0`).
pub const TYPE_STATE_MACHINE: u64 = 0x1_40b2_a44c;

/// The id stored at `slot`, numbered now if it has none yet, as the game does inline.
pub unsafe fn type_id(slot: u64) -> u32 {
    let at = va(slot);
    if rd_u32(at) == 0 {
        let next = rd_u32(va(TYPE_COUNTER)) + 1;
        std::ptr::write_unaligned(va(TYPE_COUNTER) as *mut u32, next);
        std::ptr::write_unaligned(at as *mut u32, next);
    }
    rd_u32(at)
}

/// Two-player respawn points (FINDINGS 10): per player, where it respawns when down.
/// Health interface vtable of the plain `HealthTracker` (enemies and props); avatars and companions
/// use `TS3HealthTracker`'s (FINDINGS 10).
pub const HEALTH_IF_VT: u64 = 0x1_407d_9660;
/// Health interface take-damage (`vt+0x80`), shared by both trackers; detoured by `hooks::take_damage`.
pub const TAKE_DAMAGE: u64 = 0x1_4044_1f80;

pub const RESPAWN_POINTS: u64 = 0x1_40b2_fdf8;

/// Actor manager pointer, taken by the actor-name lookups.
pub const ACTOR_MGR: u64 = 0x1_40b2_bb70;

pub unsafe fn stat_entry(stats: u64, idx: i32) -> u64 {
    rd_u64(stats + 8) + idx as u64 * 0x60
}

/// The entry's i32 value.
pub const STAT_VALUE: u64 = 0x4c;

game_fn!(frame_a, 0x1_4022_7920, fn() -> ());
game_fn!(frame_b, 0x1_4022_92a0, fn() -> ());
game_fn!(frame_c, 0x1_4022_9660, fn() -> ());
game_fn!(props_init, 0x1_4028_8540, fn(props: *mut u64) -> u64);
game_fn!(props_parse, 0x1_4050_d490, fn(props: *mut u64, spec: *const c_char) -> ());
game_fn!(props_free, 0x1_404f_8cb0, fn(props: *mut u64) -> ());
game_fn!(spawn_avatar, 0x1_405a_f7e0, fn(mgr: u64, props: *mut u64, pos: *const f32, yaw: f32, flag: u8) -> u64);
game_fn!(name_avatar, 0x1_400e_7550, fn(avatar: u64, name: *const c_char, h: u64) -> ());
game_fn!(activate, 0x1_4055_03e0, fn(world: u64, avatar: u64) -> ());
game_fn!(wake, 0x1_4055_b850, fn(avatar: u64, flag: u64) -> ());
game_fn!(remove_avatar, 0x1_4055_6230, fn(avatar: u64) -> ());
game_fn!(node_dirty, 0x1_4056_dab0, fn(node: u64) -> ());
game_fn!(quat_to_mat, 0x1_4025_6d00, fn(mat: *mut f32, quat: *const f32) -> ());
game_fn!(node_set_rot, 0x1_4056_8b50, fn(node: u64, mat: *const f32) -> ());

/// Per-player control stacks.
pub const CONTROL_STACKS: u64 = 0x1_40ac_61c0;
pub const STR_CONTROL_MAPPER: u64 = 0x1_4076_59d8;
pub const STR_CAMERA_STATE: u64 = 0x1_4075_e5a0;
pub const STR_AVATAR: u64 = 0x1_4076_d098;

pub unsafe fn vt_call(this: u64, off: u64) {
    let f: extern "system" fn(u64) = std::mem::transmute(rd_u64(rd_u64(this) + off) as usize);
    f(this)
}

pub unsafe fn vt_u8(this: u64, off: u64) -> u8 {
    let f: extern "system" fn(u64) -> u8 = std::mem::transmute(rd_u64(rd_u64(this) + off) as usize);
    f(this)
}

pub unsafe fn vt_u32(this: u64, off: u64) -> u32 {
    let f: extern "system" fn(u64) -> u32 = std::mem::transmute(rd_u64(rd_u64(this) + off) as usize);
    f(this)
}

pub unsafe fn input_assign(input: u64, player: u32, device: u32) {
    let f: extern "system" fn(u64, u32, u32) = std::mem::transmute(rd_u64(rd_u64(input) + 0x80) as usize);
    f(input, player, device)
}

pub unsafe fn input_release(input: u64, player: u32) {
    let vt = rd_u64(input);
    let f: extern "system" fn(u64, u32) = std::mem::transmute(rd_u64(vt + 0x88) as usize);
    f(input, player)
}

/// Input driver `vt+0x70(player)`: the controller device a player reads (FINDINGS 4), or 0.
pub unsafe fn player_device(input: u64, player: u32) -> u64 {
    let vt = rd_u64(input);
    let f: extern "system" fn(u64, u32) -> u64 = std::mem::transmute(rd_u64(vt + 0x70) as usize);
    f(input, player)
}

/// Controller device `vt+0x10`: its kind as a C string (`XInput` for a pad).
pub unsafe fn device_kind(device: u64) -> u64 {
    let f: extern "system" fn(u64) -> u64 = std::mem::transmute(rd_u64(rd_u64(device) + 0x10) as usize);
    f(device)
}

/// Input driver: its devices (up to 11 pointers; the keyboard's is first) and each player's device index.
pub const INPUT_DEVICES: u64 = 0x460;
pub const INPUT_DEVICE_COUNT: u64 = 11;
pub const INPUT_PLAYER_DEVICE: u64 = 0x4b8;

/// Controller device: pointer to its u32 code per control id.
pub const DEVICE_CODES: u64 = 0xb0;
/// Controller device: code k's current state is the byte at `+0x11 + 2k` (previous at `+0x10 + 2k`).
pub const DEVICE_CODE_STATE: u64 = 0x11;
/// Controller device: left stick x and y, f32 in -1..1.
pub const DEVICE_STICK_X: u64 = 0x68;
pub const DEVICE_STICK_Y: u64 = 0x6c;

pub unsafe fn input_is_assigned(input: u64, player: u32) -> u8 {
    let vt = rd_u64(input);
    let f: extern "system" fn(u64, u32) -> u8 = std::mem::transmute(rd_u64(vt + 0x90) as usize);
    f(input, player)
}

/// The game's imported `_stricmp`, read from its IAT slot.
pub unsafe fn stricmp(a: *const c_char, b: *const c_char) -> i32 {
    let f: extern "C" fn(*const c_char, *const c_char) -> i32 = std::mem::transmute(rd_u64(va(0x1_4075_9a00)) as usize);
    f(a, b)
}
