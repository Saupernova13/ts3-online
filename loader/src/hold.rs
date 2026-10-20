//! Level start hold: a copy that has loaded a level pauses until every partner on its way there has
//! loaded it too, so the level's scripts (intro, first enemies) start together.
use crate::net::proto::{flags, State};

/// A hold never lasts longer than this.
pub const TIMEOUT_MS: u64 = 120_000;

/// A partner as seen from a copy that has loaded `level`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Peer {
    /// in `level`, loaded
    Ready,
    /// loading, or in another play level it will leave for this one (level follow)
    Coming,
    /// in a menu: it does not follow, so nobody waits for it
    Away,
}

pub fn classify(level: u32, st: &State) -> Peer {
    if st.flags & flags::LOADING != 0 {
        return Peer::Coming;
    }
    match st.level {
        l if l == level => Peer::Ready,
        0 => Peer::Away,
        _ => Peer::Coming,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Why {
    Arrived,
    Timeout,
    /// this copy left the level while holding
    Left,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change {
    Hold { waiting: usize },
    Release { why: Why, held_ms: u64 },
}

#[derive(Debug, Default)]
pub struct Hold {
    /// the loaded level the hold was decided for (0: none)
    armed: u32,
    since: Option<u64>,
}

impl Hold {
    /// Per frame; `level` is the play level this copy has loaded, 0 while loading or in a menu.
    /// A level is held at most once, when it has just loaded.
    pub fn update(&mut self, level: u32, peers: &[Peer], now_ms: u64) -> Option<Change> {
        let waiting = peers.iter().filter(|p| **p == Peer::Coming).count();
        if level != self.armed {
            self.armed = level;
            let left = self.since.take().map(|t| Change::Release { why: Why::Left, held_ms: now_ms.saturating_sub(t) });
            if level != 0 && waiting > 0 {
                self.since = Some(now_ms);
                return Some(Change::Hold { waiting });
            }
            return left;
        }
        let since = self.since?;
        let held_ms = now_ms.saturating_sub(since);
        let why = if waiting == 0 {
            Why::Arrived
        } else if held_ms >= TIMEOUT_MS {
            Why::Timeout
        } else {
            return None;
        };
        self.since = None;
        Some(Change::Release { why, held_ms })
    }
}

#[cfg(windows)]
pub use live::{pause, update};

#[cfg(windows)]
mod live {
    use super::{classify, Change, Hold, Peer, Why};
    use crate::game::*;
    use crate::net::proto::level_hash;
    use crate::{log, net};
    use std::sync::{Mutex, OnceLock};
    use std::time::Instant;

    static HOLD: Mutex<Option<Hold>> = Mutex::new(None);
    static START: OnceLock<Instant> = OnceLock::new();

    /// Per frame on the game thread; `loaded` is the play level that has finished loading, "" otherwise.
    pub unsafe fn update(loaded: &str) {
        let level = level_hash(loaded);
        let peers: Vec<Peer> = if net::active() { net::remote_states().iter().map(|(_, s)| classify(level, s)).collect() } else { Vec::new() };
        let now = START.get_or_init(Instant::now).elapsed().as_millis() as u64;
        let Ok(mut guard) = HOLD.lock() else { return };
        let hold = guard.get_or_insert_with(Hold::default);
        match hold.update(level, &peers, now) {
            Some(Change::Hold { waiting }) => {
                log!("hold: {loaded} loaded; game paused until {waiting} player(s) on the way have loaded it");
                pause(true);
            }
            Some(Change::Release { why, held_ms }) => {
                let why = match why {
                    Why::Arrived => "everyone has loaded the level",
                    Why::Timeout => "timed out; starting without the players still loading",
                    Why::Left => "this copy left the level",
                };
                log!("hold: game resumed after {:.1} s: {why}", held_ms as f64 / 1000.0);
                pause(false);
            }
            None => {}
        }
    }

    /// What the pause menu's handlers do to the game (`0x1405d6360`, `0x1405f8d60`; FINDINGS 10)
    /// without the menu and the control lock: the game timer stops, and with it gameplay and
    /// scripted sequences, and the sound pauses. A posted `Pause` does not do it: kind 4 is only a
    /// request, which the front end turns into the pause menu or ignores.
    pub unsafe fn pause(on: bool) {
        let timer = va(GAME_TIMER);
        timer_set_speed(timer, if on { TIMER_STOPPED } else { TIMER_NORMAL }, 0);
        timer_sync(timer);
        if crate::hooks::call_broadcast(global(EVENTS), b"SyncTimer\0".as_ptr() as u64, 0, 0).is_none() {
            log!("hold: the event post detour is not installed; timers are not told of the pause");
        }
        // as the handlers do: `0x1404c7620` is told whether the timer's `+0x40` is set
        set_timer_scaled(if rd_u32(timer + 0x40) != 0 { 0xff } else { 0 });
        let audio = global(AUDIO_PTR);
        if audio == 0 {
            return;
        }
        if on {
            audio_pause(audio, 1);
            audio_state_pause(audio_state());
        } else {
            audio_resume(audio, 1);
            audio_state_resume(audio_state());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn st(level: u32, f: u32) -> State {
        State { id: 1, seq: 1, level, class: 0, pos: [0.0; 3], yaw: 0.0, cam: 0.0, pad: Default::default(), flags: f, target: 0, proj: Default::default() }
    }

    #[test]
    fn classify_tells_loading_from_menus() {
        assert_eq!(classify(7, &st(7, 0)), Peer::Ready);
        assert_eq!(classify(7, &st(0, flags::LOADING)), Peer::Coming);
        assert_eq!(classify(7, &st(7, flags::LOADING)), Peer::Coming);
        assert_eq!(classify(7, &st(9, 0)), Peer::Coming);
        assert_eq!(classify(7, &st(0, 0)), Peer::Away);
    }

    #[test]
    fn holds_a_new_level_until_everyone_has_loaded_it() {
        let mut h = Hold::default();
        assert_eq!(h.update(7, &[Peer::Coming, Peer::Ready], 1000), Some(Change::Hold { waiting: 1 }));
        assert_eq!(h.update(7, &[Peer::Coming, Peer::Ready], 5000), None);
        assert_eq!(h.update(7, &[Peer::Ready, Peer::Ready], 9000), Some(Change::Release { why: Why::Arrived, held_ms: 8000 }));
        // once released, the same level is not held again
        assert_eq!(h.update(7, &[Peer::Coming], 9500), None);
    }

    #[test]
    fn no_hold_alone_with_everyone_there_or_in_menus() {
        let mut h = Hold::default();
        assert_eq!(h.update(7, &[], 0), None);
        assert_eq!(h.update(8, &[Peer::Ready, Peer::Away], 10), None);
        assert_eq!(h.update(0, &[Peer::Coming], 20), None);
    }

    #[test]
    fn a_hold_times_out() {
        let mut h = Hold::default();
        h.update(7, &[Peer::Coming], 0);
        assert_eq!(h.update(7, &[Peer::Coming], TIMEOUT_MS - 1), None);
        assert_eq!(h.update(7, &[Peer::Coming], TIMEOUT_MS), Some(Change::Release { why: Why::Timeout, held_ms: TIMEOUT_MS }));
    }

    #[test]
    fn leaving_the_level_releases_and_the_next_level_holds_again() {
        let mut h = Hold::default();
        h.update(7, &[Peer::Coming], 0);
        assert_eq!(h.update(0, &[Peer::Coming], 300), Some(Change::Release { why: Why::Left, held_ms: 300 }));
        assert_eq!(h.update(9, &[Peer::Coming], 400), Some(Change::Hold { waiting: 1 }));
        // a partner that left the session no longer counts
        assert_eq!(h.update(9, &[], 500), Some(Change::Release { why: Why::Arrived, held_ms: 100 }));
    }
}
