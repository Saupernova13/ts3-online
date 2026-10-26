//! Game-code replacements and detours: player-index fixes (FINDINGS 5a-5f), the frame tick (9),
//! event-post detours (10). `table` is the one list the patch document's hook names resolve against.
use crate::game::*;
use crate::{actor, log};
use std::ffi::{c_char, CStr};
use std::sync::atomic::{AtomicU64, Ordering};

struct Hook {
    name: &'static str,
    func: u64,
    /// detours only: where the loader stores the trampoline that runs the original
    original: Option<&'static AtomicU64>,
}

fn table() -> [Hook; 14] {
    let f = |p: *const ()| p as u64;
    [
        Hook { name: "bind_joining_avatar", func: f(bind_joining_avatar as *const ()), original: None },
        Hook { name: "join_cancel", func: f(join_cancel as *const ()), original: None },
        Hook { name: "join_request", func: f(join_request as *const ()), original: None },
        Hook { name: "find_avatar_by_class", func: f(find_avatar_by_class as *const ()), original: None },
        Hook { name: "refresh_player_controls", func: f(refresh_player_controls as *const ()), original: None },
        Hook { name: "reset_player_slots", func: f(reset_player_slots as *const ()), original: None },
        Hook { name: "level_player_setup", func: f(level_player_setup as *const ()), original: None },
        Hook { name: "frame_tick", func: f(frame_tick as *const ()), original: None },
        Hook { name: "swap_pressed", func: f(swap_pressed as *const ()), original: None },
        Hook { name: "post_broadcast", func: f(post_broadcast as *const ()), original: Some(&ORIG_POST_BROADCAST) },
        Hook { name: "post_actor", func: f(post_actor as *const ()), original: Some(&ORIG_POST_ACTOR) },
        Hook { name: "post_broadcast2", func: f(post_broadcast2 as *const ()), original: Some(&ORIG_POST_BROADCAST2) },
        Hook { name: "take_damage", func: f(take_damage as *const ()), original: Some(&ORIG_TAKE_DAMAGE) },
        Hook { name: "ai_tick", func: f(ai_tick as *const ()), original: Some(&ORIG_AI_TICK) },
    ]
}

/// Address of the Rust function behind a hook name.
pub fn resolve(name: &str) -> Option<u64> {
    table().iter().find(|h| h.name == name).map(|h| h.func)
}

/// The joining player: first pad-assigned 1..=3 without an avatar, else 1.
unsafe fn joining_index() -> u32 {
    let mgr = global(MGR_PTR);
    let input = global(INPUT_PTR);
    let mut idx = 1;
    if mgr != 0 && input != 0 {
        for p in 1..=3u32 {
            if input_is_assigned(input, p) != 0 && slot_avatar(mgr, p as u64) == 0 {
                idx = p;
                break;
            }
        }
    }
    log!("hook joining_index -> {idx}");
    idx
}

/// Level setup inits slots 0/1 only, so 2/3 are set up here on first use. Their ctor index is -1
/// and slot init on -1 drains control stack -1 forever: set the index first (FINDINGS 5b).
unsafe fn ensure_player_setup(mgr: u64, idx: u32) {
    let slot = mgr + SLOT_INDEX_BASE + idx as u64 * SLOT_STRIDE;
    if rd_i32(slot) != -1 {
        return;
    }
    wr_i32(slot, idx as i32);
    slot_init(slot, 0);
    log!("hook ensure_player_setup idx={idx}: slot index set, slot init");
}

/// Replaces the end of level setup (0x1405aee38), which sets up views 0/1 only. The ShowDown
/// (conversation) camera pushes a state on every view and crashes on a stateless one (FINDINGS 5f).
pub unsafe extern "system" fn level_player_setup(mgr: u64, views: u64) {
    for i in 1..4 {
        player_setup(mgr, i);
    }
    views_finish(views);
    log!("hook level_player_setup: views 1..3 set up");
}

static FRAMES: AtomicU64 = AtomicU64::new(0);

