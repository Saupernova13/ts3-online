//! Respawn in two-player story mode (FINDINGS 10): a downed player respawns at the checkpoint its
//! partner last reached, and waits until the partner has reached one. A remote partner's stand-in
//! may never trigger the level's checkpoints on this copy, so once the wait has lasted a moment
//! this module gives the game a point: the downed player's own last checkpoint, else the partner.
//! The game also holds a downed player while its partner is down, and restarts both from the
//! checkpoint once both wait; the stand-in's Respawn component shows its owner's down and waiting
//! status so every copy makes that call the same way. The game pairs slots 0 and 1 only, so with
//! more players slot 1's stand-in speaks for every remote partner (`together`).

/// Respawn component states (`0x140645280`): down and waiting for the partner's point, then
/// respawning at it.
pub const WAITING_FOR_PARTNER: u32 = 6;
/// Respawning at the partner's point: the wait ended without a restart.
pub const RESPAWNING_AT_POINT: u32 = 7;
/// The game leaves the wait the frame a point exists; this long without one, the point is given.
pub const GRACE_MS: u64 = 2_000;
/// A player's wait is still sent this long after it restarted: it leaves the wait the frame it sees
/// everyone else waiting, which may be before they have seen it wait, or a stalled copy never does.
pub const WAIT_LATCH_MS: u64 = 1_500;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// the checkpoint the downed player itself last reached
    OwnCheckpoint,
    /// where the partner stands
    Partner,
}

/// The local player's down state as the game holds it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Down {
    pub state: u32,
    /// the game's point for this player (set when the partner reaches a checkpoint)
    pub has_point: bool,
    /// the point this player set for its partner at its own last checkpoint
    pub own_checkpoint: bool,
}

/// What slot 1's stand-in shows for the (down, waiting) status of every remote partner in the
/// level: down while all are down, waiting while all wait; None with no partner in the level.
pub fn together(owners: &[(bool, bool)]) -> Option<(bool, bool)> {
    (!owners.is_empty()).then(|| (owners.iter().all(|o| o.0), owners.iter().all(|o| o.1)))
}

/// The local player's (down, waiting) as sent, from its Respawn state: the wait is held
/// `WAIT_LATCH_MS` after a restart, and dropped at once when the player respawns at a partner instead.
#[derive(Debug, Default)]
pub struct SentWait {
    waited_at: Option<u64>,
}

impl SentWait {
    pub fn update(&mut self, down: bool, state: u32, now_ms: u64) -> (bool, bool) {
        match state {
            WAITING_FOR_PARTNER => self.waited_at = Some(now_ms),
            RESPAWNING_AT_POINT => self.waited_at = None,
            _ => {}
        }
        let waiting = self.waited_at.is_some_and(|t| now_ms.saturating_sub(t) < WAIT_LATCH_MS);
        (down || waiting, waiting)
    }
}

#[derive(Debug, Default)]
pub struct Wait {
    since: Option<u64>,
    /// a point was given and the player has not been up since
    given: bool,
}

impl Wait {
    /// Per frame; the source to give the game a point from, once per wait. While the partner is
    /// down the game holds the wait on purpose, and no point is given.
    pub fn update(&mut self, d: Down, partner_down: bool, now_ms: u64) -> Option<Source> {
        if d.state != WAITING_FOR_PARTNER || d.has_point || partner_down {
            self.since = None;
            return None;
        }
        let since = *self.since.get_or_insert(now_ms);
        if now_ms.saturating_sub(since) < GRACE_MS {
            return None;
        }
        // a later frame of the same wait starts a new grace, in case the point did not take
        self.since = Some(now_ms);
        self.given = true;
        Some(if d.own_checkpoint { Source::OwnCheckpoint } else { Source::Partner })
    }

    /// Whether to take back the given point: the player is up again, and the next down chooses
    /// afresh where the partners are then (left in place, it would respawn there at once).
    pub fn take_back(&mut self, state: u32) -> bool {
        let up = self.given && state == 0;
        if up {
            self.given = false;
        }
        up
    }
}

#[cfg(windows)]
pub use live::{local_status, update};

#[cfg(windows)]
mod live {
    use super::{together, Down, SentWait, Source, Wait, WAITING_FOR_PARTNER};
    use crate::actor;
    use crate::game::*;
    use crate::log;
    use std::ffi::c_char;
    use std::sync::{Mutex, OnceLock};
    use std::time::Instant;

    // Respawn component (`0x14064a820`)
    const STATE: u64 = 0x18;
    /// set in state 6; a waiting player whose partner has it set restarts both at the checkpoint
    const WAITING: u64 = 0x1e;
    const DOWN: u64 = 0x20;

