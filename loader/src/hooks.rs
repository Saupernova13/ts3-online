//! Rust replacements for game code with a hardcoded player index (FINDINGS 5a).
use crate::game::*;
use crate::log;
use std::ffi::{c_char, CStr};

/// Address of the Rust function behind a hook name, or None if unknown.
pub fn resolve(name: &str) -> Option<u64> {
    Some(match name {
        "bind_joining_avatar" => bind_joining_avatar as *const () as u64,
        "join_cancel" => join_cancel as *const () as u64,
        "join_request" => join_request as *const () as u64,
        "find_avatar_by_class" => find_avatar_by_class as *const () as u64,
        "refresh_player_controls" => refresh_player_controls as *const () as u64,
        "reset_player_slots" => reset_player_slots as *const () as u64,
        "level_player_setup" => level_player_setup as *const () as u64,
        "frame_tick" => frame_tick as *const () as u64,
        _ => return None,
    })
}

/// The player index that is joining: first pad-assigned player 1..=3 without an avatar (1 if none).
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

/// Level setup only prepares slots 0/1: index and empty slot init. Slots 2/3 keep the ctor's index
/// -1 (slot init would drain control stack -1 forever), so do the same for them the first time they
/// are used (FINDINGS 5b). Their views were already set up by `level_player_setup`.
unsafe fn ensure_player_setup(mgr: u64, idx: u32) {
    let slot = mgr + SLOT_INDEX_BASE + idx as u64 * SLOT_STRIDE;
    if rd_i32(slot) != -1 {
        return;
    }
    wr_i32(slot, idx as i32);
    slot_init(slot, 0);
    log!("hook ensure_player_setup idx={idx}: slot index set, slot init");
}

/// Replaces the end of level setup (0x1405aee38), which runs per-player view setup for players 0/1
/// only. Views 2/3 then had camera groups with no state, and the ShowDown (conversation) camera,
/// which pushes a state on every view, crashed on them (FINDINGS 5f). Set up views 1..=3 the same
/// way, then finish the view array as the original does.
pub unsafe extern "system" fn level_player_setup(mgr: u64, views: u64) {
    for i in 1..4 {
        player_setup(mgr, i);
    }
    views_finish(views);
    log!("hook level_player_setup: views 1..3 set up");
}

/// Replaces the three per-frame calls in the input dispatch (0x140280c18): the same calls, then
/// the co-op partner update while a session exists (FINDINGS 9).
static FRAMES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

pub unsafe extern "system" fn frame_tick() {
    frame_a();
    frame_b();
    frame_c();
    let n = FRAMES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    if n % 60 == 0 {
        crate::pad::reassert();
    }

    if crate::net::active() {
        crate::coop::update();
    }
}

unsafe fn actor_id(actor: u64) -> u64 {
    if actor != 0 {
        rd_u64(actor + 0x38)
    } else {
        0
    }
}

/// Replaces 0x1400e2490: bind the joining avatar to the joining player's slot.
pub unsafe extern "system" fn bind_joining_avatar(avatar: u64, level_info: u64, start: u64) {
    let idx = joining_index();
    let mgr = global(MGR_PTR);
    log!("hook bind_joining_avatar avatar={avatar:#x} level_info={level_info:#x} start={start:#x} idx={idx}");
    ensure_player_setup(mgr, idx);
    assign_avatar(mgr, idx, avatar);
    ev5(global(EVENTS), va(0x1_4077_1540), 0, 0, actor_id(avatar)); // "DeactivateGlow"

    // the original does not null-check player 1's avatar
    let p1 = rd_u64(mgr + SLOT_AVATAR_BASE);
    let hb = rd_u64(p1 + 0x408);
    let mut avatar = avatar;
    if flagtest(global(FLAGS), hb, hash(0xd5)) != 0 {
        ev4(global(EVENTS), va(0x1_4078_dd28), rd_u64(avatar + 0x38), 0); // "JoiningPlayer1Avatar"
        avatar = slot_avatar(mgr, idx as u64);
    }

    if level_info != 0 {
        let a = lvltest(global(FLAGS), level_info, hash(0x1f1));
        let b = lvltest(global(FLAGS), level_info, hash(0x1f6));
        let c = lvltest(global(FLAGS), level_info, hash(0x1f8));
        if (a | b | c) != 0 {
            autodrive(global(RIDES), idx, avatar, start);
            ev5(global(EVENTS), va(0x1_4078_dd40), va(0x1_4076_b2e8), 0, actor_id(avatar)); // "AvatarAutoDrive"
        }
        if idx == 1 {
            streaming(global(STREAMING), 1, level_info, 0); // only tracks players 0/1
        }
    }
}