/// Replaces the three per-frame calls in the input dispatch (0x140280c18); adds the co-op update
/// while a session exists (FINDINGS 9).
pub unsafe extern "system" fn frame_tick() {
    frame_a();
    frame_b();
    frame_c();
    let n = FRAMES.fetch_add(1, Ordering::Relaxed);
    if n % 60 == 0 {
        crate::pad::reassert();
    }

    if crate::net::active() {
        let t = std::time::Instant::now();
        crate::coop::update();
        crate::perf::frame(t.elapsed());
    }
    crate::evlog::tick();
    crate::devcmd::tick();
}

/// Replaces the avatar input handler's `LB` block (`0x14004efca`): the mapper call as before, then the
/// swap unless the avatar is a remote player's stand-in (#65). `controller+8` is the avatar.
pub unsafe extern "system" fn swap_pressed(controller: u64) {
    let avatar = rd_u64(controller + 8);
    let mapper = component_mapper(component_named(avatar, b"ControlMapper\0".as_ptr() as u64));
    if mapper != 0 {
        mapper_release(mapper);
    }
    if !crate::coop::refuses_swap(avatar) {
        avatar_swap(controller);
    }
}

// ---- detours that call the original (FINDINGS 10) ----

static ORIG_POST_BROADCAST: AtomicU64 = AtomicU64::new(0);
static ORIG_POST_ACTOR: AtomicU64 = AtomicU64::new(0);
static ORIG_POST_BROADCAST2: AtomicU64 = AtomicU64::new(0);
static ORIG_TAKE_DAMAGE: AtomicU64 = AtomicU64::new(0);
static ORIG_AI_TICK: AtomicU64 = AtomicU64::new(0);

fn original_slot(name: &str) -> Option<&'static AtomicU64> {
    table().iter().find(|h| h.name == name).and_then(|h| h.original)
}

/// Called by the loader before the detour is applied; false if `name` is not a detour.
pub fn set_original(name: &str, trampoline: u64) -> bool {
    match original_slot(name) {
        Some(s) => {
            s.store(trampoline, Ordering::Release);
            true
        }
        None => false,
    }
}

/// Trampoline that runs the hooked original, once set.
pub fn original(name: &str) -> Option<u64> {
    original_slot(name).map(|s| s.load(Ordering::Acquire)).filter(|a| *a != 0)
}

type Post4 = unsafe extern "system" fn(u64, u64, u64, u64) -> u8;
type Post5 = unsafe extern "system" fn(u64, u64, u64, u64, u64) -> u8;

/// Compares in place and stops at the first differing byte; `want` ends in a NUL.
unsafe fn is_event(name: u64, want: &[u8]) -> bool {
    name >= 0x1_0000 && want.iter().enumerate().all(|(i, c)| rd_u8(name + i as u64) == *c)
}

/// Detour of the event post broadcast (0x14049e160).
pub unsafe extern "system" fn post_broadcast(events: u64, name: u64, a: u64, b: u64) -> u8 {
    crate::evlog::record(name, a, b, None);
    if a != 0 && is_event(name, b"SwitchLevel\0") {
        crate::coop::on_local_switch(&CStr::from_ptr(a as *const c_char).to_string_lossy());
    }
    if is_event(name, b"LevelLoadComplete\0") {
        crate::coop::on_level_loaded();
    }
    if !crate::pickup::live::allows(name, b) {
        return 0;
    }
    match original("post_broadcast") {
        Some(o) => std::mem::transmute::<u64, Post4>(o)(events, name, a, b),
        None => 0,
    }
}

/// Detour of the targeted event post (0x14049e0b0); `actor` is an actor id.
pub unsafe extern "system" fn post_actor(events: u64, name: u64, a: u64, b: u64, actor: u64) -> u8 {
    crate::evlog::record(name, a, b, Some(actor));
    crate::world::on_post(name, a, b, actor);
    crate::projectile::on_post(name, b, actor);
    crate::pickup::live::on_post(name, b, actor);
    match original("post_actor") {
        Some(o) => std::mem::transmute::<u64, Post5>(o)(events, name, a, b, actor),
        None => 0,
    }
}

