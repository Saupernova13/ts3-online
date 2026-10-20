//! NPC sync (H6c): the host streams the transforms of named actors that move (`Msg::Npcs`, unreliable);
//! clients find them by reference-name hash (FINDINGS 10) and follow, smoothed.
use crate::math::{ang_delta, dist};
use std::collections::{HashMap, HashSet};

/// (reference-name hash, position, yaw)
pub type Npc = (u32, [f32; 3], f32);

pub const SAMPLE_MS: u64 = 66;
/// An actor becomes a mover once this far (m) from where it was first seen in the level.
pub const MOVER_DIST: f32 = 0.05;
/// A mover is resent once it moved this far (m) since its last send...
pub const SEND_DIST: f32 = 0.02;
/// ...or turned this much.
pub const SEND_YAW: f32 = 2.0 * std::f32::consts::PI / 180.0;
/// Every mover is sent at least this often, even when still.
pub const RESEND_MS: u64 = 1000;
/// Most entries one sample sends; the rest wait for the next one.
pub const MAX_PER_SAMPLE: usize = 480;
/// Fraction of the way to the host's position a client moves each frame.
pub const FOLLOW_LERP: f32 = 0.3;
/// Farther than this (m) from the host's position, a client snaps instead of gliding.
pub const SNAP_DIST: f32 = 3.0;

fn plausible(pos: [f32; 3], yaw: f32) -> bool {
    pos.iter().all(|v| v.is_finite() && v.abs() <= 1.0e6) && yaw.is_finite()
}

#[derive(Debug)]
struct Track {
    origin: [f32; 3],
    mover: bool,
    /// position, yaw and time of the last send
    sent: Option<([f32; 3], f32, u64)>,
}

/// Host side: which sampled actors are movers and which are due to be sent.
#[derive(Debug, Default)]
pub struct Tracker {
    level: u32,
    tracks: HashMap<u32, Track>,
}

impl Tracker {
    pub fn set_level(&mut self, level: u32) -> bool {
        if self.level == level {
            return false;
        }
        self.level = level;
        self.tracks.clear();
        true
    }

    /// Returns the due movers, biggest change first. Actors missing from `seen` are forgotten.
    pub fn sample(&mut self, now_ms: u64, seen: &[Npc]) -> Vec<Npc> {
        let alive: HashSet<u32> = seen.iter().map(|s| s.0).collect();
        self.tracks.retain(|h, _| alive.contains(h));
        let mut due: Vec<(f32, Npc)> = Vec::new();
        for &(hash, pos, yaw) in seen {
            if !plausible(pos, yaw) {
                continue;
            }
            let t = self.tracks.entry(hash).or_insert(Track { origin: pos, mover: false, sent: None });
            if !t.mover && dist(t.origin, pos) > MOVER_DIST {
                t.mover = true;
            }
            if !t.mover {
                continue;
            }
            let (moved, turned, age) = match t.sent {
                Some((p, y, at)) => (dist(p, pos), ang_delta(y, yaw).abs(), now_ms.saturating_sub(at)),
                None => (dist(t.origin, pos), 0.0, u64::MAX),
            };
            if moved > SEND_DIST || turned > SEND_YAW || age >= RESEND_MS {
                due.push((moved + turned, (hash, pos, yaw)));
            }
        }
        due.sort_by(|a, b| b.0.total_cmp(&a.0));
        due.truncate(MAX_PER_SAMPLE);
        for &(_, (hash, pos, yaw)) in &due {
            if let Some(t) = self.tracks.get_mut(&hash) {
                t.sent = Some((pos, yaw, now_ms));
            }
        }
        due.into_iter().map(|d| d.1).collect()
    }

    pub fn movers(&self) -> usize {
        self.tracks.values().filter(|t| t.mover).count()
    }
}

/// Client side: the latest host transform per actor.
#[derive(Debug, Default)]
pub struct Follower {
    level: u32,
    latest: HashMap<u32, ([f32; 3], f32)>,
}

impl Follower {
    pub fn set_level(&mut self, level: u32) -> bool {
        if self.level == level {
            return false;
        }
        self.level = level;
        self.latest.clear();
        true
    }