    // Respawn points (`RESPAWN_POINTS`): the point player 1 set, which player 0 respawns at, and
    // the one player 0 set for player 1; a valid flag, then the position and a 3x3 rotation.
    const P0_VALID: u64 = 0x40;
    const P0_POINT: u64 = 0x44;
    const P1_VALID: u64 = 0x09;
    const P1_POINT: u64 = 0x0c;
    /// position (3 floats), a pad float, the 3x3 rotation (9 floats)
    const POINT_LEN: usize = 0x34;

    static WAIT: Mutex<Option<Wait>> = Mutex::new(None);
    static SENT: Mutex<SentWait> = Mutex::new(SentWait { waited_at: None });
    static LAST: Mutex<[u32; 2]> = Mutex::new([0; 2]);
    static START: OnceLock<Instant> = OnceLock::new();

    unsafe fn respawn(avatar: u64) -> u64 {
        if avatar == 0 {
            0
        } else {
            component(avatar, b"Respawn\0".as_ptr() as *const c_char)
        }
    }

    /// The local player's (down, waiting for its partner) as sent to the partners.
    pub unsafe fn local_status(mgr: u64) -> (bool, bool) {
        let me = if mgr == 0 { 0 } else { respawn(slot_avatar(mgr, 0)) };
        let (down, state) = if me == 0 { (false, 0) } else { (rd_u8(me + DOWN) != 0, rd_u32(me + STATE)) };
        let now = START.get_or_init(Instant::now).elapsed().as_millis() as u64;
        SENT.lock().map_or((down, state == WAITING_FOR_PARTNER), |mut s| s.update(down, state, now))
    }

    /// Per frame on the game thread, with slot 0 (the local player) and slot 1 (its partner);
    /// `owners`: (player id, down, waiting) of each remote partner in the level, none for a local
    /// partner; `standing`: a partner's avatar whose owner is not down, to respawn at, else 0 for slot 1's.
    pub unsafe fn update(mgr: u64, owners: &[(u8, bool, bool)], standing: u64) {
        let points = global(RESPAWN_POINTS);
        let (me, partner) = (respawn(slot_avatar(mgr, 0)), respawn(slot_avatar(mgr, 1)));
        if points == 0 || me == 0 {
            return;
        }
        let now = START.get_or_init(Instant::now).elapsed().as_millis() as u64;
        let shown: Vec<(bool, bool)> = owners.iter().map(|o| (o.1, o.2)).collect();
        if let (Some((down, waiting)), true) = (together(&shown), partner != 0) {
            wr_u8(partner + DOWN, down as u8);
            wr_u8(partner + WAITING, waiting as u8);
        }
        log_changes(me, partner);
        let d = Down { state: rd_u32(me + STATE), has_point: rd_u8(points + P0_VALID) != 0, own_checkpoint: rd_u8(points + P1_VALID) != 0 };
        let partner_down = partner != 0 && rd_u8(partner + DOWN) != 0;
        let Ok(mut guard) = WAIT.lock() else { return };
        let wait = guard.get_or_insert_with(Wait::default);
        if wait.take_back(d.state) {
            wr_u8(points + P0_VALID, 0);
        }
        let Some(source) = wait.update(d, partner_down, now) else { return };
        let dst = (points + P0_POINT) as *mut u8;
        match source {
            Source::OwnCheckpoint => std::ptr::copy_nonoverlapping((points + P1_POINT) as *const u8, dst, POINT_LEN),
            Source::Partner => {
                let p1 = if standing != 0 { standing } else { slot_avatar(mgr, 1) };
                if p1 == 0 {
                    log!("respawn: waiting for the partner's checkpoint, with no checkpoint and no partner to respawn at");
                    return;
                }
                let mut point = [0f32; POINT_LEN / 4];
                point[..3].copy_from_slice(&actor::pos(p1));
                point[4..].copy_from_slice(&[1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0]);
                std::ptr::copy_nonoverlapping(point.as_ptr() as *const u8, dst, POINT_LEN);
            }
        }
        wr_u8(points + P0_VALID, 1);
        log!("respawn: no checkpoint from the partner on this copy; respawning at {source:?}");
    }