/// Replaces 0x1400fe910 (ScreenJoinIn cancel): release the joining pad, then the shared tail.
pub unsafe extern "system" fn join_cancel(screen: u64) -> u64 {
    let idx = joining_index();
    log!("hook join_cancel screen={screen:#x} release idx={idx}");
    input_release(global(INPUT_PTR), idx);
    join_cancel_tail(screen)
}

/// Levels where joining asks to restart the level first.
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

/// Replaces 0x1400eb6f0: a pad asks to join. Uses the first free player index throughout.
pub unsafe extern "system" fn join_request(pad: u32) {
    let ui = global(UI_PTR);
    if vt_u8(ui, 0xa0) == 0 && pad != 1 {
        return;
    }
    let mgr = global(MGR_PTR);
    let p1 = rd_u64(mgr + SLOT_AVATAR_BASE);
    let blocked = if p1 != 0 { flagtest(global(FLAGS), rd_u64(p1 + 0x408), hash(0xd1)) } else { 0 };
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

/// Replaces 0x1405bae40: avatar-by-class lookup; pick-handler calls skip avatars that belong to a player.
pub unsafe extern "system" fn find_avatar_by_class(mgr: u64, class: u64, ret: u64) -> u64 {
    let pick = va(0x1_4010_4190);
    let from_pick = ret >= pick && ret < pick + 0x577;
    let (mask, want) = (rd_u32(va(0x1_4094_84c4)), rd_u32(va(0x1_4094_84c0)));
    for i in 0..16u64 {
        let a = registry_get(rd_u64(mgr + 0x10 + i * 8));
        if a == 0 || (rd_u32(a + 0x10) & mask) != want {
            continue;
        }
        let cls = prop_string(a + 0x2e0, va(0x1_4075_d1e4), 0); // "class"
        if stricmp(cls as *const c_char, class as *const c_char) == 0 {
            if from_pick && rd_i32(a + 0x484) != -1 {
                log!("hook find_avatar_by_class skip owned avatar {a:#x} (player {})", rd_i32(a + 0x484));
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

/// Replaces the slot-1 block of the two global "controls disabled" refreshers (0x1405f8a70,
/// 0x1405d3200), which only update slots 0/1. Same rule for slots 1..=3: controls are on when the
/// slot's own disable list (`slot+0x50`) and the global one (0x1409582a8) are both empty; when the
/// avatar's flag (`+0x428`) disagrees, set it through `vtable+0x148(avatar, enabled)` (FINDINGS 5d).
pub unsafe extern "system" fn refresh_player_controls(mgr: u64) {
    let global_clear = rd_u64(va(0x1_4095_82a8)) == 0;
    for i in 1..4u64 {
        let avatar = slot_avatar(mgr, i);
        if avatar == 0 {
            continue;
        }
        let enabled = rd_u64(mgr + SLOT_INDEX_BASE + i * SLOT_STRIDE + 0x50) == 0 && global_clear;
        if rd_u8(avatar + 0x428) != enabled as u8 {
            vcall_u8(avatar, 0x148, enabled as u8);
            log!("hook refresh_player_controls slot {i} controls -> {enabled} (now {})", rd_u8(avatar + 0x428));
        }
    }
}

/// Replaces the slot-1 block of level teardown (0x1405b0ea0), which resets slots 0/1 only, so
/// slots 2/3 kept a dangling avatar after a level change (FINDINGS 5e). Same reset for slots
/// 1..=3: pop the avatar's mapper off the player's control stack, detach its camera state, drop
/// the view's "Avatar" camera, then clear the slot. Slots 2/3 also release their pad, since level
/// setup only brings back players 0/1; those players join again with Start.
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