    pub fn merge(&mut self, entries: impl IntoIterator<Item = Npc>) {
        for (hash, pos, yaw) in entries {
            if plausible(pos, yaw) {
                self.latest.insert(hash, (pos, yaw));
            }
        }
    }

    pub fn len(&self) -> usize {
        self.latest.len()
    }

    pub fn is_empty(&self) -> bool {
        self.latest.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = Npc> + '_ {
        self.latest.iter().map(|(h, (p, y))| (*h, *p, *y))
    }
}

pub fn smooth_pos(cur: [f32; 3], target: [f32; 3]) -> [f32; 3] {
    if dist(cur, target) > SNAP_DIST {
        return target;
    }
    [0, 1, 2].map(|i| cur[i] + (target[i] - cur[i]) * FOLLOW_LERP)
}

pub fn smooth_yaw(cur: f32, target: f32) -> f32 {
    cur + ang_delta(cur, target) * FOLLOW_LERP
}

#[cfg(windows)]
pub use live::update;

/// Game-thread glue.
#[cfg(windows)]
mod live {
    use super::*;
    use crate::actor::{self, place_pose};
    use crate::game::*;
    use crate::net::proto::level_hash;
    use crate::target::{named, named_actor_ptrs, RefIndex};
    use crate::{log, net};
    use std::sync::Mutex;
    use std::time::{Duration, Instant};

    const LOG_EVERY: Duration = Duration::from_secs(10);
    /// `ActorCarried+0x18`: nonzero while carried (see `coop::carry_flags`).
    const CARRIER: u64 = 0x18;

    #[derive(Default)]
    struct Live {
        tracker: Tracker,
        follower: Follower,
        last_sample: Option<Instant>,
        last_log: Option<Instant>,
    }

    static LIVE: Mutex<Option<Live>> = Mutex::new(None);

    unsafe fn carried(actor: u64) -> bool {
        let comp = find_component(actor, c"ActorCarried".as_ptr() as u64);
        comp != 0 && rd_u64(comp + CARRIER) != 0
    }

    unsafe fn avatars(mgr: u64) -> [u64; 4] {
        [0, 1, 2, 3].map(|i| slot_avatar(mgr, i))
    }

    /// Game thread, per frame; `level` is the play level hash (0 outside one).
    pub unsafe fn update(level: u32) {
        if !net::active() {
            return;
        }
        let Ok(mut guard) = LIVE.lock() else { return };
        let live = guard.get_or_insert_with(Live::default);
        live.tracker.set_level(level);
        live.follower.set_level(level);
        let mgr = global(MGR_PTR);
        if level == 0 || mgr == 0 || slot_avatar(mgr, 0) == 0 {
            return;
        }
        let skip = avatars(mgr);
        let log_due = live.last_log.is_none_or(|t| t.elapsed() >= LOG_EVERY);
        if net::is_client() {
            live.follower.merge(net::take_npcs());
            if live.follower.is_empty() {
                return;
            }
            let mut applied = 0;
            for (hash, pos, yaw) in live.follower.iter() {
                let id = named(level, hash);
                let actor = if id == 0 { 0 } else { registry_get(id) };
                if actor < 0x1_0000 || skip.contains(&actor) || carried(actor) {
                    continue;
                }
                applied += 1;
                let (cur_pos, cur_yaw) = actor::pose(actor);
                let (new_pos, new_yaw) = if cur_pos.iter().all(|v| v.is_finite()) { (smooth_pos(cur_pos, pos), smooth_yaw(cur_yaw, yaw)) } else { (pos, yaw) };
                if dist(cur_pos, new_pos) > 1.0e-4 || ang_delta(cur_yaw, new_yaw).abs() > 1.0e-3 {
                    place_pose(actor, new_pos, new_yaw);
                }
            }
            if log_due && applied > 0 {
                live.last_log = Some(Instant::now());
                log!("npc: client following {applied} actors");
            }
            return;
        }
        if live.last_sample.is_some_and(|t| t.elapsed().as_millis() < SAMPLE_MS as u128) {
            return;
        }
        let now = Instant::now();
        live.last_sample = Some(now);
        let named = named_actor_ptrs();
        let index = RefIndex::build(named.iter().map(|(n, id, _)| (n.as_str(), *id)));
        let seen: Vec<Npc> = named
            .iter()
            .filter(|(_, _, actor)| !skip.contains(actor))
            .filter_map(|(n, id, actor)| {
                let hash = level_hash(n);
                (index.get(hash) == Some(*id)).then(|| {
                    let (pos, yaw) = actor::pose(*actor);
                    (hash, pos, yaw)
                })
            })
            .collect();
        let due = live.tracker.sample(now_ms(), &seen);
        if !due.is_empty() {
            net::set_npcs(level, &due);
        }
        if log_due && live.tracker.movers() > 0 {
            live.last_log = Some(now);
            log!("npc: host streaming {} movers", live.tracker.movers());
        }
    }

