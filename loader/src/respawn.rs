//! Respawn in two-player story mode (FINDINGS 10): a downed player respawns at the checkpoint its
//! partner last reached, and waits until the partner has reached one. A remote partner's stand-in
//! may never trigger the level's checkpoints on this copy, so once the wait has lasted a moment
//! this module gives the game a point: the downed player's own last checkpoint, else the partner.

/// Respawn component states (`0x140645280`): down and waiting for the partner's point, then
/// respawning at it.
pub const WAITING_FOR_PARTNER: u32 = 6;
/// The game leaves the wait the frame a point exists; this long without one, the point is given.
pub const GRACE_MS: u64 = 2_000;

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

#[derive(Debug, Default)]
pub struct Wait {
    since: Option<u64>,
}

impl Wait {
    /// Per frame; the source to give the game a point from, once per wait.
    pub fn update(&mut self, d: Down, now_ms: u64) -> Option<Source> {
        if d.state != WAITING_FOR_PARTNER || d.has_point {
            self.since = None;
            return None;
        }
        let since = *self.since.get_or_insert(now_ms);
        if now_ms.saturating_sub(since) < GRACE_MS {
            return None;
        }
        // a later frame of the same wait starts a new grace, in case the point did not take
        self.since = Some(now_ms);
        Some(if d.own_checkpoint { Source::OwnCheckpoint } else { Source::Partner })
    }
}

#[cfg(windows)]
pub use live::update;

#[cfg(windows)]
mod live {
    use super::{Down, Source, Wait};
    use crate::actor;
    use crate::game::*;
    use crate::log;
    use std::ffi::c_char;
    use std::sync::{Mutex, OnceLock};
    use std::time::Instant;

    // Respawn component (`0x14064a820`)
    const STATE: u64 = 0x18;
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
    static LAST: Mutex<[u32; 2]> = Mutex::new([0; 2]);
    static START: OnceLock<Instant> = OnceLock::new();

    unsafe fn respawn(avatar: u64) -> u64 {
        if avatar == 0 {
            0
        } else {
            component(avatar, b"Respawn\0".as_ptr() as *const c_char)
        }
    }

    /// Per frame on the game thread, with slot 0 (the local player) and slot 1 (its partner).
    pub unsafe fn update(mgr: u64) {
        let points = global(RESPAWN_POINTS);
        let (me, partner) = (respawn(slot_avatar(mgr, 0)), respawn(slot_avatar(mgr, 1)));
        if points == 0 || me == 0 {
            return;
        }
        log_changes(me, partner);
        let d = Down { state: rd_u32(me + STATE), has_point: rd_u8(points + P0_VALID) != 0, own_checkpoint: rd_u8(points + P1_VALID) != 0 };
        let now = START.get_or_init(Instant::now).elapsed().as_millis() as u64;
        let Ok(mut guard) = WAIT.lock() else { return };
        let Some(source) = guard.get_or_insert_with(Wait::default).update(d, now) else { return };
        if partner != 0 && rd_u8(partner + DOWN) != 0 {
            log!("respawn: waiting for a point, and the partner is down here too");
        }
        let dst = (points + P0_POINT) as *mut u8;
        match source {
            Source::OwnCheckpoint => std::ptr::copy_nonoverlapping((points + P1_POINT) as *const u8, dst, POINT_LEN),
            Source::Partner => {
                let p1 = slot_avatar(mgr, 1);
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

    /// Logs the local player's and the partner's Respawn state when it changes.
    unsafe fn log_changes(me: u64, partner: u64) {
        let now = [rd_u32(me + STATE), if partner == 0 { 0 } else { rd_u32(partner + STATE) }];
        let Ok(mut last) = LAST.lock() else { return };
        if *last != now {
            log!("respawn: state local {} partner {} (was {} {})", now[0], now[1], last[0], last[1]);
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
        assert_eq!(w.update(down(WAITING_FOR_PARTNER, false, true), 0), None);
        assert_eq!(w.update(down(WAITING_FOR_PARTNER, false, true), GRACE_MS - 1), None);
        assert_eq!(w.update(down(WAITING_FOR_PARTNER, false, true), GRACE_MS), Some(Source::OwnCheckpoint));
        // the next attempt waits a full grace again
        assert_eq!(w.update(down(WAITING_FOR_PARTNER, false, true), GRACE_MS + 1), None);
        assert_eq!(w.update(down(WAITING_FOR_PARTNER, false, false), 2 * GRACE_MS), Some(Source::Partner));
    }

    #[test]
    fn leaves_the_game_alone_otherwise() {
        let mut w = Wait::default();
        // the partner reached a checkpoint: the game respawns on its own
        assert_eq!(w.update(down(WAITING_FOR_PARTNER, true, false), 0), None);
        assert_eq!(w.update(down(WAITING_FOR_PARTNER, true, false), 10 * GRACE_MS), None);
        // alive, or down in another state
        assert_eq!(w.update(down(0, false, false), 20 * GRACE_MS), None);
        assert_eq!(w.update(down(4, false, false), 30 * GRACE_MS), None);
    }

    #[test]
    fn a_new_wait_starts_a_new_grace() {
        let mut w = Wait::default();
        w.update(down(WAITING_FOR_PARTNER, false, true), 0);
        w.update(down(0, false, true), 1000);
        assert_eq!(w.update(down(WAITING_FOR_PARTNER, false, true), 1500), None);
        assert_eq!(w.update(down(WAITING_FOR_PARTNER, false, true), 1500 + GRACE_MS), Some(Source::OwnCheckpoint));
    }
}
