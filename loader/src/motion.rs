//! Enemy motions (#49): a client's enemies skip their behaviour tree (`ai`), and with it the
//! motions it plays (attacks, hit reactions), so the host streams each enemy's current motion per
//! layer (`Msg::Motions`, unreliable) and clients play each change once. A motion travels as the
//! name hash its animation set looks it up by: indexes are one copy's, names every copy's.
use std::collections::{HashMap, HashSet};

/// Motion layers streamed: the game stops 0 and 1 together (`0x14056bda0`).
pub const LAYERS: usize = 2;
/// Name hash of the current motion per layer; 0 for none.
pub type Layers = [u32; LAYERS];

/// Attacks are short: sampled twice as often as NPC transforms.
pub const SAMPLE_MS: u64 = 33;
/// Every enemy is sent at least this often, as the message can be lost.
pub const RESEND_MS: u64 = 1000;
/// Most entries one sample sends; the rest wait for the next one.
pub const MAX_PER_SAMPLE: usize = 160;

/// Host side: what was last sent per enemy.
#[derive(Debug, Default)]
pub struct Tracker {
    level: u32,
    sent: HashMap<u32, (Layers, u64)>,
}

impl Tracker {
    pub fn set_level(&mut self, level: u32) -> bool {
        if self.level == level {
            return false;
        }
        self.level = level;
        self.sent.clear();
        true
    }

    /// The enemies whose motion changed since it was last sent, then those not sent for
    /// `RESEND_MS`. Enemies missing from `seen` are forgotten.
    pub fn sample(&mut self, now_ms: u64, seen: &[(u32, Layers)]) -> Vec<(u32, Layers)> {
        let alive: HashSet<u32> = seen.iter().map(|s| s.0).collect();
        self.sent.retain(|h, _| alive.contains(h));
        let (mut due, mut stale) = (Vec::new(), Vec::new());
        for &(hash, layers) in seen {
            match self.sent.get(&hash) {
                Some((sent, _)) if *sent != layers => due.push((hash, layers)),
                None => due.push((hash, layers)),
                Some((_, at)) if now_ms.saturating_sub(*at) >= RESEND_MS => stale.push((hash, layers)),
                Some(_) => {}
            }
        }
        due.extend(stale);
        due.truncate(MAX_PER_SAMPLE);
        for &(hash, layers) in &due {
            self.sent.insert(hash, (layers, now_ms));
        }
        due
    }
}

/// Client side: the host's motions last played per enemy, so each change plays once and the
/// enemy's own locomotion is not fought every frame.
#[derive(Debug, Default)]
pub struct Follower {
    level: u32,
    played: HashMap<u32, Layers>,
}

impl Follower {
    pub fn set_level(&mut self, level: u32) -> bool {
        if self.level == level {
            return false;
        }
        self.level = level;
        self.played.clear();
        true
    }

    /// Per layer, the host's motion when it changed since it last played here.
    pub fn changes(&mut self, hash: u32, host: Layers) -> [Option<u32>; LAYERS] {
        let prev = self.played.insert(hash, host);
        std::array::from_fn(|l| (host[l] != 0 && prev.is_none_or(|p| p[l] != host[l])).then_some(host[l]))
    }
}

#[cfg(windows)]
pub use live::{dev_list, update};

/// Game-thread glue.
#[cfg(windows)]
mod live {
    use super::*;
    use crate::actor::{self, ANIM, ANIM_MOTIONS, ENTRY_HASH, ENTRY_INDEX, ENTRY_NAME, LAYER_CURRENT, LAYER_SET, SET_BUCKETS, SET_BUCKET_COUNT};
    use crate::game::*;
    use crate::net::proto::level_hash;
    use crate::target::{named, named_actor_ptrs, RefIndex};
    use crate::{ai, log, net};
    use std::ffi::{c_char, CStr};
    use std::sync::Mutex;
    use std::time::{Duration, Instant};

    const LOG_EVERY: Duration = Duration::from_secs(10);
    /// Seconds a played motion blends in over.
    const BLEND_S: f32 = 0.2;
    /// `motion_play` flags: start at the given time (what the by-name player passes for a start in ms).
    const FROM_START: u32 = 0;
    /// Bounds on walking a name table, which is game memory.
    const MAX_BUCKETS: u32 = 1 << 16;
    const MAX_CHAIN: usize = 256;

    #[derive(Default)]
    struct Live {
        tracker: Tracker,
        follower: Follower,
        /// actor id -> whether it is an enemy, decided once per actor and level
        enemies: HashMap<u64, bool>,
        /// (animation set, motion index) -> name hash
        names: HashMap<(u64, i32), u32>,
        last_sample: Option<Instant>,
        last_log: Option<Instant>,
        played: u32,
        missing: u32,
    }