    fn now_ms() -> u64 {
        static START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
        START.get_or_init(Instant::now).elapsed().as_millis() as u64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: u32 = 1;
    const B: u32 = 2;

    fn at(h: u32, x: f32) -> Npc {
        (h, [x, 0.0, 0.0], 0.0)
    }

    #[test]
    fn still_actors_are_never_sent() {
        let mut t = Tracker::default();
        for ms in (0..5000).step_by(66) {
            assert!(t.sample(ms, &[at(A, 1.0), at(B, 2.0)]).is_empty());
        }
        // jitter under 5 cm from the first sighting is not movement
        assert!(t.sample(5000, &[at(A, 1.04)]).is_empty());
        assert_eq!(t.movers(), 0);
    }

    #[test]
    fn an_actor_becomes_a_mover_past_five_cm_and_is_sent_at_once() {
        let mut t = Tracker::default();
        t.sample(0, &[at(A, 0.0)]);
        let out = t.sample(66, &[at(A, 0.06)]);
        assert_eq!(out, vec![at(A, 0.06)]);
        assert_eq!(t.movers(), 1);
        // it stays a mover after returning to where it started
        assert_eq!(t.sample(132, &[at(A, 0.0)]).len(), 1);
    }

    #[test]
    fn movers_are_sent_again_after_two_cm_or_two_degrees() {
        let mut t = Tracker::default();
        t.sample(0, &[at(A, 0.0)]);
        t.sample(66, &[at(A, 1.0)]);
        assert!(t.sample(132, &[at(A, 1.015)]).is_empty());
        assert_eq!(t.sample(198, &[at(A, 1.025)]), vec![at(A, 1.025)]);
        // measured from the last send, not the last sample
        assert!(t.sample(264, &[at(A, 1.03)]).is_empty());
        let turned = (A, [1.025, 0.0, 0.0], 3.0f32.to_radians());
        assert_eq!(t.sample(330, &[turned]), vec![turned]);
        let barely = (A, [1.025, 0.0, 0.0], 3.0f32.to_radians() + 1.0f32.to_radians());
        assert!(t.sample(396, &[barely]).is_empty());
    }

    #[test]
    fn yaw_wraps_around() {
        let mut t = Tracker::default();
        t.sample(0, &[at(A, 0.0)]);
        let a = (A, [1.0, 0.0, 0.0], std::f32::consts::PI - 0.001);
        t.sample(66, &[a]);
        let b = (A, [1.0, 0.0, 0.0], -std::f32::consts::PI + 0.001);
        assert!(t.sample(132, &[b]).is_empty(), "0.002 rad apart across the seam");
    }

    #[test]
    fn still_movers_are_refreshed_every_second() {
        let mut t = Tracker::default();
        t.sample(0, &[at(A, 0.0)]);
        t.sample(66, &[at(A, 1.0)]);
        assert!(t.sample(66 + RESEND_MS - 1, &[at(A, 1.0)]).is_empty());
        assert_eq!(t.sample(66 + RESEND_MS, &[at(A, 1.0)]).len(), 1);
        assert!(t.sample(66 + RESEND_MS + 66, &[at(A, 1.0)]).is_empty());
        assert_eq!(t.sample(66 + 2 * RESEND_MS, &[at(A, 1.0)]).len(), 1);
    }

    #[test]
    fn the_biggest_movers_go_first_and_the_cap_defers_the_rest() {
        let mut t = Tracker::default();
        let n = MAX_PER_SAMPLE as u32 + 20;
        let base: Vec<Npc> = (0..n).map(|i| at(i, 0.0)).collect();
        t.sample(0, &base);
        let moved: Vec<Npc> = (0..n).map(|i| at(i, 0.1 + i as f32 / 1000.0)).collect();
        let out = t.sample(66, &moved);
        assert_eq!(out.len(), MAX_PER_SAMPLE);
        assert_eq!(out[0].0, n - 1);
        assert!(out.windows(2).all(|w| w[0].0 > w[1].0));
        // the 20 deferred ones were not marked sent, so they come next
        let next = t.sample(132, &moved);
        assert_eq!(next.len(), 20);
        assert!(next.iter().all(|e| e.0 < 20));
    }

    #[test]
    fn a_level_change_and_vanished_actors_reset_the_state() {
        let mut t = Tracker::default();
        assert!(t.set_level(5));
        assert!(!t.set_level(5));
        t.sample(0, &[at(A, 0.0)]);
        t.sample(66, &[at(A, 1.0)]);
        assert_eq!(t.movers(), 1);
        t.sample(132, &[at(B, 0.0)]);
        assert_eq!(t.movers(), 0, "A is gone");
        t.sample(198, &[at(A, 1.0)]);
        assert!(t.sample(264, &[at(A, 1.0)]).is_empty(), "a returning actor starts afresh");
        assert!(t.set_level(6));
        assert_eq!(t.movers(), 0);
    }

    #[test]
    fn implausible_samples_are_skipped() {
        let mut t = Tracker::default();
        t.sample(0, &[at(A, 0.0)]);
        assert!(t.sample(66, &[(A, [f32::NAN, 0.0, 0.0], 0.0), (B, [2.0e6, 0.0, 0.0], 0.0), (B, [0.0; 3], f32::NAN)]).is_empty());
    }

    #[test]
    fn follower_keeps_the_latest_per_hash_and_forgets_on_level_change() {
        let mut f = Follower::default();
        f.set_level(3);
        f.merge([at(A, 1.0), at(B, 2.0)]);
        f.merge([at(A, 1.5), (B, [f32::NAN, 0.0, 0.0], 0.0)]);
        let mut all: Vec<_> = f.iter().collect();
        all.sort_by_key(|e| e.0);
        assert_eq!(all, vec![at(A, 1.5), at(B, 2.0)]);
        assert_eq!(f.len(), 2);
        assert!(!f.set_level(3));
        assert!(f.set_level(4));
        assert!(f.is_empty());
    }

    #[test]
    fn smoothing_glides_and_snaps() {
        let p = smooth_pos([0.0; 3], [1.0, 0.0, 0.0]);
        assert!((p[0] - FOLLOW_LERP).abs() < 1e-6 && p[1] == 0.0);
        // repeated steps converge
        let mut cur = [0.0; 3];
        for _ in 0..40 {
            cur = smooth_pos(cur, [1.0, 2.0, -1.0]);
        }
        assert!(dist(cur, [1.0, 2.0, -1.0]) < 1e-3);
        // beyond 3 m it snaps, at 3 m it still glides
        assert_eq!(smooth_pos([0.0; 3], [10.0, 0.0, 0.0]), [10.0, 0.0, 0.0]);
        assert!(smooth_pos([0.0; 3], [SNAP_DIST, 0.0, 0.0])[0] < SNAP_DIST);
    }

    #[test]
    fn yaw_smoothing_takes_the_short_way() {
        let pi = std::f32::consts::PI;
        let y = smooth_yaw(pi - 0.1, -pi + 0.1);
        assert!(y > pi - 0.1, "{y}");
        assert!((smooth_yaw(0.0, 1.0) - FOLLOW_LERP).abs() < 1e-6);
    }
}