/// Detour of the second event post broadcast (0x14049e250).
pub unsafe extern "system" fn post_broadcast2(events: u64, name: u64, a: u64, b: u64) -> u8 {
    crate::evlog::record(name, a, b, None);
    if !crate::pickup::live::allows(name, b) {
        return 0;
    }
    match original("post_broadcast2") {
        Some(o) => std::mem::transmute::<u64, Post4>(o)(events, name, a, b),
        None => 0,
    }
}

type TakeDamage = unsafe extern "system" fn(u64, f32, u64, u64, i32);

/// Detour of the health interface's take-damage (0x140441f80); `source` is an actor id.
pub unsafe extern "system" fn take_damage(health: u64, amount: f32, source: u64, flag: u64, kind: i32) {
    if !crate::health::before_damage(health, amount, source, kind).passes() {
        return;
    }
    if let Some(o) = original("take_damage") {
        std::mem::transmute::<u64, TakeDamage>(o)(health, amount, source, flag, kind);
    }
    crate::health::after_damage(health);
}

type AiTick = unsafe extern "system" fn(u64);

/// Detour of the behaviour controller's per-frame tick (`AiBehaviorControl` vtable `+0x80`,
/// 0x140644700); the controller's actor is at `+8`.
pub unsafe extern "system" fn ai_tick(control: u64) {
    if !crate::ai::runs(rd_u64(control + 8)) {
        return;
    }
    if let Some(o) = original("ai_tick") {
        std::mem::transmute::<u64, AiTick>(o)(control);
    }
}

/// Damages through the original, bypassing the detour.
pub unsafe fn call_take_damage(health: u64, amount: f32, source: u64, kind: i32) -> bool {
    original("take_damage").map(|o| std::mem::transmute::<u64, TakeDamage>(o)(health, amount, source, 1, kind)).is_some()
}

/// Posts through the original, bypassing the detour.
pub unsafe fn call_broadcast(events: u64, name: u64, a: u64, b: u64) -> Option<u8> {
    original("post_broadcast").map(|o| std::mem::transmute::<u64, Post4>(o)(events, name, a, b))
}

/// Posts to an actor through the original, bypassing the detour.
pub unsafe fn call_post_actor(events: u64, name: u64, a: u64, b: u64, actor: u64) -> Option<u8> {
    original("post_actor").map(|o| std::mem::transmute::<u64, Post5>(o)(events, name, a, b, actor))
}

/// Replaces 0x1400e2490: bind the joining avatar to the joining player's slot.
pub unsafe extern "system" fn bind_joining_avatar(avatar: u64, level_info: u64, start: u64) {
    let idx = joining_index();
    let mgr = global(MGR_PTR);
    log!("hook bind_joining_avatar avatar={avatar:#x} level_info={level_info:#x} start={start:#x} idx={idx}");
    ensure_player_setup(mgr, idx);
    assign_avatar(mgr, idx, avatar);
    ev5(global(EVENTS), va(0x1_4077_1540), 0, 0, actor::id(avatar)); // "DeactivateGlow"

    // like the original, player 1's avatar is not null-checked
    let p1 = rd_u64(mgr + SLOT_AVATAR_BASE);
    let hb = rd_u64(p1 + actor::AVATAR_FLAGS);
    let mut avatar = avatar;
    if flagtest(global(FLAGS), hb, hash(0xd5)) != 0 {
        ev4(global(EVENTS), va(0x1_4078_dd28), actor::id(avatar), 0); // "JoiningPlayer1Avatar"
        avatar = slot_avatar(mgr, idx as u64);
    }

    if level_info != 0 {
        let a = lvltest(global(FLAGS), level_info, hash(0x1f1));
        let b = lvltest(global(FLAGS), level_info, hash(0x1f6));
        let c = lvltest(global(FLAGS), level_info, hash(0x1f8));
        if (a | b | c) != 0 {
            autodrive(global(RIDES), idx, avatar, start);
            ev5(global(EVENTS), va(0x1_4078_dd40), va(0x1_4076_b2e8), 0, actor::id(avatar)); // "AvatarAutoDrive"
        }
        if idx == 1 {
            streaming(global(STREAMING), 1, level_info, 0); // only tracks players 0/1
        }
    }
}