    static LIVE: Mutex<Option<Live>> = Mutex::new(None);

    unsafe fn layer(actor: u64, l: usize) -> u64 {
        let anim = rd_u64(actor + ANIM);
        let motions = if anim == 0 { 0 } else { rd_u64(anim + ANIM_MOTIONS) };
        if motions == 0 {
            0
        } else {
            motion_layer(motions, l as u32)
        }
    }

    /// Walks a layer's name table (one bucket when `hash` is given) as (hash, name, index) until `f` answers.
    unsafe fn find<T>(layer: u64, hash: Option<u32>, mut f: impl FnMut(u32, u64, i32) -> Option<T>) -> Option<T> {
        let set = rd_u64(layer + LAYER_SET);
        let table = if set == 0 { 0 } else { rd_u64(set + SET_BUCKETS) };
        let n = if set == 0 { 0 } else { rd_u32(set + SET_BUCKET_COUNT) };
        if table == 0 || n == 0 || n > MAX_BUCKETS {
            return None;
        }
        let buckets = hash.map_or(0..n, |h| h % n..h % n + 1);
        for b in buckets {
            let mut e = rd_u64(table + b as u64 * 8);
            for _ in 0..MAX_CHAIN {
                if e == 0 {
                    break;
                }
                if let Some(t) = f(rd_u32(e + ENTRY_HASH), rd_u64(e + ENTRY_NAME), rd_i32(e + ENTRY_INDEX)) {
                    return Some(t);
                }
                e = rd_u64(e);
            }
        }
        None
    }

    unsafe fn current(layer: u64) -> i32 {
        rd_i32(layer + LAYER_CURRENT)
    }

    /// Name hash of the motion a layer plays, 0 for none.
    unsafe fn current_hash(live: &mut Live, layer: u64) -> u32 {
        let index = current(layer);
        if index < 0 {
            return 0;
        }
        let key = (rd_u64(layer + LAYER_SET), index);
        if let Some(h) = live.names.get(&key) {
            return *h;
        }
        let h = find(layer, None, |h, _, i| (i == index).then_some(h)).unwrap_or(0);
        live.names.insert(key, h);
        h
    }

    /// Keyed by id: a later spawn can reuse a dead enemy's address.
    unsafe fn enemy(live: &mut Live, id: u64, actor: u64) -> bool {
        *live.enemies.entry(id).or_insert_with(|| crate::health::is_enemy(actor) && ai::follows_by_name(&actor::name(actor).unwrap_or_default()))
    }

    /// Game thread, per frame; `level` is the play level hash (0 outside one).
    pub unsafe fn update(level: u32) {
        if !net::active() {
            return;
        }
        let Ok(mut guard) = LIVE.lock() else { return };
        let live = guard.get_or_insert_with(Live::default);
        let changed = live.tracker.set_level(level);
        if live.follower.set_level(level) || changed {
            // actor ids and animation sets belong to the old level
            live.enemies.clear();
            live.names.clear();
        }
        if level == 0 {
            return;
        }
        if net::is_client() {
            follow(live, level);
        } else {
            stream(live, level);
        }
    }

    unsafe fn stream(live: &mut Live, level: u32) {
        if live.last_sample.is_some_and(|t| t.elapsed().as_millis() < SAMPLE_MS as u128) {
            return;
        }
        let now = Instant::now();
        live.last_sample = Some(now);
        let named = named_actor_ptrs();
        let index = RefIndex::build(named.iter().map(|(n, id, _)| (n.as_str(), *id)));
        let mut seen = Vec::new();
        for (n, id, actor) in &named {
            let hash = level_hash(n);
            if index.get(hash) != Some(*id) || !enemy(live, *id, *actor) {
                continue;
            }
            let layers: Layers = std::array::from_fn(|l| match layer(*actor, l) {
                0 => 0,
                ly => current_hash(live, ly),
            });
            seen.push((hash, layers));
        }
        let due = live.tracker.sample(now_ms(), &seen);
        if !due.is_empty() {
            net::set_motions(level, &due);
        }
        if live.last_log.is_none_or(|t| t.elapsed() >= LOG_EVERY) && !seen.is_empty() {
            live.last_log = Some(now);
            log!("motion: host streaming the motions of {} enemies", seen.len());
        }
    }

