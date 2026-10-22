//! Buzz's rail flight (st_buzzgame, st_buzzflight): the flight controller places the ship from its
//! own rail state every frame, so a partner follows its owner's rail state, not its position.
use crate::net::proto::Rail;

/// Rail speeds beyond this (m/s) are a jump to a checkpoint, not flight.
const MAX_RATE: f32 = 200.0;
/// A report is carried forward at most this far (ms).
const MAX_LEAD_MS: u64 = 200;

/// The owner's speed along its rail, measured from its reports.
#[derive(Debug, Default)]
pub struct Follow {
    last: Option<(Rail, u64)>,
    rate: f32,
}

impl Follow {
    /// Takes a fresh report received at `now_us`.
    pub fn observe(&mut self, r: Rail, now_us: u64) {
        if let Some((prev, t)) = self.last {
            let dt = now_us.saturating_sub(t) as f32 / 1.0e6;
            let rate = (r.dist - prev.dist) / dt.max(1.0e-3);
            self.rate = if prev.rail == r.rail && (0.0..=MAX_RATE).contains(&rate) { rate } else { 0.0 };
        }
        self.last = Some((r, now_us));
    }

    /// Where the owner is now, given a report `age_ms` old.
    pub fn now(&self, r: Rail, age_ms: u64) -> Rail {
        Rail { dist: r.dist + self.rate * age_ms.min(MAX_LEAD_MS) as f32 / 1000.0, ..r }
    }
}

#[cfg(windows)]
pub use live::*;

#[cfg(windows)]
mod live {
    use super::Rail;
    use crate::game::*;
    use crate::log;

    /// The avatar's flight controller, 0 when it has none.
    pub unsafe fn controller(avatar: u64) -> u64 {
        if avatar == 0 {
            return 0;
        }
        component(avatar, STR_FLIGHT_CONTROLLER.as_ptr())
    }

    pub unsafe fn read(ctl: u64) -> Rail {
        Rail { rail: rd_i32(ctl + FLIGHT_RAIL), dist: rd_f32(ctl + FLIGHT_DIST), up: rd_f32(ctl + FLIGHT_UP), side: rd_f32(ctl + FLIGHT_SIDE) }
    }

    pub unsafe fn mode(ctl: u64) -> i32 {
        rd_i32(ctl + FLIGHT_MODE)
    }

    pub unsafe fn write(ctl: u64, r: Rail) {
        wr_i32(ctl + FLIGHT_RAIL, r.rail);
        wr_f32(ctl + FLIGHT_DIST, r.dist);
        wr_f32(ctl + FLIGHT_UP, r.up);
        wr_f32(ctl + FLIGHT_SIDE, r.side);
    }

    pub unsafe fn dev_list() {
        let mgr = global(MGR_PTR);
        for i in 0..4u64 {
            let ctl = controller(slot_avatar(mgr, i));
            if ctl == 0 {
                continue;
            }
            let r = read(ctl);
            log!("dev: flight slot {i}: mode {} rail {} dist {:.1} up {:.2} side {:.2}", mode(ctl), r.rail, r.dist, r.up, r.side);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(rail: i32, dist: f32) -> Rail {
        Rail { rail, dist, up: 1.0, side: -2.0 }
    }

    #[test]
    fn a_report_is_carried_forward_at_the_owners_speed() {
        let mut f = Follow::default();
        f.observe(at(0, 100.0), 0);
        f.observe(at(0, 103.0), 100_000); // 30 m/s
        let r = f.now(at(0, 103.0), 50);
        assert!((r.dist - 104.5).abs() < 1e-3);
        assert_eq!((r.rail, r.up, r.side), (0, 1.0, -2.0));
    }

    #[test]
    fn a_jump_or_a_new_rail_is_not_a_speed() {
        let mut f = Follow::default();
        f.observe(at(0, 100.0), 0);
        f.observe(at(0, 20.0), 100_000); // back to a checkpoint
        assert_eq!(f.now(at(0, 20.0), 100).dist, 20.0);
        f.observe(at(1, 40.0), 200_000);
        assert_eq!(f.now(at(1, 40.0), 100).dist, 40.0);
    }

    #[test]
    fn an_old_report_is_carried_at_most_200_ms() {
        let mut f = Follow::default();
        f.observe(at(0, 0.0), 0);
        f.observe(at(0, 10.0), 1_000_000); // 10 m/s
        assert!((f.now(at(0, 10.0), 5_000).dist - 12.0).abs() < 1e-3);
    }
}