/// Replaces 0x1400fe910 (ScreenJoinIn cancel).
pub unsafe extern "system" fn join_cancel(screen: u64) -> u64 {
    let idx = joining_index();
    log!("hook join_cancel screen={screen:#x} release idx={idx}");
    input_release(global(INPUT_PTR), idx);
    join_cancel_tail(screen)
}

/// Levels where joining first asks to restart.
const RESTART_LEVELS: [&str; 14] = [
    "st_train",
    "trainstub",
    "st_prisonbreak",
    "st_bonnie_woody",
    "st_bonnie_woody_rocket",
    "st_hauntedbakery",
    "st_buzzgame",
    "st_buzzgame_int",
    "st_buzzflight",
    "st_junkyard",
    "holotest-multiplayer",
    "st_andysroom",
    "st_andysbasement",
    "st_daycare",
];

pub fn needs_restart(level: &str) -> bool {
    RESTART_LEVELS.iter().any(|l| l.eq_ignore_ascii_case(level))
}

/// Replaces 0x1400eb6f0 (a pad asks to join), using the first free player index throughout.
pub unsafe extern "system" fn join_request(pad: u32) {
    let ui = global(UI_PTR);
    if vt_u8(ui, 0xa0) == 0 && pad != 1 {
        return;
    }
    let mgr = global(MGR_PTR);
    let p1 = rd_u64(mgr + SLOT_AVATAR_BASE);
    let blocked = if p1 != 0 { flagtest(global(FLAGS), rd_u64(p1 + actor::AVATAR_FLAGS), hash(0xd1)) } else { 0 };
    if can_join(mgr) == 0 || blocked != 0 {
        log!("hook join_request pad={pad} refused (can_join/blocked={blocked})");
        post_event(va(0x1_4078_dbb0), 0); // "PlayerJoinDisallowed"
        return;
    }
    let input = global(INPUT_PTR);
    let idx = vt_u32(input, 0xd8); // first free player, 0xffffffff when none
    if idx >= 4 {
        log!("hook join_request pad={pad} no free player index ({idx:#x})");
        return;
    }
    input_assign(input, idx, pad);
    let lp = level_name();
    let level = if lp != 0 { CStr::from_ptr(lp as *const c_char).to_string_lossy().into_owned() } else { String::new() };
    let screen_name = if needs_restart(&level) { va(0x1_4078_6d28) } else { va(0x1_4078_0570) };
    let scr = get_screen(ui, screen_name);
    let name = CStr::from_ptr(screen_name as *const c_char).to_string_lossy();
    log!("hook join_request pad={pad} idx={idx} level={level:?} screen={name} scr={scr:#x}");
    let ok = push_screen(scr, idx); // no null check, like the original
    if ok == 0 {
        log!("hook join_request screen failed to open, releasing idx={idx}");
        input_release(input, idx);
    }
}