    unsafe fn follow(live: &mut Live, level: u32) {
        for (hash, host) in net::take_motions() {
            let id = named(level, hash);
            let actor = if id == 0 { 0 } else { registry_get(id) };
            // only enemies whose behaviour the host runs: townspeople and companions think here
            if actor < 0x1_0000 || !ai::follows(actor) {
                continue;
            }
            for (l, motion) in live.follower.changes(hash, host).into_iter().enumerate() {
                let Some(motion) = motion else { continue };
                let ly = layer(actor, l);
                match if ly == 0 { None } else { find(ly, Some(motion), |h, _, i| (h == motion).then_some(i)) } {
                    Some(i) if current(ly) != i => {
                        motion_play(ly, i as u32, FROM_START, 0.0, BLEND_S, 0);
                        live.played += 1;
                    }
                    Some(_) => {}
                    None => live.missing += 1,
                }
            }
        }
        if live.last_log.is_none_or(|t| t.elapsed() >= LOG_EVERY) && live.played + live.missing > 0 {
            live.last_log = Some(Instant::now());
            log!("motion: client played {} of the host's enemy motions, {} not in this copy's animation sets", live.played, live.missing);
            (live.played, live.missing) = (0, 0);
        }
    }

    /// Dev command `motions`: every enemy's motion per layer.
    pub unsafe fn dev_list() {
        let mut n = 0;
        for (refname, id, actor) in named_actor_ptrs() {
            if !crate::health::is_enemy(actor) {
                continue;
            }
            let layers: Vec<String> = (0..LAYERS)
                .map(|l| match layer(actor, l) {
                    0 => format!("{l}: no layer"),
                    ly => {
                        let index = current(ly);
                        let found = find(ly, None, |h, p, i| (i == index).then(|| (h, if p == 0 { String::new() } else { CStr::from_ptr(p as *const c_char).to_string_lossy().into_owned() })));
                        let (h, name) = found.unwrap_or_default();
                        format!("{l}: {index} {name:?} {h:#x}")
                    }
                })
                .collect();
            log!("dev: motions {refname} ({id:#x}) {}", layers.join(", "));
            n += 1;
        }
        log!("dev: motions: {n} enemies");
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

    #[test]
    fn a_new_enemy_is_sent_at_once_and_then_only_when_its_motion_changes() {
        let mut t = Tracker::default();
        assert_eq!(t.sample(0, &[(A, [5, 0])]), vec![(A, [5, 0])]);
        assert!(t.sample(33, &[(A, [5, 0])]).is_empty());
        assert_eq!(t.sample(66, &[(A, [6, 0])]), vec![(A, [6, 0])], "an attack starts");
        assert_eq!(t.sample(99, &[(A, [6, 9])]), vec![(A, [6, 9])], "a second layer counts on its own");
    }

    #[test]
    fn unchanged_enemies_are_refreshed_every_second_after_the_changes() {
        let mut t = Tracker::default();
        t.sample(0, &[(A, [5, 0]), (B, [7, 0])]);
        assert!(t.sample(RESEND_MS - 1, &[(A, [5, 0]), (B, [7, 0])]).is_empty());
        assert_eq!(t.sample(RESEND_MS, &[(A, [5, 0]), (B, [8, 0])]), vec![(B, [8, 0]), (A, [5, 0])]);
        assert!(t.sample(RESEND_MS + 33, &[(A, [5, 0]), (B, [8, 0])]).is_empty());
    }

    #[test]
    fn the_cap_defers_the_rest_and_vanished_enemies_and_levels_reset() {
        let mut t = Tracker::default();
        let many: Vec<(u32, Layers)> = (0..MAX_PER_SAMPLE as u32 + 7).map(|i| (i, [i, 0])).collect();
        assert_eq!(t.sample(0, &many).len(), MAX_PER_SAMPLE);
        assert_eq!(t.sample(33, &many).len(), 7, "the deferred ones were not marked sent");
        t.sample(66, &[(A, [5, 0])]);
        assert_eq!(t.sample(99, &[(B, [5, 0]), (A, [5, 0])]), vec![(B, [5, 0])], "B was forgotten when it vanished");
        assert!(t.set_level(3));
        assert!(!t.set_level(3));
        assert_eq!(t.sample(132, &[(A, [5, 0])]), vec![(A, [5, 0])], "a new level starts afresh");
    }

    #[test]
    fn a_client_plays_each_change_of_the_hosts_motion_once() {
        let mut f = Follower::default();
        assert_eq!(f.changes(A, [5, 0]), [Some(5), None], "first sight: play what the host plays; 0 is no motion");
        assert_eq!(f.changes(A, [5, 0]), [None, None], "a resend changes nothing");
        assert_eq!(f.changes(A, [6, 3]), [Some(6), Some(3)]);
        assert_eq!(f.changes(A, [6, 0]), [None, None], "a layer that stops is left to the game");
        assert_eq!(f.changes(B, [6, 0]), [Some(6), None], "per enemy");
        assert!(f.set_level(2));
        assert_eq!(f.changes(A, [6, 3]), [Some(6), Some(3)], "a new level forgets what played");
    }
}