    /// Logs the local player's Respawn state and the partner's shown status when they change.
    unsafe fn log_changes(me: u64, partner: u64) {
        let shown = |p: u64| if p == 0 { 0 } else { rd_u8(p + DOWN) as u32 | (rd_u8(p + WAITING) as u32) << 1 };
        let now = [rd_u32(me + STATE), shown(partner)];
        let Ok(mut last) = LAST.lock() else { return };
        if *last != now {
            log!("respawn: local state {}, partner down|waiting {} (was {} {})", now[0], now[1], last[0], last[1]);
            *last = now;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn down(state: u32, has_point: bool, own: bool) -> Down {
        Down { state, has_point, own_checkpoint: own }
    }

    #[test]
    fn gives_a_point_after_the_grace() {
        let mut w = Wait::default();
        assert_eq!(w.update(down(WAITING_FOR_PARTNER, false, true), false, 0), None);
        assert_eq!(w.update(down(WAITING_FOR_PARTNER, false, true), false, GRACE_MS - 1), None);
        assert_eq!(w.update(down(WAITING_FOR_PARTNER, false, true), false, GRACE_MS), Some(Source::OwnCheckpoint));
        // the next attempt waits a full grace again
        assert_eq!(w.update(down(WAITING_FOR_PARTNER, false, true), false, GRACE_MS + 1), None);
        assert_eq!(w.update(down(WAITING_FOR_PARTNER, false, false), false, 2 * GRACE_MS), Some(Source::Partner));
    }

    #[test]
    fn leaves_the_game_alone_otherwise() {
        let mut w = Wait::default();
        // the partner reached a checkpoint: the game respawns on its own
        assert_eq!(w.update(down(WAITING_FOR_PARTNER, true, false), false, 0), None);
        assert_eq!(w.update(down(WAITING_FOR_PARTNER, true, false), false, 10 * GRACE_MS), None);
        // alive, or down in another state
        assert_eq!(w.update(down(0, false, false), false, 20 * GRACE_MS), None);
        assert_eq!(w.update(down(4, false, false), false, 30 * GRACE_MS), None);
    }

    #[test]
    fn no_point_is_given_while_the_partner_is_down() {
        let mut w = Wait::default();
        let d = down(WAITING_FOR_PARTNER, false, true);
        assert_eq!(w.update(d, true, 0), None);
        assert_eq!(w.update(d, true, 10 * GRACE_MS), None);
    }

    #[test]
    fn the_local_wait_is_sent_a_while_after_a_restart() {
        let mut s = SentWait::default();
        assert_eq!(s.update(false, 0, 0), (false, false));
        assert_eq!(s.update(true, 4, 50), (true, false));
        assert_eq!(s.update(true, WAITING_FOR_PARTNER, 100), (true, true));
        // restarted at the checkpoint (5, then up): still sent as down and waiting
        assert_eq!(s.update(false, 5, 120), (true, true));
        assert_eq!(s.update(false, 0, 100 + WAIT_LATCH_MS - 1), (true, true));
        assert_eq!(s.update(false, 0, 100 + WAIT_LATCH_MS), (false, false));
    }

    #[test]
    fn respawning_at_a_partner_ends_the_wait_at_once() {
        let mut s = SentWait::default();
        assert_eq!(s.update(true, WAITING_FOR_PARTNER, 100), (true, true));
        assert_eq!(s.update(true, RESPAWNING_AT_POINT, 200), (true, false));
        assert_eq!(s.update(false, 0, 220), (false, false));
    }

    #[test]
    fn slot_one_speaks_for_every_partner() {
        assert_eq!(together(&[]), None);
        assert_eq!(together(&[(true, true)]), Some((true, true)));
        assert_eq!(together(&[(true, true), (false, false)]), Some((false, false)), "one still stands");
        assert_eq!(together(&[(true, true), (true, false), (true, true)]), Some((true, false)), "all down, one not waiting yet");
        assert_eq!(together(&[(true, true), (true, true), (true, true)]), Some((true, true)));
    }

    #[test]
    fn a_given_point_is_taken_back_once_the_player_is_up() {
        let mut w = Wait::default();
        assert!(!w.take_back(0), "nothing given");
        w.update(down(WAITING_FOR_PARTNER, false, false), false, 0);
        assert_eq!(w.update(down(WAITING_FOR_PARTNER, false, false), false, GRACE_MS), Some(Source::Partner));
        assert!(!w.take_back(WAITING_FOR_PARTNER));
        assert!(!w.take_back(RESPAWNING_AT_POINT), "still respawning");
        assert!(w.take_back(0));
        assert!(!w.take_back(0), "once");
    }

    #[test]
    fn a_new_wait_starts_a_new_grace() {
        let mut w = Wait::default();
        w.update(down(WAITING_FOR_PARTNER, false, true), false, 0);
        w.update(down(0, false, true), false, 1000);
        assert_eq!(w.update(down(WAITING_FOR_PARTNER, false, true), false, 1500), None);
        assert_eq!(w.update(down(WAITING_FOR_PARTNER, false, true), false, 1500 + GRACE_MS), Some(Source::OwnCheckpoint));
    }
}
