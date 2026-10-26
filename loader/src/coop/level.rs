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

/// The story game board, a menu from which a client still follows the host into a level.
pub(super) const STORY_BOARD: &str = "gui_storymode_boardgame";

pub(super) fn is_story_board(name: &str) -> bool {
    name.eq_ignore_ascii_case(STORY_BOARD)
}

/// Where this copy is, for following.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Here {
    Play(u32),
    /// on the story board; `left` is the level it just finished while the host is still in it
    Board { left: Option<u32> },
    Menu,
}

/// The host level this copy should move to; `None` when it is there already or must not follow.
fn follow_target(here: Here, host_hash: u32) -> Option<u32> {
    if host_hash == 0 {
        return None;
    }
    match here {
        Here::Play(l) => (l != host_hash).then_some(host_hash),
        Here::Board { left } => (left != Some(host_hash)).then_some(host_hash),
        Here::Menu => None,
    }
}

/// The host level a client should switch to; the host's name must match the hash in its state.
/// From the board a client never asks the host to follow, so only a level-to-level follow cools down.
fn decide_follow(here: Here, host_hash: u32, host_name: Option<&str>, mismatch_for: Duration, since_switch: Duration) -> Option<String> {
    follow_target(here, host_hash)?;
    let name = host_name.filter(|n| level_hash(n) == host_hash)?;
    let cooled = !matches!(here, Here::Play(_)) || since_switch >= FOLLOW_COOLDOWN;
    (mismatch_for >= FOLLOW_AFTER && cooled).then(|| name.to_string())
}

/// The play level this copy left for the board, kept while the host is still in it.
#[derive(Debug, Default)]
struct LeftBehind {
    left: Option<u32>,
    last_play: u32,
}

impl LeftBehind {
    fn update(&mut self, local: u32, on_board: bool, host_hash: u32) -> Option<u32> {
        if local != 0 {
            self.last_play = local;
            self.left = None;
        } else if on_board {
            if self.last_play != 0 {
                self.left = Some(self.last_play);
                self.last_play = 0;
            }
            if self.left.is_some_and(|l| l != host_hash) {
                self.left = None;
            }
        } else {
            *self = Self::default();
        }
        self.left
    }
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

/// Client: how long the follow target has been there.
fn mismatch_duration(target: Option<u32>, now: Instant) -> Duration {
    let Ok(mut since) = MISMATCH_SINCE.lock() else { return Duration::ZERO };
    if target.is_none() {
        *since = None;
        return Duration::ZERO;
    }
    now.saturating_duration_since(*since.get_or_insert(now))
}

static LEFT: Mutex<LeftBehind> = Mutex::new(LeftBehind { left: None, last_play: 0 });

/// Where this client is; logs once each time it starts waiting on the board for the host's next level.
fn locate(local: u32, on_board: bool, host: u32) -> Here {
    let (was, left) = match LEFT.lock() {
        Ok(mut b) => (b.left, b.update(local, on_board, host)),
        Err(_) => (None, None),
    };
    if local != 0 {
        return Here::Play(local);
    }
    if !on_board {
        return Here::Menu;
    }
    if let (Some(l), None) = (left, was) {
        log!("coop: on the story board after level {l:#010x}, which the host still plays; waiting for its next level");
    }
    Here::Board { left }
}

/// Per frame; `local` is the play level hash (0 outside one), `on_board` that this copy is on the story board.
pub(super) unsafe fn follow_levels(local: u32, on_board: bool) {
    let now = Instant::now();
    if net::is_client() {
        let host_name = net::host_level_name();
        let host_state = net::remote_states().into_iter().find(|(id, _)| *id == 0).map(|(_, s)| s);
        let host = host_level(host_state.as_ref(), host_name.as_deref());
        let here = locate(local, on_board, host);
        let mismatch = mismatch_duration(follow_target(here, host), now);
        let since_switch = LAST_SWITCH.lock().ok().and_then(|t| *t).map_or(Duration::MAX, |t| now.saturating_duration_since(t));
        if let Some(name) = decide_follow(here, host, host_name.as_deref(), mismatch, since_switch) {
            if matches!(here, Here::Board { .. }) {
                log!("coop: following the host from the story board to level {name}");
            } else {
                log!("coop: following the host to level {name}");
            }
            note_switch();
            mismatch_duration(None, now);
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
        let go = |l, h, n: Option<&str>, m, s| decide_follow(if l == 0 { Here::Menu } else { Here::Play(l) }, h, n, m, s);
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
    fn the_story_board_is_known_by_name() {
        assert!(is_story_board("gui_storymode_boardgame") && is_story_board("GUI_StoryMode_BoardGame"));
        assert!(!is_story_board("gui_toybarn") && !is_story_board("frontend") && !is_story_board(""));
    }

    #[test]
    fn a_client_on_the_board_follows_the_host_into_a_level() {
        let (toybox, daycare) = (level_hash("ST_ToyBox"), level_hash("ST_Daycare"));
        let board = |left| Here::Board { left };
        assert_eq!(follow_target(board(None), daycare), Some(daycare));
        assert_eq!(follow_target(board(Some(toybox)), daycare), Some(daycare));
        assert_eq!(follow_target(board(Some(daycare)), daycare), None, "it waits after finishing the host's level");
        assert_eq!(follow_target(board(None), 0), None, "the host is on the board or in a menu too");
        assert_eq!(follow_target(Here::Menu, daycare), None, "never out of the front end");
        assert_eq!(follow_target(Here::Play(daycare), daycare), None);
        assert_eq!(follow_target(Here::Play(toybox), daycare), Some(daycare));
    }

    #[test]
    fn only_a_follow_between_two_levels_waits_for_the_cooldown() {
        let daycare = level_hash("ST_Daycare");
        let (long, soon) = (Duration::from_secs(2), Duration::from_secs(1));
        let board = Here::Board { left: None };
        assert_eq!(decide_follow(board, daycare, Some("ST_Daycare"), long, soon).as_deref(), Some("ST_Daycare"));
        assert_eq!(decide_follow(Here::Play(level_hash("ST_ToyBox")), daycare, Some("ST_Daycare"), long, soon), None);
        assert_eq!(decide_follow(board, daycare, Some("ST_Daycare"), Duration::from_millis(1499), soon), None);
        assert_eq!(decide_follow(board, daycare, Some("ST_Attic"), long, soon), None, "the name must match the hash");
    }

    #[test]
    fn just_left_holds_while_the_host_stays_and_is_forgotten_when_it_moves() {
        let (l, m) = (level_hash("st_train"), level_hash("st_daycare"));
        let mut b = LeftBehind::default();
        assert_eq!(b.update(l, false, l), None, "in a play level nothing is left behind");
        assert_eq!(b.update(0, true, l), Some(l), "finished first: on the board, the host still in it");
        assert_eq!(b.update(0, true, l), Some(l));
        assert_eq!(b.update(0, true, 0), None, "the host left it");
        assert_eq!(b.update(0, true, l), None, "a replay of the same level is followed");
        assert_eq!(b.update(m, false, m), None);
        assert_eq!(b.update(0, false, m), None, "a menu other than the board leaves nothing behind");
    }
}