/// Replaces 0x1405bae40 (avatar by class); calls from the pick handler skip player-owned avatars.
/// `ret` is the caller's return address, which only the `entry_caller` thunk passes (in R8).
pub unsafe extern "system" fn find_avatar_by_class(mgr: u64, class: u64, ret: u64) -> u64 {
    let pick = va(0x1_4010_4190);
    let from_pick = ret >= pick && ret < pick + 0x577;
    let (mask, want) = (rd_u32(va(0x1_4094_84c4)), rd_u32(va(0x1_4094_84c0)));
    for i in 0..16u64 {
        let a = registry_get(rd_u64(mgr + 0x10 + i * 8));
        if a == 0 || (rd_u32(a + 0x10) & mask) != want {
            continue;
        }
        let cls = prop_string(a + actor::PROPS, va(0x1_4075_d1e4), 0); // "class"
        if stricmp(cls as *const c_char, class as *const c_char) == 0 {
            if from_pick && rd_i32(a + actor::PLAYER_INDEX) != -1 {
                log!("hook find_avatar_by_class skip owned avatar {a:#x} (player {})", rd_i32(a + actor::PLAYER_INDEX));
                continue;
            }
            if from_pick {
                log!("hook find_avatar_by_class -> {a:#x}");
            }
            return a;
        }
    }
    if from_pick {
        log!("hook find_avatar_by_class -> none");
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restart_levels_match_case_insensitively() {
        assert!(needs_restart("ST_Train"));
        assert!(needs_restart("holotest-multiplayer"));
        assert!(!needs_restart("st_toybox"));
        assert!(!needs_restart(""));
    }
}

/// Replaces the slot-1 block of both "controls disabled" refreshers (0x1405f8a70, 0x1405d3200),
/// which cover slots 0/1 only (FINDINGS 5d). `slot+0x50`: slot disable list; 0x1409582a8: global
/// list; avatar `+0x428`: controls flag, set via `vtable+0x148(avatar, enabled)`.
pub unsafe extern "system" fn refresh_player_controls(mgr: u64) {
    let global_clear = rd_u64(va(0x1_4095_82a8)) == 0;
    for i in 1..4u64 {
        let avatar = slot_avatar(mgr, i);
        if avatar == 0 {
            continue;
        }
        let enabled = rd_u64(mgr + SLOT_INDEX_BASE + i * SLOT_STRIDE + 0x50) == 0 && global_clear;
        if actor::controls_enabled(avatar) != enabled {
            vcall_u8(avatar, 0x148, enabled as u8);
            log!("hook refresh_player_controls slot {i} controls -> {enabled} (now {})", actor::controls_enabled(avatar));
        }
    }
}

/// Replaces the slot-1 block of level teardown (0x1405b0ea0), which resets slots 0/1 only (2/3
/// would keep a dangling avatar, FINDINGS 5e). Slots 2/3 also release their pad: level setup
/// restores players 0/1 only, so the others rejoin with Start.
pub unsafe extern "system" fn reset_player_slots(mgr: u64) {
    let stacks = va(CONTROL_STACKS);
    for i in 1..4u32 {
        let slot = mgr + SLOT_INDEX_BASE + i as u64 * SLOT_STRIDE;
        let avatar = rd_u64(slot + 8);
        if avatar != 0 {
            let mapper = control_mapper(avatar);
            if mapper != 0 {
                cstack_pop(stacks, mapper, i);
                cstack_unregister(stacks, mapper, i, 1);
                component_set(component_ref(avatar, va(STR_CONTROL_MAPPER)), 0);
            }
            let state = find_component(avatar, va(STR_CAMERA_STATE));
            if state != 0 {
                camera_state_set_view(state, 0);
            }
        }
        let views = rd_u64(mgr + 0xd8);
        if views != 0 && rd_i32(views) > i as i32 {
            let view = rd_u64(rd_u64(views + 8) + i as u64 * 8);
            if view != 0 {
                let cam = view_find_camera(view, va(STR_AVATAR));
                view_pop_camera(view, view_find_camera(view, va(STR_AVATAR)));
                view_remove_camera(view, cam);
            }
        }
        wr_i32(slot, -1);
        wr_u64(slot + 8, 0);
        wr_u64(slot + 0x10, 0);
        list_clear(slot + 0x18);
        if i >= 2 {
            let input = global(INPUT_PTR);
            if input != 0 && input_is_assigned(input, i) != 0 {
                input_release(input, i);
            }
            if avatar != 0 {
                log!("hook reset_player_slots slot {i}: avatar {avatar:#x} cleared, pad released");
            }
        }
    }
}
