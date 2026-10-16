//! Rust replacements for game code with a hardcoded player index (FINDINGS 5a).
use crate::game::*;
use crate::log;
use std::ffi::c_char;

/// Address of the Rust function behind a hook name, or None if unknown.
pub fn resolve(name: &str) -> Option<u64> {
    Some(match name {
        "bind_joining_avatar" => bind_joining_avatar as *const () as u64,
        "join_cancel" => join_cancel as *const () as u64,
        "release_joining" => release_joining as *const () as u64,
        "find_avatar_by_class" => find_avatar_by_class as *const () as u64,
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

/// Called from the join handler's failure path instead of `release(1)`.
pub unsafe extern "system" fn release_joining() {
    let idx = joining_index();
    log!("hook release_joining idx={idx}");
    input_release(global(INPUT_PTR), idx);
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
