//! Level sync: a client follows the host's level, the host follows a client's switch.
use crate::game::*;
use crate::net::proto::{flags, level_hash, State};
use crate::{hooks, log, net};
use std::ffi::{c_char, CStr};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// A client follows the host's level after differing this long.
const FOLLOW_AFTER: Duration = Duration::from_millis(1500);
/// No level follow this soon after a level switch of this copy.
const FOLLOW_COOLDOWN: Duration = Duration::from_secs(15);

static MISMATCH_SINCE: Mutex<Option<Instant>> = Mutex::new(None);
/// Last level switch here, by the game itself or by following.
static LAST_SWITCH: Mutex<Option<Instant>> = Mutex::new(None);

pub(super) unsafe fn local_level_name() -> String {
    let name = level_name();
    if name == 0 {
        return String::new();
    }
    CStr::from_ptr(name as *const c_char).to_string_lossy().into_owned()
}

/// Without a `LevelLoadComplete`, a level counts as loaded once its avatar has been up this long.
const LOAD_CAP: Duration = Duration::from_secs(60);

/// The game posts `LevelLoadComplete` once streaming is done, then plays the level's
/// `postLoadMovie` in the same update (FINDINGS 10), so the next frame after it is a playing level.
pub(super) fn level_loaded(complete: bool, avatar_up_for: Option<Duration>) -> bool {
    avatar_up_for.is_some_and(|t| complete || t >= LOAD_CAP)
}

/// The front end and `gui_*` levels (such as the story game board) are menus.
pub fn is_play_level(name: &str) -> bool {
    !name.is_empty() && !name.get(..4).is_some_and(|p| p.eq_ignore_ascii_case("gui_")) && !name.eq_ignore_ascii_case("frontend")
}

/// The host level a client should switch to; the host's name must match the hash in its state.
fn decide_follow(local_hash: u32, host_hash: u32, host_name: Option<&str>, mismatch_for: Duration, since_switch: Duration) -> Option<String> {
    if local_hash == 0 || host_hash == 0 || local_hash == host_hash {
        return None;
    }
    let name = host_name.filter(|n| level_hash(n) == host_hash)?;
    (mismatch_for >= FOLLOW_AFTER && since_switch >= FOLLOW_COOLDOWN).then(|| name.to_string())
}

/// The host's level hash; while the host loads, the level it is loading, so both load at once.
fn host_level(st: Option<&State>, name: Option<&str>) -> u32 {
    match st {
        Some(s) if s.flags & flags::LOADING != 0 => name.map_or(0, level_hash),
        Some(s) => s.level,
        None => 0,
    }
}

fn note_switch() {
    if let Ok(mut t) = LAST_SWITCH.lock() {
        *t = Some(Instant::now());
    }
}

/// Runs on each `SwitchLevel` the game posts through the detour (not our own `call_broadcast` posts),
/// before the game handles it.
pub fn on_local_switch(name: &str) {
    note_switch();
    if net::is_client() && is_play_level(name) {
        net::request_level(name);
    }
}

unsafe fn post_switch_level(name: &str) {
    hooks::call_broadcast(global(EVENTS), b"SwitchLevel\0".as_ptr() as u64, leak_cstr(name), 0);
}

/// Client: how long the local and host levels have differed.
fn mismatch_duration(local: u32, host: u32, now: Instant) -> Duration {
    let Ok(mut since) = MISMATCH_SINCE.lock() else { return Duration::ZERO };
    if local == 0 || host == 0 || local == host {
        *since = None;
        return Duration::ZERO;
    }
    now.saturating_duration_since(*since.get_or_insert(now))
}

/// Per frame; `local` is the play level hash (0 outside one).
pub(super) unsafe fn follow_levels(local: u32) {
    let now = Instant::now();
    if net::is_client() {
        let host_name = net::host_level_name();
        let host_state = net::remote_states().into_iter().find(|(id, _)| *id == 0).map(|(_, s)| s);
        let host = host_level(host_state.as_ref(), host_name.as_deref());
        let mismatch = mismatch_duration(local, host, now);
        let since_switch = LAST_SWITCH.lock().ok().and_then(|t| *t).map_or(Duration::MAX, |t| now.saturating_duration_since(t));
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loaded_after_the_load_complete_event_with_an_avatar() {
        let up = |s| Some(Duration::from_secs(s));
        assert!(level_loaded(true, up(0)));
        assert!(!level_loaded(true, None));
        assert!(!level_loaded(false, up(59)));
        assert!(level_loaded(false, up(60)));
    }

    #[test]
    fn a_loading_host_is_followed_to_the_level_it_loads() {
        let st = |level, f| State { id: 0, seq: 1, level, class: 0, pos: [0.0; 3], yaw: 0.0, cam: 0.0, pad: Default::default(), flags: f, target: 0, proj: Default::default(), rail: Default::default() };
        let train = level_hash("st_train");
        assert_eq!(host_level(Some(&st(0, flags::LOADING)), Some("st_train")), train);
        assert_eq!(host_level(Some(&st(0, flags::LOADING)), None), 0);
        assert_eq!(host_level(Some(&st(train, 0)), Some("westerntown")), train);
        assert_eq!(host_level(Some(&st(0, 0)), Some("st_train")), 0);
        assert_eq!(host_level(None, Some("st_train")), 0);
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
}
