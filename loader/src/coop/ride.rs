//! A partner rides Bullseye when its owner does (FINDINGS 10). A story level mounts each copy's own
//! player at its start, before any partner is bound, so the partner gets a horse of its own the way
//! the game summons one (`0x140220db0`); while it rides, the horse is what is placed back.
use crate::actor;
use crate::game::*;
use crate::log;
use std::ffi::c_char;

const RETRY_US: u64 = 3_000_000;
/// Horses summoned for one partner before giving up; each failed try may leave one behind.
const MAX_SUMMONS: u32 = 2;

#[derive(Debug, Default)]
pub(super) struct Ride {
    /// the partner's own horse (actor id), 0 for none
    pub(super) horse: u64,
    next_try_us: u64,
    summons: u32,
    gave_up: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Step {
    Idle,
    /// put the partner on the horse it has
    Mount,
    /// summon a horse, then mount
    Summon,
    GiveUp,
}

impl Ride {
    /// What to do this frame; `horse_alive` is whether `horse` still names an actor.
    pub(super) fn step(&mut self, owner_riding: bool, partner_riding: bool, horse_alive: bool, now: u64) -> Step {
        if !owner_riding || partner_riding || self.gave_up || now < self.next_try_us {
            return Step::Idle;
        }
        self.next_try_us = now + RETRY_US;
        if self.horse != 0 && horse_alive {
            return Step::Mount;
        }
        if self.summons >= MAX_SUMMONS {
            self.gave_up = true;
            return Step::GiveUp;
        }
        self.summons += 1;
        Step::Summon
    }
}

/// The game's `Riding` state: a summoned horse, and Train Rescue's horse that runs by itself.
pub(super) unsafe fn riding(avatar: u64) -> bool {
    actor::in_state(avatar, b"Riding\0")
}

/// Spawns a Bullseye at the avatar as the summon does; its actor id, 0 on failure.
pub(super) unsafe fn summon(avatar: u64) -> u64 {
    let mut props = [0u64; 3];
    props_init(props.as_mut_ptr());
    props_parse(props.as_mut_ptr(), va(BULLSEYE_PROPS) as *const c_char);
    let pos = actor::pos(avatar);
    let horse = spawn_actor(props.as_mut_ptr(), pos.as_ptr(), 0.0, 0, 0);
    props_free(props.as_mut_ptr());
    actor::id(horse)
}

/// The summon's mount steps: the rider's blackboard is told its horse, then the forced ride is
/// requested.
pub(super) unsafe fn mount(avatar: u64, horse: u64) {
    let bb = blackboard(avatar, 0);
    if bb == 0 {
        log!("coop: no blackboard on avatar {avatar:#x}; cannot mount it");
        return;
    }
    blackboard_set(bb, b"mountActor\0".as_ptr() as *const c_char, horse);
    if !actor::request_state(avatar, b"BullseyeForceRide\0") {
        log!("coop: no state machine on avatar {avatar:#x}; cannot mount it");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summons_once_then_mounts_the_same_horse() {
        let mut r = Ride::default();
        assert_eq!(r.step(false, false, false, 0), Step::Idle);
        assert_eq!(r.step(true, false, false, 0), Step::Summon);
        r.horse = 7;
        // retries wait
        assert_eq!(r.step(true, false, true, RETRY_US - 1), Step::Idle);
        assert_eq!(r.step(true, false, true, RETRY_US), Step::Mount);
        // riding: nothing to do
        assert_eq!(r.step(true, true, true, 10 * RETRY_US), Step::Idle);
    }

    #[test]
    fn gives_up_after_the_summons_failed() {
        let mut r = Ride::default();
        assert_eq!(r.step(true, false, false, 0), Step::Summon);
        assert_eq!(r.step(true, false, false, RETRY_US), Step::Summon);
        assert_eq!(r.step(true, false, false, 2 * RETRY_US), Step::GiveUp);
        assert_eq!(r.step(true, false, false, 3 * RETRY_US), Step::Idle);
    }
}
